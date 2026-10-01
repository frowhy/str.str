//! STR bundle GUI 编辑器（Rust + Slint）。
//!
//! 所有 `.str.toml` 的读取与写回均经由 `str-format` 库，
//! 写出字节一律是规范 §4.9 的 canonical 形式，保证 Git diff 干净、校验器零告警。

// Windows release 构建不附带控制台（「命令提示符」）窗口。
//
// 不设置该属性时，release 二进制被标记为 **console 子系统**：双击运行会多出
// 一个黑窗口，窗口关闭还会连带结束 GUI 进程；stdout/stderr 也绑定到它。
// debug 构建**刻意保留**控制台 —— 开发期要看 `STR_DEBUG=1` 时的诊断输出。
// 若 release 下也需要日志，应改写为落文件（`eprintln!` 在 GUI 子系统下无处可去）。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{Model, ModelRc, SharedString, VecModel, Weak};
use str_format::bundle::{Bundle, Scan, Visit};
use str_format::meta::{Entry, Kind, Meta, MetaLoad, RefItem};
use str_format::meta_edit;
use str_format::util;
use str_format::validate;

slint::include_modules!();

// ── 思维导图布局常量（px）──
/// 节点标题行高（需与 app.slint 节点标题行、EntryListPanel 高度扣减一致）。
const NODE_H: f32 = 28.0;
const NODE_VGAP: f32 = 10.0;
// 渲染 / 键枚举的**全局预算**：挂载可以构成任意深的 DAG，病态 bundle（大量
// 交叉挂载 + 全部展开）下渲染出现次数与嵌套实例键数量会指数增长 —— 不设上限
// 会让「展开全部」或一次重排挂死 UI（实测 490 分支 / 442 挂载即可触发）。上限
// 远超正常 bundle 的规模（数万行），只拦截病态形态。
const TREE_ROW_CAP: usize = 100_000;
const MIND_NODE_CAP: usize = 100_000;
const EXPAND_ALL_KEY_CAP: usize = 100_000;
const NODE_HGAP: f32 = 70.0;
const MIND_PAD: f32 = 20.0;
/// 列表行高，对标 Finder 列表视图（本机实测 40px @2x = **20pt**，字体 13px）。
/// 需与 app.slint 中结构树行 / 条目行的 `height` 及上下半区放置区高度
/// （各行高一半）保持一致。
const ENTRY_ROW_H: f32 = 20.0;
/// 面板末尾放置区高度。
const ENTRY_END_H: f32 = 16.0;
/// 节点内容区底部留白。
const ENTRY_PAD_BOTTOM: f32 = 4.0;
/// 分支内底部「内容」伸缩条高度。
const CONTENT_BAR_H: f32 = 22.0;
/// 未展开内容时的节点高度：标题行 + 内容伸缩箭头复用标题行下方留白
/// （箭头贴标题文字底，即 NODE_H / 2 + 6 → 20px；+ 按钮 18px + 底距 2px）。
/// 不再额外占用整行 22px，避免标题与箭头之间出现两段留白叠加的间距。
const NODE_H_COMPACT: f32 = 40.0;
/// 外置子树按钮占位（按钮 18px + 间隙）：连线起点与画布右缘需越过它。
const SUBTREE_BTN_EXTENT: f32 = 26.0;
/// 连线线宽（px）。
const EDGE_W: f32 = 2.0;
/// 内容展开时节点宽度：与列表视图行同构的四列布局（display 40% + title + role 56 + size 60）。
const NODE_EXPANDED_W: f32 = 360.0;

/// 左侧列表 / 导图共用的可视行。
#[derive(Debug)]
struct VisibleRow {
    /// 对应 `Scan::visits` 的下标。
    visit: usize,
    depth: usize,
    title: String,
    type_str: String,
    expanded: bool,
    has_children: bool,
    /// 是否含内容条目（`node` / `branch` 之外）：控制「展开 / 收起内容」可用性。
    has_entries: bool,
    /// 软链接挂载行（规范 §4.6.1）：本行是挂载进来的视图，身份 = 目标分支。
    /// 硬链接行也是挂载行形态（身份 = 目标），以 `hard` 区分。
    mounted: bool,
    /// 硬链接行（表现与软链接一致：身份 / 信息 / 内容 = 目标，内容只读），
    /// 额外渲染**自有子分支**（见 `link_visit`）。
    hard: bool,
    /// 挂载点所在分支（真实行 = None），供「移除挂载」定位要摘的 `entries[]` 行。
    mount_parent: Option<usize>,
    /// 硬链接行 = 自身分支的 visit（自有子结构的磁盘父级，「新建子分支」落点）；
    /// 其余行为 None。
    link_visit: Option<usize>,
    /// 本行的**实例展开键**：真实行 = 自身 rel；挂载行 = 「挂载点 ␟ mount ␟ 目标」
    /// 实例键 —— 展开收起只作用于本行，与真身及其它挂载视图互不关联。
    expand_key: String,
}

/// 挂载子分支（`role = "link"`）的解析结果：目标 visit 下标 + 挂载点别名。
///
/// 软链接（`hard = false`）：目标的完整视图，身份 = 目标。
/// 硬链接（`hard = true`）：**表现与软链接一致**（身份 = 目标：点行选中真身、
/// 信息 / 内容 / 展开态都指向目标、渲染目标的子树），仅多两件事 —— ① 在自身
/// 之下渲染**自有子分支**（`link_idx` = 自身分支 visit，`path = id` 的真实分支），
/// 新建子分支落自有结构；② 内容只读（`sel_hard` 守卫）。
#[derive(Clone)]
struct MountChild {
    visit: usize,
    title: Option<String>,
    hard: bool,
    /// 硬链接自身分支的 visit（自有子结构的磁盘父级）；软链接为 None。
    link_idx: Option<usize>,
}

/// 编辑器状态。
struct Editor {
    bundle: Option<Bundle>,
    scan: Option<Scan>,
    /// 子分支索引（见 `children_index`）：随 scan 一起建立，供行构建 / 导图布局
    /// 按节点 O(1) 取用。
    kids: Vec<Vec<usize>>,
    /// 软链接挂载索引（见 `mount_index`）：父分支 → 挂载进来的子分支（规范 §4.6.1）。
    /// 与 `kids` 平行、随 scan 一起重建。
    mounts: Vec<Vec<MountChild>>,
    rows: Vec<VisibleRow>,
    /// 已展开分支的身份键集合（见 `visit_key`；跨 rescan 稳定）。
    expanded: HashSet<String>,
    /// 挂载选择器的**独立行模型与展开集合**：打开对话框时以 `expanded` 现状为
    /// 起点拷贝，对话框内的展开 / 收起只改 `pick_expanded` / `pick_rows`、不回写
    /// 树 —— 否则复用 `toggle-expand` 会连带 `collapse_subtree_state` 挪走主窗口
    /// 选中、清空内容展开（用户没碰过树，树却变了）。两份模型必须分开：树与
    /// 选择器都渲染行列表，共享可见性就会「选择器展开、树里也冒出可见行」。
    pick_expanded: HashSet<String>,
    pick_rows: Vec<VisibleRow>,
    /// 当前选中分支的身份键（见 `visit_key`）。
    selected: Option<String>,
    /// `visit_key` → `Scan::visits` 下标（随 scan 重建），身份键的 O(1) 反查。
    visit_by_rel: HashMap<String, usize>,
    /// 「内容」页选中的条目路径（None = 未选中；路径跨 rescan 稳定）。
    selected_entry_path: Option<String>,
    /// 「内容」页已展开的内容文件夹（以分支内绝对路径为键）。
    expanded_dirs: HashSet<String>,
    /// 导图节点内已展开内容的分支身份键集合（见 `visit_key`；跨 rescan 稳定）。
    content_expanded: HashSet<String>,
    /// 当前选中是否经由**硬链接**行 / 节点进入（规范 §4.6.1）：内容只读。
    /// 由行 / 节点点击处置位，`select_visit` 复位（真实位置选择恒为 false）。
    sel_hard: bool,
    /// 当前选中经由**软挂载行 / 节点**进入时的挂载点（挂载所在分支的 visit）。
    /// 挂载行身份 = 目标真身 —— 菜单栏 / 信息页的「删除分支」据此改道为
    /// 「移除挂载」，防止误删目标分支。随 `sel_hard` 一起维护、`select_visit` 复位。
    sel_mount_parent: Option<usize>,
    /// 当前选中行的**展开键**（含实例上下文前缀，如 `{实例键}␟{rel}`）。菜单栏
    /// 「展开/收起子树」据此作用于所选行所处的实例上下文，而非真身 rel 键。
    sel_expand_key: Option<String>,
    /// 批量操作（内容条目多选）的条目路径集合（仅当前选中分支内；路径跨 rescan 稳定）。
    /// Finder 语义：⌘/Ctrl 点击切换、Shift 点击范围、⌘A 全选、点空白 / 切分支清空。
    entry_multi: HashSet<String>,
    /// Shift 范围多选的锚点（条目列表行下标）。
    entry_anchor: Option<usize>,
    /// 结构树模型句柄：行点击时原地更新选中标记，避免重建模型破坏双击手势。
    rows_model: RefCell<Option<Rc<VecModel<BranchRow>>>>,
    /// 导图节点模型句柄：节点右键时原地更新选中标记，避免重建模型销毁
    /// 正在显示右键菜单的节点组件（菜单项点击失效）。
    mind_nodes_model: RefCell<Option<Rc<VecModel<MindNode>>>>,
    /// 内容条目模型句柄：行数不变时原地更新，避免右键盘某行时重建模型
    /// 销毁承载右键菜单的行组件（与 rows_model 同款）。
    entries_model: RefCell<Option<Rc<VecModel<EntryRow>>>>,
    /// 导图节点内嵌内容面板的行模型句柄（键 = visit 下标）：节点模型原地更新时
    /// 若行数不变则复用同一句柄，节点内右键菜单 / 拖拽不会被打断。
    node_entry_models: RefCell<HashMap<usize, Rc<VecModel<EntryRow>>>>,
    /// scan 代次：open / rescan 递增。visit 下标与结构随之变化，用作布局缓存
    /// 与行模型句柄的失效依据。
    scan_gen: u64,
    /// 导图布局缓存（签名 → 布局）。几何只与可见行结构、展开集合有关，
    /// 与选中 / 多选无关——选中变化不该重排整棵树（见 `mind_sig`）。
    mind_cache: RefCell<Option<(u64, MindOut)>>,
    /// 小地图位图最近一次烘焙的键（布局签名 + 映射参数 + 选中路径 + 用色）：
    /// 键不变就跳过重烘焙（selection_spine 只影响高亮路径）。
    mini_raster_key: RefCell<Option<String>>,
}

impl Editor {
    fn new() -> Self {
        Self {
            bundle: None,
            scan: None,
            rows: Vec::new(),
            pick_rows: Vec::new(),
            expanded: HashSet::new(),
            pick_expanded: HashSet::new(),
            kids: Vec::new(),
            mounts: Vec::new(),
            selected: None,
            visit_by_rel: HashMap::new(),
            selected_entry_path: None,
            expanded_dirs: HashSet::new(),
            content_expanded: HashSet::new(),
            entry_multi: HashSet::new(),
            entry_anchor: None,
            rows_model: RefCell::new(None),
            mind_nodes_model: RefCell::new(None),
            entries_model: RefCell::new(None),
            node_entry_models: RefCell::new(HashMap::new()),
            scan_gen: 0,
            mind_cache: RefCell::new(None),
            mini_raster_key: RefCell::new(None),
            sel_hard: false,
            sel_mount_parent: None,
            sel_expand_key: None,
        }
    }

    fn open(&mut self, path: &Path) -> Result<(), String> {
        let bundle = Bundle::new(path).map_err(|e| e.to_string())?;
        let scan = bundle.scan().map_err(|e| e.to_string())?;
        self.expanded = scan
            .visits
            .first()
            .map(|v| v.rel.clone())
            .into_iter()
            .collect();
        self.selected = self.expanded.iter().next().cloned();
        self.bundle = Some(bundle);
        self.kids = children_index(&scan);
        self.mounts = mount_index(&scan);
        self.visit_by_rel = rel_index(&scan);
        self.scan = Some(scan);
        self.selected_entry_path = None;
        self.content_expanded.clear();
        self.entry_multi.clear();
        self.entry_anchor = None;
        self.scan_gen += 1;
        self.reset_entry_models();
        self.rebuild();
        Ok(())
    }

    /// 内容行模型句柄按 visit 下标缓存，visit 空间随 scan 变化 → 必须清空。
    fn reset_entry_models(&mut self) {
        self.node_entry_models.borrow_mut().clear();
        *self.entries_model.borrow_mut() = None;
    }

    /// 重新扫描磁盘（按 id 保留展开与选中状态）。
    fn rescan(&mut self) -> Result<(), String> {
        let bundle = self.bundle.as_ref().ok_or("未打开 bundle")?;
        let scan = bundle.scan().map_err(|e| e.to_string())?;
        self.kids = children_index(&scan);
        // 条目多选剪除磁盘上已消失的路径（批量操作后的幸存失败项保持选中，便于重试）
        if let Some(vi) = self.selected_idx_in_visits() {
            let dir = scan.visits[vi].dir.clone();
            self.entry_multi.retain(|p| dir.join(p).exists());
        } else {
            self.entry_multi.clear();
        }
        self.visit_by_rel = rel_index(&scan);
        self.mounts = mount_index(&scan);
        self.scan = Some(scan);
        if self
            .selected
            .as_deref()
            .is_some_and(|key| self.visit_idx(key).is_none())
        {
            self.selected = self
                .scan
                .as_ref()
                .and_then(|s| s.visits.first())
                .map(|v| v.rel.clone());
        }
        self.scan_gen += 1;
        self.reset_entry_models();
        self.rebuild();
        Ok(())
    }

    /// 依据展开集合重建可视行。
    ///
    /// **两份模型**：`rows` 按树的 `expanded`、`pick_rows` 按选择器的
    /// `pick_expanded` 各建各的 —— 树与选择器都渲染行列表，可见性绝不能共享
    /// （否则选择器展开的分支会在树里冒出可见行）。
    fn rebuild(&mut self) {
        self.rows = Vec::new();
        self.pick_rows = Vec::new();
        if let Some(scan) = self.scan.as_ref() {
            if scan.root_index.is_some() {
                dfs_rows(scan, &self.kids, &self.mounts, 0, &self.expanded, &mut self.rows);
                dfs_rows(
                    scan, &self.kids, &self.mounts, 0, &self.pick_expanded, &mut self.pick_rows,
                );
            }
        }
    }

    /// 身份键（见 `visit_key`）→ visit 下标。
    fn visit_idx(&self, key: &str) -> Option<usize> {
        self.visit_by_rel.get(key).copied()
    }

    fn selected_visit(&self) -> Option<&Visit> {
        let key = self.selected.as_deref()?;
        let idx = self.visit_idx(key)?;
        self.scan.as_ref()?.visits.get(idx)
    }

    fn selected_idx_in_visits(&self) -> Option<usize> {
        self.visit_idx(self.selected.as_deref()?)
    }

    fn select_visit(&mut self, visit: usize) {
        // 默认按「真实位置选择」复位；硬链接行 / 节点的点击在调用后置位。
        self.sel_hard = false;
        self.sel_mount_parent = None;
        self.sel_expand_key = None;
        let new_key = self
            .scan
            .as_ref()
            .map(|s| visit_key(s, visit).to_string());
        // 条目多选只属于当前分支：**切换**分支才清空（Finder 语义）。
        // 同分支重复激活（EntryListPanel 的「先 activate 再操作」约定）不得清空，
        // 否则 ⌘/Shift 多选与范围锚点会在每次点击时被摧毁。
        let changed = self.selected.as_deref() != new_key.as_deref();
        self.selected = new_key;
        self.selected_entry_path = None;
        if changed {
            self.entry_multi.clear();
            self.entry_anchor = None;
        }
    }
}

// ─────────────────── 批量操作（内容条目，Finder 语义）───────────────────

/// 批量操作的单项结果：`status` = "✓" 成功 / "✗" 失败（汇总对话框逐行展示）。
/// `new_path` = 落地后的登记路径（粘贴 = 去重后的目标路径；其余操作为空），
/// 供「粘贴后激活被粘贴条目」使用。
struct BatchOutcome {
    id: String,
    title: String,
    status: &'static str,
    msg: String,
    new_path: String,
}

/// 内容条目批量操作：持有执行所需的全部数据（Send + Sync，可跨线程 / 可重试）。
///
/// 语义与单条目操作一致（同一代码路径）：删除 = 废纸篓（可恢复）+ `entries[]` 登记同步；
/// 重命名 = 磁盘 rename + 登记路径同步；粘贴 = 逐项 `apply_clip_at` 落到发起粘贴的分支。
#[derive(Clone)]
enum ContentOp {
    /// 删除条目（trash + 登记同步；≡ 单条目「移到废纸篓」）。
    Trash { branch_dir: PathBuf },
    /// 批量重命名（磁盘 rename + 登记路径同步；≡ 单条目「重命名」）。
    Rename {
        branch_dir: PathBuf,
        renames: Vec<(String, String)>,
    },
    /// 批量粘贴（发起粘贴时固定目标分支与剪贴板快照；≡ 单条目「粘贴」）。
    /// bundle_root 用于剪切时的源登记清理边界（跨分支剪切必须以 bundle 根为界）。
    /// `into_folder` = Some(文件夹相对路径) 时目标是**内容文件夹**：子项不登记，
    /// 仅文件系统落地（count 由 batch_finished 统一维护）。
    Paste {
        clips: Vec<ClipItem>,
        bundle_root: PathBuf,
        visit_dir: PathBuf,
        depth: usize,
        id_version: usize,
        into_folder: Option<String>,
    },
}

/// 批量执行期间复用的**写会话**：把逐项执行的
/// 「`read_meta`（解析整份 TOML）+ `save`（重写整份 TOML）」收敛为
/// 「整批读一次 + 内存累积 + 每 `FLUSH_EVERY` 项/收尾各写一次」；
/// 并把批量删除的**废纸篓调用按批合并**。
///
/// 两个性能事实（均为真机实测/源码核实）：
/// 1. `trash` 5.x 在 macOS **默认 `DeleteMethod::Finder`**（源码 `src/macos/mod.rs`），
///    即每次调用都 spawn 一个 `osascript` 进程 —— 逐项调用 ≈ 数百 ms/项，是 936 项
///    批量删除耗时 ≈ 8.5 分钟（≈0.55s/项）的**主因**；原生 `NSFileManager.trashItem`
///    实测仅 **0.9 ms/项**。故这里改为攒批后 `trash::delete_all(paths)`（一次调用），
///    既保留 Finder 语义（「放回原处」可用、声音一次），又把进程 spawn 次数从 n 降到 n/200。
/// 2. `.str.toml` 的解析/重写是 O(bundle 内容)，逐项做即 O(n²) 写放大。
///
/// 崩溃安全：每 `FLUSH_EVERY` 项落盘一次；废纸篓成功后才撤登记（失败则保留登记）。
struct WriteSession {
    bundle: Bundle,
    dir: PathBuf,
    meta: Meta,
    /// 距上次落盘的累计变更数。
    dirty: usize,
    /// 已排队待移入废纸篓的条目相对路径（`commit_trash` 后清空）。
    trash_queue: Vec<String>,
}

impl WriteSession {
    fn open(dir: &Path) -> Result<Self, String> {
        let bundle = Bundle::new(dir.to_path_buf()).map_err(|e| e.to_string())?;
        let meta = read_meta(&bundle, dir)?;
        Ok(Self {
            bundle,
            dir: dir.to_path_buf(),
            meta,
            dirty: 0,
            trash_queue: Vec::new(),
        })
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.dirty == 0 {
            return Ok(());
        }
        self.meta.touch();
        self.meta
            .save(&self.bundle.meta_path(&self.dir))
            .map_err(|e| e.to_string())?;
        self.dirty = 0;
        Ok(())
    }

    /// 累计到阈值才真正落盘（`FLUSH_EVERY` 项一次）。
    fn maybe_flush(&mut self) -> Result<(), String> {
        if self.dirty >= FLUSH_EVERY {
            self.flush()?;
        }
        Ok(())
    }

    /// 把条目排入删除队列；队列满即合并成**一次**废纸篓调用。
    fn queue_trash(&mut self, id: &str) -> Result<(), String> {
        self.trash_queue.push(id.to_string());
        if self.trash_queue.len() >= TRASH_BATCH {
            self.commit_trash()?;
        }
        Ok(())
    }

    /// 一次把队列里的条目交给废纸篓；**成功后才撤登记**（失败则登记保留，
    /// 磁盘与 `.str.toml` 不会不一致），随后按需落盘。
    fn commit_trash(&mut self) -> Result<(), String> {
        if self.trash_queue.is_empty() {
            return Ok(());
        }
        let paths: Vec<PathBuf> = self
            .trash_queue
            .iter()
            .map(|p| self.dir.join(p))
            .filter(|p| p.exists())
            .collect();
        trash_delete_all(&paths)?;
        let queued = std::mem::take(&mut self.trash_queue);
        for p in queued {
            self.meta.remove_entry_path(&p);
            self.dirty += 1;
        }
        self.maybe_flush()
    }
}

/// 每多少项合并一次废纸篓调用（一次原生多路径调用）。
const TRASH_BATCH: usize = 200;

/// 「小批量」阈值：不超过它时优先走 Finder 方式，以保留废纸篓的「放回原处」。
/// 超过它（或 Finder 调用失败）则走原生 `NSFileManager`（快且大批量可靠）。
const TRASH_FINDER_MAX: usize = 20;

/// 把多个路径**一次**交给废纸篓。
///
/// macOS 必须用 `DeleteMethod::NsFileManager`（原生 `NSFileManager.trashItem`）：
/// `trash` 5.x 的默认 `DeleteMethod::Finder` 走 `osascript` 调 Finder，**大批量路径
/// 不可靠**（真机实测 200 路径/次会整批报 `Error during a 'trash' operation:
/// Os { code: 1, description: "The AppleScri…" }`；与本项目此前「`osascript -e` 多段
/// 形式丢尾部参数」是同一类问题），且每次调用 spawn 一个进程 ≈ 数百 ms/次。
/// 原生方法实测 **0.9 ms/项**，一次调用可带任意多路径。
///
/// ⚠️ 代价：Finder 方式才会在废纸篓里提供「放回原处」；原生方式不保证该菜单项。
fn trash_delete_all(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        // 分档（方案 B，真机验证过的折中）：
        // - **小批量**（≤ `TRASH_FINDER_MAX`）先试 Finder 方式：只有它才会在废纸篓里
        //   提供「放回原处」，而小规模 AppleScript 是可靠的；
        // - **大批量**、或小批量 Finder 失败 → 原生 `NSFileManager`（一次调用带全部路径，
        //   实测 0.9 ms/项；代价是「放回原处」不保证）。
        if paths.len() <= TRASH_FINDER_MAX {
            let mut ctx = trash::TrashContext::default();
            ctx.set_delete_method(DeleteMethod::Finder);
            match ctx.delete_all(paths.iter()) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    if debug_on() {
                        eprintln!("[str-gui] Finder 方式移入废纸篓失败，降级原生：{err}");
                    }
                }
            }
        }
        // Finder 失败可能**已经删掉一部分**，降级前按存在性过滤：否则对已入废纸篓的路径
        // 再次调用会让整批报错，进而让「已删文件」仍留在登记里（磁盘/`.str.toml` 不一致）。
        let rest: Vec<&PathBuf> = paths.iter().filter(|p| p.exists()).collect();
        if rest.is_empty() {
            return Ok(());
        }
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        return ctx.delete_all(rest).map_err(|e| e.to_string());
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete_all(paths.iter()).map_err(|e| e.to_string())
    }
}

/// 每多少项把内存里的登记变更落盘一次。
const FLUSH_EVERY: usize = 200;

impl ContentOp {
    /// `index` = 项在批量清单中的下标（粘贴按序取剪贴板项）；`id` = 清单 id。
    /// 成功返回 (提示文案, 落地后的登记路径)。
    ///
    /// `session`：Trash / Rename 复用同一个 `WriteSession`（整批一次 `Bundle::new`，
    /// `.str.toml` 内存累积、按阈值与收尾落盘）；Paste 走 `apply_clip_at` 自管读写，忽略之。
    fn run_with(
        &self,
        session: &mut Option<WriteSession>,
        index: usize,
        id: &str,
    ) -> Result<(String, String), String> {
        match self {
            ContentOp::Trash { branch_dir } => {
                if session.is_none() {
                    *session = Some(WriteSession::open(branch_dir)?);
                }
                let s = session.as_mut().expect("session 刚被填充");
                // 只入队：真正的废纸篓调用按 `TRASH_BATCH` 合并（见 `commit_trash`），
                // 避免逐项 spawn `osascript` —— 真机实测这才是 ~0.5s/项 的主因。
                s.queue_trash(id)?;
                Ok((String::new(), String::new()))
            }
            ContentOp::Rename {
                branch_dir,
                renames,
            } => {
                let new_name = renames
                    .iter()
                    .find(|(old, _)| old == id)
                    .map(|(_, new)| new.clone())
                    .ok_or_else(|| format!("重命名映射缺失：{id}"))?;
                std::fs::rename(branch_dir.join(id), branch_dir.join(&new_name))
                    .map_err(|e| e.to_string())?;
                if session.is_none() {
                    *session = Some(WriteSession::open(branch_dir)?);
                }
                let s = session.as_mut().expect("session 刚被填充");
                let entry = s
                    .meta
                    .entries
                    .iter()
                    .find(|x| x.path == id)
                    .cloned()
                    .ok_or("未找到条目")?;
                s.meta.remove_entry_path(id);
                let mut ne = entry;
                ne.path = new_name.clone();
                s.meta.upsert_entry(&ne);
                s.maybe_flush()?;
                Ok((String::new(), new_name))
            }
            ContentOp::Paste {
                clips,
                bundle_root,
                visit_dir,
                depth,
                id_version,
                into_folder,
            } => {
                let clip = clips.get(index).ok_or("粘贴项不存在")?;
                let bundle = Bundle::new(bundle_root.clone()).map_err(|e| e.to_string())?;
                if let Some(rel) = into_folder {
                    // 内容文件夹目标：**不得**对文件夹做 read_meta+save（会凭空把它
                    // 变成分支）—— 仅文件系统落地，count 由 batch_finished 统一维护。
                    let folder_fs = visit_dir.join(rel);
                    if !folder_fs.is_dir() {
                        return Err(format!("目标文件夹不存在：{rel}"));
                    }
                    let existing: HashSet<String> = std::fs::read_dir(&folder_fs)
                        .map(|rd| {
                            rd.filter_map(|x| x.ok())
                                .map(|x| x.file_name().to_string_lossy().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let name = dedup_name(&existing, &basename(&clip.name));
                    let dst = folder_fs.join(&name);
                    if clip.is_cut {
                        move_file(&clip.src_path, &dst)?;
                        // 源若是**分支**（有 .str.toml）撤登记；内容文件夹子行本就不登记。
                        if let Some(sp) = clip.src_path.parent() {
                            if sp.starts_with(bundle_root) && sp.join(".str.toml").exists() {
                                let mut sm = read_meta(&bundle, sp)?;
                                sm.remove_entry_path(&clip.name);
                                sm.touch();
                                sm.save(&bundle.meta_path(sp)).map_err(|e| e.to_string())?;
                            }
                        }
                    } else if clip.role == "dir" {
                        copy_dir_recursive(&clip.src_path, &dst)?;
                    } else {
                        std::fs::copy(&clip.src_path, &dst).map_err(|e| e.to_string())?;
                    }
                    let path = format!("{rel}/{name}");
                    return Ok((
                        format!(
                            "已{} {name}（→ {path}）",
                            if clip.is_cut { "移动" } else { "粘贴" }
                        ),
                        path,
                    ));
                }
                let (msg, registered) =
                    apply_clip_at(&bundle, visit_dir, *depth, *id_version, clip)?;
                Ok((format!("{msg}（→ {registered}）"), registered))
            }
        }
    }
}

/// 后台线程执行批量操作：顺序逐项（`.str.toml` 登记写回需要互斥），
/// 逐项推进进度（`invoke_from_event_loop` **直写状态栏**，不再有模态进度浮层），
/// 结束后把结果写入 `results_slot`，仅在**有失败**时弹汇总对话框并触发 rescan。
///
/// `summary_title` = 汇总对话框标题（同时用作状态栏成功摘要的判定依据）；
/// `progress_label` = 进度前缀（如 `批量删除中`），与标题分开以保持进度文本可读。
///
/// 线程只持有路径与弱 UI 句柄（不碰 `Rc<RefCell<Editor>>`）；
/// 完成后的重扫 / 模型同步经 `invoke_batch_finished()` 回到主线程完成。
fn spawn_gui_batch(
    app_weak: Weak<AppWindow>,
    results_slot: std::sync::Arc<std::sync::Mutex<(ContentOp, Vec<BatchOutcome>)>>,
    summary_title: &str,
    progress_label: &str,
    items: Vec<(String, String)>,
) {
    let title = summary_title.to_string();
    let label = progress_label.to_string();
    std::thread::spawn(move || {
        let op = match results_slot.lock() {
            Ok(slot) => slot.0.clone(),
            Err(_) => return,
        };
        let total = items.len();
        let mut results: Vec<BatchOutcome> = Vec::with_capacity(total);
        // 整批共用的写会话（Trash / Rename 用；Paste 自管读写）。
        let mut session: Option<WriteSession> = None;
        for (i, (id, t)) in items.iter().enumerate() {
            {
                let w = app_weak.clone();
                let label = label.clone();
                let text = format!("{label} {n}/{total} · {t}", n = i + 1);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(a) = w.upgrade() {
                        // 直写状态栏（不走 `show_status`：它是 `main` 内的嵌套 fn，
                        // 后台线程不可见）。终态由 `on_batch_finished` 用 `show_status`
                        // 收尾 —— 那次调用会重启 6s 定时器，因此进度文本最终会被清空。
                        a.set_status(SharedString::from(text));
                    }
                });
            }
            match op.run_with(&mut session, i, id) {
                Ok((msg, new_path)) => results.push(BatchOutcome {
                    id: id.clone(),
                    title: t.clone(),
                    status: "✓",
                    msg,
                    new_path,
                }),
                Err(msg) => results.push(BatchOutcome {
                    id: id.clone(),
                    title: t.clone(),
                    status: "✗",
                    msg,
                    new_path: String::new(),
                }),
            }
        }
        // 收尾：① 把队列里剩余条目**一次**交给废纸篓；② 把内存里的登记变更一次写盘。
        // 任一失败都补一条失败结果（会进汇总对话框与「重试失败项」，不会被静默吞掉）。
        if let Some(mut s) = session.take() {
            let finish = s.commit_trash().and_then(|()| s.flush());
            if let Err(msg) = finish {
                results.push(BatchOutcome {
                    id: String::new(),
                    title: "删除 / 登记写回".to_string(),
                    status: "✗",
                    msg,
                    new_path: String::new(),
                });
            }
        }
        let failed = results.iter().filter(|r| r.status == "✗").count();
        if let Ok(mut slot) = results_slot.lock() {
            slot.1 = results;
        }
        let w = app_weak.clone();
        let slot = std::sync::Arc::clone(&results_slot);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(a) = w.upgrade() {
                let rows: Vec<BatchResultRow> = slot
                    .lock()
                    .map(|v| {
                        v.1.iter()
                            .map(|r| BatchResultRow {
                                status: r.status.into(),
                                title: r.title.clone().into(),
                                message: r.msg.clone().into(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                a.set_summary_title(title.into());
                a.set_summary_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
                a.set_summary_has_failed(failed > 0);
                a.set_batch_busy(false);
                // **全部成功不再弹汇总框**（成功不打扰）：改由状态栏给一句摘要
                // （见 `on_batch_finished`）；有失败才弹——失败框承载状态栏放不下的
                // 「逐项原因」与状态栏点不到的「重试失败项」。
                a.set_summary_visible(failed > 0);
                a.invoke_batch_finished();
            }
        });
    });
}

/// 收集多选条目（path, 显示名）：先按当前分支的**可视行顺序**，再补上选区里
/// 此刻不可见（所属文件夹被折叠）但**仍在磁盘上**的路径。
///
/// 少了这步，「选区计数」（`entry_multi.len()`，UI 直接显示）会大于「批量动作
/// 实际处理条数」（此前只遍历可视行）—— 用户报告的「批量操作数量不正确」。
/// 补入的路径照旧按 rescan 的同一判据（存在性）过滤，与 `entry_multi` 的剪除规则一致。
fn collect_entry_multi(e: &Editor) -> Vec<(String, String)> {
    let rows = e
        .selected_idx_in_visits()
        .map(|vi| build_entry_rows_for(e, vi))
        .unwrap_or_default();
    let mut out: Vec<(String, String)> = rows
        .iter()
        .filter(|v| entry_multi_eligible(v))
        .filter(|v| e.entry_multi.contains(&v.path))
        .map(|v| (v.path.clone(), basename(&v.path)))
        .collect();
    let dir = e.selected_visit().map(|v| v.dir.clone());
    let mut rest: Vec<String> = e
        .entry_multi
        .iter()
        .filter(|p| !out.iter().any(|(q, _)| q == *p))
        .filter(|p| dir.as_ref().is_some_and(|d| d.join(p).exists()))
        .cloned()
        .collect();
    rest.sort();
    out.extend(rest.into_iter().map(|p| {
        let display = basename(&p);
        (p, display)
    }));
    out
}

/// 当前拖拽组在**可视行序**里是否连续（选区行中间不夹未选中行）。
///
/// 连续组（含单条）凡与组相接的插入边界 —— 组前、组内、组后紧邻 —— 都是顺序
/// 不变的无效落点，Slint 侧据此抑制插入指示线（`DndApi.src-contig`，拖拽开始
/// 时经 `src-contig-for` 求值）；非连续组任何落点都会让其余成员移动补位，全部
/// 放行（仅可视上拼在一起的段内部无效，由 Slint 按 `in-multi` 相邻自行判定）。
/// 被抓行不在选区内（单条拖拽）视为连续，由调用方先行判断。
fn entry_multi_contiguous(e: &Editor) -> bool {
    let rows = e
        .selected_idx_in_visits()
        .map(|vi| build_entry_rows_for(e, vi))
        .unwrap_or_default();
    let idxs: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, v)| e.entry_multi.contains(&v.path))
        .map(|(i, _)| i)
        .collect();
    idxs.windows(2).all(|w| w[1] == w[0] + 1)
}

/// 当前拖拽组是否含**未登记**成员（内容文件夹子行 / 未登记顶层项）。
///
/// 判据与 `on_drop_row` 整组落点的 `has_unregistered` **完全同源**（都取
/// `collect_entry_multi` = 载荷路径来源、都对照 `meta.entries`）：有成员不在
/// 登记清单 ⇒ 「重排」不可表达，落点退化为移入目录。Slint 侧据此（`DndApi.
/// src-mixed`）把文件夹行**上半区**的指示从插入线切换为整行移入高亮，使提示
/// 与实际落点一致；单条拖拽（被抓行不在选区内）恒为 false。
fn entry_multi_has_unregistered(e: &Editor) -> bool {
    let registered: HashSet<&str> = e
        .selected_visit()
        .and_then(|v| v.meta.as_ref())
        .map(|m| m.entries.iter().map(|en| en.path.as_str()).collect())
        .unwrap_or_default();
    collect_entry_multi(e)
        .iter()
        .any(|(p, _)| !registered.contains(p.as_str()))
}

/// 可进入批量选区的行：**分支行以外的一切行** —— 顶层登记条目 + 子目录里的行。
/// （子目录文件此前被 `depth == 0` 挡在选区外，用户报告「子目录中的文件无法多选」。）
/// 分支行（`node` / `branch`）在结构树里呈现，不参与内容批量；未登记的**顶层**项保持
/// 原语义（不参与批量，避免与登记项的操作路径分叉）。
/// 选择 / 高亮 / 操作三方必须共用同一判定，否则会出现「高亮了却操作不到」。
fn entry_multi_eligible(v: &VisibleEntry) -> bool {
    if matches!(v.role.as_str(), "node" | "branch") {
        return false;
    }
    v.depth > 0 || v.entries_idx.is_some()
}

/// 全部分支的直接子分支索引：`index[i]` = 分支 i 的子分支，按父级 `entries[]`
/// 的登记顺序（目录名兜底）排列。
///
/// 扫描后建**一次**，之后各处按索引取用。早期实现是「每次调用都全量过滤
/// visits」的 `ordered_children`，而它被行构建、导图布局按节点逐个调用 ——
/// 两处因此都退化成 O(N²)（2441 节点实测 debug：rebuild 149ms、layout 366ms，
/// 而 visits 本身只翻 3 倍，耗时翻了约 8 倍）。
fn children_index(scan: &Scan) -> Vec<Vec<usize>> {
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); scan.visits.len()];
    for (i, v) in scan.visits.iter().enumerate() {
        if let Some(p) = v.parent {
            if let Some(bucket) = kids.get_mut(p) {
                bucket.push(i);
            }
        }
    }
    for (p, bucket) in kids.iter_mut().enumerate() {
        let order: Vec<&str> = scan.visits[p]
            .meta
            .as_ref()
            .map(|m| {
                m.entries
                    .iter()
                    .filter(|e| e.is_branch())
                    .map(|e| e.path.as_str())
                    .collect()
            })
            .unwrap_or_default();
        if order.is_empty() {
            continue;
        }
        bucket.sort_by_key(|&c| {
            let name = scan.visits[c]
                .dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            order
                .iter()
                .position(|p| *p == name)
                .unwrap_or(order.len() + c)
        });
    }
    kids
}

/// 挂载索引（规范 §4.6.1）：父分支 visit 下标 → 其挂载的子分支，按 `entries[]`
/// 顺序排列。目标经 `Scan::resolve` 解析；解析不到（悬空）、目标是 ROOT、或 id
/// 重复（无法唯一指向）的挂载一律跳过 —— 校验器会另行报错。
/// 硬链接同样进本索引（表现与软链接一致，身份 = 目标）；`link_idx` = 自身分支
/// visit（自有子结构的挂载点）。目标悬空的硬链接降级为 `visit = link_idx`
/// （显示自身 meta，内容为空），避免整行消失。
fn mount_index(scan: &Scan) -> Vec<Vec<MountChild>> {
    let mut out: Vec<Vec<MountChild>> = vec![Vec::new(); scan.visits.len()];
    for (i, v) in scan.visits.iter().enumerate() {
        let Some(meta) = v.meta.as_ref() else {
            continue;
        };
        for e in &meta.entries {
            if !e.is_link() {
                continue;
            }
            let hard = e.mode.as_deref() == Some("hard");
            let Some(t) = e.target.as_deref() else {
                continue;
            };
            let Some(dst) = scan.resolve(t) else {
                continue;
            };
            if scan.visits[dst].depth == 0 {
                continue;
            }
            // 硬链接自身分支 visit（path = 自身 id）；目录缺失（ghost）时跳过。
            let link_idx = if hard {
                match scan.resolve(&e.path) {
                    Some(li) => Some(li),
                    None => continue,
                }
            } else {
                None
            };
            if let Some(bucket) = out.get_mut(i) {
                bucket.push(MountChild {
                    visit: dst,
                    title: e.title.clone(),
                    hard,
                    link_idx,
                });
            }
        }
    }
    out
}

/// 软链接成环预检（与 CLI `link_add` / 校验器 `check_link_cycles` 同源）。
///
/// 图的边集 = **真实父子边**（父 → 子）∪ **挂载边**（挂载点 → 目标）。沿这个图从
/// 目标出发能走到自己 ⇒ 渲染会无限递归。注意「挂进祖先」正因此而按环拒绝
/// （祖先 → 自己是真实边），而「挂进自己的后代」不是环（DAG 合法，子树多处可见）。
fn link_would_cycle(scan: &Scan, src: usize, dst: usize) -> bool {
    if src == dst {
        return true;
    }
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); scan.visits.len()];
    for (i, v) in scan.visits.iter().enumerate() {
        if let Some(p) = v.parent {
            if let Some(b) = out_edges.get_mut(p) {
                b.push(i);
            }
        }
        if let Some(meta) = v.meta.as_ref() {
            for e in &meta.entries {
                if !e.is_link() {
                    continue;
                }
                if let Some(t) = &e.target {
                    if let Some(j) = scan.resolve(t) {
                        if let Some(b) = out_edges.get_mut(i) {
                            b.push(j);
                        }
                    }
                }
            }
        }
    }
    let mut stack = vec![dst];
    let mut seen: HashSet<usize> = HashSet::new();
    while let Some(i) = stack.pop() {
        if i == src {
            return true;
        }
        if !seen.insert(i) {
            continue;
        }
        if let Some(next) = out_edges.get(i) {
            stack.extend(next.iter().copied());
        }
    }
    false
}

/// 挂载 / 关联目标的**禁选集合**（visit 下标）：ROOT、id 重复或缺失者。
/// 模式 `0`/`1`（软 / 硬挂载）再排除自身与祖先（挂进祖先 = 环）；硬链接目标
/// 不限后代（1.15.0 撤回「不得挂载自己的后代」：硬链接不渲染目标结构、内容
/// 只读，目标位于挂载点子树内不产生自嵌套）。模式 `2`（关联线 `refs[]`）只排除
/// 自身——自关联非法（规范 §4.5），指向祖先 / 后代均合法（环判定只沿 `refs`
/// 边，由校验器 `E_REF_CYCLE` 兜底）。
fn mount_banned(e: &Editor, mode: i32) -> HashSet<usize> {
    let Some(scan) = e.scan.as_ref() else {
        return HashSet::new();
    };
    let mut banned: HashSet<usize> = HashSet::new();
    if let Some(src_idx) = e.selected_idx_in_visits() {
        banned.insert(src_idx);
        if mode != 2 {
            banned.extend(scan.ancestors(src_idx));
        }
    }
    if let Some(root) = scan.root_index {
        banned.insert(root);
    }
    // id 重复（Finder 复制所致，E_ID_DUP）/ 缺 id 的分支无法唯一指向，一律禁选。
    for (i, v) in scan.visits.iter().enumerate() {
        let dup = v
            .meta
            .as_ref()
            .and_then(|m| m.id.as_deref())
            .map(|id| scan.by_id.get(id).map(|h| h.len()).unwrap_or(0) != 1)
            .unwrap_or(true);
        if dup {
            banned.insert(i);
        }
    }
    banned
}

/// 挂载对话框的**逐行可选标记**（与选择器行模型平行，`sync_ui` 填进
/// `BranchRow.pickable`）：候选 = 选择器可见行 ∩ 非禁选。
/// `mode`：0 = 软链接、1 = 硬链接、2 = 关联线（refs）。
fn mount_pickables(e: &Editor, mode: i32, rows: &[VisibleRow]) -> Vec<bool> {
    if e.selected_idx_in_visits().is_none() {
        return vec![false; rows.len()];
    }
    let banned = mount_banned(e, mode);
    rows.iter().map(|r| !banned.contains(&r.visit)).collect()
}

/// `VisibleRow` → Slint `BranchRow`（树与挂载选择器共用同一种行渲染）。
/// `pickable` 只有选择器行有意义（树行恒 false，无人读取）。
/// 结构树一行的**内容自然像素宽**（横向滚动用）：基础内缩 8px + 每级缩进
/// 14px + 箭头列 10px + 间距 4px + 标题 + 4px + 类型副列 + 右内边距 10px，
/// 另加 6px 余量。标题 / 类型按字体字号估算（同 est_node_w 的口径）。
fn row_content_px(depth: usize, title: &str, type_str: &str) -> f32 {
    8.0 + depth as f32 * 14.0
        + 10.0
        + 4.0
        + est_text_w(title, 12.0, 6.5)
        + 6.0
        + 4.0
        + est_text_w(type_str, 10.0, 6.0)
        + 10.0
        + 6.0
}

fn branch_row_of(
    e: &Editor,
    r: &VisibleRow,
    pickable: bool,
    ref_marks: &std::collections::HashSet<usize>,
    spine_marks: &std::collections::HashSet<usize>,
) -> BranchRow {
    BranchRow {
        visit: r.visit as i32,
        title: r.title.clone().into(),
        latin: is_latin_only(&r.title),
        type_str: r.type_str.clone().into(),
        depth: r.depth as i32,
        kind: e
            .scan
            .as_ref()
            .and_then(|s| s.visits.get(r.visit))
            .and_then(|v| v.meta.as_ref())
            .and_then(|m| m.kind.as_ref())
            .map(|k| SharedString::from(k.as_str()))
            .unwrap_or("—".into()),
        is_selected: e
            .selected
            .as_deref()
            .zip(e.scan.as_ref())
            .is_some_and(|(key, scan)| visit_key(scan, r.visit) == key),
        ref_mark: ref_marks.contains(&r.visit),
        parent_mark: !ref_marks.contains(&r.visit) && spine_marks.contains(&r.visit),
        expanded: r.expanded,
        has_children: r.has_children,
        has_entries: r.has_entries,
        mounted: r.mounted,
        hard: r.hard,
        mount_parent: r.mount_parent.map(|p| p as i32).unwrap_or(-1),
        link_visit: r.link_visit.map(|p| p as i32).unwrap_or(-1),
        expand_key: r.expand_key.clone().into(),
        content_px: row_content_px(r.depth, &r.title, &r.type_str),
        pickable,
    }
}

/// 收缩 `visit` 分支时的状态清理：递归收集子树内所有下级分支，移除它们的
/// 分支展开与内容展开状态；激活态（选中分支 / 条目选中）位于子树内时一并移除。
fn collapse_subtree_state(e: &mut Editor, visit: usize) {
    let Some(scan) = e.scan.as_ref() else {
        return;
    };
    let mut stack = vec![visit];
    let mut subtree_keys: Vec<String> = Vec::new();
    let mut selected_in_subtree = false;
    while let Some(i) = stack.pop() {
        if e.selected.as_deref() == Some(visit_key(scan, i)) {
            selected_in_subtree = true;
        }
        for &c in e.kids.get(i).map(|v| v.as_slice()).unwrap_or_default() {
            stack.push(c);
            subtree_keys.push(visit_key(scan, c).to_string());
        }
    }
    for key in &subtree_keys {
        e.expanded.remove(key);
        e.content_expanded.remove(key);
    }
    if selected_in_subtree {
        // 收回的子树包含选中分支：若直接清空选中，树里没有任何行高亮、
        // 右侧栏也清空——焦点凭空丢失。改为选中**收回的分支本身**（它仍可见，
        // Finder/VSCode 同语义：收起父级后选中落在父级上），并清空内容选区。
        e.selected = Some(visit_key(scan, visit).to_string());
        e.selected_entry_path = None;
        e.entry_multi.clear();
        e.entry_anchor = None;
    }
}

/// 内容视图来源：普通分支 = 自身；硬链接分支（§4.6.1 `mode = "hard"`）= `target`
/// 指向的内容来源分支（只读视图）。目标悬空（校验器报 `E_LINK_NO_TARGET`）时
/// 返回 `None`，调用方按空内容处理。
fn content_visit<'a>(scan: &'a Scan, visit: &'a Visit) -> Option<&'a Visit> {
    match &visit.hard_link_to {
        Some(t) => scan.resolve(t).and_then(|i| scan.visits.get(i)),
        None => Some(visit),
    }
}

/// 选中经由**硬链接**行 / 节点进入（`sel_hard`）时，解析该硬链接自身分支的
/// visit（挂载等结构写入的落点）：在挂载索引里找指向当前选中（目标）分支的
/// 硬链接挂载。返回：
/// - `Some(idx)` —— 恰有一条硬链接挂载指向该目标，`idx` = 自身分支 visit；
/// - `None` —— 找不到（选中态与来源行不一致，如残留状态）：调用方按普通
///   分支落点处理；
/// - `Err` —— 同一目标被多处硬挂载，无法判定来源行（拒绝执行，提示改用
///   硬链接行右键菜单，那里带精确落点）。
fn selected_hard_link_idx(
    e: &Editor,
    selected: usize,
) -> Result<Option<usize>, String> {
    let mut hits: Vec<usize> = Vec::new();
    for ms in &e.mounts {
        for m in ms {
            if m.hard && m.visit == selected {
                if let Some(link_idx) = m.link_idx {
                    hits.push(link_idx);
                }
            }
        }
    }
    match hits.as_slice() {
        [only] => Ok(Some(*only)),
        [] => Ok(None),
        _ => Err(
            "同一目标被多处硬挂载，无法判定落点：请在目标硬链接行上右键选择「挂载已有分支…」"
                .into(),
        ),
    }
}

/// 摘除一条软挂载（规范 §4.6.1 规则 6：只摘引用）：从 parent 分支的 `entries[]`
/// 移除指向 target 的 `role = "link"` 行并落盘、rescan。目标分支与数据不动。
/// 返回 (挂载点标题, 目标标题)。右键「移除挂载」与删除入口对挂载选中态的改道
/// （本会话修复：菜单栏 ⌘⇧⌫ / 信息页「删除分支」在挂载行选中态下误删目标）
/// 共用此实现。
fn unmount_link(
    e: &mut Editor,
    parent_idx: usize,
    target_idx: usize,
) -> Result<(String, String), String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
    let scan = e.scan.as_ref().ok_or("未选择分支")?;
    let target_id = scan.visits[target_idx]
        .meta
        .as_ref()
        .and_then(|m| m.id.clone())
        .ok_or("目标分支缺少 id")?;
    let parent = &scan.visits[parent_idx];
    let parent_title = visit_title(parent);
    let dst_title = visit_title(&scan.visits[target_idx]);
    let mut meta = read_meta(bundle, &parent.dir)?;
    if !meta
        .entries
        .iter()
        .any(|en| en.is_link() && en.path == target_id)
    {
        return Err(format!("「{parent_title}」下没有指向「{dst_title}」的挂载"));
    }
    meta.remove_entry_path(&target_id);
    meta.touch();
    meta.save(&bundle.meta_path(&parent.dir))
        .map_err(|err| err.to_string())?;
    e.rescan()?;
    Ok((parent_title, dst_title))
}

/// 分支是否存在**内容条目**：排除子分支与链接声明行（链接是结构，已在树 /
/// 导图以「⤷ / ≡」分支行呈现，不该把内容按钮点亮）。硬链接分支看**内容来源**
/// （目标）的条目。决定「展开 / 收起内容」可用性与导图内容按钮可见性。
fn has_content_entries(scan: &Scan, visit: &Visit) -> bool {
    content_visit(scan, visit)
        .and_then(|v| v.meta.as_ref())
        .map(|m| m.entries.iter().any(|en| !en.is_branch() && !en.is_link()))
        .unwrap_or(false)
}

/// 分支身份键 = `Visit::rel`（bundle 内相对路径）。
///
/// **不用 `meta.id`**：id 可被复制 —— 在 Finder 里把分支目录复制到别处（⌥ 拖拽）时
/// `.str.toml` 一并被复制，于是两个**同名**分支共享同一 id（规范侧由 `E_ID_DUP` 报错）。
/// 这时任何 `id → 下标` 的解析都落到「首次命中」：点第二个同名分支会激活第一个，
/// 且按 id 比较的选中判定会把两个同名节点**同时点亮** —— 即「同名会产生激活意外」。
/// `rel` 由目录结构决定、天然唯一且跨 rescan 稳定，故 GUI 内一切分支记账（选中 /
/// 展开集合 / 内容展开集合）都用它；CLI 侧 `id` 语义不变（它仍是规范里的标识符）。
fn visit_key<'a>(scan: &'a Scan, idx: usize) -> &'a str {
    scan.visits.get(idx).map(|v| v.rel.as_str()).unwrap_or("")
}

/// `visit_key` → `Scan::visits` 下标（随 scan 重建，供身份键 O(1) 反查）。
fn rel_index(scan: &Scan) -> HashMap<String, usize> {
    scan.visits
        .iter()
        .enumerate()
        .map(|(i, v)| (v.rel.clone(), i))
        .collect()
}

/// 行的呈现形态：真实位置，或挂载进来的视图（软链接 / 硬链接）。
enum RowDisplay {
    Real,
    /// 挂载视图（软 / 硬）：身份 = 目标分支（点行选中真身、展开态与真身共享）。
    /// 硬链接（`hard = true`）额外在自身之下渲染**自有子分支**（`link_idx` =
    /// 自身分支 visit，`path = id` 的真实分支），新建子分支落自有结构。
    Mounted { alias: Option<String>, parent: usize, hard: bool, link_idx: Option<usize> },
}

/// 递归渲染一个分支及其子树（真实孩子 + 链接挂载的孩子）。
///
/// 软链接挂载行（`mounted = true`）的身份 = 目标分支（`visit` 即目标下标，点行 =
/// 选中真身，symlink 语义），渲染目标的整棵子树；硬链接行（`hard = true`）**仅内容
/// 关联**：呈现为带「≡」识别标识的可展开真实分支（不渲染目标的子分支、内容只读）。
/// 两种挂载行的展开态 / 身份都按目标分支的 rel 记账，与真实位置共享。
/// **防环守卫**：`path` 是当前渲染链上的分支集合 —— 软链接目标已在链上（挂进自己的
/// 祖先 / 自身 / 互相挂载）就不再展开，否则无限递归（校验器 `E_LINK_CYCLE` 会报，
/// 但 GUI 必须能安全打开**非法** bundle）。
fn dfs_rows(
    scan: &Scan,
    kids: &[Vec<usize>],
    mounts: &[Vec<MountChild>],
    idx: usize,
    expanded: &HashSet<String>,
    out: &mut Vec<VisibleRow>,
) {
    let mut path: Vec<usize> = Vec::new();
    emit_branch(
        scan,
        kids,
        mounts,
        idx,
        scan.visits[idx].depth,
        RowDisplay::Real,
        expanded,
        "",
        &mut path,
        out,
    );
}

/// 实例上下文键组合：`prefix` 为空 = 真实上下文（键原样）；非空 = 挂载实例
/// 上下文，键 = `{实例键}␟{基础键}` —— 实例子树内的展开态整体独立于真身与
/// 其它实例（回归：挂载行展开曾连带显示真身视图已展开的子孙）。
fn compose_key(prefix: &str, base: String) -> String {
    if prefix.is_empty() {
        base
    } else {
        format!("{prefix}\u{1f}{base}")
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_branch(
    scan: &Scan,
    kids: &[Vec<usize>],
    mounts: &[Vec<MountChild>],
    idx: usize,
    depth: usize,
    display: RowDisplay,
    expanded: &HashSet<String>,
    prefix: &str,
    path: &mut Vec<usize>,
    out: &mut Vec<VisibleRow>,
) {
    // 全局预算（见 TREE_ROW_CAP）。
    if out.len() >= TREE_ROW_CAP {
        return;
    }
    let visit = &scan.visits[idx];
    let children: &[usize] = kids.get(idx).map(|v| v.as_slice()).unwrap_or_default();
    let mounted_list: &[MountChild] =
        mounts.get(idx).map(|v| v.as_slice()).unwrap_or_default();
    // 硬链接分支（§4.6.1 `mode = "hard"`）：`hard_link_to` 标记其内容视图语义 ——
    // 有自己的身份与子分支（正常展开），内容面板显示目标内容且只读（`hard = true`
    // 仅供守卫与标识用）。软挂载行（Mounted）恒为软链接（hard 已由真实分支承担）。
    // 硬链接分支（§4.6.1 `mode = "hard"`）不以普通子分支行出现：它的行是**挂载行
    // 形态**（身份 = 目标，表现与软链接一致），由挂载父分支的递归经 mounts 渲染。
    if matches!(display, RowDisplay::Real) && visit.hard_link_to.is_some() {
        return;
    }
    let (mounted, hard, mount_parent, title, link_idx) = match &display {
        RowDisplay::Real => (false, false, None, visit_title(visit), None),
        // 挂载行：软「⤷」= 目标完整视图；硬「≡」= 内容引用 + 自有结构。
        // 身份都是目标 —— 点行选中真身、展开态与真身共享；前缀只在行标题上，
        // `visit_title`（真身标题，状态栏 / 对话框用）保持干净。
        RowDisplay::Mounted { alias, parent, hard, link_idx } => (
            // mounted 标记只给软链接（「移除挂载」走 unmount）；硬链接走
            // delete-branch + op-visit（摘引用 + 删自有目录）。
            !*hard,
            *hard,
            Some(*parent),
            if *hard {
                format!("≡ {}", alias.clone().unwrap_or_else(|| visit_title(visit)))
            } else {
                format!("⤷ {}", alias.clone().unwrap_or_else(|| visit_title(visit)))
            },
            *link_idx,
        ),
    };
    // 展开键：真实行 = 自身 rel（既有语义）；挂载行 = **实例键**（挂载点 ␟
    // mount[-hard] ␟ 目标）—— 展开收起只作用于本行，与真身及其它挂载视图
    // （含同挂载点指向同目标的软 / 硬链接对）互不关联。处于挂载实例上下文
    // （prefix 非空）时，键再组合实例前缀：实例子树内所有行的展开态都独立。
    let base_key = match &display {
        RowDisplay::Mounted { parent, hard, .. } => {
            mount_expand_key(scan, *parent, idx, *hard)
        }
        RowDisplay::Real => visit_key(scan, idx).to_string(),
    };
    let expand_key = compose_key(prefix, base_key);
    let (child_kids, child_mounts): (&[usize], &[MountChild]) = (children, mounted_list);
    // 硬链接行额外渲染**自有子分支**（link 分支磁盘目录下的真实子分支）。
    let own_children = link_idx
        .map(|li| {
            !kids.get(li).map(|v| v.is_empty()).unwrap_or(true)
                || !mounts.get(li).map(|v| v.is_empty()).unwrap_or(true)
        })
        .unwrap_or(false);
    let has_children = if hard {
        // 硬链接行只渲染自有结构：目标的分支结构不跟过来（规范 §4.6.1）。
        own_children
    } else {
        !child_kids.is_empty() || !child_mounts.is_empty() || own_children
    };
    out.push(VisibleRow {
        visit: idx,
        depth,
        title,
        type_str: visit_type(visit),
        expanded: expanded.contains(&expand_key),
        has_children,
        has_entries: has_content_entries(scan, visit),
        mounted,
        hard,
        mount_parent,
        link_visit: link_idx,
        expand_key: expand_key.clone().into(),
    });
    // ROOT 也受展开态控制：折叠根即隐藏全部一级分支。
    if !expanded.contains(&expand_key) {
        return;
    }
    // 递归上下文：挂载行 / 硬链接行把自身实例键作为子树的上下文前缀
    // （实例子树内所有行的展开键 = `{实例键}␟{行键}`，独立记账）；
    // 真实行延续当前上下文。
    let child_prefix: String = if mounted || hard {
        expand_key.clone()
    } else {
        prefix.to_string()
    };
    path.push(idx);
    // 硬链接行：**目标的分支结构不跟过来**——其子分支要操作请到目标分支；
    // 仅渲染自有结构（见下方 link_idx 段）。
    if !hard {
        for &c in child_kids {
            emit_branch(
                scan, kids, mounts, c, depth + 1, RowDisplay::Real, expanded, &child_prefix,
                path, out,
            );
        }
        for m in child_mounts {
            // 挂载目标已在当前渲染链上 ⇒ 环（或自我挂载）：跳过，不再展开。
            if path.contains(&m.visit) {
                continue;
            }
            emit_branch(
                scan,
                kids,
                mounts,
                m.visit,
                depth + 1,
                RowDisplay::Mounted {
                    alias: m.title.clone(),
                    parent: idx,
                    hard: m.hard,
                    link_idx: m.link_idx,
                },
                expanded,
                &child_prefix,
                path,
                out,
            );
        }
    }
    // 硬链接行：自有子分支渲染在目标子树之后（新建子分支落自有结构，在这里可见）；
    // 可见性跟随行本身（= 目标的展开键），与软链接同款。
    if let Some(li) = link_idx {
        path.push(li);
        for &c in kids.get(li).map(|v| v.as_slice()).unwrap_or_default() {
            emit_branch(
                scan, kids, mounts, c, depth + 1, RowDisplay::Real, expanded, &child_prefix,
                path, out,
            );
        }
        for m in mounts.get(li).map(|v| v.as_slice()).unwrap_or_default() {
            if path.contains(&m.visit) {
                continue;
            }
            emit_branch(
                scan,
                kids,
                mounts,
                m.visit,
                depth + 1,
                RowDisplay::Mounted {
                    alias: m.title.clone(),
                    parent: li,
                    hard: m.hard,
                    link_idx: m.link_idx,
                },
                expanded,
                &child_prefix,
                path,
                out,
            );
        }
        path.pop();
    }
    path.pop();
}

fn visit_title(visit: &Visit) -> String {
    visit
        .meta
        .as_ref()
        .and_then(|m| m.title.clone())
        .unwrap_or_else(|| "（无标题）".into())
}

/// 名称列表槽位的统一实现：**最多列 3 个**，超出以「…」收尾。
///
/// 状态栏是单行 + `overflow: elide`，全量列举会把「来源 / 目标」这类关键信息挤掉；
/// 单条（1 个名称）走 `「名称」`，多条（≥2）一律 `<N> 项：<最多 3 个名称…>`——
/// 这条规则同时适用于拷贝 / 剪切 / 移动 / 导入 / Finder 显示，避免「有的列名有的不列」。
fn name_list(names: &[String]) -> String {
    const MAX: usize = 3;
    if names.len() <= MAX {
        names.join("、")
    } else {
        format!("{}…", names[..MAX].join("、"))
    }
}

/// 按**分支目录**取分支标题：状态栏的「来源 → 目标」提示需要一个可读的分支名，
/// 而调用点往往只有 `Path`（如 `visit_dir` / `src_parent_dir`）。
/// 只读当前 scan（**不新增 IO**）；目录不在 scan 中时退回目录名（UUID 目录则退回空串）。
fn title_of_dir(e: &Editor, dir: &Path) -> String {
    // 目录名回退同时覆盖两种情形：① 目录不在 scan 中；② 分支**没有 `title`**
    // —— `visit_title` 对无标题分支返回「（无标题）」，直接写进状态栏很难看
    // （`str init` 未带 `--title` 的 bundle 根分支就会这样，真机验收实测到）。
    let by_name = || {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "根分支".to_string())
    };
    match e
        .scan
        .as_ref()
        .and_then(|s| s.visits.iter().find(|v| v.dir == dir))
    {
        Some(v) => {
            let title = visit_title(v);
            if title == "（无标题）" {
                by_name()
            } else {
                title
            }
        }
        None => by_name(),
    }
}

fn visit_type(visit: &Visit) -> String {
    visit
        .meta
        .as_ref()
        .map(|m| {
            m.r#type.clone().unwrap_or_else(|| {
                if m.kind == Some(Kind::Root) {
                    "root".into()
                } else {
                    String::new()
                }
            })
        })
        .unwrap_or_else(|| "（.str.toml 解析失败）".into())
}

// ── 思维导图布局：水平树 + 子树垂直带（band）模型 ──
// 每个子树预留 sub_h = max(自身高度, 子带跨度) 的垂直带；节点固定在自己带中心，
// 子带围绕该中心对称堆叠。内容展开使父节点变高时只会撑大自己所在的带，
// 数学上保证任何节点都不会越出带外与兄弟子树重叠；单子分支连线恒为水平直线。
struct MindOut {
    nodes: Vec<MindNode>,
    edges: Vec<MindEdge>,
    /// 小地图专用连线：缩略图不画外置子树按钮、无需为它让位，且竖线取间距的
    /// 1/3 处（左段短、右段长）——小尺寸下分叉点更靠近父节点，右侧分叉更易辨认。
    mini_edges: Vec<MindEdge>,
    /// 画布连线合并后的分块 Path（见 `canvas_edge_chunks`）。
    /// 逐条矩形连线在超大导图下是数千个 item，逐帧遍历/裁剪判定开销巨大；
    /// 合并成少量「列带」Path 后，屏幕外的整块由渲染器直接跳过。
    edge_chunks: Vec<MindEdgeChunk>,
    /// 小地图连线：整图一条 Path（缩略图必须显示全图，无需分块）。
    mini_edge_chunks: Vec<MindEdgeChunk>,
    /// refs 关联线（§4.5）：虚线 + 标签，逐条渲染（数量级远小于父子边）。
    /// 只画两端都在当前布局里的关联 —— 折叠 / 离屏实例没有坐标可锚。
    ref_edges: Vec<RefEdge>,
    w: f32,
    h: f32,
    /// 画布右缘为「外置子树按钮」预留的横向占位（有子分支的节点才留，否则 0）。
    /// 小地图不画按钮，故面板宽度与缩略比例都按 `w - edge_offset` 计算——
    /// 二者同源才能保证缩略内容正好落在面板留白之内（否则右侧会溢出面板）。
    edge_offset: f32,
}

/// 生成一条肘形连线的三段（水平 → 竖直 → 水平）。
/// `x_from`/`x_end` 为连线两端（调用方负责与两端节点留出间距）。
fn push_elbow_edges(
    out: &mut Vec<MindEdge>,
    x_from: f32,
    y_parent: f32,
    x_mid: f32,
    x_end: f32,
    y_end: f32,
) {
    out.push(MindEdge {
        x: x_from,
        y: y_parent,
        w: x_mid - x_from,
        h: EDGE_W,
    });
    let (vy, vh) = if y_end >= y_parent {
        (y_parent, y_end - y_parent)
    } else {
        (y_end, y_parent - y_end)
    };
    out.push(MindEdge {
        x: x_mid,
        y: vy,
        w: EDGE_W,
        h: vh,
    });
    out.push(MindEdge {
        x: x_mid,
        y: y_end,
        w: x_end - x_mid,
        h: EDGE_W,
    });
}

// ── 连线合并为分块 Path ──
// 每条连线由 3 个矩形段拼成，逐段一个 Slint Rectangle ⇒ 一份大导图有数千个
// item，每帧的遍历、裁剪判定与绘制调用都随之线性增长。改为「一段 = 一条居中
// 描边线段」合并进少量 Path 后：item 数从 O(连线数) 降到 O(列数)。
/// 连线段描边的半厚：包络盒需按它外扩，才能覆盖整条描边的实际像素。
const EDGE_HALF: f32 = EDGE_W / 2.0;
/// 画布连线的分块带宽（内容坐标 px）：同带线段在 x 上相邻，块包络盒互不重叠，
/// 屏外整块可被渲染器裁剪判定跳过。取值略大于一个「列间距 + 节点宽」量级，
/// 使每块的包络盒紧凑（800 节点量级约分 5~15 块）。
const EDGE_BAND_W: f32 = 400.0;

/// 矩形连线段的「中轴」端点：矩形 (x, y, w, h) 的填充范围恰好等于
/// 以中轴为脊、粗 min(w, h) 的描边，故二者可等价替换（描边宽度取 EDGE_W，
/// 与矩形厚度一致；极短的退化段在两种画法下同样不可见）。
fn edge_spine(e: &MindEdge) -> (f32, f32, f32, f32) {
    if e.w >= e.h {
        let cy = e.y + e.h / 2.0;
        (e.x, cy, e.x + e.w, cy)
    } else {
        let cx = e.x + e.w / 2.0;
        (cx, e.y, cx, e.y + e.h)
    }
}

/// 一组连线段 → 一个 Path 块。`relative` 为 true 时命令串坐标相对块包络盒
/// （画布 Path 用 `fit: preserve`，坐标即元素本地 px）；为 false 时保留内容
/// 绝对坐标（小地图 Path 用 viewbox 缩放到面板尺寸）。
fn make_edge_chunk(edges: &[&MindEdge], relative: bool) -> MindEdgeChunk {
    use std::fmt::Write as _;
    let (mut x0, mut y0) = (f32::MAX, f32::MAX);
    let (mut x1, mut y1) = (f32::MIN, f32::MIN);
    for e in edges {
        let (ax, ay, bx, by) = edge_spine(e);
        x0 = x0.min(ax).min(bx);
        y0 = y0.min(ay).min(by);
        x1 = x1.max(ax).max(bx);
        y1 = y1.max(ay).max(by);
    }
    x0 -= EDGE_HALF;
    y0 -= EDGE_HALF;
    x1 += EDGE_HALF;
    y1 += EDGE_HALF;
    let (ox, oy) = if relative { (x0, y0) } else { (0.0, 0.0) };
    let mut commands = String::with_capacity(edges.len() * 32);
    for e in edges {
        let (ax, ay, bx, by) = edge_spine(e);
        let _ = write!(
            commands,
            "M {:.2} {:.2} L {:.2} {:.2} ",
            ax - ox,
            ay - oy,
            bx - ox,
            by - oy
        );
    }
    MindEdgeChunk {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
        commands: commands.into(),
    }
}

/// 画布连线：按线段左缘所在的列带分组（`EDGE_BAND_W` 宽），每组一个 Path 块。
fn canvas_edge_chunks(edges: &[MindEdge]) -> Vec<MindEdgeChunk> {
    let band_of = |e: &MindEdge| (e.x / EDGE_BAND_W).floor().max(0.0) as usize;
    let Some(bands) = edges.iter().map(band_of).max().map(|m| m + 1) else {
        return Vec::new();
    };
    let mut groups: Vec<Vec<&MindEdge>> = vec![Vec::new(); bands];
    for e in edges {
        // 越过带宽的横段按左缘归档（块包络盒按实际端点取，仍保证覆盖整段）。
        groups[band_of(e).min(bands - 1)].push(e);
    }
    groups
        .iter()
        .filter(|g| !g.is_empty())
        .map(|g| make_edge_chunk(g, true))
        .collect()
}

/// 小地图连线：整图一条 Path（内容绝对坐标，Slint 侧用 viewbox 缩放到面板）。
fn mini_edge_chunks(edges: &[MindEdge]) -> Vec<MindEdgeChunk> {
    if edges.is_empty() {
        return Vec::new();
    }
    let refs: Vec<&MindEdge> = edges.iter().collect();
    vec![make_edge_chunk(&refs, false)]
}

// ── 小地图「密度位图」形态（超大导图）──
// 内容被压缩到缩略图尺寸后，单个节点块往往不足 2px：无论怎么逐项绘制都只会
// 互相堆叠成认不出的色块（既看不出结构，也认不出个体）。此时改把「节点 + 连线」
// 在 Rust 侧一次性烘焙成一张位图：覆盖度映射透明度（叠得越多越实）、连线保留
// 树的骨架、选中分支的祖先链用强调色标出。收益有二：内容铺满面板（两轴独立
// 缩放，极扁/极长的导图也能占满），且缩略图每帧只画 1 个 Image（O(N+E) → O(1)）。

/// 密度位图的像素缓冲（纯数据，便于单测）。
struct MiniRaster {
    px: Vec<u8>,
    w: u32,
    h: u32,
}

/// 向覆盖度缓冲某点累加（越界忽略）。
fn raster_add(cov: &mut [f32], w: usize, h: usize, x: isize, y: isize, v: f32) {
    if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
        return;
    }
    cov[y as usize * w + x as usize] += v;
}

/// 在覆盖度缓冲上填一个矩形（坐标已换算到位图像素；至少 1px，极小节点也留痕）。
fn raster_rect(
    cov: &mut [f32],
    hot: &mut [bool],
    w: usize,
    h: usize,
    r: (f32, f32, f32, f32),
    on_spine: bool,
) {
    let x0 = r.0.floor().max(0.0) as usize;
    let y0 = r.1.floor().max(0.0) as usize;
    let x1 = (r.2.ceil().max(r.0 + 1.0)).clamp(0.0, w as f32) as usize;
    let y1 = (r.3.ceil().max(r.1 + 1.0)).clamp(0.0, h as f32) as usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = y * w + x;
            cov[i] += 1.0;
            if on_spine {
                hot[i] = true;
            }
        }
    }
}

/// 一条轴对齐细线（连线中轴）写进覆盖度缓冲，1px 宽。
fn raster_line(cov: &mut [f32], w: usize, h: usize, a: (f32, f32), b: (f32, f32), v: f32) {
    let horizontal = (a.1 - b.1).abs() <= (a.0 - b.0).abs();
    if horizontal {
        let y = a.1.round() as isize;
        let (lo, hi) = (a.0.min(b.0).round() as isize, a.0.max(b.0).round() as isize);
        for x in lo..=hi {
            raster_add(cov, w, h, x, y, v);
        }
    } else {
        let x = a.0.round() as isize;
        let (lo, hi) = (a.1.min(b.1).round() as isize, a.1.max(b.1).round() as isize);
        for y in lo..=hi {
            raster_add(cov, w, h, x, y, v);
        }
    }
}

/// 把导图烘焙成缩略图密度位图。
/// - `box_wh`：缩略图内容盒（逻辑 px，Slint 侧 `mini-box-*` 提供）；
/// - `scales`：(sx, sy) 缩略图逻辑 px / 内容 px（密度形态两轴不等，铺满面板）；
/// - `dpr`：设备像素比，位图按物理像素生成（Retina 下更清晰）；
/// - `spine`：选中分支祖先链在节点序列中的下标（强调色标出）；
/// - `ink` / `accent`：普通色与强调色（由 Slint 侧回读，避免颜色两处定义）。
fn mini_density_raster(
    out: &MindOut,
    box_wh: (f32, f32),
    scales: (f32, f32),
    dpr: f32,
    spine: &[usize],
    ink: [u8; 3],
    accent: [u8; 3],
) -> MiniRaster {
    let (bw, bh) = box_wh;
    let (sx, sy) = scales;
    let w = ((bw.max(1.0) * dpr).round() as usize).max(1);
    let h = ((bh.max(1.0) * dpr).round() as usize).max(1);
    // 内容 px → 位图 px
    let kx = sx * (w as f32 / bw.max(1.0));
    let ky = sy * (h as f32 / bh.max(1.0));
    let mut cov = vec![0.0f32; w * h];
    let mut hot = vec![false; w * h];
    let mut on_path = vec![false; out.nodes.len()];
    for &i in spine {
        if let Some(s) = on_path.get_mut(i) {
            *s = true;
        }
    }

    // 先画连线（骨架），再画节点覆盖其上：与画布同样的层序。
    for e in &out.edges {
        let (ax, ay, bx, by) = edge_spine(e);
        raster_line(&mut cov, w, h, (ax * kx, ay * ky), (bx * kx, by * ky), 0.7);
    }
    for (i, n) in out.nodes.iter().enumerate() {
        raster_rect(
            &mut cov,
            &mut hot,
            w,
            h,
            (n.x * kx, n.y * ky, (n.x + n.w) * kx, (n.y + n.h) * ky),
            on_path[i],
        );
    }

    // 覆盖度 → 透明度：饱和曲线（叠得越多越实，但不会糊成一块实心）。
    let mut px = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let c = cov[i];
        if c <= 0.0 {
            continue;
        }
        let a = 0.7 * (c / (c + 1.2));
        let (rgb, a) = if hot[i] {
            (accent, a.max(0.9))
        } else {
            (ink, a)
        };
        px[i * 4] = rgb[0];
        px[i * 4 + 1] = rgb[1];
        px[i * 4 + 2] = rgb[2];
        px[i * 4 + 3] = (a * 255.0).round() as u8;
    }
    MiniRaster {
        px,
        w: w as u32,
        h: h as u32,
    }
}

/// 选中分支的祖先链（含自身）在节点序列中的下标。
///
/// 节点序列是**后序**（子节点先于父节点入列）：选中项之后第一个「渲染深度更小」
/// 的节点即其父节点，继续回溯直到根 —— 一次顺序扫描即可（O(N)）。深度必须用
/// **渲染深度**（`MindNode::depth`）：挂载视图（软 / 硬）的 visit 是目标分支，
/// 扫描深度 ≠ 渲染深度，用扫描深度回溯会跳过挂载点、链就断错。
/// 同一目标可被多处渲染（真身 + 多个挂载视图，`is_selected` 同真）—— 对**所有**
/// 选中副本取脊柱并集（与结构树「真身 + 挂载行同时高亮」一致）。
fn selection_spine(nodes: &[MindNode]) -> Vec<usize> {
    let mut chain: Vec<usize> = Vec::new();
    let starts: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.is_selected)
        .map(|(i, _)| i)
        .collect();
    for start in starts {
        chain.push(start);
        let mut d = nodes[start].depth;
        for j in start + 1..nodes.len() {
            let dj = nodes[j].depth;
            if dj < d {
                if !chain.contains(&j) {
                    chain.push(j);
                }
                d = dj;
                if d == 0 {
                    break;
                }
            }
        }
    }
    chain
}

/// Slint 颜色 → RGB 三元组（密度位图只用色相：透明度由覆盖度决定）。
fn rgb_of(c: slint::Color) -> [u8; 3] {
    [c.red(), c.green(), c.blue()]
}

/// 密度位图 → Slint Image。
fn mini_density_image(r: &MiniRaster) -> slint::Image {
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&r.px, r.w, r.h);
    slint::Image::from_rgba8(buf)
}

/// 分支及其全部后代的 visit 下标（删除分支前调用，用于清理指向它们的关联）。
fn subtree_indices(e: &Editor, root: usize) -> Vec<usize> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(kids) = e.kids.get(out[i]) {
            out.extend_from_slice(kids);
        }
        i += 1;
    }
    out
}

fn mind_layout(e: &Editor) -> MindOut {
    let mut out = MindOut {
        nodes: Vec::new(),
        edges: Vec::new(),
        mini_edges: Vec::new(),
        edge_chunks: Vec::new(),
        mini_edge_chunks: Vec::new(),
        ref_edges: Vec::new(),
        w: 0.0,
        h: 0.0,
        edge_offset: 0.0,
    };
    let Some(scan) = e.scan.as_ref() else {
        return out;
    };
    if scan.root_index.is_none() {
        return out;
    }
    // 预计算各节点自身高度（内容展开时含内嵌面板）。
    let n = scan.visits.len();
    let mut node_hs = vec![NODE_H; n];
    for (i, v) in scan.visits.iter().enumerate() {
        let show = e.content_expanded.contains(visit_key(scan, i));
        let rows_n = if show {
            build_entry_rows_for(e, i).len()
        } else {
            0
        };
        let has_entries = has_content_entries(scan, v);
        node_hs[i] = if rows_n == 0 {
            if has_entries {
                NODE_H_COMPACT
            } else {
                NODE_H
            }
        } else {
            // 标题行 + 行列表 + 末尾放置区 + 底部留白 + 内容伸缩条。
            NODE_H + rows_n as f32 * ENTRY_ROW_H + ENTRY_END_H + ENTRY_PAD_BOTTOM + CONTENT_BAR_H
        };
    }
    // 子树带高自深向浅递推。
    // ⚠ 软链接会打破「孩子深度必大于父」的前提：挂载点（深层）可能挂入浅层目标，
    // 而浅层目标的 sub_hs 在深度序里**更晚**才最终化 —— 单趟递推会让挂载点用到
    // 陈旧（偏小）的目标高度，渲染时 `total > band_h` 直接把 `clamp` 崩掉
    // （min > max，真机 SIGABRT）。改为**迭代到不动点**：高度只增不减、有上界
    // （≤ 全树高度和），必收敛；对含环的非法 bundle 另加趟数上限兜底。
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(scan.visits[i].depth));
    let mut sub_hs = node_hs.clone();
    let mut inst_cache = InstHeightCache::default();
    for _ in 0..=n {
        let mut changed = false;
        for &i in &order {
            // 展开判定 = 真实键：真实上下文的渲染以 rel 键为准。挂载实例的带高
            // 由 ctx_child_band 按各自实例键精确计入 —— 不能再用「任一实例展开
            // 即预留」的保守近似：嵌套实例键（`实例键␟rel`）的展开不体现在 rel
            // 高度表里，保守值会小于渲染实际高度 → 子带溢出、兄弟子树互相压盖。
            let rel_i = visit_key(scan, i);
            if !e.expanded.contains(rel_i) {
                continue;
            }
            // 孩子带高与渲染（mind_dfs）同式：真实孩子 = rel 上下文子树带高；
            // 挂载孩子 = 实例键精确高度（展开才计子树，收起仅节点高）。
            let mut children: Vec<MindChild> = e
                .kids
                .get(i)
                .map(|v| v.as_slice())
                .unwrap_or_default()
                .iter()
                .copied()
                .filter(|&c| scan.visits[c].hard_link_to.is_none())
                .map(|c| MindChild {
                    visit: c,
                    hard: false,
                    link_idx: None,
                    alias: None,
                    is_mount: false,
                    mount_parent: i,
                })
                .collect();
            for m in e.mounts.get(i).map(|v| v.as_slice()).unwrap_or_default() {
                if m.visit == i {
                    continue;
                }
                children.push(MindChild {
                    visit: m.visit,
                    hard: m.hard,
                    link_idx: m.link_idx,
                    alias: m.title.clone(),
                    is_mount: true,
                    mount_parent: i,
                });
            }
            if children.is_empty() {
                continue;
            }
            let span: f32 = children
                .iter()
                .map(|ch| ctx_child_band(e, &node_hs, &sub_hs, scan, ch, "", &mut inst_cache) + NODE_VGAP)
                .sum::<f32>()
                - NODE_VGAP;
            let new_h = span.max(node_hs[i]);
            if new_h > sub_hs[i] + f32::EPSILON {
                sub_hs[i] = new_h;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut mount_path: Vec<usize> = Vec::new();
    mind_dfs(
        e, &node_hs, &sub_hs, 0, MIND_PAD, NODE_VGAP, sub_hs[0], 0, false, -1, false,
        None, None,
        scan.visits[0].rel.clone(),
        String::new(),
        &mut inst_cache,
        &mut mount_path, &mut out,
    );
    out.h = NODE_VGAP + sub_hs[0] + NODE_VGAP;
    // 缩略图的内容实际右缘（不含按钮占位）：用它换算面板宽度与缩放比例。
    let content_right = out.nodes.iter().map(|n| n.x + n.w).fold(0.0f32, f32::max);
    // 画布右缘需覆盖外置子树按钮的横向占位。
    out.w = out
        .nodes
        .iter()
        .map(|n| {
            n.x + n.w
                + if n.has_children {
                    SUBTREE_BTN_EXTENT
                } else {
                    0.0
                }
        })
        .fold(0.0f32, f32::max)
        + MIND_PAD;
    out.edge_offset = out.w - (content_right + MIND_PAD);
    // 连线几何一次算好后合并成 Path：Slint 侧只渲染少量 Path 元素（见 MindEdgeChunk）。
    out.edge_chunks = canvas_edge_chunks(&out.edges);
    out.mini_edge_chunks = mini_edge_chunks(&out.mini_edges);
    out.ref_edges = build_ref_edges(e, &out.nodes);
    // 注意：节点 `ref_mark`（选中驱动高亮）**不在这里算** —— 布局缓存签名与
    // 选中无关，命中缓存时不会重算；高亮由 sync_ui 在模型刷新时按当前选中
    // 动态回填（见 sel_ref_marked）。
    out
}

/// 与**当前选中分支**直接关联的 visit 集合（refs 语义，§4.5）：选中分支的
/// refs 目标 ∪ 指向选中分支的关联线源分支。用于「选中时高亮关联项」——
/// 高亮是选中驱动的渲染数据，不进布局缓存签名。
fn sel_ref_marked(e: &Editor) -> std::collections::HashSet<usize> {
    let mut marked = std::collections::HashSet::new();
    let (Some(scan), Some(sel)) = (e.scan.as_ref(), e.selected_idx_in_visits()) else {
        return marked;
    };
    if let Some(m) = scan.visits[sel].meta.as_ref() {
        for r in &m.refs {
            if let Some(t) = scan.resolve(&r.target) {
                marked.insert(t);
            }
        }
    }
    if let Some(sel_id) = scan.visits[sel].meta.as_ref().and_then(|m| m.id.clone()) {
        for (i, v) in scan.visits.iter().enumerate() {
            if let Some(m) = v.meta.as_ref() {
                if m.refs.iter().any(|r| r.target == sel_id) {
                    marked.insert(i);
                }
            }
        }
    }
    marked
}

/// 关联线管理对话框的行模型（双向）：正向 = 当前选中分支 `refs[]` 指向的
/// 其它分支；反向 = 其它分支 `refs[]` 指向本分支（被关联项同样可见、可管理，
/// 反向行的修改 / 删除落在**源分支** meta 上）。
fn collect_ref_rows(e: &Editor) -> (Vec<RefRow>, String) {
    let Some(scan) = e.scan.as_ref() else {
        return (Vec::new(), String::new());
    };
    let Some(idx) = e.selected_idx_in_visits() else {
        return (Vec::new(), String::new());
    };
    let visit = &scan.visits[idx];
    let title = visit_title(visit);
    let sel_id = visit.meta.as_ref().and_then(|m| m.id.clone());
    let mut rows: Vec<RefRow> = Vec::new();
    if let Some(m) = visit.meta.as_ref() {
        for r in &m.refs {
            let peer = scan
                .resolve(&r.target)
                .map(|i| visit_title(&scan.visits[i]))
                .unwrap_or_else(|| format!("（未解析：{}）", r.target));
            rows.push(make_ref_row(r, false, peer));
        }
    }
    if let Some(sel_id) = sel_id {
        for (i, v) in scan.visits.iter().enumerate() {
            if i == idx {
                continue;
            }
            if let Some(m) = v.meta.as_ref() {
                for r in &m.refs {
                    if r.target == sel_id {
                        rows.push(make_ref_row(r, true, visit_title(v)));
                    }
                }
            }
        }
    }
    (rows, title)
}

fn make_ref_row(r: &RefItem, incoming: bool, peer: String) -> RefRow {
    RefRow {
        id: r.id.clone().into(),
        target_title: peer.into(),
        rel: r.rel.clone().into(),
        label: r.title.clone().unwrap_or_default().into(),
        incoming,
    }
}

/// **关联项的祖先链**（父级）：父级高亮用（非常淡的一档）。
///
/// 语义 = 取 refs 关联项集合（[`sel_ref_marked`]）中每个分支的祖先链 ——
/// 尤其是导图视图下目标分支被**折叠**（不在布局内、关联线画不出来）时，
/// 淡高亮的祖先链能指出「关联项藏在哪个分支下」。**选中分支自身的祖先链
/// （含 ROOT）整条排除** —— 那是「我在哪」的导航语义，不与关联高亮混色；
/// ROOT 只在它**直接就是关联项**（ROOT 的 refs 指向选中分支）时走强高亮
/// （`sel_ref_marked` 已收录）。关联项自身同样不入淡链（走强高亮）。
fn sel_spine_marks(e: &Editor) -> std::collections::HashSet<usize> {
    let mut set = std::collections::HashSet::new();
    let ref_set = sel_ref_marked(e);
    if ref_set.is_empty() {
        return set;
    }
    let (Some(scan), Some(sel)) = (e.scan.as_ref(), e.selected_idx_in_visits()) else {
        return set;
    };
    // 选中分支自身的完整祖先链（含 ROOT 与自身）：整条排除。
    let own: std::collections::HashSet<usize> = scan.ancestors(sel).into_iter().collect();
    for idx in &ref_set {
        for a in scan.ancestors(*idx) {
            if own.contains(&a) || ref_set.contains(&a) {
                continue;
            }
            set.insert(a);
        }
    }
    set
}

/// 导图上的 refs 关联线（规范 §4.5）：两端**锚定在节点矩形边界**（从中心连线
/// 与矩形边的交点出发，而不是穿过节点内部 —— 中心连线会被节点层盖住，只在
/// 节点间隙露出一小截，几乎不可见），中点放标签（ref 的 `title`，缺省用 `rel`）。
/// 同一对节点的多条关联线互相重叠 —— 可接受（标签相同；管理入口在右键「关联线…」）。
fn build_ref_edges(e: &Editor, nodes: &[MindNode]) -> Vec<RefEdge> {
    let Some(scan) = e.scan.as_ref() else {
        return Vec::new();
    };
    let pos: std::collections::HashMap<usize, (f32, f32, f32, f32)> = nodes
        .iter()
        .map(|n| (n.visit as usize, (n.x, n.y, n.w, n.h)))
        .collect();
    let mut out = Vec::new();
    for n in nodes {
        let Some(meta) = scan
            .visits
            .get(n.visit as usize)
            .and_then(|v| v.meta.as_ref())
        else {
            continue;
        };
        for r in &meta.refs {
            let Some(t_idx) = scan.resolve(&r.target) else {
                continue;
            };
            if t_idx == n.visit as usize {
                continue;
            }
            let Some(&(tx, ty, tw, th)) = pos.get(&t_idx) else {
                continue;
            };
            let label = r.title.clone().unwrap_or_else(|| r.rel.clone());
            let mut edge = make_ref_edge((n.x, n.y, n.w, n.h), (tx, ty, tw, th), &label);
            edge.src_visit = n.visit;
            edge.dst_visit = t_idx as i32;
            out.push(edge);
        }
    }
    out
}

/// 从矩形中心射向外部点 `(tx, ty)` 的线段与矩形边界的交点（关联线的出 / 入锚点）。
fn rect_exit_point(x: f32, y: f32, w: f32, h: f32, tx: f32, ty: f32) -> (f32, f32) {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (dx, dy) = (tx - cx, ty - cy);
    if dx.abs() < f32::EPSILON && dy.abs() < f32::EPSILON {
        return (cx, cy);
    }
    let mut best = f32::INFINITY;
    if dx.abs() > f32::EPSILON {
        best = best.min((w / 2.0) / dx.abs());
    }
    if dy.abs() > f32::EPSILON {
        best = best.min((h / 2.0) / dy.abs());
    }
    (cx + dx * best, cy + dy * best)
}

/// 一条 refs 关联线的几何：沿「源矩形边界 → 目标矩形边界」的中轴直线拆成
/// 「10px 实 6px 断」的虚线段（Slint Path 无 dasharray 属性），包络盒与命令串
/// 口径同 `MindEdgeChunk`；标签放在可见段的中点（不会被节点盖住）。
fn make_ref_edge(
    src: (f32, f32, f32, f32),
    dst: (f32, f32, f32, f32),
    label: &str,
) -> RefEdge {
    use std::fmt::Write as _;
    let (ax, ay) = rect_exit_point(src.0, src.1, src.2, src.3, dst.0 + dst.2 / 2.0, dst.1 + dst.3 / 2.0);
    let (bx, by) = rect_exit_point(dst.0, dst.1, dst.2, dst.3, src.0 + src.2 / 2.0, src.1 + src.3 / 2.0);
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = if len > f32::EPSILON {
        (dx / len, dy / len)
    } else {
        (1.0, 0.0)
    };
    let (x0, y0) = (ax.min(bx) - 2.0, ay.min(by) - 2.0);
    let (dash, gap) = (10.0f32, 6.0f32);
    let mut commands = String::with_capacity(((len / (dash + gap)) as usize + 2) * 32);
    let mut t = 0.0f32;
    while t < len {
        let end = (t + dash).min(len);
        let _ = write!(
            commands,
            "M {:.2} {:.2} L {:.2} {:.2} ",
            ax + ux * t - x0,
            ay + uy * t - y0,
            ax + ux * end - x0,
            ay + uy * end - y0
        );
        t = end + gap;
    }
    RefEdge {
        x: x0,
        y: y0,
        w: dx.abs() + 4.0,
        h: dy.abs() + 4.0,
        commands: commands.into(),
        label_x: (ax + bx) / 2.0,
        label_y: (ay + by) / 2.0,
        label: label.into(),
        src_visit: -1,
        dst_visit: -1,
        mark: false,
    }
}

/// 节点（visit 下标）是否为当前选中分支。
fn node_is_selected(e: &Editor, visit_idx: usize) -> bool {
    e.selected
        .as_deref()
        .zip(e.scan.as_ref())
        .is_some_and(|(key, scan)| visit_key(scan, visit_idx) == key)
}

/// 导图布局签名：只覆盖**影响几何**的输入——可见行结构（标题 / 类型 / 展开、
/// 是否有子分支或内容）、内容展开的节点集合及其可见行数、scan 代次。
/// 选中、多选集合、条目文案等只影响渲染数据，不参与签名（由缓存命中路径
/// 原地刷新），故点选分支 / ⌘ 多选不会触发整树重排。
fn mind_sig(e: &Editor) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    e.scan_gen.hash(&mut h);
    for r in &e.rows {
        r.visit.hash(&mut h);
        r.title.hash(&mut h);
        r.type_str.hash(&mut h);
        r.expanded.hash(&mut h);
        r.has_children.hash(&mut h);
        r.has_entries.hash(&mut h);
    }
    if let Some(scan) = e.scan.as_ref() {
        // 展开集合整体排序入哈希：挂载实例键的展开收起**不一定**反映在 e.rows
        // （对应树上下文收起时该行不存在），漏哈希会导致布局缓存命中陈旧布局
        // ——「展开按钮有时无效」的根源。
        let mut expanded_keys: Vec<&str> = e.expanded.iter().map(|s| s.as_str()).collect();
        expanded_keys.sort_unstable();
        expanded_keys.hash(&mut h);
        // 内容展开集合按身份键（rel）排序入哈希：集合里可能残留上一代 scan 的键，
        // 但 `scan_gen` 已入哈希，rescan 必然换签名，故无需再过滤存在性。
        let mut content_keys: Vec<&str> =
            e.content_expanded.iter().map(|s| s.as_str()).collect();
        content_keys.sort_unstable();
        content_keys.hash(&mut h);
        // 内容行数决定节点高度（条目增删、内嵌文件夹展开都会变）。
        for i in 0..scan.visits.len() {
            let show = e.content_expanded.contains(visit_key(scan, i));
            if show {
                build_entry_rows_for(e, i).len().hash(&mut h);
            }
        }
    }
    h.finish()
}

/// 把节点内嵌内容面板的可见行写入缓存句柄：行数不变则**原地**更新（节点内
/// 右键菜单 / 拖拽所在组件不被销毁），行数变化（内容增删、展开文件夹）才换句柄。
fn apply_node_entry_rows(e: &Editor, visit_idx: usize, rows: Vec<EntryRow>) -> ModelRc<EntryRow> {
    let mut map = e.node_entry_models.borrow_mut();
    match map.get(&visit_idx) {
        Some(handle) if handle.row_count() == rows.len() => {
            for (i, r) in rows.into_iter().enumerate() {
                handle.set_row_data(i, r);
            }
            ModelRc::from(handle.clone())
        }
        _ => {
            let handle = Rc::new(VecModel::from(rows));
            map.insert(visit_idx, handle.clone());
            ModelRc::from(handle)
        }
    }
}

/// 导图布局的渲染孩子：`is_mount` 区分挂载实例与真实位置——同一 visit 可同时
/// 出现（真身 + 软/硬挂载视图），实例的展开键与挂载标记互不关联。
struct MindChild {
    visit: usize,
    hard: bool,
    link_idx: Option<usize>,
    alias: Option<String>,
    is_mount: bool,
    mount_parent: usize,
}

/// 实例上下文带高缓存（`inst_band_h` 的记忆化 + 进行中守卫；挂载 DAG 合法时
/// 无环，守卫仅防**非法** bundle 的病态结构导致栈溢出）。
#[derive(Default)]
struct InstHeightCache {
    memo: HashMap<(String, usize), f32>,
    busy: HashSet<(String, usize)>,
}

/// 真实孩子 `c` 在上下文 `prefix` 下的带高：真实上下文（prefix 空）沿用既有
/// fixpoint 高度表 `sub_hs`（快路径）；实例上下文按 `{prefix}␟{rel}` 判定展开，
/// 展开则递归求实例上下文的带高。
fn ctx_real_band(
    e: &Editor,
    node_hs: &[f32],
    sub_hs: &[f32],
    scan: &Scan,
    c: usize,
    prefix: &str,
    cache: &mut InstHeightCache,
) -> f32 {
    if prefix.is_empty() {
        return sub_hs[c];
    }
    let key = compose_key(prefix, visit_key(scan, c).to_string());
    if !e.expanded.contains(&key) {
        return node_hs[c];
    }
    inst_band_h(e, node_hs, sub_hs, scan, c, prefix, cache)
}

/// 孩子 `ch` 在上下文 `prefix` 下的带高（与 mind_dfs 渲染同式）：
/// - 软挂载实例：收起 = 目标节点高；展开 = 实例上下文（键 = `{prefix}␟{实例键}`）
///   的目标子树带高；
/// - 硬挂载实例：展开 = 自有子分支 / 自有挂载的带跨度（同样按实例上下文记账）；
/// - 真实孩子：见 `ctx_real_band`。
///
/// `sub_hs` 是**真实上下文**（rel 键）的高度表，不适用于实例上下文 —— 此前挂载
/// 行展开曾按 rel 键记账，连带显示真身视图已展开的子孙（回归修复）。
fn ctx_child_band(
    e: &Editor,
    node_hs: &[f32],
    sub_hs: &[f32],
    scan: &Scan,
    ch: &MindChild,
    prefix: &str,
    cache: &mut InstHeightCache,
) -> f32 {
    let key = compose_key(prefix, mount_expand_key(scan, ch.mount_parent, ch.visit, ch.hard));
    if ch.is_mount && ch.hard {
        if !e.expanded.contains(&key) {
            return node_hs[ch.visit];
        }
        let mut own = 0.0f32;
        if let Some(li) = ch.link_idx {
            for &k in e.kids.get(li).map(|v| v.as_slice()).unwrap_or_default() {
                own += ctx_real_band(e, node_hs, sub_hs, scan, k, &key, cache) + NODE_VGAP;
            }
            for m in e.mounts.get(li).map(|v| v.as_slice()).unwrap_or_default() {
                let mc = MindChild {
                    visit: m.visit,
                    hard: m.hard,
                    link_idx: m.link_idx,
                    alias: m.title.clone(),
                    is_mount: true,
                    mount_parent: li,
                };
                own += ctx_child_band(e, node_hs, sub_hs, scan, &mc, &key, cache) + NODE_VGAP;
            }
        }
        if own > 0.0 {
            own -= NODE_VGAP;
        }
        own.max(node_hs[ch.visit])
    } else if ch.is_mount {
        if !e.expanded.contains(&key) {
            node_hs[ch.visit]
        } else {
            inst_band_h(e, node_hs, sub_hs, scan, ch.visit, &key, cache)
        }
    } else {
        ctx_real_band(e, node_hs, sub_hs, scan, ch.visit, prefix, cache)
    }
}

/// `idx` 子树在实例上下文 `prefix` 下（且 idx 已展开）的带高 =
/// max(自身节点高, 孩子带跨度)。记忆化；孩子构造与 mind_dfs 一致
/// （真实孩子排除硬链接分支 + 挂载孩子）。
fn inst_band_h(
    e: &Editor,
    node_hs: &[f32],
    sub_hs: &[f32],
    scan: &Scan,
    idx: usize,
    prefix: &str,
    cache: &mut InstHeightCache,
) -> f32 {
    let mkey = (prefix.to_string(), idx);
    if let Some(&h) = cache.memo.get(&mkey) {
        return h;
    }
    if !cache.busy.insert(mkey.clone()) {
        return node_hs[idx];
    }
    let mut children: Vec<MindChild> = e
        .kids
        .get(idx)
        .map(|v| v.as_slice())
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&c| scan.visits[c].hard_link_to.is_none())
        .map(|c| MindChild {
            visit: c,
            hard: false,
            link_idx: None,
            alias: None,
            is_mount: false,
            mount_parent: idx,
        })
        .collect();
    for m in e.mounts.get(idx).map(|v| v.as_slice()).unwrap_or_default() {
        children.push(MindChild {
            visit: m.visit,
            hard: m.hard,
            link_idx: m.link_idx,
            alias: m.title.clone(),
            is_mount: true,
            mount_parent: idx,
        });
    }
    let mut total = 0.0f32;
    for ch in &children {
        total += ctx_child_band(e, node_hs, sub_hs, scan, ch, prefix, cache) + NODE_VGAP;
    }
    if !children.is_empty() {
        total -= NODE_VGAP;
    }
    let h = total.max(node_hs[idx]);
    cache.busy.remove(&mkey);
    cache.memo.insert(mkey, h);
    h
}

/// 在垂直带 [band_top, band_top + band_h] 内布置节点及其子树，返回节点中心 y。
/// 带高由调用方保证 ≥ 节点自身高度：父节点中心 = 首/末子中心的中点并钳制在带内。
#[allow(clippy::too_many_arguments)]
fn mind_dfs(
    e: &Editor,
    node_hs: &[f32],
    sub_hs: &[f32],
    idx: usize,
    x: f32,
    band_top: f32,
    band_h: f32,
    depth: usize,
    mounted: bool,
    mount_parent: i32,
    hard: bool,
    link_idx: Option<usize>,
    alias: Option<String>,
    // 本节点完整展开键（父层算好：`{实例前缀}␟{rel|实例键}`）与**孩子上下文**
    // （挂载节点 = 自身键；真实节点 = 沿用父上下文）—— 与 emit_branch 同构。
    expand_key: String,
    prefix: String,
    cache: &mut InstHeightCache,
    path: &mut Vec<usize>,
    out: &mut MindOut,
) -> f32 {
    let scan = e.scan.as_ref().unwrap();
    // 全局预算（见 MIND_NODE_CAP）：病态 bundle 下超出即停止下探，
    // 返回带中心近似值保持连线端点有限合理。
    if out.nodes.len() >= MIND_NODE_CAP {
        return band_top + band_h / 2.0;
    }
    let visit = &scan.visits[idx];
    let key = visit_key(scan, idx);
    let is_root = visit.depth == 0;
    let real_owned: Vec<usize> = e
        .kids
        .get(idx)
        .map(|v| v.as_slice())
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&c| scan.visits[c].hard_link_to.is_none())
        .collect();
    let real: &[usize] = &real_owned;
    let mounts_here: &[MountChild] =
        e.mounts.get(idx).map(|v| v.as_slice()).unwrap_or_default();
    // ⚠ `has_children` 必须与展开态无关：折叠时 `children` 为空，但外置「展开子树」
    // 按钮靠它显示 —— 用展开后的列表判定会让折叠节点丢失按钮（v1.0 引入的回归）。
    // 硬链接节点的**自有子分支**也算孩子（link 分支磁盘目录下的真实/挂载孩子）。
    let own_children = link_idx
        .map(|li| {
            !e.kids.get(li).map(|v| v.is_empty()).unwrap_or(true)
                || !e.mounts.get(li).map(|v| v.is_empty()).unwrap_or(true)
        })
        .unwrap_or(false);
    let has_children = if hard {
        // 硬链接节点只渲染自有结构：目标的分支结构不跟过来（规范 §4.6.1）。
        own_children
    } else {
        !real.is_empty() || !mounts_here.is_empty() || own_children
    };
    // 展开键由父层算好传入（真实节点 = rel；挂载节点 = 实例键；实例上下文内
    // 再组合实例前缀）—— 展开收起只作用于本节点，与真身及其它挂载视图
    // （含同挂载点指向同目标的软 / 硬链接对）互不关联。
    // 渲染视角的孩子：普通分支 = 真实子分支 + 挂载的目标；**硬链接节点 = 仅自有
    // 子分支**。同一目标的软 / 硬链接对各自渲染（不再按 visit 去重）。
    // `is_mount` 区分挂载实例与真实位置——同 visit 可同时出现，实例键互不关联。
    let mut children: Vec<MindChild> = Vec::new();
    if e.expanded.contains(&expand_key) {
        if !hard {
            for &c in real {
                children.push(MindChild {
                    visit: c,
                    hard: false,
                    link_idx: None,
                    alias: None,
                    is_mount: false,
                    mount_parent: idx,
                });
            }
            if let Some(ms) = e.mounts.get(idx) {
                for m in ms {
                    if m.visit == idx || path.contains(&m.visit) {
                        continue; // 挂进自己 / 自己的祖先 ⇒ 跳过（防环）
                    }
                    children.push(MindChild {
                        visit: m.visit,
                        hard: m.hard,
                        link_idx: m.link_idx,
                        alias: m.title.clone(),
                        is_mount: true,
                        mount_parent: idx,
                    });
                }
            }
        }
        if let Some(li) = link_idx {
            for &c in e.kids.get(li).map(|v| v.as_slice()).unwrap_or_default() {
                if children.iter().any(|ch| ch.visit == c) {
                    continue;
                }
                children.push(MindChild {
                    visit: c,
                    hard: false,
                    link_idx: None,
                    alias: None,
                    is_mount: false,
                    mount_parent: li,
                });
            }
            for m in e.mounts.get(li).map(|v| v.as_slice()).unwrap_or_default() {
                if m.visit == idx
                    || path.contains(&m.visit)
                    || children.iter().any(|ch| ch.visit == m.visit)
                {
                    continue;
                }
                children.push(MindChild {
                    visit: m.visit,
                    hard: m.hard,
                    link_idx: m.link_idx,
                    alias: m.title.clone(),
                    is_mount: true,
                    mount_parent: li,
                });
            }
        }
    }

    // 节点内容展开时内嵌完整内容列表面板（与列表视图同一 EntryRow 模型）。
    let show_entries = e.content_expanded.contains(key);
    let rows: Vec<EntryRow> = if show_entries {
        let none = HashSet::new();
        visible_to_rows(&build_entry_rows_for(e, idx), node_multi(e, idx, &none))
    } else {
        Vec::new()
    };

    // 标识字形与结构树同款：软链接「⤷」（目标完整视图）、硬链接「≡」（内容引用）。
    let title = if hard {
        format!("≡ {}", alias.clone().unwrap_or_else(|| visit_title(visit)))
    } else if mounted {
        format!("⤷ {}", alias.unwrap_or_else(|| visit_title(visit)))
    } else {
        alias.unwrap_or_else(|| visit_title(visit))
    };
    let type_str = visit_type(visit);
    // 是否存在内容条目（控制内容按钮可见性，与展开态无关）；硬链接看目标。
    let has_any_entries = has_content_entries(scan, visit);
    // 内容展开时节点加宽以容纳面板；子列位置随实际宽度右移。
    let w = if rows.is_empty() {
        est_node_w(&title, &type_str)
    } else {
        NODE_EXPANDED_W
    };
    let node_h = node_hs[idx];

    // 节点固定在自己垂直带的中心；子带围绕该中心对称堆叠（越界时钳制在带内）。
    // 单子带恰好关于节点中心对称 → 连接线为一条水平直线；多子带时仅上下两端出肘。
    let y = band_top + band_h / 2.0;
    let child_x = x + w + NODE_HGAP;
    let mut centers: Vec<f32> = Vec::new();
    if !children.is_empty() {
        // 孩子的带高：按上下文 `prefix` 记账（真实上下文走 sub_hs 快路径；挂载
        // 实例上下文按实例键递归求带高，见 ctx_child_band）—— 挂载行展开只显示
        // 一级，子树内展开态与真身 / 其它实例互不关联。
        let bands: Vec<f32> = children
            .iter()
            .map(|ch| ctx_child_band(e, node_hs, sub_hs, scan, ch, &prefix, cache))
            .collect();
        let total: f32 = bands.iter().map(|b| b + NODE_VGAP).sum::<f32>() - NODE_VGAP;
        // 防御式夹取：正常布局下 total ≤ band_h（带高含全部孩子），但面对**非法**
        // bundle（校验器会报环）不得 panic —— 下界取不到时钳到 band_top 即可，
        // 视觉后果只是子带溢出，比 SIGABRT 好得多。
        let lo = band_top;
        let hi = (band_top + band_h - total).max(lo);
        let mut cursor = (y - total / 2.0).clamp(lo, hi);
        for (ch, &cb) in children.iter().zip(&bands) {
            path.push(idx);
            // 挂载进来的孩子（软链接 = 完整视图，标题由 mind_dfs 加「⤷」标识），
            // 父都记为**挂载声明的真正所在分支**（ch.mount_parent：普通节点 =
            // 本节点；硬链接节点自有结构下 = link 分支 li —— 与 ctx_child_band
            // 的键组合、树侧 emit_branch 的 `parent: li` 一致。此前传 idx（=
            // 硬链接的挂载目标），导致硬链接自有结构下软挂载的展开键与带高
            // 计算用的键不一致 → 带高按收起、渲染按展开 → 子带溢出错位）；
            // 真实孩子 mounted = false。硬链接孩子 mounted = false（走
            // delete-branch + op-visit）。
            let (child_mounted, child_mount_parent) = if ch.is_mount && !ch.hard {
                (true, ch.mount_parent as i32)
            } else if ch.hard {
                (false, ch.mount_parent as i32)
            } else {
                (false, -1)
            };
            // 递归上下文：孩子的完整展开键 = `{上下文}␟{rel|实例键}`；孩子上下文
            // （供其子树）= 挂载孩子取自身键，真实孩子延续当前上下文。
            let (child_ek, child_prefix) = if ch.is_mount {
                let ek = compose_key(
                    &prefix,
                    mount_expand_key(scan, child_mount_parent.max(0) as usize, ch.visit, ch.hard),
                );
                (ek.clone(), ek)
            } else {
                (
                    compose_key(&prefix, visit_key(scan, ch.visit).to_string()),
                    prefix.clone(),
                )
            };
            centers.push(mind_dfs(
                e, node_hs, sub_hs, ch.visit, child_x, cursor, cb, depth + 1, child_mounted,
                child_mount_parent, ch.hard, ch.link_idx, ch.alias.clone(), child_ek,
                child_prefix, cache, path, out,
            ));
            path.pop();
            cursor += cb + NODE_VGAP;
        }
    }

    let is_selected = e.selected.as_deref() == Some(key);
    let children_expanded = e.expanded.contains(&expand_key);
    out.nodes.push(MindNode {
        visit: idx as i32,
        x,
        y: y - node_h / 2.0,
        w,
        h: node_h,
        title: title.into(),
        type_str: type_str.into(),
        is_root,
        is_selected,
        expanded: show_entries && has_any_entries,
        children_expanded,
        has_children,
        mounted,
        mount_parent,
        hard,
        link_visit: link_idx.map(|p| p as i32).unwrap_or(-1),
        expand_key: expand_key.clone().into(),
        depth: depth as i32,
        has_entries: has_any_entries,
        ref_mark: false,
        parent_mark: false,
        entry_rows: apply_node_entry_rows(e, idx, rows),
    });

    if !children.is_empty() {
        // 连线终点取子节点实际位置：同一层的兄弟节点同列（共用 `child_x`），直接
        // 用该值即可 —— 早期实现按 visit 在全量 nodes 里线性查找，使整棵布局退化
        // 为 O(N²)（大导图下每次重排都要多花数十毫秒）。
        for &cy in &centers {
            let cx = child_x;
            // 画布连线：起点越过外置子树按钮，竖线取间距中点。
            let x1 = x + w + SUBTREE_BTN_EXTENT;
            push_elbow_edges(&mut out.edges, x1, y, x1 + NODE_HGAP / 2.0, cx, cy);
            // 缩略图连线：不画按钮无需让位；两端各留半个按钮占位的呼吸间距
            // （不接到两端节点上），中段按 1:2 划分 → 左段短、右段长。
            let g = SUBTREE_BTN_EXTENT / 2.0;
            let x_from = x + w + g;
            let x_end = cx - g;
            let x_mid = x_from + (x_end - x_from) / 3.0;
            push_elbow_edges(&mut out.mini_edges, x_from, y, x_mid, x_end, cy);
        }
    }
    y
}

/// 估算文本渲染宽度（px）：CJK 字形宽于 ASCII，按字体字号分别计。
fn est_text_w(text: &str, cjk_px: f32, ascii_px: f32) -> f32 {
    text.chars()
        .map(|c| if c.is_ascii() { ascii_px } else { cjk_px })
        .sum()
}

/// 依据标题 + 类型估算节点宽度：布局中类型文本按首选宽度分配、
/// 弹性标题只能拿剩余空间，因此节点宽度必须先容纳二者，否则标题被挤没。
/// （两个伸缩按钮均外置于节点之外，不占节点宽度。）
fn est_node_w(title: &str, type_str: &str) -> f32 {
    let title_w = est_text_w(title, 12.0, 7.0) + 6.0;
    let type_w = if type_str.is_empty() {
        0.0
    } else {
        est_text_w(type_str, 10.0, 6.0) + 4.0
    };
    (16.0 + title_w + type_w).clamp(120.0, 520.0)
}

/// 解析标签输入：逗号 / 中文逗号 / 空白分隔，去空项。
fn parse_tags(input: &str) -> Vec<String> {
    input
        .split([',', '，', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 读取某分支的 `Meta`（失败转错误消息）。
fn read_meta(bundle: &Bundle, dir: &Path) -> Result<Meta, String> {
    match bundle.read_meta(dir).map_err(|e| e.to_string())? {
        MetaLoad::Ok(m, _) => Ok(*m),
        MetaLoad::Failed(issues) => Err(format!(
            ".str.toml 解析失败：{}",
            issues
                .first()
                .map(|i| i.message.clone())
                .unwrap_or_default()
        )),
    }
}

/// 字节数友好显示。
fn fmt_size(v: Option<i64>) -> String {
    match v {
        None => "—".into(),
        Some(n) if n < 1024 => format!("{n} B"),
        Some(n) if n < 1024 * 1024 => format!("{:.1} KB", n as f64 / 1024.0),
        Some(n) => format!("{:.1} MB", n as f64 / 1048576.0),
    }
}

/// 内部剪贴板（仿 Finder 的复制/剪切；跨应用粘贴请配合系统剪贴板同步）。
#[derive(Clone)]
struct ClipItem {
    /// 源条目绝对路径（文件 / 目录 / 分支目录）。
    src_path: PathBuf,
    /// 源条目名。
    name: String,
    /// 源条目 role（payload / asset / dir / node / branch）。
    role: String,
    /// 是否为剪切（粘贴时执行移动而非复制）。
    is_cut: bool,
}

/// Finder 风格重名：`a.txt` → `a 副本.txt` → `a 副本 2.txt`。
fn dedup_name(existing: &HashSet<String>, name: &str) -> String {
    if !existing.contains(name) {
        return name.to_string();
    }
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) if !b.is_empty() => (b.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    let mut candidate = format!("{base} 副本{ext}");
    let mut n = 2;
    while existing.contains(&candidate) {
        candidate = format!("{base} 副本 {n}{ext}");
        n += 1;
    }
    candidate
}

/// 递归复制目录（不含 `.str.toml` 语义，仅内容容器）。
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 递归复制分支：重建 `.str.toml`（新 id、新 ref id、修正子分支指向），payload 按位拷贝。
///
/// 返回新分支 id；`depth` 为目标父分支深度（用于决定 node/branch）。
fn copy_branch_recursive(
    bundle: &Bundle,
    src_dir: &Path,
    dst_parent_dir: &Path,
    depth: usize,
    id_version: usize,
    mapping: &mut std::collections::HashMap<String, String>,
) -> Result<String, String> {
    let src_meta = read_meta(bundle, src_dir)?;
    let new_id = util::new_uuid(id_version);
    let dst_dir = dst_parent_dir.join(&new_id);
    std::fs::create_dir_all(&dst_dir).map_err(|e| e.to_string())?;

    let kind = if depth == 0 { Kind::Node } else { Kind::Branch };
    let mut new_meta = meta_edit::meta_from_text(&meta_edit::render_branch_meta(
        kind,
        &new_id,
        src_meta.r#type.as_deref(),
        src_meta.title.as_deref(),
        src_meta.summary.as_deref(),
        &util::now_rfc3339(),
    ))
    .map_err(|e| e.to_string())?;
    new_meta.set_str_array("tags", &src_meta.tags);
    if let Some(old_id) = &src_meta.id {
        mapping.insert(old_id.clone(), new_id.clone());
    }

    for e in &src_meta.entries {
        let mut ne = e.clone();
        match e.role.as_str() {
            "node" | "branch" => {
                let child_src = src_dir.join(&e.path);
                let child_id = copy_branch_recursive(
                    bundle,
                    &child_src,
                    &dst_dir,
                    depth + 1,
                    id_version,
                    mapping,
                )?;
                ne.id = Some(child_id.clone());
                ne.path = child_id;
            }
            "payload" | "asset" => {
                let from = src_dir.join(&e.path);
                let to = dst_dir.join(&e.path);
                std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
            }
            "dir" => {
                copy_dir_recursive(&src_dir.join(&e.path), &dst_dir.join(&e.path))?;
            }
            _ => {}
        }
        new_meta.upsert_entry(&ne);
    }

    // 跨枝关联：目标在复制子树内则重映射，否则保留；关联线自身换新 id。
    for r in &src_meta.refs {
        let mut rr = r.clone();
        if let Some(t) = mapping.get(&rr.target) {
            rr.target = t.clone();
        }
        rr.id = util::new_uuid_v7();
        new_meta.push_ref(&rr);
    }

    new_meta
        .save(&bundle.meta_path(&dst_dir))
        .map_err(|e| e.to_string())?;
    Ok(new_id)
}

/// 把文本写入系统剪贴板（「拷贝路径」用；各平台尽力而为）。
const CLIP_TEXT_KEY: &str = "clipboard-text.applescript";
const CLIP_TEXT_SCRIPT: &str = r#"on run argv
	set the clipboard to ((item 1 of argv) as text)
end run
"#;

#[cfg(target_os = "macos")]
fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let script = clipboard_script_path(CLIP_TEXT_KEY)?;
    let out = std::process::Command::new("osascript")
        .arg(&script)
        .arg(text)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(windows)]
fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let ps = format!("Set-Clipboard -Value '{}'", text.replace('\'', "''"));
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    for prog in ["wl-copy", "xclip -selection clipboard"] {
        let mut parts = prog.split_whitespace();
        let Some(bin) = parts.next() else { continue };
        let mut cmd = std::process::Command::new(bin);
        cmd.args(parts).stdin(std::process::Stdio::piped());
        if let Ok(mut child) = cmd.spawn() {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().map(|s| s.success()).unwrap_or(false) {
                return Ok(());
            }
        }
    }
    Err("未找到可用的剪贴板程序（wl-copy / xclip）".into())
}

/// macOS 常见终端：`(pgrep 进程名, 应用路径候选)`。进程名用于探测正在运行的
/// 终端；**路径**用于 `open -a`（比名字可靠 —— iTerm 的包是 `iTerm.app`，
/// 而进程名是 iTerm2，`open -a iTerm2` 并不保证命中）。
#[cfg(target_os = "macos")]
const MAC_TERMINALS: [(&str, &[&str]); 8] = [
    (
        "iTerm2",
        &["/Applications/iTerm.app", "/Applications/iTerm2.app"],
    ),
    ("Warp", &["/Applications/Warp.app"]),
    ("Ghostty", &["/Applications/Ghostty.app"]),
    ("kitty", &["/Applications/kitty.app"]),
    ("Alacritty", &["/Applications/Alacritty.app"]),
    ("WezTerm", &["/Applications/WezTerm.app"]),
    ("Hyper", &["/Applications/Hyper.app"]),
    ("Tabby", &["/Applications/Tabby.app"]),
];

/// `.command` 关联到这些 bundle id 时视为「不是终端」而放弃：「在终端中显示」
/// 却打开编辑器显然是错的（`open -b` 同样会成功返回，链就断在这里了）。
#[cfg(target_os = "macos")]
const NOT_TERMINAL_BUNDLES: [&str; 9] = [
    "com.apple.dt.Xcode",
    "com.apple.TextEdit",
    "com.apple.finder",
    "com.apple.Preview",
    "com.microsoft.VSCode",
    "com.sublimetext.4",
    "com.sublimetext.3",
    "com.barebones.bbedit",
    "com.panic.Nova",
];

/// `.command` 文件（UTI `com.apple.terminal.shell-script`）的默认处理程序
/// bundle id —— 即用户在 Finder「显示简介 → 打开方式」里设定的默认终端。
///
/// 直接读 LaunchServices 偏好即可拿到，**不需要** Apple Events / Finder /
/// 探针文件，也就没有 TCC 授权框与超时问题（旧方案用 osascript 问 Finder，
/// 会被授权框卡住且要维护 /tmp 探针；更早的名字猜测（`open -a iTerm2`）在
/// iTerm 上根本命中不了 —— 它的包名是 iTerm.app）。
///
/// 读不到（用户从未改过、plist 结构异常）时返回 None，由候选链兜底到
/// Terminal.app —— macOS 上 `.command` 的声明方本来就是 Terminal。
#[cfg(target_os = "macos")]
fn macos_command_handler_bundle_id() -> Option<String> {
    use std::sync::OnceLock;
    // 会话内缓存：plist 导出约 85KB / 65ms，默认终端不会频繁变。
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let out = std::process::Command::new("defaults")
                .args([
                    "export",
                    "com.apple.LaunchServices/com.apple.launchservices.secure",
                    "-",
                ])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            parse_command_handler_from_ls_handlers_xml(&String::from_utf8_lossy(&out.stdout))
        })
        .clone()
}

/// 从 `defaults export` 的 LSHandlers XML 里取 `.command`（UTI
/// `com.apple.terminal.shell-script`）的角色 bundle id。
///
/// LSHandlers 是 dict 数组，每个数组项一层 dict，内部还可能嵌
/// `LSHandlerPreferredVersions`（其值常为 `-` = 无覆盖）：必须按嵌套层级成块
/// 扫描，块内优先取非 `-` 的角色值 —— 否则会读到内层占位符或串到相邻条目上。
#[cfg(target_os = "macos")]
fn parse_command_handler_from_ls_handlers_xml(xml: &str) -> Option<String> {
    const UTI: &str = "com.apple.terminal.shell-script";
    const ROLES: [&str; 4] = [
        "LSHandlerRoleAll",
        "LSHandlerRoleViewer",
        "LSHandlerRoleShell",
        "LSHandlerRoleEditor",
    ];
    /// 在一条 handler 字典里取角色 bundle id。
    fn role_of(block: &[&str]) -> Option<String> {
        for role in ROLES {
            let key = format!("<key>{role}</key>");
            // 同一角色键在块内可能出现两次：内层 PreferredVersions 的 `-`
            // 占位在前、外层真实值在后 —— 逐个出现位置找第一个有效值，
            // 只看首个位置会被占位符挡住。
            for (i, l) in block.iter().enumerate() {
                if !l.contains(key.as_str()) {
                    continue;
                }
                let v = block
                    .get(i + 1)
                    .map(|s| {
                        s.trim_start_matches("<string>")
                            .trim_end_matches("</string>")
                            .trim()
                    })
                    .unwrap_or_default();
                if !v.is_empty() && v != "-" {
                    return Some(v.to_string());
                }
            }
        }
        None
    }

    let mut lines = xml.lines().map(str::trim);
    // 顶层是 `<dict><key>LSHandlers</key><array>…`：先定位键，再只看 array
    // 内的条目。**不能**裸扫 `<dict>` 边界 —— 顶层那个 dict 会把整个文件
    // 包成一块，于是读到文件里第一个非 "-" 的角色值（实测读到无关应用的
    // bundle id：com.seriflabs.affinitydesigner）。
    if !lines.by_ref().any(|l| l.contains("<key>LSHandlers</key>")) {
        return None;
    }
    let mut in_array = false;
    let mut depth = 0usize;
    let mut block: Vec<&str> = Vec::new();
    for line in lines {
        if !in_array {
            if line.starts_with("<array>") {
                in_array = true;
            }
            continue;
        }
        if line.starts_with("</array>") && depth == 0 {
            break;
        }
        if line.starts_with("<dict>") {
            if depth == 0 {
                block.clear();
            }
            depth += 1;
        }
        if depth > 0 {
            block.push(line);
        }
        if line.starts_with("</dict>") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                if block.iter().any(|l| l.contains(UTI)) {
                    if let Some(id) = role_of(&block) {
                        return Some(id);
                    }
                }
                block.clear();
            }
        }
    }
    None
}

/// `open -b <bundle id> <目录>`：把目录交给指定应用打开（终端会新开窗口并切到
/// 该目录）。bundle id 不合法时 `open` 会以非 0 退出，调用方据此回退。
#[cfg(target_os = "macos")]
fn try_open_bundle(bundle: &str, dir: &Path) -> Result<(), String> {
    let out = std::process::Command::new("open")
        .arg("-b")
        .arg(bundle)
        .arg(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `open -a <应用名或绝对路径> <目录>`：候选链里每个终端都这么打开。
#[cfg(target_os = "macos")]
fn try_open_a(app: &str, dir: &Path) -> Result<(), String> {
    let out = std::process::Command::new("open")
        .arg("-a")
        .arg(app)
        .arg(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// 在终端中打开目录（「在终端中显示」用；各平台尽力而为）。
fn open_terminal_at(dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // macOS 无「默认终端」系统 API，选择顺序：
        //   1. STR_TERMINAL 显式覆盖（名字或路径，`open -a`）；
        //   2. 系统 `.command` 默认处理程序（读 LaunchServices 偏好得 bundle id，
        //      `open -b`）——用户可在 Finder 里改，这就是「默认终端」的事实定义；
        //   3. 终端注入的环境变量 TERM_PROGRAM / LC_TERMINAL（从终端内启动应用
        //      时会继承，直接指明用户的终端）；
        //   4. 正在运行的常见终端（pgrep 免权限探测）；
        //   5. 已安装的常见终端（路径存在即可用）；
        //   6. Terminal.app 兜底。
        // 候选统一为**路径或应用名**，逐个 `open -a <候选> <目录>`，失败换下一个。
        // TERM_PROGRAM / LC_TERMINAL 值 → 应用名映射。
        const TERM_PROGRAM_MAP: [(&str, &str); 8] = [
            ("Apple_Terminal", "Terminal"),
            ("iTerm.app", "iTerm"),
            ("iTerm2", "iTerm"),
            ("WarpTerminal", "Warp"),
            ("ghostty", "Ghostty"),
            ("Ghostty", "Ghostty"),
            ("kitty", "kitty"),
            ("WezTerm", "WezTerm"),
        ];
        let mut candidates: Vec<String> = Vec::new();
        let push = |list: &mut Vec<String>, c: String| {
            let c = c.trim_end_matches('/').to_string();
            if !c.is_empty() && !list.iter().any(|x| x.eq_ignore_ascii_case(&c)) {
                list.push(c);
            }
        };
        // 1) 显式覆盖优先：设置了就先用它，失败再走系统关联
        if let Ok(t) = std::env::var("STR_TERMINAL") {
            let t = t.trim().to_string();
            if !t.is_empty() {
                push(&mut candidates, t.clone());
                if let Ok(()) = try_open_a(&t, dir) {
                    return Ok(());
                }
                candidates.clear();
            }
        }
        // 2) 系统 `.command` 默认处理程序：最贴合「默认终端」的定义
        let handler = macos_command_handler_bundle_id();
        if let Some(bundle) = handler.as_deref() {
            let usable = !NOT_TERMINAL_BUNDLES
                .iter()
                .any(|b| b.eq_ignore_ascii_case(bundle));
            if !usable && debug_on() {
                eprintln!("[str-gui] .command 默认处理程序 {bundle} 不是终端，跳过");
            }
            if usable && try_open_bundle(bundle, dir).is_ok() {
                return Ok(());
            }
        }
        // 3) 回退：环境变量 → 运行中 → 已安装 → Terminal.app
        for var in ["TERM_PROGRAM", "LC_TERMINAL"] {
            if let Ok(v) = std::env::var(var) {
                if let Some((_, app)) = TERM_PROGRAM_MAP.iter().find(|(k, _)| *k == v) {
                    push(&mut candidates, (*app).to_string());
                }
            }
        }
        // 正在运行的终端：一次 pgrep 搞定全部进程名（`-l` 输出 "pid 名称"，
        // 逐名 8 次 spawn 纯属浪费 —— 每次都要 fork + exec）。
        let pattern = MAC_TERMINALS
            .iter()
            .map(|(p, _)| *p)
            .collect::<Vec<_>>()
            .join("|");
        let running_out = std::process::Command::new("pgrep")
            .args(["-ilx", &pattern])
            .output();
        let running_names: Vec<String> = running_out
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for (proc_name, paths) in MAC_TERMINALS {
            if !running_names
                .iter()
                .any(|n| n.eq_ignore_ascii_case(proc_name))
            {
                continue;
            }
            // 优先取实际存在的应用路径（`open -a <路径>` 必命中）
            match paths.iter().find(|p| Path::new(p).exists()) {
                Some(p) => push(&mut candidates, (*p).to_string()),
                None => push(&mut candidates, proc_name.to_string()),
            }
        }
        // 已安装的常见终端（路径存在 = `open -a <路径>` 必命中）
        for (_, paths) in MAC_TERMINALS {
            for p in paths {
                if Path::new(p).exists() {
                    push(&mut candidates, (*p).to_string());
                }
            }
        }
        let terminal_path = "/System/Applications/Utilities/Terminal.app";
        push(
            &mut candidates,
            if Path::new(terminal_path).exists() {
                terminal_path.to_string()
            } else {
                "Terminal".to_string()
            },
        );
        let mut last_err = String::from("未找到可用的终端");
        for t in &candidates {
            match try_open_a(t, dir) {
                Ok(()) => return Ok(()),
                Err(msg) => last_err = msg,
            }
        }
        Err(last_err)
    }
    #[cfg(windows)]
    {
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NoExit",
                "-Command",
                &format!("Set-Location -LiteralPath '{}'", dir.display()),
            ])
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (prog, args) in [
            ("x-terminal-emulator", vec!["--working-directory"]),
            ("gnome-terminal", vec!["--working-directory"]),
            ("konsole", vec!["--workdir"]),
        ] {
            let mut cmd = std::process::Command::new(prog);
            cmd.args(&args).arg(dir);
            if cmd.spawn().is_ok() {
                return Ok(());
            }
        }
        Err("未找到可用的终端程序".into())
    }
}

/// 在系统文件管理器中显示（macOS：`open -R`）。
/// 在 Finder 中显示（可多条）：按父目录分组，每组用 AppleScript `select`
/// 在**一个窗口内多选**（`open -R` 多路径实测只选中 1 项）。
/// 注意：`POSIX file … as alias` 对符号链接路径（/tmp）会失败 —— 调用前先
/// `canonicalize`；个别组失败时兜底 `open -R`（单选降级，不致命）。
#[cfg(target_os = "macos")]
const REVEAL_KEY: &str = "reveal.applescript";
#[cfg(target_os = "macos")]
const REVEAL_SCRIPT: &str = r#"on run argv
	set theItems to {}
	repeat with p in argv
		set end of theItems to (POSIX file (p as text) as alias)
	end repeat
	tell application "Finder"
		activate
		select theItems
	end tell
end run
"#;
#[cfg(target_os = "macos")]
fn reveal_in_file_manager_multi(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    // 规范化（解符号链接）+ 按父目录分组（每组一个窗口内多选）。
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for p in paths {
        let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let parent = canon.parent().unwrap_or(Path::new("/")).to_path_buf();
        match groups.last_mut() {
            Some(group) if group[0].parent() == Some(&parent) => group.push(canon),
            _ => groups.push(vec![canon]),
        }
    }
    let script = clipboard_script_path(REVEAL_KEY)?;
    let mut failed = 0usize;
    for group in &groups {
        let arg_refs: Vec<&str> = group
            .iter()
            .map(|p| p.to_str().unwrap_or_default())
            .collect();
        let out = std::process::Command::new("osascript")
            .arg(&script)
            .args(&arg_refs)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            failed += 1;
        }
    }
    if failed < groups.len() {
        return Ok(());
    }
    // 全组失败 → 兜底：open -R（至少定位第一条，不中断流程）。
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-R").args(paths);
    let _ = cmd.spawn();
    Err("在 Finder 中显示失败".into())
}

#[cfg(not(target_os = "macos"))]
fn reveal_in_file_manager_multi(paths: &[PathBuf]) -> Result<(), String> {
    for p in paths {
        reveal_in_file_manager(p);
    }
    Ok(())
}

fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg("/select,")
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// 用系统默认程序打开 URL（帮助菜单的在线文档 / 仓库 / 反馈入口）：
/// macOS `open`、Windows `rundll32`、其它 `xdg-open`。
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// `DndApi.src-visit` 的值：当前激活分支的 visits 下标（无选中 → -1）。
///
/// 该属性的不变量是「等于激活分支」，拖拽期间由被拖面板临时改写为拖拽源，
/// 收尾（含取消）后必须复位 —— 否则取消拖拽后仍指向旧拖拽源。
fn drag_source_visit(e: &Editor) -> i32 {
    e.selected_idx_in_visits().map(|i| i as i32).unwrap_or(-1)
}

/// 在线入口 URL：0 = 格式规范、1 = 仓库主页、2 = 问题反馈、
/// 3 = str-gui/README.md（欢迎引导里的技能接入指引，指向仓库内文档）。
fn doc_url(repo: &str, which: i32) -> String {
    let repo = repo.trim_end_matches('/');
    match which {
        0 => format!("{repo}/blob/main/SPEC.md"),
        1 => repo.to_string(),
        3 => format!("{repo}/blob/main/str-gui/README.md"),
        _ => format!("{repo}/issues"),
    }
}

/// 「帮助 → 快捷键一览…」的内容表：按菜单分组（`head` = 组首行，列表里画组标题）。
///
/// 只列**应用自身的交互**（菜单快捷键 + 鼠标/触控手势 + 对话框按键）；修饰键字形
/// 按平台切换：macOS 显示 ⌘⌥⇧，其余平台显示 Ctrl+/Alt+/Shift+（与 `@keys(Control+…)`
/// 的跨平台语义一致：Slint 在 macOS 上把 Control 映射为 Command）。
fn shortcut_table(mac: bool) -> Vec<Shortcut> {
    fn push(out: &mut Vec<Shortcut>, group: &str, name: &str, keys: String) {
        let head = out.last().is_none_or(|r| r.group.as_str() != group);
        out.push(Shortcut {
            head,
            group: group.into(),
            name: name.into(),
            keys: keys.into(),
        });
    }

    let (m, a, s) = if mac {
        ("⌘", "⌥", "⇧")
    } else {
        ("Ctrl+", "Alt+", "Shift+")
    };
    let del = if mac { "⌫" } else { "Del" };
    let mut out: Vec<Shortcut> = Vec::new();

    push(&mut out, "文件", "打开文件…", format!("{m}O"));
    push(&mut out, "文件", "新建文件…", format!("{m}N"));
    push(&mut out, "文件", "刷新（重新扫描磁盘）", format!("{m}R"));

    push(&mut out, "编辑", "全选条目", format!("{m}A"));
    push(
        &mut out,
        "编辑",
        "复制 / 剪切 / 粘贴",
        format!("{m}C / {m}X / {m}V"),
    );
    push(&mut out, "编辑", "制作副本", format!("{m}D"));
    push(&mut out, "编辑", "快速查看（系统预览，仅 macOS）", "空格".into());
    push(&mut out, "编辑", "删除条目", format!("{m}{del}"));
    push(&mut out, "编辑", "保存条目修改", format!("{m}S"));

    push(&mut out, "分支", "新建子分支…", format!("{s}{m}N"));
    push(&mut out, "分支", "保存分支信息", format!("{s}{m}S"));
    push(&mut out, "分支", "删除分支", format!("{s}{m}{del}"));

    push(&mut out, "工具", "在 Finder 中显示分支", format!("{s}{m}R"));
    push(&mut out, "工具", "在 Finder 中显示内容", format!("{a}{m}R"));
    push(&mut out, "工具", "校验 bundle", format!("{s}{m}V"));

    push(
        &mut out,
        "视图",
        "列表视图 / 导图视图",
        format!("{m}1 / {m}2"),
    );
    push(
        &mut out,
        "视图",
        "放大 / 缩小 / 实际大小",
        format!("{m}+ / {m}- / {m}0"),
    );

    push(&mut out, "鼠标与触控", "导图画布：拖拽平移", "拖拽".into());
    push(
        &mut out,
        "鼠标与触控",
        "小地图：单击 / 拖动 = 把视口移到该处",
        "单击 / 拖动".into(),
    );
    push(
        &mut out,
        "鼠标与触控",
        "小地图：滚轮 = 缩放，双击 = 回到 100%",
        "滚轮 / 双击".into(),
    );
    push(
        &mut out,
        "鼠标与触控",
        "双击结构树行 / 导图节点 = 展开·收起",
        "双击".into(),
    );
    push(
        &mut out,
        "鼠标与触控",
        "双击内容条目 = 文件夹展开·收起 / 文件用系统程序打开",
        "双击".into(),
    );
    push(
        &mut out,
        "鼠标与触控",
        "拖放条目跨分支移动；外部文件拖入 = 导入当前分支",
        "拖放".into(),
    );
    push(
        &mut out,
        "鼠标与触控",
        "右键结构树行 / 导图节点 / 内容条目 = 对应操作菜单",
        "右键".into(),
    );

    push(
        &mut out,
        "其他",
        "对话框：Esc 取消 / 关闭，Enter 确定",
        "Esc / Enter".into(),
    );
    push(
        &mut out,
        "其他",
        concat!(
            "输入框聚焦时：全选 / 复制 / 剪切 / 粘贴 / 删除到行首归输入框，",
            "菜单里的同名动作暂停（避免抢走按键）"
        ),
        format!("{m}A {m}C {m}X {m}V {m}{del} {s}{m}V"),
    );

    out
}

/// 「内容」页的一个可见行：顶层内容条目，或已展开文件夹的子项。
struct VisibleEntry {
    /// `meta.entries` 下标（文件夹子行 = None，不参与编辑/登记）。
    entries_idx: Option<usize>,
    /// 显示路径：顶层 = 条目 path；子项 = "目录/子项"。
    path: String,
    role: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    /// 拖放/导入目标目录。
    drop_dir: PathBuf,
    /// 实际文件系统路径。
    fs_path: PathBuf,
    size: Option<i64>,
    /// 顶层条目的 title（文件夹子行无）。
    title: Option<String>,
}

/// 当前选中行在可视行里的下标（无选中 = None）。
fn selected_row_index(app: &AppWindow) -> Option<usize> {
    let idx = app.get_entry_selected();
    if idx < 0 {
        None
    } else {
        Some(idx as usize)
    }
}

/// 构建当前选中分支的可见内容行。
fn build_entry_rows(e: &Editor) -> Vec<VisibleEntry> {
    match e.selected_idx_in_visits() {
        Some(idx) => build_entry_rows_for(e, idx),
        None => Vec::new(),
    }
}

/// 构建指定分支的可见内容行（展开的文件夹列出磁盘子项，按名排序）。
fn build_entry_rows_for(e: &Editor, visit_idx: usize) -> Vec<VisibleEntry> {
    let Some(visit) = e.scan.as_ref().and_then(|s| s.visits.get(visit_idx)) else {
        return Vec::new();
    };
    // 硬链接分支（§4.6.1 mode = "hard"）：内容 = 目标分支的内容条目（只读视图，
    // 文件只有一份、在目标目录里）；自身 entries 只允许子分支（结构）。写操作由
    // `sel_hard` 守卫拦截。目标悬空（校验器会报）时回退为空列表。
    let source = e.scan.as_ref().and_then(|s| content_visit(s, visit));
    let Some((meta, dir)) = source.and_then(|v| v.meta.as_ref().map(|m| (m, &v.dir))) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, en) in meta.entries.iter().enumerate() {
        // 内容面板只列内容：`branch` 是子分支结构，`link` 是挂载声明（已在
        // 结构树 / 导图以「⤷ / ≡」分支行呈现）——都不该以
        // 内容条目的样子出现在这里。
        if en.is_branch() || en.is_link() {
            continue;
        }
        let is_dir = en.role == "dir";
        // 文件实际在**内容来源分支**的目录里（硬链接视图下即目标目录）。
        let fs_path = dir.join(&en.path);
        let dir_key = fs_path.to_string_lossy().to_string();
        let expanded = is_dir && e.expanded_dirs.contains(&dir_key);
        out.push(VisibleEntry {
            entries_idx: Some(idx),
            path: en.path.clone(),
            role: en.role.clone(),
            depth: 0,
            is_dir,
            expanded,
            // 文件夹行的拖放目标是文件夹内部；文件行是内容来源分支目录
            // （硬链接视图下写操作被 sel_hard 守卫拦截，落点仅作展示）。
            drop_dir: if is_dir {
                fs_path.clone()
            } else {
                dir.clone()
            },
            fs_path: fs_path.clone(),
            size: en.size,
            title: en.title.clone(),
        });
        if is_dir && expanded {
            push_dir_children(e, &fs_path, &en.path, 1, &mut out);
        }
    }
    out
}

/// 递归列出已展开内容文件夹的磁盘子项（子文件夹同样可展开）。
fn push_dir_children(
    e: &Editor,
    fs_dir: &Path,
    rel: &str,
    depth: usize,
    out: &mut Vec<VisibleEntry>,
) {
    let mut children: Vec<std::fs::DirEntry> = std::fs::read_dir(fs_dir)
        .map(|rd| rd.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();
    children.sort_by_key(|c| c.file_name());
    for c in children {
        let name = c.file_name().to_string_lossy().to_string();
        // 忽略 Finder 噪声文件（.DS_Store / ._ 开头 / __MACOSX 等）。
        if is_noise_name(&name) {
            continue;
        }
        let child_rel = format!("{rel}/{name}");
        let c_is_dir = c.path().is_dir();
        let c_key = c.path().to_string_lossy().to_string();
        let expanded = c_is_dir && e.expanded_dirs.contains(&c_key);
        let size = if c_is_dir {
            None
        } else {
            std::fs::metadata(c.path()).ok().map(|m| m.len() as i64)
        };
        out.push(VisibleEntry {
            entries_idx: None,
            path: child_rel.clone(),
            role: if c_is_dir { "dir" } else { "file" }.to_string(),
            depth,
            is_dir: c_is_dir,
            expanded,
            // 子文件夹行拖放目标 = 其内部；子文件行 = 所在文件夹。
            drop_dir: if c_is_dir {
                c.path().to_path_buf()
            } else {
                fs_dir.to_path_buf()
            },
            fs_path: c.path(),
            size,
            title: None,
        });
        if c_is_dir && expanded {
            push_dir_children(e, &c.path(), &child_rel, depth + 1, out);
        }
    }
}

/// 节点内嵌面板要用的条目多选集合：**只有当前激活分支**才带多选高亮。
///
/// `entry_multi` 是「当前分支内的多选路径集合」，而各分支常有**同名条目**
/// （都叫 `README.md` / `payload` 之类）。若把它无条件交给每个节点的内容面板，
/// 非激活节点会因为**相对路径相同**而一起点亮 —— 与结构树「同名串台」同一类问题。
/// 列表视图不受影响（那里本来就只渲染当前分支）。
fn node_multi<'a>(
    e: &'a Editor,
    visit_idx: usize,
    none: &'a HashSet<String>,
) -> &'a HashSet<String> {
    match e.selected_idx_in_visits() {
        Some(i) if i == visit_idx => &e.entry_multi,
        _ => none,
    }
}

/// 把「内容」页可见行转成 UI 模型行（多选标记按当前集合烧入）。
fn visible_to_rows(vis: &[VisibleEntry], multi: &HashSet<String>) -> Vec<EntryRow> {
    vis.iter()
        .map(|v| {
            // 显示名：文件夹子行只显示真实文件名（不带父路径前缀）。
            // 只求值一次，供 display 与光学居中判定（latin）共用。
            let display = basename(&v.path);
            EntryRow {
                path: v.path.clone().into(),
                display: display.clone().into(),
                latin: is_latin_only(&display),
                role: v.role.clone().into(),
                title: v.title.clone().unwrap_or_default().into(),
                size: fmt_size(v.size).into(),
                // 分支行（node/branch）只在结构树中呈现；内容条目（含未登记的
                // 文件夹子行）才是内容列表的可用目标。此前这里硬编码 false，
                // 导致 Slint 侧无法用该字段区分「分支行」与「内容行」。
                is_branch: matches!(v.role.as_str(), "node" | "branch"),
                is_file: !v.is_dir && v.entries_idx.map(|_| true).unwrap_or(true),
                depth: v.depth as i32,
                is_dir: v.is_dir,
                expanded: v.expanded,
                in_multi: multi.contains(&v.path),
            }
        })
        .collect()
}

/// 「内容」页可见条目的路径列表（与 UI 行一一对应）。
/// 条目角色：登记条目取 meta；未登记（文件夹子行等）按文件系统推断。
/// 不存在 → None。
fn entry_role_for(visit: &Visit, path: &str) -> Option<String> {
    if let Some(en) = visit
        .meta
        .as_ref()
        .and_then(|m| m.entries.iter().find(|x| x.path == path))
    {
        return Some(en.role.clone());
    }
    let full = visit.dir.join(path);
    if !full.exists() {
        return None;
    }
    Some(if full.is_dir() {
        "dir".to_string()
    } else {
        "payload".to_string()
    })
}

// 读取当前动作目标（相对路径列表；空 = 无选中）。
fn menu_targets(app: &AppWindow) -> Vec<String> {
    app.global::<EntryApi>()
        .get_menu_targets()
        .as_str()
        .lines()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .collect()
}

/// 动作目标所属分支：右键记录（menu-target-visit）优先，回退激活分支。
fn action_visit<'a>(app: &AppWindow, e: &'a Editor) -> Option<&'a Visit> {
    let vi = match app.global::<EntryApi>().get_menu_target_visit() {
        v if v >= 0 => v as usize,
        _ => e.selected_idx_in_visits()?,
    };
    e.scan.as_ref()?.visits.get(vi)
}

/// 解析拖拽载荷 `"visit|path"`；非法返回 None。
fn parse_entry_transfer(transfer: &str) -> Option<(usize, String)> {
    let (visit, path) = transfer.split_once('|')?;
    let visit = visit.parse::<usize>().ok()?;
    let path = path.trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some((visit, path))
    }
}

/// 解析**批量**拖拽载荷 `"visit|path1\npath2…"`（多选拖拽；兼容单目标旧格式）。
fn parse_entry_transfer_multi(transfer: &str) -> Option<(usize, Vec<String>)> {
    let (visit, rest) = transfer.split_once('|')?;
    let visit = visit.parse::<usize>().ok()?;
    let paths: Vec<String> = rest
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if paths.is_empty() {
        None
    } else {
        Some((visit, paths))
    }
}

/// 计算条目的显示路径：目标目录相对分支根 + 文件名（根目录时仅为文件名）。
fn in_dir_child_path(visit_dir: &Path, target_dir: &Path, name: &str) -> String {
    match target_dir.strip_prefix(visit_dir) {
        Ok(rel) => {
            let rel = rel.to_string_lossy().to_string();
            if rel.is_empty() {
                name.to_string()
            } else {
                format!("{rel}/{name}")
            }
        }
        Err(_) => name.to_string(),
    }
}

/// 展开落地条目沿途的全部父级内容文件夹（粘贴 / 移动 / 新建后目标立即可见）。
/// `rel_dir` = 分支内相对目录路径（如 "a/b"——沿途含它自己全部展开）；
/// `expanded_dirs` 以分支内绝对路径为键，建行模型时读取 ⇒ 必须在 rescan 之前调用。
fn expand_dir_chain(e: &mut Editor, visit_dir: &Path, rel_dir: &str) {
    let mut acc = visit_dir.to_path_buf();
    for seg in rel_dir.split('/') {
        if seg.is_empty() {
            continue;
        }
        acc.push(seg);
        e.expanded_dirs.insert(acc.to_string_lossy().to_string());
    }
}

/// 文件名（去掉父路径前缀）。
fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// 该文本是否**全为拉丁**：不含 CJK / 假名 / 谚文 / 全角 / emoji 等「满高字形」。
///
/// 用途：行内容的 1px 光学居中补偿只对拉丁行生效 —— 满高字形的墨迹本身就接近行盒
/// 中心，再下移反而偏低（CJK 实测会低 1.5px）。
///
/// 判定口径：U+2E80（CJK 部首区起点）之后一律算满高字形 —— 覆盖 CJK 统一表意
/// (4E00)、扩展 A (3400)、兼容 (F900)、假名 (3040)、谚文 (AC00/1100)、全角 (FF00)、
/// emoji 与扩展 B+ (1F300/20000)。**只做一次整数比较、无分配**。
///
/// 性能：在**建模型时**调用（每个可见行一次，字符串长度线性），结果存在 UI 模型的
/// `latin` 布尔字段里，渲染期只读布尔值 ⇒ 无每帧字符串开销。
fn is_latin_only(s: &str) -> bool {
    s.chars().all(|c| (c as u32) < 0x2E80)
}

/// 当前打开 bundle 的根目录（未打开 → None）。
fn e_bundle_root(editor: &Rc<RefCell<Editor>>) -> Option<PathBuf> {
    editor.borrow().bundle.as_ref().map(|b| b.root.clone())
}

/// 从系统剪贴板的文件路径反推应用内条目：角色取自源分支 `.str.toml` 的登记
/// （系统剪贴板是唯一事实来源，内部剪贴板只服务非 macOS 平台）。
/// 路径不在 bundle 内 → None（外部文件走导入分支）。
fn derive_clip(scan: &Scan, src: &Path, is_cut: bool) -> Option<ClipItem> {
    // 找包含 src 的**最深** visit 目录（visit.dir 之间存在嵌套）。
    let mut visit: Option<&Visit> = None;
    for v in &scan.visits {
        if src.starts_with(&v.dir) {
            let deeper = match visit {
                Some(p) => v.dir.as_os_str().len() > p.dir.as_os_str().len(),
                None => true,
            };
            if deeper {
                visit = Some(v);
            }
        }
    }
    let visit = visit?;
    let rel = src
        .strip_prefix(&visit.dir)
        .ok()?
        .to_string_lossy()
        .to_string();
    let name = basename(&rel);
    let entry = visit
        .meta
        .as_ref()
        .and_then(|m| m.entries.iter().find(|x| x.path == rel));
    let role = match entry {
        Some(en) => en.role.clone(),
        None => {
            if src.is_dir() {
                "dir".to_string()
            } else {
                "payload".to_string()
            }
        }
    };
    Some(ClipItem {
        src_path: src.to_path_buf(),
        name,
        role,
        is_cut,
    })
}

/// Finder / 系统噪声文件：隐藏文件（`.DS_Store`、`._*`）、`__MACOSX`、`Thumbs.db`。
fn is_noise_name(name: &str) -> bool {
    name.starts_with('.') || name == "__MACOSX" || name.eq_ignore_ascii_case("Thumbs.db")
}

/// 跨分支整组移动（拖到树分支 / 行级跨分支落点共用）：逐项移动 payload/asset
/// 文件与 `role = "dir"` 的内容文件夹（fs 重命名 + 源登记移除 + 目标登记 upsert，
/// 含重名去重与角色保全；目录登记为 dir + count，子项不逐项登记）；
/// 未登记的磁盘项（含文件夹子行）按文件系统移动后按类型登记。
/// 最后一次性重扫，返回汇总文案（含失败明细）。
/// 落点参数（与单条 `move_entry_across` 同规则）：`dst_vis/at_end/to_index/after`
/// 描述目标面板里的悬停行 —— 文件夹行 = 移入该文件夹；顶层条目行 = 登记到顶层
/// 序列其前/后；其余（末尾/子行）= 顶层序列末尾。结构树分支落点无行概念，传
/// `&[]` 即退化为「登记到末尾」。
fn move_group_to_branch(
    e: &mut Editor,
    bundle: &Bundle,
    src_visit: usize,
    paths: &[String],
    to_visit: usize,
    dst_vis: &[VisibleEntry],
    at_end: bool,
    to_index: usize,
    after: bool,
    into_dir: bool,
) -> Result<String, String> {
    let scan = e.scan.as_ref().ok_or("未打开 bundle")?;
    if src_visit == to_visit {
        return Err("源与目标是同一分支".into());
    }
    if src_visit >= scan.visits.len() || to_visit >= scan.visits.len() {
        return Err("分支无效".into());
    }
    // 源 / 目标分支标题（状态栏「来源 → 目标」用；在移动前一次性取下）。
    let (src_dir, dst_root, src_title, dst_title) = {
        let scan = e.scan.as_ref().unwrap();
        (
            scan.visits[src_visit].dir.clone(),
            scan.visits[to_visit].dir.clone(),
            visit_title(&scan.visits[src_visit]),
            visit_title(&scan.visits[to_visit]),
        )
    };
    let mut sm = read_meta(bundle, &src_dir)?;
    let mut dst_meta = read_meta(bundle, &dst_root)?;
    // 落点解析：目的地目录与顶层插入位（None = 顶层序列末尾）。文件夹行**仅在
    // into_dir（下半区手势）时才是移入其内部**，上半区 = 插到该文件夹之前
    //（与单条 move_entry_across 同规则、与插入指示一致）。
    let hovered = if at_end || to_index >= dst_vis.len() {
        None
    } else {
        dst_vis.get(to_index)
    };
    let (dst_folder, insert_pos) = match hovered {
        Some(row) if row.is_dir && into_dir => (Some(row.drop_dir.clone()), None),
        Some(row) if row.entries_idx.is_some() => {
            let before = dst_vis[..to_index]
                .iter()
                .filter(|v| v.entries_idx.is_some())
                .count();
            (None, Some(before + usize::from(after)))
        }
        _ => (None, None),
    };
    let dst_dir = dst_folder.clone().unwrap_or_else(|| dst_root.clone());
    let mut existing: HashSet<String> = if dst_folder.is_some() {
        std::fs::read_dir(&dst_dir)
            .map(|rd| {
                rd.filter_map(|x| x.ok())
                    .map(|x| x.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        dst_meta.entries.iter().map(|x| x.path.clone()).collect()
    };
    let mut moved: Vec<String> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    for path in paths {
        // 文件夹子行没有 meta 条目：直接按文件系统移动到目标分支根目录。
        let entry = sm.entries.iter().find(|x| x.path == *path).cloned();
        let src_fs = src_dir.join(path);
        match entry {
            // 分支条目（node/branch）只在结构树里呈现，不参与内容迁移。
            Some(entry) if entry.is_branch() => {
                errs.push(format!("{path}：分支条目不能作为内容移动"));
                continue;
            }
            Some(entry) => {
                // 已登记条目：payload/asset 文件，或 `role = "dir"` 的内容文件夹。
                // **文件夹同样可整体移动** —— 与单条 `move_entry_across`、同分支拖拽
                // 同规则（此前这里只放行 payload/asset，用户报告「无法拖拽移动文件夹」）：
                // 目录登记为 dir（带 count），子项不逐项登记。
                let name = dedup_name(&existing, path);
                let dst_fs = dst_dir.join(&name);
                // **必须在移动之前**判定「是不是目录」：`move_file` 之后源路径已不存在，
                // `src_fs.is_dir()` 恒为 false ⇒ 文件夹会被当文件读（「读取失败」）。
                let src_is_dir = src_fs.is_dir();
                if src_is_dir {
                    // 防自吞：目标目录位于源文件夹内部 = 把整棵子树搬进自己。
                    if dst_dir.starts_with(&src_fs) {
                        errs.push(format!("{path}：不能把文件夹移入其自身内部"));
                        continue;
                    }
                    // 已在目标目录内（嵌套分支等极端落点）= 位置未变，不移动文件。
                    if src_fs.parent() == Some(dst_dir.as_path()) {
                        continue;
                    }
                }
                if move_file(&src_fs, &dst_fs).is_err() {
                    errs.push(format!("{path}：移动失败"));
                    continue;
                }
                // 先落盘再读回：目录登记 count，文件登记 size + sha256。
                // 读取失败时保留源登记（文件已在磁盘上，至少不凭空丢清单项）。
                let new_entry = if src_is_dir {
                    let count = std::fs::read_dir(&dst_fs).map(|rd| rd.count()).unwrap_or(0);
                    Entry {
                        path: name.clone(),
                        role: "dir".to_string(),
                        count: Some(count as i64),
                        title: entry.title.clone(),
                        note: entry.note.clone(),
                        ..Default::default()
                    }
                } else {
                    let Ok(bytes) = std::fs::read(&dst_fs) else {
                        errs.push(format!("{path}：读取失败"));
                        continue;
                    };
                    Entry {
                        path: name.clone(),
                        role: entry.role.clone(),
                        title: entry.title.clone(),
                        note: entry.note.clone(),
                        size: Some(bytes.len() as i64),
                        sha256: Some(util::sha256_bytes(&bytes)),
                        ..Default::default()
                    }
                };
                existing.insert(name.clone());
                sm.remove_entry_path(path);
                dst_meta.upsert_entry(&new_entry);
                moved.push(name);
            }
            None => {
                if !src_fs.exists() {
                    errs.push(format!("{path}：源文件不存在"));
                    continue;
                }
                let fs_names: HashSet<String> = std::fs::read_dir(&dst_dir)
                    .map(|rd| {
                        rd.filter_map(|x| x.ok())
                            .map(|x| x.file_name().to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let mut merged = existing.clone();
                merged.extend(fs_names);
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                let name = dedup_name(&merged, &name);
                // 同上：必须在移动之前判定（未登记的文件夹子行也走这条臂）。
                let src_is_dir = src_fs.is_dir();
                if move_file(&src_fs, &dst_dir.join(&name)).is_err() {
                    errs.push(format!("{path}：移动失败"));
                    continue;
                }
                existing.insert(name.clone());
                if src_is_dir {
                    let count = std::fs::read_dir(dst_dir.join(&name))
                        .map(|rd| rd.count())
                        .unwrap_or(0);
                    dst_meta.upsert_entry(&Entry {
                        path: name.clone(),
                        role: "dir".to_string(),
                        count: Some(count as i64),
                        ..Default::default()
                    });
                } else {
                    let bytes = match std::fs::read(dst_dir.join(&name)) {
                        Ok(b) => b,
                        Err(_) => {
                            errs.push(format!("{path}：读取失败"));
                            continue;
                        }
                    };
                    dst_meta.upsert_entry(&Entry {
                        path: name.clone(),
                        role: "payload".to_string(),
                        size: Some(bytes.len() as i64),
                        sha256: Some(util::sha256_bytes(&bytes)),
                        ..Default::default()
                    });
                }
                moved.push(name);
            }
        }
    }
    if !moved.is_empty() {
        // 移入内容文件夹：维护该文件夹条目的 count（不逐项登记，与既有规则一致）。
        if let Some(folder) = &dst_folder {
            let rel = folder
                .strip_prefix(&dst_root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(mut den) = dst_meta.entries.iter().find(|x| x.path == rel).cloned() {
                den.count = Some(std::fs::read_dir(folder).map(|rd| rd.count() as i64).unwrap_or(0));
                dst_meta.upsert_entry(&den);
            }
        }
        // 顶层落点：把整组按原相对顺序插到目标插入位（重写 order）。
        if let Some(pos) = insert_pos {
            let mut seq: Vec<Entry> = dst_meta
                .entries
                .iter()
                .filter(|en| !en.is_branch())
                .cloned()
                .collect();
            let members: Vec<Entry> = moved
                .iter()
                .filter_map(|n| seq.iter().position(|en| en.path == *n).map(|i| seq.remove(i)))
                .collect();
            let at = pos.min(seq.len());
            for (offset, item) in members.into_iter().enumerate() {
                seq.insert(at + offset, item);
            }
            let snapshot = dst_meta.entries.clone();
            for en in snapshot.iter().filter(|en| !en.is_branch()) {
                dst_meta.remove_entry_path(&en.path);
            }
            for (i, mut en) in seq.into_iter().enumerate() {
                en.order = Some(i as i64);
                dst_meta.upsert_entry(&en);
            }
        }
        sm.touch();
        sm.save(&bundle.meta_path(&src_dir))
            .map_err(|err| err.to_string())?;
        dst_meta.touch();
        dst_meta
            .save(&bundle.meta_path(&dst_root))
            .map_err(|err| err.to_string())?;
    }
    // 落入内容文件夹：展开其沿途父级（必须在 rescan 前设置）。
    if let Some(folder) = &dst_folder {
        if let Ok(rel) = folder.strip_prefix(&dst_root) {
            expand_dir_chain(e, &dst_root, &rel.to_string_lossy());
        }
    }
    e.rescan()?;
    // 导图视图：目的地节点的内容面板同步展开，保证落地项可见。
    if let Some(scan) = e.scan.as_ref() {
        e.content_expanded
            .insert(visit_key(scan, to_visit).to_string());
    }
    // 与粘贴同语义：高亮落地的目标项（选区 = 成功移动的**新**路径，主选中 =
    // 第一个）。必须在 rescan 之后设置，否则新路径会被「磁盘不存在」剪除；
    // 目的地是内容文件夹时，新路径 = "文件夹相对路径/文件名"。
    let moved_paths: Vec<String> = moved
        .iter()
        .map(|n| match &dst_folder {
            Some(f) => in_dir_child_path(&dst_root, f, n),
            None => n.clone(),
        })
        .collect();
    if !moved_paths.is_empty() {
        e.entry_multi = moved_paths.iter().cloned().collect();
        e.entry_anchor = None;
        e.selected_entry_path = moved_paths.first().cloned();
    }
    if moved.is_empty() {
        return Err(errs
            .first()
            .cloned()
            .unwrap_or_else(|| "无条目被移动".into()));
    }
    let err_note = if errs.is_empty() {
        String::new()
    } else {
        format!("（{} 项失败：{}）", errs.len(), errs.join("；"))
    };
    if moved.len() == 1 && errs.is_empty() {
        // 单条：来源与目标都点名（目标 = 目的地分支标题）。
        return Ok(format!(
            "已从「{src_title}」移动「{}」→「{dst_title}」。",
            paths[0]
        ));
    }
    Ok(format!(
        "已从「{src_title}」移动 {} 项 →「{dst_title}」：{}{}",
        moved.len(),
        name_list(&moved),
        err_note
    ))
}

/// 磁盘移动（rename 失败退化为 copy + delete，跨卷兜底）。
fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst).map_err(|e| e.to_string())?;
            std::fs::remove_file(src).map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

/// 粘贴 / 制作副本单项落地（**不含重扫**）：返回（提示文案，登记路径）。
/// 登记路径用于粘贴后**激活**被粘贴条目（Finder 语义：粘贴完选中新条目）。
/// 批量粘贴（后台线程，重扫由批量完成回调统一做）与单条粘贴共用。
fn apply_clip_at(
    bundle: &Bundle,
    visit_dir: &Path,
    depth: usize,
    id_version: usize,
    clip: &ClipItem,
) -> Result<(String, String), String> {
    if !clip.src_path.exists() {
        return Err("剪贴板中的源条目已不存在".into());
    }
    let cut = clip.is_cut;
    // 剪切时源、目标不能是同一目录。
    if cut {
        let src_parent = clip.src_path.parent().unwrap_or(Path::new(""));
        if src_parent == visit_dir {
            return Err("源与目标是同一分支".into());
        }
    }
    let mut meta = read_meta(bundle, visit_dir)?;
    let existing: HashSet<String> = meta.entries.iter().map(|x| x.path.clone()).collect();
    // 状态栏「来源 → 目标」用：目标 = 目的地分支标题；来源 = 剪切时的源分支标题。
    // 复制/粘贴的来源是剪贴板本身，故不读源 `.str.toml`（避免在后台批量线程里多一次 IO）。
    let dst_title = meta.title.clone().unwrap_or_default();
    let src_title = if cut {
        clip.src_path
            .parent()
            .and_then(|p| read_meta(bundle, p).ok())
            .and_then(|m| m.title.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let msg;
    let registered;
    match clip.role.as_str() {
        "payload" | "asset" => {
            // clip.name 可能是**子目录内**的相对路径（内容文件夹子行）——
            // 目标文件名只取 basename，否则落点拼回原位（剪切粘贴原地打转）。
            let name = dedup_name(&existing, &basename(&clip.name));
            let dst = visit_dir.join(&name);
            if cut {
                move_file(&clip.src_path, &dst)?;
            } else {
                std::fs::copy(&clip.src_path, &dst).map_err(|err| err.to_string())?;
            }
            let bytes = std::fs::read(&dst).map_err(|err| err.to_string())?;
            meta.upsert_entry(&Entry {
                path: name.clone(),
                role: clip.role.clone(),
                size: Some(bytes.len() as i64),
                sha256: Some(util::sha256_bytes(&bytes)),
                ..Default::default()
            });
            registered = name.clone();
            msg = if cut {
                format!("已从「{src_title}」移动「{name}」→「{dst_title}」。")
            } else {
                format!("已从剪贴板粘贴「{name}」→「{dst_title}」。")
            };
        }
        "dir" => {
            let name = dedup_name(&existing, &basename(&clip.name));
            let dst = visit_dir.join(&name);
            if cut {
                std::fs::rename(&clip.src_path, &dst).map_err(|err| err.to_string())?;
            } else {
                copy_dir_recursive(&clip.src_path, &dst)?;
            }
            let count = std::fs::read_dir(&dst).map(|rd| rd.count()).unwrap_or(0);
            meta.upsert_entry(&Entry {
                path: name.clone(),
                role: "dir".to_string(),
                count: Some(count as i64),
                ..Default::default()
            });
            registered = name.clone();
            msg = if cut {
                format!("已从「{src_title}」移动「{name}/」→「{dst_title}」。")
            } else {
                format!("已从剪贴板粘贴「{name}/」→「{dst_title}」。")
            };
        }
        "node" | "branch" => {
            if cut {
                // 同 bundle 内移动分支：保持 id，仅目录换位 + 父级登记转移。
                let src_meta = read_meta(bundle, &clip.src_path)?;
                let name = clip.name.clone();
                std::fs::rename(&clip.src_path, visit_dir.join(&name))
                    .map_err(|err| err.to_string())?;
                let role = if depth == 0 { "node" } else { "branch" };
                meta.upsert_entry(&Entry {
                    path: name.clone(),
                    role: role.to_string(),
                    id: Some(name.clone()),
                    r#type: src_meta.r#type.clone(),
                    title: src_meta.title.clone(),
                    ..Default::default()
                });
                registered = name.clone();
                msg = format!("已从「{src_title}」移动分支「{name}」→「{dst_title}」。");
            } else {
                let mut mapping = std::collections::HashMap::new();
                let new_id = copy_branch_recursive(
                    bundle,
                    &clip.src_path,
                    visit_dir,
                    depth,
                    id_version,
                    &mut mapping,
                )?;
                let new_meta = read_meta(bundle, &visit_dir.join(&new_id))?;
                let role = if depth == 0 { "node" } else { "branch" };
                meta.upsert_entry(&Entry {
                    path: new_id.clone(),
                    role: role.to_string(),
                    id: Some(new_id.clone()),
                    r#type: new_meta.r#type.clone(),
                    title: new_meta.title.clone(),
                    ..Default::default()
                });
                registered = new_id;
                msg = format!(
                    "已从剪贴板粘贴分支「{}」→「{dst_title}」（新 id「{}」）。",
                    clip.name, registered
                );
            }
        }
        other => return Err(format!("暂不支持 role = {other} 的条目")),
    }
    // 剪切：移除源分支的 entries 登记。
    if cut {
        let src_parent = clip.src_path.parent().ok_or("无法定位源分支目录")?;
        // 仅当源目录是**分支**（有 .str.toml）才需要撤登记 —— 内容文件夹的子行本就
        // 不登记；对无 .str.toml 的目录做 read_meta+save 会凭空把该目录变成分支。
        if src_parent.starts_with(&bundle.root) && src_parent.join(".str.toml").exists() {
            let mut sm = read_meta(bundle, src_parent)?;
            sm.remove_entry_path(&clip.name);
            sm.touch();
            sm.save(&bundle.meta_path(src_parent))
                .map_err(|err| err.to_string())?;
        }
    }
    meta.touch();
    meta.save(&bundle.meta_path(visit_dir))
        .map_err(|err| err.to_string())?;
    Ok((msg, registered))
}

/// 粘贴到**内容文件夹**（文件夹行右键「粘贴」）。
///
/// 内容文件夹不是分支、没有 `.str.toml`，其子项**不登记**（与拖拽移入同规则）：
/// 只做文件系统落地 + 维护该 dir 条目的 count；**不得**对文件夹目录做
/// read_meta+save —— 会凭空把它变成分支。返回成功落地的新路径（分支内相对
/// 路径 "文件夹/文件名"）。剪切成源目录 == 目标文件夹时跳过（原地剪切 = 无效）。
fn paste_into_folder(
    e: &mut Editor,
    clips: &[ClipItem],
    folder_rel: &str,
) -> Result<Vec<String>, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
    let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
    let visit_dir = e.scan.as_ref().unwrap().visits[visit_idx].dir.clone();
    let folder_fs = visit_dir.join(folder_rel);
    if !folder_fs.is_dir() {
        return Err(format!("目标文件夹不存在：{folder_rel}"));
    }
    let mut pasted: Vec<String> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    for clip in clips {
        // 分支（node/branch）不能进入内容文件夹 —— bundle 树只长在分支序列里。
        if !matches!(clip.role.as_str(), "payload" | "asset" | "dir") {
            errs.push(format!("{}：分支不能粘贴到内容文件夹", clip.name));
            continue;
        }
        if !clip.src_path.exists() {
            errs.push(format!("{}：源条目已不存在", clip.name));
            continue;
        }
        // 已在该文件夹内：剪切 = 无效跳过；拷贝 = 走下方去重生成副本。
        if clip.is_cut && clip.src_path.parent() == Some(folder_fs.as_path()) {
            continue;
        }
        let existing: HashSet<String> = std::fs::read_dir(&folder_fs)
            .map(|rd| {
                rd.filter_map(|x| x.ok())
                    .map(|x| x.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let name = dedup_name(&existing, &basename(&clip.name));
        let dst = folder_fs.join(&name);
        if clip.is_cut {
            move_file(&clip.src_path, &dst)?;
        } else if clip.role == "dir" {
            copy_dir_recursive(&clip.src_path, &dst)?;
        } else {
            std::fs::copy(&clip.src_path, &dst).map_err(|err| err.to_string())?;
        }
        // 剪切：源若是**分支**（有 .str.toml）撤登记；内容文件夹子行本就不登记。
        if clip.is_cut {
            if let Some(sp) = clip.src_path.parent() {
                if sp.starts_with(&bundle.root) && sp.join(".str.toml").exists() {
                    let mut sm = read_meta(&bundle, sp)?;
                    sm.remove_entry_path(&clip.name);
                    sm.touch();
                    sm.save(&bundle.meta_path(sp))
                        .map_err(|err| err.to_string())?;
                }
            }
        }
        pasted.push(in_dir_child_path(&visit_dir, &folder_fs, &name));
    }
    // 维护该文件夹条目的 count（分支 meta 里的 dir 条目）。
    if !pasted.is_empty() {
        let mut meta = read_meta(&bundle, &visit_dir)?;
        if let Some(mut den) = meta.entries.iter().find(|x| x.path == folder_rel).cloned() {
            den.count =
                Some(std::fs::read_dir(&folder_fs).map(|rd| rd.count() as i64).unwrap_or(0));
            meta.upsert_entry(&den);
            meta.touch();
            meta.save(&bundle.meta_path(&visit_dir))
                .map_err(|err| err.to_string())?;
        }
    }
    // 展开目标文件夹沿途父级（必须在 rescan 前设置，建行模型时读取）。
    expand_dir_chain(e, &visit_dir, folder_rel);
    e.rescan()?;
    if pasted.is_empty() {
        return Err(errs
            .first()
            .cloned()
            .unwrap_or_else(|| "没有条目被粘贴（均已在目标文件夹内）".into()));
    }
    Ok(pasted)
}

/// 把剪贴板条目落到当前选中分支（含重扫）：单条粘贴 / 制作副本使用。
/// 批量粘贴走 `ContentOp::Paste`（重扫由批量完成回调统一做）。
fn apply_clip(e: &mut Editor, clip: &ClipItem) -> Result<(String, String), String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
    let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
    let (visit_dir, depth, id_version) = {
        let scan = e.scan.as_ref().ok_or("未选择分支")?;
        let v = &scan.visits[visit_idx];
        (
            v.dir.clone(),
            v.depth,
            scan.visits
                .first()
                .and_then(|v| v.meta.as_ref())
                .map(|m| m.policies.id_version)
                .unwrap_or(7),
        )
    };
    let r = apply_clip_at(&bundle, &visit_dir, depth, id_version, clip)?;
    e.rescan()?;
    Ok(r)
}

/// 把外部文件/目录复制进分支并登记条目，返回导入名列表。
///
/// `register = true`：目标为分支目录本身，逐文件登记 payload 条目；
/// `register = false`：目标为分支内的内容文件夹（role = dir），只复制文件并更新其 count。
fn import_paths_into(
    bundle: &Bundle,
    visit_dir: &Path,
    target_dir: &Path,
    register: bool,
    paths: &[PathBuf],
) -> Result<Vec<String>, String> {
    let mut meta = read_meta(bundle, visit_dir)?;
    let mut names = Vec::new();
    if register {
        let mut existing: HashSet<String> = meta.entries.iter().map(|x| x.path.clone()).collect();
        for p in paths {
            let Some(name0) = p.file_name().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            if !p.exists() {
                continue;
            }
            let name = dedup_name(&existing, &name0);
            existing.insert(name.clone());
            let dst = visit_dir.join(&name);
            if p.is_dir() {
                copy_dir_recursive(p, &dst)?;
                let count = std::fs::read_dir(&dst).map(|rd| rd.count()).unwrap_or(0);
                meta.upsert_entry(&Entry {
                    path: name.clone(),
                    role: "dir".to_string(),
                    count: Some(count as i64),
                    ..Default::default()
                });
            } else {
                std::fs::copy(p, &dst).map_err(|e| e.to_string())?;
                let bytes = std::fs::read(&dst).map_err(|e| e.to_string())?;
                meta.upsert_entry(&Entry {
                    path: name.clone(),
                    role: "payload".to_string(),
                    size: Some(bytes.len() as i64),
                    sha256: Some(util::sha256_bytes(&bytes)),
                    ..Default::default()
                });
            }
            names.push(name);
        }
    } else {
        // 内容文件夹：文件不单独登记，仅更新文件夹条目的 count。
        let mut existing: HashSet<String> = std::fs::read_dir(target_dir)
            .map(|rd| {
                rd.filter_map(|x| x.ok())
                    .map(|x| x.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        for p in paths {
            let Some(name0) = p.file_name().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            if !p.exists() {
                continue;
            }
            let name = dedup_name(&existing, &name0);
            existing.insert(name.clone());
            let dst = target_dir.join(&name);
            if p.is_dir() {
                copy_dir_recursive(p, &dst)?;
            } else {
                std::fs::copy(p, &dst).map_err(|e| e.to_string())?;
            }
            names.push(name);
        }
        let rel = target_dir
            .strip_prefix(visit_dir)
            .map_err(|_| "目标目录无效".to_string())?
            .to_string_lossy()
            .to_string();
        if let Some(en) = meta.entries.iter().find(|x| x.path == rel).cloned() {
            let count = std::fs::read_dir(target_dir)
                .map(|rd| rd.count())
                .unwrap_or(0);
            let mut ne = en;
            ne.count = Some(count as i64);
            meta.upsert_entry(&ne);
        }
    }
    if names.is_empty() {
        return Err("没有可导入的文件".into());
    }
    meta.touch();
    meta.save(&bundle.meta_path(visit_dir))
        .map_err(|e| e.to_string())?;
    Ok(names)
}

/// 分支拖拽（下半区 = 插到 target 之后）的执行计划。
/// 目标层级 = **target 的父级**：src 与 target 同父 = 同级重排；
/// 跨父级 = 结构移动（fs 目录移动 + 源父级撤登记 + 新父级按位登记），
/// 因此任意层级之间都能拖（含「移出父级到上一级」）。
enum BranchMoveOp {
    /// 同父级重排：重写 parent_dir 的分支条目 order。
    Reorder { parent_dir: std::path::PathBuf, seq: Vec<Entry> },
    /// 跨层级移动：fs 移动 + 两侧登记（pos = 新父级分支序列中的插入位）。
    Move {
        src_dir: std::path::PathBuf,
        src_parent_dir: std::path::PathBuf,
        new_parent_dir: std::path::PathBuf,
        name: String,
        entry: Entry,
        pos: usize,
    },
}

/// 分支拖拽规划：`src` 拖到 `target` 的后（visits 下标）。
/// 返回 Ok(None) = 顺序不变的无效落点（拖到自身 / 紧邻原位）。
fn branch_move_plan(
    e: &Editor,
    src: usize,
    target: usize,
    after: bool,
) -> Result<Option<BranchMoveOp>, String> {
    let scan = e.scan.as_ref().ok_or("未打开 bundle")?;
    if src >= scan.visits.len() || target >= scan.visits.len() || src == target {
        return Err("分支无效".into());
    }
    // 目标必须是分支行（有父级）；ROOT 行不能作为排序目标。
    let target_parent_idx = scan.visits[target].parent.ok_or("ROOT 不能作为排序目标")?;
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
    let target_parent_dir = scan.visits[target_parent_idx].dir.clone();
    let name_of = |v: usize| {
        scan.visits[v]
            .dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| "无法取得分支 id".to_string())
    };
    let src_name = name_of(src)?;
    let tgt_name = name_of(target)?;
    let src_parent_idx = scan.visits[src].parent.ok_or("ROOT 不可拖动")?;
    let meta = read_meta(bundle, &target_parent_dir)?;
    let mut seq: Vec<Entry> = meta
        .entries
        .iter()
        .filter(|en| en.is_branch())
        .cloned()
        .collect();
    if src_parent_idx == target_parent_idx {
        // 同级重排：移除 src 后按位回插；原位 = 无效点。
        // （tpos 必须在移除 src 之后计算，否则位置未修正。）
        let src_pos = seq
            .iter()
            .position(|en| en.path == src_name)
            .ok_or("未找到源分支条目")?;
        let item = seq.remove(src_pos);
        let tpos = (seq
            .iter()
            .position(|en| en.path == tgt_name)
            .ok_or("未找到目标分支条目")?
            + usize::from(after))
        .min(seq.len());
        if tpos == src_pos {
            return Ok(None);
        }
        seq.insert(tpos, item);
        return Ok(Some(BranchMoveOp::Reorder {
            parent_dir: target_parent_dir,
            seq,
        }));
    }
    // 跨层级移动：新父级不得在源子树内（成环）。
    // （src 不在该序列中，tpos 直接按目标位置计算。）
    let mut cur = Some(target_parent_idx);
    while let Some(i) = cur {
        if i == src {
            return Err("不能把分支移入其自身子树".into());
        }
        cur = scan.visits[i].parent;
    }
    let tpos = (seq
        .iter()
        .position(|en| en.path == tgt_name)
        .ok_or("未找到目标分支条目")?
        + usize::from(after))
    .min(seq.len());
    let src_dir = scan.visits[src].dir.clone();
    let src_meta = read_meta(bundle, &src_dir)?;
    let entry = Entry {
        path: src_name.clone(),
        role: if scan.visits[target_parent_idx].depth == 0 {
            "node"
        } else {
            "branch"
        }
        .to_string(),
        id: src_meta.id.clone(),
        r#type: src_meta.r#type.clone(),
        title: src_meta.title.clone(),
        ..Default::default()
    };
    Ok(Some(BranchMoveOp::Move {
        src_dir,
        src_parent_dir: scan.visits[src_parent_idx].dir.clone(),
        new_parent_dir: target_parent_dir,
        name: src_name,
        entry,
        pos: tpos,
    }))
}

/// 结构树拖拽落点有效性（Slint 侧 `tree-drop-ok`）：同级排序或跨层级移动、
/// 且该边界会实际改变顺序（拖到自身 / 紧邻原位 = 顺序不变的无效点）。
/// ROOT 行下半区特殊：「after ROOT」无意义 → 成为 ROOT 的**第一个子分支**。
fn tree_drop_ok(e: &Editor, src: usize, target: usize, after: bool) -> bool {
    let Some(scan) = e.scan.as_ref() else {
        return false;
    };
    if scan.visits[target].depth == 0 {
        // ROOT 下半区：src 已是 ROOT 的第一个子分支 = 原位无效。
        if scan.visits[src].parent != Some(target) {
            return true;
        }
        let src_name = scan.visits[src]
            .dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        return scan.visits[target]
            .meta
            .as_ref()
            .map(|m| {
                m.entries
                    .iter()
                    .find(|en| en.is_branch() || en.is_link())
                    .map(|en| en.path != src_name)
                    .unwrap_or(true)
            })
            .unwrap_or(false);
    }
    matches!(branch_move_plan(e, src, target, after), Ok(Some(_)))
}

/// 结构树上半区落点有效性（Slint 侧 `tree-nest-ok`）：拖入为子分支。
/// 不能拖到自身、也不能拖进自身子树（成环）；ROOT 自身不可拖动。
fn tree_nest_ok(e: &Editor, src: usize, target: usize) -> bool {
    let Some(scan) = e.scan.as_ref() else {
        return false;
    };
    if src >= scan.visits.len() || target >= scan.visits.len() || src == target {
        return false;
    }
    if scan.visits[src].depth == 0 {
        return false;
    }
    // 目标在源子树内 ⇒ 沿目标祖先链必经 src。
    let mut cur = Some(target);
    while let Some(i) = cur {
        if i == src {
            return false;
        }
        cur = scan.visits[i].parent;
    }
    true
}

// ── 挂载行拖拽（软 / 硬链接）：只动挂载声明，目标真身与数据不动 ──

/// 分支 visit 的磁盘目录名（= 自身 id，`path = id` 语义）。
fn visit_dir_name(scan: &Scan, idx: usize) -> Option<String> {
    scan.visits
        .get(idx)?
        .dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
}

/// 挂载点（任意分支）`entries[]` 的**结构行**：子分支（node/branch）+ 挂载声明
/// （link）。内容条目不参与结构排序。
fn structure_rows(e: &Editor, branch_idx: usize) -> Vec<Entry> {
    let Some(scan) = e.scan.as_ref() else {
        return Vec::new();
    };
    let Some(bundle) = e.bundle.as_ref() else {
        return Vec::new();
    };
    let Ok(meta) = read_meta(bundle, &scan.visits[branch_idx].dir) else {
        return Vec::new();
    };
    meta.entries
        .into_iter()
        .filter(|en| en.is_branch() || en.is_link())
        .collect()
}

/// 结构行重排计划：把 `src_path` 行移到 `tgt_path` 行之后（`after`）/ 之前，
/// 重编 `order`。`None` = 顺序不变的无效边界（拖到自身 / 紧邻原位）。
fn reorder_plan(
    mut seq: Vec<Entry>,
    src_path: &str,
    tgt_path: &str,
    after: bool,
) -> Option<Vec<Entry>> {
    let src_pos = seq.iter().position(|en| en.path == src_path)?;
    let item = seq.remove(src_pos);
    let tpos = (seq
        .iter()
        .position(|en| en.path == tgt_path)?
        + usize::from(after))
    .min(seq.len());
    if tpos == src_pos {
        return None;
    }
    seq.insert(tpos, item);
    for (i, en) in seq.iter_mut().enumerate() {
        en.order = Some(i as i64);
    }
    Some(seq)
}

/// 被拖挂载的声明行：在挂载点 `entries[]` 中按 target 分支 id 定位（软：`path` =
/// target id；硬：`path` = 自身 id —— 同父同目标唯一，`E_LINK_DUP`）。
fn mount_link_entry(
    e: &Editor,
    mount_parent_idx: usize,
    target_visit: usize,
) -> Result<Entry, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
    let scan = e.scan.as_ref().ok_or("未选择分支")?;
    if mount_parent_idx >= scan.visits.len() || target_visit >= scan.visits.len() {
        return Err("分支无效".into());
    }
    let target_id = scan.visits[target_visit]
        .meta
        .as_ref()
        .and_then(|m| m.id.clone())
        .or_else(|| visit_dir_name(scan, target_visit))
        .ok_or("目标分支缺少 id")?;
    let dir = &scan.visits[mount_parent_idx].dir;
    let meta = read_meta(bundle, dir)?;
    meta.entries
        .iter()
        .find(|en| en.is_link() && en.target.as_deref() == Some(target_id.as_str()))
        .cloned()
        .ok_or_else(|| "未找到挂载声明".to_string())
}

/// 挂载行的**实例展开键**（与 emit_branch / mind_dfs / all_branch_keys 同式）：
/// 挂载点 rel ␟ mount ␟ 目标 rel —— 展开收起只作用于本行实例。
/// 挂载行的**实例展开键**（与 emit_branch / mind_dfs / all_branch_keys 同式）：
/// 挂载点 rel ␟ mount[-hard] ␟ 目标 rel —— 展开收起只作用于本行实例。`hard`
/// 维度区分同一挂载点下指向同一目标的软 / 硬链接对（合法配置，二者互不关联）。
fn mount_expand_key(scan: &Scan, mount_parent: usize, target: usize, hard: bool) -> String {
    format!(
        "{}\u{1f}mount{}\u{1f}{}",
        visit_key(scan, mount_parent),
        if hard { "-hard" } else { "" },
        visit_key(scan, target)
    )
}

/// 挂载拖拽落点行的「实际挂载父级」visit：真实行 = 该行自身；软挂载行 = 目标
/// 分支（写入目标结构，与「软链接视图下新建子分支落目标」同规则）；硬链接行 =
/// 自有分支（`link_visit`，与「新建子分支落自有结构」同规则）。
fn mount_drop_parent_visit(
    _scan: &Scan,
    target_visit: usize,
    target_hard: bool,
    target_link_visit: i32,
) -> Option<usize> {
    if target_hard {
        usize::try_from(target_link_visit).ok()
    } else {
        Some(target_visit)
    }
}

/// 挂载行拖拽 — 重挂载落点有效性（Slint 侧 `tree-mount-nest-ok`）：目标行可以是
/// 真实行（落到它之下）、软挂载行（落到其目标分支）或硬链接行（落到其自有结构），
/// 且把挂载目标挂到解析出的挂载点之下不成环（真实父子边 ∪ 挂载边，同
/// `link_would_cycle`，含自我挂载）。
/// 挂载拖拽落点通用守卫：被拖的是**硬链接行**（`src_link_visit` = 自有分支）
/// 时，落点不得位于其自有结构内（含自身）—— 把声明移进自己 = 自嵌套，且
/// `link_would_cycle` 查不到这种环（目标到自有分支无现有边）。返回 true = 合法。
fn mount_drop_not_inside_self(
    scan: &Scan,
    new_parent: usize,
    src_link_visit: i32,
) -> bool {
    let Some(li) = usize::try_from(src_link_visit).ok().filter(|&li| li < scan.visits.len()) else {
        return true;
    };
    let mut cur = Some(new_parent);
    while let Some(i) = cur {
        if i == li {
            return false;
        }
        cur = scan.visits[i].parent;
    }
    true
}

fn tree_mount_nest_ok(
    e: &Editor,
    src_target: usize,
    target_visit: usize,
    target_hard: bool,
    target_link_visit: i32,
    src_link_visit: i32,
) -> bool {
    let Some(scan) = e.scan.as_ref() else {
        return false;
    };
    if src_target >= scan.visits.len() || target_visit >= scan.visits.len() {
        return false;
    }
    let Some(new_parent) =
        mount_drop_parent_visit(scan, target_visit, target_hard, target_link_visit)
    else {
        return false;
    };
    if !mount_drop_not_inside_self(scan, new_parent, src_link_visit) {
        return false;
    }
    !link_would_cycle(scan, new_parent, src_target)
}

/// 挂载行拖拽 — 下半区落点判定（Slint 侧 `tree-mount-drop-ok`），返回动作：
/// - `1` = **同挂载点内重排**：目标行是挂载点的结构兄弟行（真实兄弟行 / 兄弟
///   挂载行），把被拖声明插到它之后；
/// - `2` = **重挂载为该行的第一个子分支**：目标行是不属于同一挂载点的真实行
///   （含挂载父节点自身 —— 行底插入线正在首子之前的位置，落到这里 = 移到最前）；
/// - `0` = 无效（挂载行 / 硬链接行落点、成环、原位边界等）。
#[allow(clippy::too_many_arguments)]
fn tree_mount_drop_ok(
    e: &Editor,
    mount_parent: usize,
    src_target: usize,
    target_visit: usize,
    target_mounted: bool,
    target_hard: bool,
    target_mount_parent: i32,
    target_link_visit: i32,
    after: bool,
    src_link_visit: i32,
) -> i32 {
    let Some(scan) = e.scan.as_ref() else {
        return 0;
    };
    if mount_parent >= scan.visits.len() || src_target >= scan.visits.len() {
        return 0;
    }
    // 目标行在其挂载点 entries[] 里的结构行 path（仅**同挂载点兄弟行**重排需要；
    // 非兄弟链接行走下方重挂载分支 —— 软挂载行落到其目标、硬链接行落到自有结构）。
    let sibling_path = if (target_mounted || target_hard)
        && target_mount_parent == mount_parent as i32
    {
        if target_hard {
            // 硬链接行：结构行 path = 自身分支 id（目录名）。
            usize::try_from(target_link_visit)
                .ok()
                .and_then(|li| visit_dir_name(scan, li))
        } else {
            // 软挂载行：结构行 path = 目标 id（目录名）。
            visit_dir_name(scan, target_visit)
        }
    } else if !(target_mounted || target_hard)
        && scan.visits[target_visit].parent == Some(mount_parent)
    {
        // 真实兄弟行。
        visit_dir_name(scan, target_visit)
    } else {
        None
    };
    let Ok(src_entry) = mount_link_entry(e, mount_parent, src_target) else {
        return 0;
    };
    if let Some(tgt_path) = sibling_path {
        if src_entry.path == tgt_path {
            return 0;
        }
        return reorder_plan(
            structure_rows(e, mount_parent),
            &src_entry.path,
            &tgt_path,
            after,
        )
        .map(|_| 1)
        .unwrap_or(0);
    }
    // 非兄弟行：重挂载为该行解析出的挂载父级（真实行 = 自身；软挂载行 = 目标；
    // 硬链接行 = 自有结构）的**第一个子分支**（成环 / 自我挂载 / 落进被拖硬链接
    // 自身内部拒绝）。
    let Some(new_parent) =
        mount_drop_parent_visit(scan, target_visit, target_hard, target_link_visit)
    else {
        return 0;
    };
    if !mount_drop_not_inside_self(scan, new_parent, src_link_visit) {
        return 0;
    }
    if link_would_cycle(scan, new_parent, src_target) {
        return 0;
    }
    // 新挂载点已有同 path 条目（最典型：目标已是它的真实子分支）⇒ 拒绝。
    if new_parent != mount_parent {
        let Ok(entry) = mount_link_entry(e, mount_parent, src_target) else {
            return 0;
        };
        let Some(bundle) = e.bundle.as_ref() else {
            return 0;
        };
        let Ok(nm) = read_meta(bundle, &scan.visits[new_parent].dir) else {
            return 0;
        };
        if nm.entries.iter().any(|en| en.path == entry.path) {
            return 0;
        }
    }
    2
}

/// 重挂载：把挂载声明（软 / 硬）从 `mount_parent_idx` 摘下、插入为
/// `new_parent_idx` 的**第一个子分支**。硬链接的自有目录随挂载点一起搬家
/// （fs 移动 + kind 随新深度同步）；软链接不动任何磁盘内容。目标分支与数据
/// 始终不动。
fn mount_move_into_child(
    e: &mut Editor,
    mount_parent_idx: usize,
    target_visit: usize,
    new_parent_idx: usize,
) -> Result<String, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
    let scan = e.scan.as_ref().ok_or("未选择分支")?;
    if mount_parent_idx >= scan.visits.len() || new_parent_idx >= scan.visits.len() {
        return Err("落点分支无效".into());
    }
    // 成环：从目标出发沿真实父子边 ∪ 挂载边能走到新挂载点（含自我挂载）⇒ 拒绝。
    if link_would_cycle(scan, new_parent_idx, target_visit) {
        return Err("该挂载会形成环（不能挂到自身或自己的子树内）".into());
    }
    let entry = mount_link_entry(e, mount_parent_idx, target_visit)?;
    let old_parent_dir = scan.visits[mount_parent_idx].dir.clone();
    // 新挂载点已有同 path 条目（最典型：目标本身就是它的真实子分支）⇒
    // upsert 会顶掉既有登记，拒绝。同挂载点重挂（首插）不受影响——移动会
    // 先释放自己的 path。
    if new_parent_idx != mount_parent_idx {
        let nm = read_meta(&bundle, &scan.visits[new_parent_idx].dir)?;
        if nm.entries.iter().any(|en| en.path == entry.path) {
            return Err(format!(
                "「{}」下已存在同路径条目，无法重挂载",
                visit_title(&scan.visits[new_parent_idx])
            ));
        }
    }
    let new_parent_dir = scan.visits[new_parent_idx].dir.clone();
    let new_parent_title = visit_title(&scan.visits[new_parent_idx]);
    let hard = entry.mode.as_deref() == Some("hard");
    let dst_title = visit_title(&scan.visits[target_visit]);
    // 硬链接：自有目录随挂载点搬家（含自有子分支）；落点不得在其自身内部。
    if hard {
        let own_id = entry.id.clone().ok_or("硬链接缺少 id")?;
        let own_dir = old_parent_dir.join(&own_id);
        if new_parent_dir.starts_with(&own_dir) {
            return Err("不能把硬链接移入其自身内部".into());
        }
        let new_depth = scan.visits[new_parent_idx].depth + 1;
        move_file(&own_dir, &new_parent_dir.join(&own_id))?;
        let role = if new_depth == 1 { "node" } else { "branch" };
        sync_moved_branch_kind(&bundle, &new_parent_dir.join(&own_id), role)?;
    }
    // 旧挂载点摘登记 → 新挂载点登记（保留 mode / target / id / 标题等全部字段，
    // 插入为第一个子分支）。
    let mut old_meta = read_meta(&bundle, &old_parent_dir)?;
    old_meta.remove_entry_path(&entry.path);
    old_meta.touch();
    old_meta.save(&bundle.meta_path(&old_parent_dir))
        .map_err(|err| err.to_string())?;
    let mut new_meta = read_meta(&bundle, &new_parent_dir)?;
    new_meta.upsert_entry(&entry);
    make_first_branch(&mut new_meta, &entry.path);
    new_meta.touch();
    new_meta.save(&bundle.meta_path(&new_parent_dir))
        .map_err(|err| err.to_string())?;
    e.rescan()?;
    Ok(if hard {
        format!("已重挂载硬链接「{dst_title}」→「{new_parent_title}」之下（自有目录随迁，目标分支与数据不变）。")
    } else {
        format!("已重挂载：「{dst_title}」→「{new_parent_title}」之下。")
    })
}

/// 挂载行同挂载点内重排：结构行（branch ∪ link）序列中把 `src_path` 移到
/// `dst_path` 之后/之前并重编 `order`。目标分支与数据不动。
fn mount_reorder(
    e: &mut Editor,
    mount_parent_idx: usize,
    src_path: &str,
    dst_path: &str,
    after: bool,
) -> Result<String, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
    let scan = e.scan.as_ref().ok_or("未选择分支")?;
    if mount_parent_idx >= scan.visits.len() {
        return Err("落点分支无效".into());
    }
    let dir = scan.visits[mount_parent_idx].dir.clone();
    let mut meta = read_meta(&bundle, &dir)?;
    let seq: Vec<Entry> = meta
        .entries
        .iter()
        .filter(|en| en.is_branch() || en.is_link())
        .cloned()
        .collect();
    let Some(new_seq) = reorder_plan(seq, src_path, dst_path, after) else {
        return Ok("顺序未变化。".to_string());
    };
    for en in &new_seq {
        meta.remove_entry_path(&en.path);
    }
    for en in new_seq {
        meta.upsert_entry(&en);
    }
    meta.touch();
    meta.save(&bundle.meta_path(&dir))
        .map_err(|err| err.to_string())?;
    e.rescan()?;
    Ok("已调整挂载顺序。".into())
}

/// 移动分支后按新深度同步其自身 `.str.toml` 的 `kind`（深度 1 = node，≥2 = branch）。
/// kind 已正确（深度未变化）时空操作。`role` 为父级登记所用 role 字面量
/// （node/branch，与 moved 新深度的 kind 同规则），否则校验报
/// E_KIND_DEPTH / E_ENTRY_ROLE_DEPTH。
fn sync_moved_branch_kind(bundle: &Bundle, moved_dir: &Path, role: &str) -> Result<(), String> {
    let want = if role == "node" { Kind::Node } else { Kind::Branch };
    let mut m = read_meta(bundle, moved_dir)?;
    if m.kind == Some(want) {
        return Ok(());
    }
    // kind 字段是文档解析视图（touch→resync 会按底层 TOML 重算），
    // 必须经 set_str 写入文档，直接赋值字段无效。
    m.set_str("kind", want.as_str());
    m.touch();
    m.save(&bundle.meta_path(moved_dir)).map_err(|e| e.to_string())
}

/// 「插入为第一个子分支」的 `order` 值：取目标分支现有结构行（branch ∪ link）
/// 的最小 `order` 减一；全无 `order` 时用 1（显示排序中 `None` 视为最大，
/// 新行仍排最前）。末尾位置由「最后一个子分支的下半区」覆盖，二者合起来
/// 子分支序列的所有位置都可达。
/// 把 `path` 指定的行落位为**第一个子分支**：移到子分支区（`branch` / `link`）最前，
/// 并按现行相对顺序对全子分支区重新**连续编号**（0..n-1）。
///
/// 为什么不用 `min - 1` 抢位：规范 §4.6 要求 `order` 为 **≥ 0 的整数**，反复
/// 「移为第一个子分支」会让 `order` 一路减到负值（现场已出现）。重排保持既有
/// 相对顺序不变，代价只是同区其余行的 `order` 值被规整 —— 排序结果（§4.9 按
/// `(order, path)`）不受影响。
fn make_first_branch(meta: &mut Meta, path: &str) {
    // 摘出目标行（其 `order` 将由重排统一给出）。
    let item = match meta.entries.iter().position(|en| en.path == path) {
        Some(i) => meta.entries.remove(i),
        None => return,
    };
    let mut rows: Vec<Entry> = meta
        .entries
        .iter()
        .filter(|en| en.is_branch() || en.is_link())
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        (a.order.unwrap_or(i64::MAX), a.path.as_str()).cmp(&(
            b.order.unwrap_or(i64::MAX),
            b.path.as_str(),
        ))
    });
    rows.insert(0, item);
    for (i, en) in rows.iter_mut().enumerate() {
        en.order = Some(i as i64);
        meta.upsert_entry(en);
    }
}

/// 分支结构移动：把 `src` 分支移入 `target` 分支作为**第一个子分支**
/// （fs 目录移动 + 源父级撤登记 + 目标登记；角色随新深度 node/branch）。
/// 分支自身 `.str.toml` 随目录移动，内部条目相对路径不受影响。
fn branch_move_into_child(e: &mut Editor, src: usize, target: usize) -> Result<String, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
    let scan = e.scan.as_ref().ok_or("未打开 bundle")?;
    if src >= scan.visits.len() || target >= scan.visits.len() || src == target {
        return Err("分支无效".into());
    }
    if scan.visits[src].depth == 0 {
        return Err("ROOT 不可拖动".into());
    }
    let mut cur = Some(target);
    while let Some(i) = cur {
        if i == src {
            return Err("不能把分支移入其自身内部".into());
        }
        cur = scan.visits[i].parent;
    }
    let src_parent_idx = scan.visits[src].parent.ok_or("ROOT 不可拖动")?;
    let src_dir = scan.visits[src].dir.clone();
    let target_dir = scan.visits[target].dir.clone();
    let src_parent_dir = scan.visits[src_parent_idx].dir.clone();
    let target_title = visit_title(&scan.visits[target]);
    let name = src_dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or("无法取得分支 id")?;
    let new_role = if scan.visits[target].depth == 0 { "node" } else { "branch" };
    let _ = scan;

    let src_meta = read_meta(&bundle, &src_dir)?;
    move_file(&src_dir, &target_dir.join(&name))?;
    // 源父级撤登记。
    let mut pm = read_meta(&bundle, &src_parent_dir)?;
    pm.remove_entry_path(&name);
    pm.touch();
    pm.save(&bundle.meta_path(&src_parent_dir))
        .map_err(|err| err.to_string())?;
    // 目标分支登记为第一个子分支（上半区嵌套语义；末尾走最后子分支的下半区）。
    let mut tm = read_meta(&bundle, &target_dir)?;
    tm.upsert_entry(&Entry {
        path: name.clone(),
        role: new_role.to_string(),
        id: src_meta.id.clone(),
        r#type: src_meta.r#type.clone(),
        title: src_meta.title.clone(),
        ..Default::default()
    });
    make_first_branch(&mut tm, &name);
    tm.touch();
    tm.save(&bundle.meta_path(&target_dir))
        .map_err(|err| err.to_string())?;
    // 深度变化 → 同步被移动分支自身 _meta 的 kind。
    sync_moved_branch_kind(&bundle, &target_dir.join(&name), &new_role)?;
    // 展开目标分支（必须在 rescan 前设置：rescan 内部的 rebuild 按展开集合
    // 建行，rescan 之后再插入要等下一次操作才会体现 → 「延迟一步展开」）。
    // visit_key 为 rel 键，跨 rescan 稳定，可取自移动前的 scan。
    if let Some(scan) = e.scan.as_ref() {
        if let Some(i) = scan.visits.iter().position(|v| v.dir == target_dir) {
            e.expanded.insert(visit_key(scan, i).to_string());
        }
    }
    e.rescan()?;
    // 与内容策略对齐：高亮落地的分支（选中）。
    if let Some(scan) = e.scan.as_ref() {
        let new_dir = target_dir.join(&name);
        if let Some(i) = scan.visits.iter().position(|v| v.dir == new_dir) {
            e.selected = Some(visit_key(scan, i).to_string());
        }
    }
    Ok(format!(
        "已把分支「{name}」从「{}」移入「{target_title}」下。",
        title_of_dir(e, &src_parent_dir)
    ))
}

/// 跨节点移动条目：把 src 分支的条目移入 dst 分支，按悬停位置插入目标序列。
/// 落点策略：悬停文件夹行 → 移入该文件夹（不登记顶层）；
/// 悬停目标顶层条目行 → 登记并插到其前/后；其余（末尾/子行）→ 登记到顶层序列末尾。
fn move_entry_across(
    e: &mut Editor,
    src_visit: usize,
    path: &str,
    dst_visit: usize,
    dst_vis: &[VisibleEntry],
    at_end: bool,
    to_index: usize,
    after: bool,
    into_dir: bool,
) -> Result<String, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
    let (src_dir, dst_dir, src_title, dst_title) = {
        let scan = e.scan.as_ref().ok_or("未打开 bundle")?;
        if src_visit >= scan.visits.len() || dst_visit >= scan.visits.len() {
            return Err("源或目标分支无效".into());
        }
        (
            scan.visits[src_visit].dir.clone(),
            scan.visits[dst_visit].dir.clone(),
            visit_title(&scan.visits[src_visit]),
            visit_title(&scan.visits[dst_visit]),
        )
    };
    let src_meta = read_meta(&bundle, &src_dir)?;
    let dst_meta = read_meta(&bundle, &dst_dir)?;

    // 目的地目录与插入位（None = 目标顶层序列末尾）。文件夹行**仅在 into_dir
    // （下半区手势）时才是移入其内部**：上半区落点 = 插到该文件夹之前（与插入
    // 指示一致——此前无视 into_dir，指示显示插入却落进了文件夹）。
    let hovered = if at_end || to_index >= dst_vis.len() {
        None
    } else {
        dst_vis.get(to_index)
    };
    let (dst_folder, insert_pos) = match hovered {
        Some(row) if row.is_dir && into_dir => (Some(row.drop_dir.clone()), None),
        Some(row) if row.entries_idx.is_some() => {
            let before = dst_vis[..to_index]
                .iter()
                .filter(|v| v.entries_idx.is_some())
                .count();
            (None, Some(before + usize::from(after)))
        }
        _ => (None, None),
    };

    let registered = src_meta.entries.iter().find(|x| x.path == path).cloned();

    let existing: HashSet<String> = dst_meta.entries.iter().map(|x| x.path.clone()).collect();
    let base_name = std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let name = dedup_name(&existing, &base_name);
    let dest_dir = dst_folder.clone().unwrap_or_else(|| dst_dir.clone());

    let src_fs = src_dir.join(path);
    if !src_fs.exists() {
        return Err("源文件不存在".into());
    }
    // **必须在移动之前**判定（`move_file` 之后源路径已不存在，`is_dir()` 恒 false
    // ⇒ 未登记的文件夹会被当文件读 → 「读取失败」）。
    let src_is_dir = src_fs.is_dir();
    move_file(&src_fs, &dest_dir.join(&name))?;

    // 源分支：移除顶层登记（文件夹子行本就无登记）。
    if registered.is_some() {
        let mut sm = src_meta;
        sm.remove_entry_path(path);
        sm.touch();
        sm.save(&bundle.meta_path(&src_dir))
            .map_err(|err| err.to_string())?;
    }

    let mut dm = dst_meta;
    let mut highlight = name.clone();
    match (registered.as_ref(), dst_folder.as_ref()) {
        (Some(entry), None) => {
            // 登记到目标顶层序列的插入位。
            let ne = if entry.role == "dir" {
                let count = std::fs::read_dir(dest_dir.join(&name))
                    .map(|rd| rd.count())
                    .unwrap_or(0);
                Entry {
                    path: name.clone(),
                    role: "dir".to_string(),
                    count: Some(count as i64),
                    title: entry.title.clone(),
                    note: entry.note.clone(),
                    ..Default::default()
                }
            } else {
                let bytes = std::fs::read(dest_dir.join(&name)).map_err(|err| err.to_string())?;
                Entry {
                    path: name.clone(),
                    role: entry.role.clone(),
                    title: entry.title.clone(),
                    note: entry.note.clone(),
                    size: Some(bytes.len() as i64),
                    sha256: Some(util::sha256_bytes(&bytes)),
                    ..Default::default()
                }
            };
            let mut seq: Vec<Entry> = dm
                .entries
                .iter()
                .filter(|en| !en.is_branch())
                .cloned()
                .collect();
            seq.insert(insert_pos.unwrap_or(seq.len()).min(seq.len()), ne);
            let snapshot = dm.entries.clone();
            for en in snapshot.iter().filter(|en| !en.is_branch()) {
                dm.remove_entry_path(&en.path);
            }
            for (i, mut en) in seq.into_iter().enumerate() {
                en.order = Some(i as i64);
                dm.upsert_entry(&en);
            }
        }
        (_, Some(folder)) => {
            // 移入目标分支内的文件夹：更新该文件夹条目的 count。
            if let Ok(rel) = folder.strip_prefix(&dst_dir) {
                let rel = rel.to_string_lossy().to_string();
                if let Some(den) = dm
                    .entries
                    .iter()
                    .find(|x| x.path == rel && x.role == "dir")
                    .cloned()
                {
                    let count = std::fs::read_dir(folder).map(|rd| rd.count()).unwrap_or(0);
                    let mut ne = den;
                    ne.count = Some(count as i64);
                    dm.upsert_entry(&ne);
                }
                if registered.is_some() {
                    highlight = format!("{rel}/{name}");
                }
            }
        }
        (None, None) => {
            // 文件夹子行移到目标根目录：按类型登记。
            if src_is_dir {
                let count = std::fs::read_dir(dest_dir.join(&name))
                    .map(|rd| rd.count())
                    .unwrap_or(0);
                dm.upsert_entry(&Entry {
                    path: name.clone(),
                    role: "dir".to_string(),
                    count: Some(count as i64),
                    ..Default::default()
                });
            } else {
                let bytes = std::fs::read(dest_dir.join(&name)).map_err(|err| err.to_string())?;
                dm.upsert_entry(&Entry {
                    path: name.clone(),
                    role: "payload".to_string(),
                    size: Some(bytes.len() as i64),
                    sha256: Some(util::sha256_bytes(&bytes)),
                    ..Default::default()
                });
            }
        }
    }

    dm.touch();
    dm.save(&bundle.meta_path(&dst_dir))
        .map_err(|err| err.to_string())?;
    // 落入内容文件夹：展开其沿途父级（必须在 rescan 前设置）。
    if let Some(folder) = &dst_folder {
        if let Ok(rel) = folder.strip_prefix(&dst_dir) {
            expand_dir_chain(e, &dst_dir, &rel.to_string_lossy());
        }
    }
    e.rescan()?;
    // 导图视图：目的地节点的内容面板同步展开，保证落地项可见。
    if let Some(scan) = e.scan.as_ref() {
        e.content_expanded
            .insert(visit_key(scan, dst_visit).to_string());
    }
    e.selected_entry_path = Some(highlight);
    Ok(format!("已从「{src_title}」移动「{path}」→「{dst_title}」。"))
}

/// 剪贴板 AppleScript 的落盘与执行：脚本按 key 缓存到临时目录（每进程一份），
/// 之后每次调用只起 osascript 进程。
///
/// **必须用脚本文件，不能用多段 `-e`**：实测 `osascript -e … -e … -- args`
/// 会不可靠地丢弃尾部参数——3 个路径只写进剪贴板 2 个（正是「复制数量正确、
/// 粘贴数量不对」的根源：粘贴优先读系统剪贴板）。文件形式
/// （`osascript /path/to/script arg…`）经对照实验往返完全一致。
#[cfg(target_os = "macos")]
fn clipboard_script_path(key: &'static str) -> Result<PathBuf, String> {
    static CACHE: std::sync::OnceLock<
        Result<std::collections::HashMap<&'static str, PathBuf>, String>,
    > = std::sync::OnceLock::new();
    let map = CACHE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("str-gui-osascript-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut map = std::collections::HashMap::new();
        for (k, s) in [
            (CLIP_WRITE_KEY, CLIP_WRITE_SCRIPT),
            (CLIP_READ_KEY, CLIP_READ_SCRIPT),
            (CLIP_CLEAR_KEY, CLIP_CLEAR_SCRIPT),
            (CLIP_CUT_READ_KEY, CLIP_CUT_READ_SCRIPT),
            (REVEAL_KEY, REVEAL_SCRIPT),
            (CLIP_TEXT_KEY, CLIP_TEXT_SCRIPT),
        ] {
            let p = dir.join(k);
            std::fs::write(&p, s).map_err(|e| e.to_string())?;
            map.insert(k, p);
        }
        Ok(map)
    });
    map.as_ref()
        .map(|m| m.get(key).cloned().unwrap_or_default())
        .map_err(|e| e.clone())
}

const CLIP_WRITE_KEY: &str = "clipboard-write.applescript";
const CLIP_READ_KEY: &str = "clipboard-read.applescript";

const CLIP_WRITE_SCRIPT: &str = r#"use framework "Foundation"
use scripting additions
-- argv[0] = 剪切标记（"0" 拷贝 / "1" 剪切），argv[1..] = 源文件路径。
-- 剪切标记作为**首个 item 的附加表示**写入：单独 setString:forType: 会失败。
on run argv
    set cutFlag to (item 1 of argv) as text
    set itemsList to current application's NSMutableArray's array()
    set firstItem to true
    repeat with i from 2 to (count of argv)
        set p to (item i of argv)
        set fileUrl to (current application's NSURL's fileURLWithPath:p)
        set rep to (fileUrl's dataRepresentation)
        set oneItem to (current application's NSPasteboardItem's alloc()'s init())
        (oneItem's setData:rep forType:(current application's NSPasteboardTypeFileURL))
        if firstItem then
            (oneItem's setString:cutFlag forType:"com.str.str-gui.cut")
            set firstItem to false
        end if
        (itemsList's addObject:oneItem)
    end repeat
    set pb to current application's NSPasteboard's generalPasteboard()
    pb's clearContents()
    pb's writeObjects:itemsList
    delay 0.5
end run
"#;

const CLIP_READ_SCRIPT: &str = r#"use framework "Foundation"
use scripting additions
on run
    set pb to current application's NSPasteboard's generalPasteboard()
    set pbItems to pb's pasteboardItems()
    set out to ""
    repeat with i from 1 to (count of pbItems)
        set oneItem to (pbItems's objectAtIndex:(i - 1))
        set rep to (oneItem's dataForType:(current application's NSPasteboardTypeFileURL))
        if rep is not missing value then
            set fileUrl to (current application's NSURL's alloc()'s initWithDataRepresentation:rep relativeToURL:(missing value))
            set out to out & ((fileUrl's |path|()) as text) & linefeed
        end if
    end repeat
    return out
end run
"#;

/// 剪切标记（自定义 pasteboard 类型）：macOS 文件没有系统级「剪切」，
/// 应用内剪切用该标记携带（作为**首个粘贴板 item 的附加表示**随 URL 一起写入——
/// 单独在 writeObjects 之后 setString:forType: 会返回 false 且标记丢失）；
/// 跨应用粘贴时其它程序会忽略它。读取见 CLIP_CUT_READ_*。

/// 清空系统剪贴板（剪切粘贴成功后调用，Finder 语义：剪切的条目粘贴一次即失效）。
const CLIP_CLEAR_KEY: &str = "clipboard-clear.applescript";
const CLIP_CLEAR_SCRIPT: &str = r#"use framework "Foundation"
use scripting additions
on run
    current application's NSPasteboard's generalPasteboard()'s clearContents()
end run
"#;

/// 读取剪切标记：无标记返回 "0"，有标记返回 "1"。
const CLIP_CUT_READ_KEY: &str = "clipboard-cut-read.applescript";
const CLIP_CUT_READ_SCRIPT: &str = r#"use framework "Foundation"
use scripting additions
on run
    set pb to current application's NSPasteboard's generalPasteboard()
    set m to (pb's stringForType:"com.str.str-gui.cut")
    if m is missing value then return "0"
    return m as text
end run
"#;

/// 把文件/目录写入 macOS 系统剪贴板（与 Finder 拷贝同格式）。
/// `cut` = 是否打剪切标记（应用内剪切：粘贴时据此执行移动）。
#[cfg(target_os = "macos")]
fn mac_write_clipboard_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    let script = clipboard_script_path(CLIP_WRITE_KEY)?;
    let mut args: Vec<String> = vec![if cut { "1".into() } else { "0".into() }];
    args.extend(
        paths
            .iter()
            .map(|p| p.to_str().unwrap_or_default().to_string()),
    );
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut cmd = std::process::Command::new("osascript");
    cmd.arg(&script).args(&arg_refs);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        if debug_on() {
            eprintln!("[str-gui] mac clipboard write {} 项", paths.len());
        }
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// 读取 macOS 系统剪贴板中的文件路径（NSURL 对象），无文件类内容时返回空。
#[cfg(target_os = "macos")]
fn mac_read_clipboard_files() -> Result<Vec<PathBuf>, String> {
    let script = clipboard_script_path(CLIP_READ_KEY)?;
    let out = {
        let mut cmd = std::process::Command::new("osascript");
        cmd.arg(&script);
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            String::from_utf8_lossy(&out.stdout).to_string()
        } else {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
    };
    let paths: Vec<PathBuf> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    if debug_on() {
        eprintln!("[str-gui] mac clipboard read {} 项", paths.len());
    }
    Ok(paths)
}

/// 清空系统剪贴板（剪切条目粘贴成功后调用，Finder 语义：粘贴一次即失效）。
#[cfg(target_os = "macos")]
fn mac_clear_clipboard() -> Result<(), String> {
    let script = clipboard_script_path(CLIP_CLEAR_KEY)?;
    let out = std::process::Command::new("osascript")
        .arg(&script)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// 读取系统剪贴板上的「剪切」标记（无标记 = 拷贝）。
#[cfg(target_os = "macos")]
fn mac_read_clipboard_cut() -> bool {
    let script = match clipboard_script_path(CLIP_CUT_READ_KEY) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let out = match std::process::Command::new("osascript")
        .arg(&script)
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => return false,
    };
    out == "1"
}

// ── 外观：进程级配色 + 向 OS 上报原生外观 ───────────────────────────────────
// Widget 侧主题（含菜单弹出层）由「进程级 ColorScheme」驱动：它经
// `ColorSchemeSelector.color-scheme ← SlintInternal.color-scheme` 最终落到
// `FluentPalette.background` 等。仅写 .slint 的 `Palette.color-scheme` 不够 ——
// 它的来源是后端上报的系统值，时序不定会让菜单在 #1C1C1C（近黑）与浅色间抖动（偶发黑底）。
// 故由 Rust 经 Window::set_color_scheme 显式设定该进程级值（确定性来源）；原生窗口装饰
// （标题栏）macOS 上 Slint 无通路，仍走 AppKit 的 NSAppearance。

// 与 .slint 的 `appearance-mode` 保持一致
const APPEARANCE_SYSTEM: i32 = 0;
const APPEARANCE_LIGHT: i32 = 1;
const APPEARANCE_DARK: i32 = 2;

/// 统一入口：先设进程级配色（驱动 FluentPalette/菜单背景），再上报原生外观。
fn apply_appearance(app: &AppWindow, mode: i32) {
    use i_slint_core::items::ColorScheme;
    // 跟随系统：必须用「真实的系统值」，绝不能传 Unknown —— FluentPalette.dark-color-scheme
    // 对 unknown 会走 `unknown == dark → false`，即强制浅色，反而破坏跟随系统。
    let scheme = match mode {
        APPEARANCE_DARK => ColorScheme::Dark,
        APPEARANCE_LIGHT => ColorScheme::Light,
        APPEARANCE_SYSTEM | _ => {
            if system_prefers_dark() {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            }
        }
    };
    if debug_on() {
        eprintln!("[str-gui] apply_appearance mode={mode} scheme={scheme:?}");
    }
    app.window().set_color_scheme(scheme);
    report_app_appearance_native(mode);
}

#[cfg(target_os = "macos")]
fn report_app_appearance_native(mode: i32) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSApplication,
    };

    // 只设置**窗口级** `NSWindow.appearance`，不碰 `NSApp.appearance`：
    // 标题栏 / 交通灯与窗口内容一同明暗；而主菜单栏与 muda 生成的菜单弹窗
    // 仍走 NSApp（nil → 跟随系统），不会出现「被强制外观」与系统冲突的闪烁。
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let appearance = unsafe {
        match mode {
            APPEARANCE_DARK => NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua),
            APPEARANCE_LIGHT => NSAppearance::appearanceNamed(NSAppearanceNameAqua),
            // 跟随系统：清空强制外观（nil → 窗口跟随系统）。
            _ => None,
        }
    };
    let windows = NSApplication::sharedApplication(mtm).windows();
    if windows.is_empty() {
        // 窗口尚未创建：等下一轮复查（调用方为 2s 周期）。
        return;
    }
    std::thread_local! {
        static APPLIED: std::cell::Cell<i32> = const { std::cell::Cell::new(i32::MIN) };
    }
    if APPLIED.with(|c| c.get()) == mode {
        return;
    }
    for window in windows.iter() {
        window.setAppearance(appearance.as_deref());
    }
    APPLIED.with(|c| c.set(mode));
}

#[cfg(not(target_os = "macos"))]
fn report_app_appearance_native(_mode: i32) {}

/// 系统当前是否为深色外观（macOS 走 NSApp 的 effectiveAppearance）。
#[cfg(target_os = "macos")]
fn system_prefers_dark() -> bool {
    use objc2_app_kit::{NSAppearanceNameDarkAqua, NSApplication};

    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return false;
    };
    let name = NSApplication::sharedApplication(mtm)
        .effectiveAppearance()
        .name();
    // NSAppearanceName 是 NSString 的子类，比较走 isEqualToString
    unsafe { name.isEqualToString(NSAppearanceNameDarkAqua) }
}

#[cfg(not(target_os = "macos"))]
fn system_prefers_dark() -> bool {
    false
}

/// `STR_DEBUG=1` 打开诊断输出（与 vendored winit 的拖放/主题诊断同一开关）。
/// 用于在无法本地复现的平台上定位「事件有没有到、落点算在哪、外观有没有推下去」。
fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("STR_DEBUG").is_some())
}

/// macOS：实时读取物理修饰键（系统级真值，不依赖 Slint 的键盘事件跟踪）。
/// 返回（切换键 = ⌘，范围键 = Shift）。
///
/// Slint 的 `PointerEvent.modifiers` 来自内核对修饰键 KeyboardInput 的登记，
/// ⌘+点击场景下时序/合成事件会导致读到 false——⌘ 点击因此退化为单选替换
/// （选区永远单条，复制/粘贴/删除/拖拽的数量全部错误）。NSEvent 类方法是
/// AppKit 当前的实际按键状态，无此依赖；回调本身就在主线程执行，直接可用。
#[cfg(target_os = "macos")]
fn native_toggle_shift() -> (bool, bool) {
    use objc2_app_kit::{NSEvent, NSEventModifierFlags};
    // 注意：不要在这里调 CGEventSourceFlagsState —— 在 AppKit 事件分发回调内
    // 调它会拿不到锁、把整个事件循环卡死（真机实测：普通按键都失去响应）。
    let flags = NSEvent::modifierFlags_class();
    (
        flags.contains(NSEventModifierFlags::Command),
        flags.contains(NSEventModifierFlags::Shift),
    )
}

// ── macOS 快速查看（Quick Look）：空格键调起系统预览面板 ──
//
// Finder 语义：选中一个或多个内容条目后按空格，弹出系统 `QLPreviewPanel`
// （多条目可在面板内左右切换，Esc 关闭）。走系统面板而非自绘预览窗，是为了
// 复用系统已注册的 Quick Look 扩展（图片 / PDF / 音视频 / 代码 / Office…），
// 观感也与 Finder 一致。
//
// 三个必须遵守的约定：
//   1. 面板的 `dataSource` 是弱引用 ⇒ Rust 侧必须强持有，否则回调落到已释放对象；
//   2. 每个预览项是独立的 `QLItem`（实现 QLPreviewItem 的 `previewItemURL`）——
//      不能让数据源既当数据源又当条目（index 无处携带）；
//   3. 只认 file:// NSURL，路径必须是绝对路径。
#[cfg(target_os = "macos")]
mod quicklook {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, NSObject};
    use objc2::{define_class, msg_send, AnyThread, DefinedClass};
    use objc2_foundation::{NSString, NSURL};
    use std::cell::RefCell;
    use std::path::PathBuf;

    // QLPreviewPanel 位于 QuickLookUI.framework（Quartz 伞框架的子框架）。
    #[link(name = "QuickLookUI", kind = "framework")]
    extern "C" {}

    /// 一个预览项：QLPreviewItem 协议唯一必需的方法 `previewItemURL`。
    #[derive(Clone)]
    struct ItemIvars {
        url: Retained<NSURL>,
    }

    define_class!(
        // SAFETY: 父类 NSObject 无子类化约束；本类不实现 Drop。
        #[unsafe(super(NSObject))]
        #[name = "StrQuickLookItem"]
        #[ivars = ItemIvars]
        struct QLItem;

        impl QLItem {
            #[unsafe(method_id(previewItemURL))]
            fn preview_item_url(&self) -> Retained<NSURL> {
                self.ivars().url.clone()
            }
        }
    );

    impl QLItem {
        fn new(path: &PathBuf) -> Retained<Self> {
            let name = NSString::from_str(&path.to_string_lossy());
            let this = Self::alloc().set_ivars(ItemIvars {
                url: NSURL::fileURLWithPath(&name),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    /// 面板数据源：QLPreviewPanelDataSource + delegate 的控制权方法。
    #[derive(Clone)]
    struct SourceIvars {
        items: Vec<Retained<QLItem>>,
    }

    define_class!(
        // SAFETY: 同上。全部方法只被 AppKit 在主线程调用。
        #[unsafe(super(NSObject))]
        #[name = "StrQuickLookSource"]
        #[ivars = SourceIvars]
        struct QLSource;

        impl QLSource {
            #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
            fn number_of_items(&self, _panel: &AnyObject) -> isize {
                self.ivars().items.len() as isize
            }

            #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
            fn item_at(&self, _panel: &AnyObject, index: isize) -> Option<Retained<QLItem>> {
                self.ivars().items.get(index as usize).cloned()
            }

            // 显式接管面板控制权：否则系统会沿 responder 链另找数据源，面板可能空白。
            #[unsafe(method(acceptsPreviewPanelControl:))]
            fn accepts_control(&self, _panel: &AnyObject) -> bool {
                true
            }

            #[unsafe(method(beginPreviewPanelControl:))]
            fn begin_control(&self, panel: &AnyObject) {
                unsafe {
                    let _: () = msg_send![panel, setDataSource: self];
                    let _: () = msg_send![panel, setDelegate: self];
                    let _: () = msg_send![panel, reloadData];
                }
            }

            #[unsafe(method(endPreviewPanelControl:))]
            fn end_control(&self, panel: &AnyObject) {
                unsafe {
                    let _: () = msg_send![panel, setDataSource: Option::<&AnyObject>::None];
                }
            }
        }
    );

    impl QLSource {
        fn new(items: Vec<Retained<QLItem>>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(SourceIvars { items });
            unsafe { msg_send![super(this), init] }
        }
    }

    thread_local! {
        /// 面板只弱引用数据源 ⇒ Rust 侧强持有（见模块注释约定 1）。
        static SOURCE: RefCell<Option<Retained<QLSource>>> = const { RefCell::new(None) };
    }

    /// 打开（或**就地更新**）预览面板：已可见时只换数据源与当前项，不重复弹窗。
    ///
    /// 用 `orderFront:` 而不是 `makeKeyAndOrderFront:`——后者会让面板成为 key
    /// window，应用的方向键随即被它吃掉（用它翻页），而本应用要求**预览期间
    /// 方向键仍然归内容列表**（↑↓ 移动选中、→← 展开收起）。不抢 key 的代价是
    /// 面板自己收不到 Esc / 空格，这两者改由应用接（Finder 的空格切换语义）。
    pub(super) fn show(paths: &[PathBuf]) -> Result<(), String> {
        let Some(_mtm) = objc2::MainThreadMarker::new() else {
            return Err("不在主线程".into());
        };
        let Some(cls) = AnyClass::get(c"QLPreviewPanel") else {
            return Err("系统 Quick Look 不可用".into());
        };
        if paths.is_empty() {
            return Err("没有可预览的条目".into());
        }
        let items: Vec<Retained<QLItem>> = paths.iter().map(QLItem::new).collect();
        let source = QLSource::new(items);
        SOURCE.with(|c| *c.borrow_mut() = Some(source.clone()));
        let Some(panel): Option<Retained<AnyObject>> =
            (unsafe { msg_send![cls, sharedPreviewPanel] })
        else {
            return Err("无法创建 Quick Look 面板".into());
        };
        unsafe {
            let _: () = msg_send![&panel, setDataSource: &*source];
            let _: () = msg_send![&panel, setDelegate: &*source];
            // NSPanel：只在需要时才成为 key。QLPreviewPanel 在应用激活时会自动
            // becomeKey 抢走键盘 —— 预览期间方向键必须归内容列表，故关掉这条路径。
            if let Some(nspanel) = AnyClass::get(c"NSPanel") {
                let is_panel: bool = msg_send![&panel, isKindOfClass: nspanel];
                if is_panel {
                    let _: () = msg_send![&panel, setBecomesKeyOnlyIfNeeded: true];
                }
            }
            // reloadData / 定位项必须在面板**显示之后**：显示前调用
            // `setCurrentPreviewItemIndex` 实测会让面板整个不出现（真机复现：
            // 状态栏报成功、屏幕无面板）。
            let visible: bool = msg_send![&panel, isVisible];
            if !visible {
                let _: () = msg_send![&panel, orderFront: Option::<&AnyObject>::None];
            }
            let _: () = msg_send![&panel, reloadData];
            let _: () = msg_send![&panel, setCurrentPreviewItemIndex: 0isize];
            // 把 key window 还给应用主窗口（面板仍在最前，只是不持有键盘）。
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                restore_key_window(mtm);
            }
        }
        Ok(())
    }

    /// 把 key window 还给应用主窗口（`QLPreviewPanel` 之外的第一个可见窗口）。
    unsafe fn restore_key_window(mtm: objc2::MainThreadMarker) {
        use objc2::runtime::NSObjectProtocol as _;
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let ql = AnyClass::get(c"QLPreviewPanel");
        for w in app.windows().iter() {
            if let Some(ql) = ql {
                if w.isKindOfClass(ql) {
                    continue;
                }
            }
            if w.isVisible() {
                let _: () = msg_send![&w, makeKeyAndOrderFront: Option::<&AnyObject>::None];
                return;
            }
        }
    }

    /// 关闭预览面板。
    pub(super) fn close() {
        let Some(cls) = AnyClass::get(c"QLPreviewPanel") else {
            return;
        };
        let Some(panel): Option<Retained<AnyObject>> =
            (unsafe { msg_send![cls, sharedPreviewPanel] })
        else {
            return;
        };
        unsafe {
            let _: () = msg_send![&panel, orderOut: Option::<&AnyObject>::None];
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod quicklook {
    use std::path::PathBuf;

    pub(super) fn show(_paths: &[PathBuf]) -> Result<(), String> {
        Err("快速查看仅支持 macOS（系统 Quick Look 面板）。".into())
    }

    pub(super) fn close() {}
}

// ── 应用配置：最近打开的 bundle + 欢迎引导标记 ──
// 存放在用户配置目录（应用级状态，不写进任何 bundle —— bundle 里只放资源数据）。

/// 配置目录：macOS = `~/Library/Application Support/str-gui`；
/// Windows = `%APPDATA%\str-gui`；其余 = `$XDG_CONFIG_HOME/str-gui` 或 `~/.config/str-gui`。
fn app_config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support/str-gui"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|h| PathBuf::from(h).join("str-gui"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("str-gui"))
    }
}

/// 最近列表容量上限（超出即丢弃最旧的）。
const RECENT_MAX: usize = 10;

/// 打开路径的**规范形式**：绝对化 + 解析 `..` / 符号链接（macOS `/tmp` →
/// `/private/tmp`）。最近列表只存规范路径 —— `str-gui .` 这类相对参数若原样入库，
/// 换个工作目录启动就再也打不开；同一 bundle 也不会因写法不同出现两个条目。
/// 规范化失败（目录刚被删等）时退回「绝对化的原始路径」。
fn canonical_bundle_path(raw: &Path) -> PathBuf {
    std::fs::canonicalize(raw).unwrap_or_else(|_| {
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(raw)
        }
    })
}

/// 读取最近打开列表（新→旧；自动剔除已不存在的路径）。
/// 历史遗留的**非绝对路径**行（修复前的 `str-gui .` 产物）直接丢弃：原意图不可考，
/// 按「当前工作目录」解析只会得到错误的目录。
fn recent_paths() -> Vec<PathBuf> {
    match app_config_dir() {
        Some(dir) => recent_paths_in(&dir),
        None => Vec::new(),
    }
}

fn recent_paths_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(dir.join("recent.txt")) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| canonical_bundle_path(&p))
        .filter(|p| p.is_dir() && seen.insert(p.display().to_string()))
        .take(RECENT_MAX)
        .collect()
}

/// 把路径提到最近列表最前（去重、截断到上限），并落盘。
/// 入库前统一规范化为绝对路径 —— 这是最近列表唯一的写入口，四个打开入口
/// （路径参数 / 对话框 / 最近列表 / 拖拽）都在这里收敛。
fn push_recent(path: &Path) {
    if let Some(dir) = app_config_dir() {
        push_recent_in(&dir, path);
    }
}

fn push_recent_in(dir: &Path, path: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = canonical_bundle_path(path);
    let mut list: Vec<PathBuf> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("recent.txt")) {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let p = canonical_bundle_path(Path::new(line));
            if !list.contains(&p) {
                list.push(p);
            }
        }
    }
    list.insert(0, path);
    list.truncate(RECENT_MAX);
    let body = list
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(dir.join("recent.txt"), body + "\n");
}

fn clear_recent() {
    if let Some(dir) = app_config_dir() {
        let _ = std::fs::remove_file(dir.join("recent.txt"));
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // macOS：原生应用菜单自带「关于」，其面板信息由 vendored Slint 的 extra 补丁
    // 在建菜单时从环境变量读取 —— 必须在 AppWindow::new()（建菜单）之前设置。
    #[cfg(target_os = "macos")]
    {
        std::env::set_var("STR_ABOUT_NAME", "STR 编辑器");
        std::env::set_var(
            "STR_ABOUT_VERSION",
            format!(
                "v{}（规范 v{}）",
                env!("CARGO_PKG_VERSION"),
                str_format::SPEC_VERSION
            ),
        );
        std::env::set_var(
            "STR_ABOUT_COPYRIGHT",
            format!(
                "{} License · {}",
                env!("CARGO_PKG_LICENSE"),
                env!("CARGO_PKG_REPOSITORY")
            ),
        );
    }

    let app = AppWindow::new()?;
    // 先让 UI 知道系统当前是否为深色：「跟随系统」模式下生效态与 Palette 都依赖它。
    // 此后 dark-mode 重算会触发 .slint 的 `changed dark-mode`，配色与上报随之刷新。
    app.set_system_dark(system_prefers_dark());
    // macOS 原生「关于」由系统应用菜单承担，帮助菜单里不再出现「关于」项（见 app.slint）。
    #[cfg(target_os = "macos")]
    let is_macos = true;
    #[cfg(not(target_os = "macos"))]
    let is_macos = false;
    app.set_is_macos(is_macos);

    // 「关于」对话框静态信息（编译期常量）
    app.set_about_app_version(env!("CARGO_PKG_VERSION").into());
    app.set_about_lib_version(str_format::LIB_VERSION.into());
    app.set_about_spec_version(str_format::SPEC_VERSION.into());
    app.set_about_str_major(format!("{}", str_format::STR_MAJOR).into());
    app.set_about_repository(env!("CARGO_PKG_REPOSITORY").into());
    app.set_about_license(env!("CARGO_PKG_LICENSE").into());

    // 「帮助 → 快捷键一览…」的内容：静态表，修饰键字形按平台切换。
    app.set_shortcuts(ModelRc::from(Rc::new(VecModel::from(shortcut_table(
        is_macos,
    )))));

    // ── 最近打开的 bundle + 首次启动欢迎引导（应用级状态，存用户配置目录）──
    refresh_recent(&app);

    // 系统外观可能在运行中变化，而 AppKit 没有现成的 Rust 侧通知回调可用，
    // 故用 UI 线程定时器低频复查（2s，开销可忽略）；只在结果变化时写回属性，
    // 避免每次都触发 dark-mode 重算与 Palette 重写。
    let app_weak = app.as_weak();
    let appearance_timer = slint::Timer::default();
    appearance_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(2),
        move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let dark = system_prefers_dark();
            if app.get_system_dark() != dark {
                app.set_system_dark(dark);
            }
            // 顺带补一次外观：启动时窗口可能尚未进入 NSApp.windows()，
            // 周期复查可让标题栏装饰最终生效（函数内部已做「已匹配则跳过」）。
            apply_appearance(&app, app.get_appearance_mode());
        },
    );

    let editor = Rc::new(RefCell::new(Editor::new()));
    let clipboard: Rc<RefCell<Vec<ClipItem>>> = Rc::new(RefCell::new(Vec::new()));
    // 内容条目批量操作的共享槽：当前操作 + 最近一轮逐项结果（供「重试失败项」）。
    let batch_results: std::sync::Arc<std::sync::Mutex<(ContentOp, Vec<BatchOutcome>)>> =
        std::sync::Arc::new(std::sync::Mutex::new((
            ContentOp::Trash {
                branch_dir: PathBuf::new(),
            },
            Vec::new(),
        )));

    // ── 拖拽载荷编解码（data-transfer 在 Slint 侧不透明）──
    {
        let ed_multi = editor.clone();
        let dnd = app.global::<DndApi>();
        // 闭包内读取 DndApi 源状态用（global 句柄借用 app，不能 move 进 'static
        // 闭包 —— 经 weak 句柄 upgrade 取）。
        let app_weak_dnd = app.as_weak();
        // 拖拽行在多选集合内 → 载荷携带整组多选（Finder 语义：拖一个选中项 = 拖全部）。
        // 多目标载荷格式："visit|path1\npath2…"（单目标保持旧格式 "visit|path"）。
        // 多选拖拽的整组载荷文本（`entry_to_transfer` 与 `payload_for` 共用，避免漂移）：
        // 拖拽行属于选区 → `visit|p1\np2…`，顺序取 `collect_entry_multi`（与批量动作同一
        // 来源、且是**可视行序**；HashSet 遍历顺序任意会让首项/落点语义漂移）；否则退回
        // 单路径原文。格式与 `parse_entry_transfer_multi` 对齐：**首个路径紧贴 '|'**。
        fn multi_transfer_text(e: &Editor, visit: &str, path: &str, fallback: &str) -> String {
            if !e.entry_multi.contains(path) {
                return fallback.to_string();
            }
            let mut out = String::from(visit);
            out.push('|');
            for (i, (p, _)) in collect_entry_multi(e).iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(p);
            }
            out
        }
        let ed_payload = editor.clone();
        dnd.on_payload_for(move |visit, path| {
            let e = ed_payload.borrow();
            let fallback = format!("{visit}|{path}");
            multi_transfer_text(&e, &visit.to_string(), &path, &fallback).into()
        });
        let ed_contig = editor.clone();
        dnd.on_src_contig_for(move |_visit, path| {
            let e = ed_contig.borrow();
            // 与 multi_transfer_text 同判据：被抓行不在选区内 = 单条拖拽 = 连续。
            (!e.entry_multi.contains(path.as_str()) || entry_multi_contiguous(&e)).into()
        });
        let ed_mixed = editor.clone();
        dnd.on_src_mixed_for(move |_visit, path| {
            let e = ed_mixed.borrow();
            // 单条拖拽不属「混合组」——哪怕被抓行未登记：单条 fs 移动可落在任意
            // 插入位（sub-row 路径自带 root_insert_pos），指示应照常显示插入线。
            // 「重排不可表达」只发生在**多条**选区含未登记成员时。
            (e.entry_multi.len() > 1
                && e.entry_multi.contains(path.as_str())
                && entry_multi_has_unregistered(&e))
            .into()
        });
        dnd.on_entry_to_transfer(move |s: SharedString| {
            let expanded = match s.split_once('|') {
                Some((visit, path)) => {
                    let e = ed_multi.borrow();
                    multi_transfer_text(&e, visit, path, &s)
                }
                None => s.to_string(),
            };
            if debug_on() {
                eprintln!("[str-gui] entry-to-transfer {s:?} -> {expanded:?}");
            }
            slint::DataTransfer::from(SharedString::from(expanded))
        });
        dnd.on_transfer_to_entry(|d| {
            let t = d.plain_text().unwrap_or_default();
            // 挂载载荷（"mount|…"）不是条目路径：内容落点一律忽略。
            if t.starts_with("mount|") {
                SharedString::new()
            } else {
                t
            }
        });
        dnd.on_branch_to_transfer(|visit| {
            slint::DataTransfer::from(SharedString::from(format!("branch|{visit}")))
        });
        dnd.on_mount_to_transfer(|parent, target| {
            slint::DataTransfer::from(SharedString::from(format!("mount|{parent}|{target}")))
        });
        dnd.on_transfer_to_branch(|d| {
            d.plain_text()
                .unwrap_or_default()
                .strip_prefix("branch|")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(-1)
        });
        let ed_tree = editor.clone();
        dnd.on_tree_drop_ok(move |src, target, after| {
            let e = ed_tree.borrow();
            let (Ok(src), Ok(target)) = (usize::try_from(src), usize::try_from(target)) else {
                return false;
            };
            tree_drop_ok(&e, src, target, after)
        });
        let ed_nest = editor.clone();
        dnd.on_tree_nest_ok(move |src, target| {
            let e = ed_nest.borrow();
            let (Ok(src), Ok(target)) = (usize::try_from(src), usize::try_from(target)) else {
                return false;
            };
            tree_nest_ok(&e, src, target)
        });
        // 挂载行拖拽（软 / 硬链接）：src 取自 DndApi 拖拽源状态。
        let ed_mnest = editor.clone();
        let app_weak_nest = app_weak_dnd.clone();
        dnd.on_tree_mount_nest_ok(
            move |target, _target_mounted, target_hard, target_link_visit| {
                let Some(app) = app_weak_nest.upgrade() else { return false; };
                let dnd = app.global::<DndApi>();
                let e = ed_mnest.borrow();
                let mp = dnd.get_tree_src_mount_parent();
                let tv = dnd.get_tree_src_mount_target();
                let sl = dnd.get_tree_src_mount_link();
                let (Ok(mp), Ok(tv)) = (usize::try_from(mp), usize::try_from(tv)) else {
                    return false;
                };
                let _ = mp;
                tree_mount_nest_ok(
                    &e,
                    tv,
                    usize::try_from(target).unwrap_or(usize::MAX),
                    target_hard,
                    target_link_visit,
                    sl,
                )
            },
        );
        let ed_mdrop = editor.clone();
        let app_weak_drop = app_weak_dnd.clone();
        dnd.on_tree_mount_drop_ok(
            move |target, target_mounted, target_hard, target_mount_parent, target_link_visit, after| {
                let Some(app) = app_weak_drop.upgrade() else { return 0; };
                let dnd = app.global::<DndApi>();
                let e = ed_mdrop.borrow();
                let mp = dnd.get_tree_src_mount_parent();
                let tv = dnd.get_tree_src_mount_target();
                let sl = dnd.get_tree_src_mount_link();
                let (Ok(mp), Ok(tv)) = (usize::try_from(mp), usize::try_from(tv)) else {
                    return 0;
                };
                tree_mount_drop_ok(
                    &e,
                    mp,
                    tv,
                    usize::try_from(target).unwrap_or(usize::MAX),
                    target_mounted,
                    target_hard,
                    target_mount_parent,
                    target_link_visit,
                    after,
                    sl,
                )
            },
        );
        // 目标行在其挂载点 entries[] 中的结构行 path（重排落盘定位用）。
        let ed_mpath = editor.clone();
        dnd.on_mount_row_path(
            move |visit, mounted, hard, _mount_parent, link_visit| {
                let e = ed_mpath.borrow();
                let path = usize::try_from(visit)
                    .ok()
                    .and_then(|v| {
                        let scan = e.scan.as_ref()?;
                        if mounted {
                            // 软挂载行：结构行 path = 目标 id（目录名）。
                            visit_dir_name(scan, v)
                        } else if hard {
                            // 硬链接行：结构行 path = 自身分支 id。
                            let li = usize::try_from(link_visit).ok()?;
                            visit_dir_name(scan, li)
                        } else {
                            // 真实行：path = 自身 id。
                            visit_dir_name(scan, v)
                        }
                    })
                    .unwrap_or_default();
            SharedString::from(path)
            },
        );
        dnd.on_transfer_has_files(|d| d.has_file_paths());
        dnd.on_transfer_to_files(|d| {
            d.file_paths()
                .map(|paths| {
                    paths
                        .map(|p| p.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
                .into()
        });
    }

    fn with_editor<R>(
        editor: &Rc<RefCell<Editor>>,
        f: impl FnOnce(&mut Editor) -> Result<R, String>,
    ) -> Result<R, String> {
        let mut e = editor.borrow_mut();
        f(&mut e)
    }

    // 状态栏自动清除定时器（见 show_status）。
    thread_local! {
        static STATUS_CLEAR_TIMER: std::cell::RefCell<Option<slint::Timer>> =
            const { std::cell::RefCell::new(None) };
    }

    /// 用持久化的最近列表刷新 UI 模型（打开成功 / 清除后调用）。
    fn refresh_recent(app: &AppWindow) {
        app.set_recent_files(ModelRc::from(Rc::new(VecModel::from(
            recent_paths()
                .into_iter()
                .map(|p| SharedString::from(p.display().to_string()))
                .collect::<Vec<_>>(),
        ))));
    }

    // ── 无限画布：把视口居中到内容包围盒（flick 布局变化时由 slint 触发）──
    {
        let app_weak = app.as_weak();
        app.on_center_canvas(move |vp_w, vp_h| {
            if vp_w <= 0.0 || vp_h <= 0.0 {
                return;
            }
            let app = app_weak.upgrade().unwrap();
            let mw = app.get_mind_w();
            let mh = app.get_mind_h();
            let zoom = app.get_mind_zoom();
            let canvas_w = (mw * 2.0).max(4000.0);
            let canvas_h = (mh * 2.0).max(3000.0);
            // 内容中心对齐视口中心（像素坐标含缩放）。content-x 是 viewport 在
            // Flickable 内的位置：滚动到内容像素坐标 p 时 content-x = -p。
            let off_x = (canvas_w - mw) / 2.0;
            let off_y = (canvas_h - mh) / 2.0;
            let scroll_x = (off_x + mw / 2.0) * zoom - vp_w / 2.0;
            let scroll_y = (off_y + mh / 2.0) * zoom - vp_h / 2.0;
            app.set_mind_vpx(-scroll_x);
            app.set_mind_vpy(-scroll_y);
        });
    }

    /// 展开 / 收起某分支的**子分支**（列表树双击、导图节点双击、两处右键菜单
    /// 与菜单栏「分支」共用同一实现，避免行为漂移）。
    fn toggle_subtree(e: &mut Editor, app: &AppWindow, visit: usize) {
        let key = e
            .scan
            .as_ref()
            .map(|s| visit_key(s, visit).to_string())
            .filter(|k| !k.is_empty());
        let Some(key) = key else { return };
        if e.expanded.remove(&key) {
            collapse_subtree_state(e, visit);
        } else {
            e.expanded.insert(key);
        }
        e.rebuild();
        sync_ui(app, e);
    }

    /// 展开 / 收起某分支的**内容条目**面板（仅视图状态，不写盘）。
    fn toggle_branch_content(e: &mut Editor, app: &AppWindow, visit: usize) {
        let key = e
            .scan
            .as_ref()
            .map(|s| visit_key(s, visit).to_string())
            .filter(|k| !k.is_empty());
        let Some(key) = key else { return };
        if !e.content_expanded.remove(&key) {
            e.content_expanded.insert(key);
        }
        sync_detail(app, e);
    }

    /// **当前可见**的分支身份键（见 `visit_key`）：与列表树 / 导图此刻渲染出的节点一致 ——
    /// 即 `e.rows`（沿「已展开链」可达的行）对应的分支，含 ROOT。
    ///
    /// 「展开全部内容」以此为基准：只为看得见的分支展开内容面板。
    /// 注意不能用 `expanded` 集合本身 —— 它是「谁的子分支被展开」，而不是
    /// 「谁被渲染」；用错会导致根节点没有内容条目时整项被误判为不可用
    /// （且只展开了一个节点，看起来像没反应）。
    fn visible_branch_keys(e: &Editor) -> Vec<String> {
        let Some(scan) = e.scan.as_ref() else {
            return Vec::new();
        };
        e.rows
            .iter()
            .filter_map(|r| scan.visits.get(r.visit))
            .map(|v| v.rel.clone())
            .collect()
    }

    /// 菜单栏「分支」项的可用性：选中分支是否存在子分支 / 内容条目。
    ///
    /// `sync_ui` 末尾会调用 `sync_detail`，故在此维护即可覆盖
    /// 「选中变化」与「模型重建（增删分支 / 切换展开）」两条路径。
    fn sync_sel_flags(app: &AppWindow, e: &Editor) {
        let flags = e.scan.as_ref().and_then(|s| {
            let idx = e.selected_idx_in_visits()?;
            Some((
                e.kids.get(idx).is_some_and(|v| !v.is_empty()),
                s.visits.get(idx).is_some_and(|v| has_content_entries(s, v)),
            ))
        });
        let (children, entries) = flags.unwrap_or((false, false));
        app.set_sel_has_children(children);
        app.set_sel_has_entries(entries);

        // 全树 / 可见范围的同类判定：决定「展开全部 / 收起全部」是否可用。
        let any_children = e
            .scan
            .as_ref()
            .is_some_and(|s| s.visits.iter().any(|v| v.parent.is_some()));
        // 内容侧与展开动作同一口径：只看**当前渲染出来的节点**（`e.rows`）。
        let any_visible_entries = e.scan.as_ref().is_some_and(|s| {
            e.rows
                .iter()
                .any(|r| s.visits.get(r.visit).is_some_and(|v| has_content_entries(s, v)))
        });
        app.set_any_has_children(any_children);
        app.set_any_visible_has_entries(any_visible_entries);
    }

    // ── 选中分支 → 信息页表单 ──
    fn sync_detail(app: &AppWindow, e: &Editor) {
        sync_sel_flags(app, e);
        // 导图整树布局 + 模型只服务导图视图：列表视图下跳过（2441 节点实测
        // debug 下 layout 72ms + 2441 个节点模型，全部白算 —— 切视图时补同步）。
        // 画布 / 小地图都只在导图视图存在，列表视图读不到这些值。
        if !app.get_view_mind() {
            sync_selected_detail(app, e);
            return;
        }
        // 思维导图不依赖选中状态：无选中时仍渲染完整布局（仅无高亮），
        // 否则点击空白取消选中会把整个画布清空。
        //
        // 布局缓存：几何只取决于「可见行结构 + 展开集合 + 内容展开节点的行数」，
        // 与选中 / 多选集合**无关**。选中变化（点行、⌘ 多选、右键换节点）以前每次
        // 都整树重排并重建连线 / 小地图位图，大 bundle 下每次点击都是数十毫秒的
        // 白算；签名一致时只刷新选中标记与节点内容行数据。
        let sig = mind_sig(e);
        let fresh = {
            let mut slot = e.mind_cache.borrow_mut();
            match slot.as_ref() {
                Some((s, _)) if *s == sig => false,
                _ => {
                    *slot = Some((sig, mind_layout(e)));
                    true
                }
            }
        };
        let guard = e.mind_cache.borrow();
        let Some((_, mind)) = guard.as_ref() else {
            drop(guard);
            return;
        };
        // refs 关联项高亮（选中驱动）：与当前选中分支直接关联的节点标 ref_mark；
        // 父级链（祖先）标 parent_mark（更淡）。每次都算（布局缓存签名与选中
        // 无关），高亮随选中即时亮 / 灭。
        let marked = sel_ref_marked(e);
        let spine = sel_spine_marks(e);
        if fresh {
            // 内容尺寸与节点数必须**先**写回：小地图的映射参数（比例 / 盒尺寸 / 密度
            // 判据）都是 Slint 侧按这几个值算的绑定，晚写就会读到上一轮布局的旧值。
            // 缩略图内容宽度需扣掉子树按钮占位（面板宽度与缩略比例都用它），否则右侧
            // 会多出一段空白——或反过来，比例基准不扣时内容会溢出面板。
            app.set_mind_edge_offset(mind.edge_offset);
            app.set_mind_w(mind.w);
            app.set_mind_h(mind.h);
            app.set_mind_node_count(mind.nodes.len() as i32);
            // 连线以「分块 Path」渲染（逐条矩形的 item 数在超大导图下不可接受）。
            // 几何未变时连线形状也不变，无需重建。
            app.set_mind_edge_chunks(ModelRc::from(Rc::new(VecModel::from(
                mind.edge_chunks.clone(),
            ))));
            app.set_mind_mini_edge_chunks(ModelRc::from(Rc::new(VecModel::from(
                mind.mini_edge_chunks.clone(),
            ))));
        }
        // refs 关联线模型：**每次都重建**（数量级小）—— mark 随选中变化，而选中
        // 变化不触发布局重算（不进缓存签名），故不能只在 fresh 分支写。
        let ref_edges: Vec<RefEdge> = mind
            .ref_edges
            .iter()
            .map(|r| {
                let mut r2 = r.clone();
                r2.mark = e
                    .selected_idx_in_visits()
                    .map(|s| s as i32 == r.src_visit || s as i32 == r.dst_visit)
                    .unwrap_or(false);
                r2
            })
            .collect();
        app.set_mind_ref_edges(ModelRc::from(Rc::new(VecModel::from(ref_edges))));
        // 节点模型尽量原地更新：整体替换会重建所有节点组件，正在显示右键
        // 菜单的那个节点被销毁 → 菜单项点击失效（首次右键选中分支即触发，
        // 与结构树 rows_model 同款问题）。节点数变化时（展开/收起等）才整体替换。
        let existing_nodes = e.mind_nodes_model.borrow().clone();
        match existing_nodes {
            Some(handle) if handle.row_count() == mind.nodes.len() => {
                if fresh {
                    for (i, n) in mind.nodes.iter().enumerate() {
                        let mut n2 = n.clone();
                        n2.ref_mark = marked.contains(&(n.visit as usize));
                        n2.parent_mark =
                            !n2.ref_mark && spine.contains(&(n.visit as usize));
                        handle.set_row_data(i, n2);
                    }
                } else {
                    // 缓存命中：几何不变，只把选中标记 / 关联高亮与节点内容行数据
                    // 刷新到位。行模型句柄保持不动（内容行原地更新 → 节点内右键
                    // 菜单 / 拖拽所在组件不会被销毁）。
                    for (i, n) in mind.nodes.iter().enumerate() {
                        let mut patch: Option<MindNode> = None;
                        let sel = node_is_selected(e, n.visit as usize);
                        // 基准必须取**模型里的当前值**，不能取缓存布局里的 n.is_selected：
                        // 后者是「上次构建布局那一刻」的选中态，之后被改写的是模型而非缓存。
                        // 拿缓存当基准会漏写 —— 选中 A → 改选 B → 取消选中时，取消这一步
                        // 里 B 的缓存值（未选中）恰好等于新值，于是跳过写入，B 的高亮永远
                        // 清不掉（=「分支激活态无法正确移除」）。
                        if handle.row_data(i).map(|m| m.is_selected) != Some(sel) {
                            let x = patch.get_or_insert_with(|| n.clone());
                            x.is_selected = sel;
                        }
                        let want_mark = marked.contains(&(n.visit as usize));
                        if handle.row_data(i).map(|m| m.ref_mark) != Some(want_mark) {
                            let x = patch.get_or_insert_with(|| n.clone());
                            x.ref_mark = want_mark;
                        }
                        let want_spine =
                            !want_mark && spine.contains(&(n.visit as usize));
                        if handle.row_data(i).map(|m| m.parent_mark) != Some(want_spine) {
                            let x = patch.get_or_insert_with(|| n.clone());
                            x.parent_mark = want_spine;
                        }
                        if n.expanded && n.has_entries {
                            let none = HashSet::new();
                            let rows = visible_to_rows(
                                &build_entry_rows_for(e, n.visit as usize),
                                node_multi(e, n.visit as usize, &none),
                            );
                            let h = apply_node_entry_rows(e, n.visit as usize, rows);
                            if h != n.entry_rows {
                                let x = patch.get_or_insert_with(|| n.clone());
                                x.entry_rows = h;
                            }
                        }
                        if let Some(x) = patch {
                            handle.set_row_data(i, x);
                        }
                    }
                }
            }
            _ => {
                let handle = Rc::new(VecModel::from(
                    mind.nodes
                        .iter()
                        .map(|n| {
                            let mut n2 = n.clone();
                            n2.ref_mark = marked.contains(&(n.visit as usize));
                            n2.parent_mark =
                                !n2.ref_mark && spine.contains(&(n.visit as usize));
                            n2
                        })
                        .collect::<Vec<_>>(),
                ));
                app.set_mind_nodes(ModelRc::from(handle.clone()));
                *e.mind_nodes_model.borrow_mut() = Some(handle);
            }
        }
        // 小地图密度形态：映射参数（盒尺寸 / 两轴比例 / 判据 / 用色）都由 Slint 侧
        // 计算，这里回读后烘焙位图 —— 两边用同一组数值，不会各算一套。
        // ⚠ 必须在节点模型原地刷新**之后**：脊柱（高亮路径）读的是**模型当前**的
        // is_selected —— 缓存布局里的选中态是构建那一刻的，选中变化只改模型不重排
        // 布局，直接用缓存会把高亮画在旧位置上（「高亮不正确且不及时」的根因）。
        if app.get_mini_dense() {
            // Slint 在窗口显示前只报 1.0，密度图固定按 2x 生成：位图很小（184×116
            // 量级），Retina 下不糊，1x 屏上略缩也看不出差别。
            let dpr = app.window().scale_factor().clamp(2.0, 3.0);
            let (bw, bh) = (app.get_mini_box_w(), app.get_mini_box_h());
            let (sx, sy) = (app.get_mini_sx(), app.get_mini_sy());
            let ink = rgb_of(app.get_mini_ink());
            let accent = rgb_of(app.get_mini_accent());
            let selected = e.selected.clone().unwrap_or_default();
            // 键含布局签名 / 映射参数 / 用色 / 选中键：只有这些变化才需要重烘焙。
            let key = format!(
                "{sig}|{dpr:.2}|{bw:.2}x{bh:.2}|{sx:.4}x{sy:.4}|{ink:?}{accent:?}|{selected}"
            );
            if e.mini_raster_key.borrow().as_deref() != Some(key.as_str()) {
                // 脊柱回溯的数据源 = 节点模型的当前行（含原地刷新后的选中态）。
                let spine_nodes: Vec<MindNode> = match e.mind_nodes_model.borrow().clone() {
                    Some(h) => (0..h.row_count()).filter_map(|i| h.row_data(i)).collect(),
                    None => mind.nodes.clone(),
                };
                let spine = selection_spine(&spine_nodes);
                let raster =
                    mini_density_raster(mind, (bw, bh), (sx, sy), dpr, &spine, ink, accent);
                app.set_mind_mini_image(mini_density_image(&raster));
                *e.mini_raster_key.borrow_mut() = Some(key);
            }
        }
        drop(guard);

        // 内容尺寸变化时 slint 侧会经 flick 的 changed width 触发 center-canvas 居中。
        sync_selected_detail(app, e);
    }

    /// 预览跟随：面板开着时，把选中项（主选中）喂给预览。面板是 `orderFront`
    /// 显示的（不抢 key），方向键始终归列表 —— 选中变、预览跟着变。
    fn sync_quick_look_preview(app: &AppWindow, e: &Editor) {
        if !app.global::<EntryApi>().get_quick_look_open() {
            return;
        }
        let target = e
            .selected_visit()
            .map(|v| v.dir.clone())
            .and_then(|dir| e.selected_entry_path.as_ref().map(|p| dir.join(p)));
        if let Some(p) = target {
            let _ = quicklook::show(&[p]);
        }
    }

    /// 按行号选中内容条目（**单选 = 单元素选区**，Finder 语义：与多选同一集合、
    /// 同一高亮）。方向键移动选中、快速查看面板翻页同步选中都走这里。
    fn select_entry_index(app: &AppWindow, e: &mut Editor, index: usize) {
        e.entry_multi.clear();
        e.entry_anchor = Some(index);
        let path = build_entry_rows(e).get(index).map(|v| v.path.clone());
        if let Some(p) = &path {
            e.entry_multi.insert(p.clone());
        }
        app.global::<EntryApi>()
            .set_multi_count(e.entry_multi.len() as i32);
        e.selected_entry_path = path;
        // 重建条目模型：让 in-multi 高亮（单元素选区）与选中态一致。
        sync_detail(app, e);
        app.set_info_tab(1);
        sync_quick_look_preview(app, e);
    }

    /// 选中分支 → 信息侧栏表单（与视图无关：列表 / 导图两种视图都要维护）。
    fn sync_selected_detail(app: &AppWindow, e: &Editor) {
        let Some(v) = e.selected_visit() else {
            app.set_has_selection(false);
            app.set_is_root(false);
            app.set_d_kind("—".into());
            app.set_d_id("".into());
            app.set_d_revision("".into());
            app.set_d_updated("".into());
            app.set_d_title("".into());
            app.set_d_type("".into());
            app.set_d_summary("".into());
            app.set_d_tags("".into());
            app.set_entries(ModelRc::default());
            app.set_entry_selected(-1);
            // 动作目标必须随选中一起清空：否则无选中时触发条目动作（快捷键 /
            // 菜单未灰化路径）会读到上一个分支与上一批路径，操作落到错误位置。
            app.global::<EntryApi>().set_menu_target_visit(-1);
            app.global::<EntryApi>().set_menu_targets("".into());
            app.global::<EntryApi>().set_multi_count(0);
            app.set_sel_hard(false);
            *e.entries_model.borrow_mut() = None;
            app.global::<DndApi>().set_src_visit(-1);
            return;
        };
        // 硬链接视图（§4.6.1 `mode = "hard"`）：信息页与内容一样关联**目标分支** ——
        // 标题 / 类型 / 摘要 / 标签等元信息所有权在目标分支（保存被 sel_hard 守卫
        // 拦截，编辑请到目标分支）；硬链接自身 meta 只承载自有结构与身份。
        // 目标悬空时回落到自身 meta，避免表单莫名全空。
        let meta = e
            .scan
            .as_ref()
            .and_then(|s| content_visit(s, v))
            .map(|tv| tv.meta.as_deref())
            .unwrap_or_else(|| v.meta.as_deref());
        let is_root = v.depth == 0;
        app.set_has_selection(true);
        // sel-hard 同步（Rust 侧守卫与挂载兜底落点都读它）：导图点击路径由 Slint
        // 侧先置属性、select-visit 读取；树行点击路径在 Rust 侧维护，这里回写
        // Slint 属性保持两侧一致。
        app.set_sel_hard(e.sel_hard);
        app.set_is_root(is_root);
        app.set_d_kind(
            meta.and_then(|m| m.kind.as_ref().map(|k| SharedString::from(k.as_str())))
                .unwrap_or("—".into()),
        );
        app.set_d_id(meta.and_then(|m| m.id.as_deref()).unwrap_or("").into());
        app.set_d_revision(
            meta.and_then(|m| m.revision)
                .map(|r| r.to_string())
                .unwrap_or_default()
                .into(),
        );
        app.set_d_updated(
            meta.and_then(|m| m.updated_at.as_deref())
                .unwrap_or("")
                .into(),
        );
        app.set_d_title(meta.and_then(|m| m.title.as_deref()).unwrap_or("").into());
        app.set_d_type(meta.and_then(|m| m.r#type.as_deref()).unwrap_or("").into());
        app.set_d_summary(meta.and_then(|m| m.summary.as_deref()).unwrap_or("").into());
        app.set_d_tags(meta.map(|m| m.tags.join(", ")).unwrap_or_default().into());

        // 内容 entries[]：只显示内容条目（node/branch 在结构树中呈现），
        // 已展开的内容文件夹列出磁盘子项。
        // 内容行只构建一次：条目模型与右侧编辑区（fill_entry_fields）共用，
        // 避免同一次同步里重复 read_dir 遍历同一分支。
        let vis = build_entry_rows(e);
        let entry_model: Vec<EntryRow> = visible_to_rows(&vis, &e.entry_multi);
        // 行数不变则原地更新（同 rows_model）：整体替换会重建所有条目行组件，
        // 右键落点行被销毁 → 菜单项点击失效（多选存在时右键未选中行即触发）。
        let existing_entries = e.entries_model.borrow().clone();
        match existing_entries {
            Some(handle) if handle.row_count() == entry_model.len() => {
                for (i, r) in entry_model.into_iter().enumerate() {
                    handle.set_row_data(i, r);
                }
            }
            _ => {
                let handle = Rc::new(VecModel::from(entry_model));
                app.set_entries(ModelRc::from(handle.clone()));
                *e.entries_model.borrow_mut() = Some(handle);
            }
        }

        // 条目编辑区（复用上面已构建的可见行，不再重复遍历磁盘）
        fill_entry_fields(app, e, &vis);

        // 拖拽源分支（当前选中分支）。
        app.global::<DndApi>().set_src_visit(drag_source_visit(e));
    }

    fn fill_entry_fields(app: &AppWindow, e: &Editor, vis: &[VisibleEntry]) {
        // 选中条目按路径定位（跨 rescan 稳定）；文件夹子行不可编辑。
        let position = e
            .selected_entry_path
            .as_ref()
            .and_then(|p| vis.iter().position(|v| &v.path == p));
        let entry = position
            .and_then(|pos| vis.get(pos).and_then(|v| v.entries_idx))
            .and_then(|idx| {
                // 硬链接视图的内容条目在**目标分支**的 meta 里（见 content_visit）。
                e.selected_visit()
                    .and_then(|v| e.scan.as_ref().and_then(|s| content_visit(s, v)))
                    .and_then(|v| v.meta.as_ref())
                    .and_then(|m| m.entries.get(idx))
            });
        // 子行（无 entries 条目）同样高亮选中，只是详情面板不可编辑。
        app.set_entry_selected(match position {
            Some(pos) => pos as i32,
            None => -1,
        });
        app.set_entry_editable(entry.is_some());
        // 子行标题 = 文件名（只读展示）。
        let child_name = position
            .and_then(|pos| vis.get(pos))
            .filter(|v| v.entries_idx.is_none())
            .and_then(|v| v.path.rsplit('/').next().map(str::to_string));
        app.set_e_title(
            entry
                .and_then(|en| en.title.as_deref())
                .map(str::to_string)
                .or(child_name)
                .unwrap_or_default()
                .into(),
        );
        app.set_e_note(entry.and_then(|en| en.note.as_deref()).unwrap_or("").into());
        app.set_e_order(
            entry
                .and_then(|en| en.order)
                .map(|o| o.to_string())
                .unwrap_or_default()
                .into(),
        );
        let vrow = position.and_then(|pos| vis.get(pos));
        app.set_e_kind(vrow.map(|v| v.role.as_str()).unwrap_or("—").into());
        app.set_e_path(vrow.map(|v| v.path.as_str()).unwrap_or("").into());
        // 多选摘要：右侧栏在「多选无主选中」时显示（⌘A / 整组反选后不再空白），
        // 「多选有主选中」时作为编辑范围提示（编辑仅作用于当前条目）。
        if e.entry_multi.len() > 1 {
            let items = collect_entry_multi(e);
            let names: Vec<String> = items.iter().map(|(_, n)| n.clone()).take(5).collect();
            let mut s = format!("已选中 {} 个条目", items.len());
            if !names.is_empty() {
                s.push_str("：");
                s.push_str(&names.join("、"));
            }
            if items.len() > names.len() {
                s.push_str(" 等");
            }
            app.global::<EntryApi>().set_multi_summary(s.into());
        } else {
            app.global::<EntryApi>().set_multi_summary("".into());
        }
        // 选区计数以本次构建的可见行为准：批量移动 / 删除后 rescan 会剪除已消失
        // 的路径，处理器各自 set 的计数可能比集合大，这里统一回写保证一致。
        app.global::<EntryApi>()
            .set_multi_count(e.entry_multi.len() as i32);
        // 动作目标：与右侧栏视图同步的「当前生效选区」解析结果（相对路径，
        // \n 分隔）。所有条目动作回调（打开/显示/拷贝/剪切/制作副本/重命名…）
        // 都从这里取目标、**不接收行号**——行号空间错位与右键时序问题由此根除。
        app.global::<EntryApi>()
            .set_menu_target_visit(e.selected_idx_in_visits().map(|i| i as i32).unwrap_or(-1));
        if e.entry_multi.len() > 1 {
            let targets: Vec<String> = collect_entry_multi(e).into_iter().map(|(p, _)| p).collect();
            app.global::<EntryApi>()
                .set_menu_targets(targets.join("\n").into());
        } else {
            let t = e.selected_entry_path.clone().unwrap_or_default();
            app.global::<EntryApi>().set_menu_targets(t.into());
        }
    }

    // ── 列表 / 导图 / 选中 → UI ──
    /// 状态栏文本自动清除：每次设置后 6s 清空（单发定时器随每次设置重启）。
    /// 此前状态文本常驻底栏（成功 / 失败提示永不消失），改为临时提示。
    /// 所有调用都在 UI 线程（Slint 回调内），thread_local 定时器安全。
    ///
    /// # 文案规范（**所有**写入状态栏的文案都必须遵循；新增文案前先读这一节）
    ///
    /// 状态栏是单行 12px + `overflow: elide`（`ui/app.slint`），因此
    /// **信息槽位固定、顺序固定、长度受控**（目标 ≤ 40 个汉字）。
    /// 规范要点（附带实现 helper）：
    ///
    /// 1. **四类句式**（不允许第五类）
    ///    - 成功：`已<动词> <对象>（<数量/位置>）。`
    ///    - 失败：`<动作>失败：<原因>`（原因原样透传底层 `Err`，不改写；句尾不强制句号）
    ///    - 空态：`<缺什么的具体说明>。`（如「请先选择一个分支。」）
    ///    - 无变更：`<未变更原因>。`（如「顺序未变化。」「已在目标目录内，位置未变。」）
    /// 2. **位置变化类必须写全「来源 → 目标」**：`已从「源」<动词> <对象描述> →「目标」`。
    ///    - 目标 = 分支标题（`title_of_dir`）/ 内容文件夹相对路径 / 剪贴板类型
    ///      （`系统剪贴板` / `内部剪贴板`）；同分支内换目录写 `已在「分支」内…`。
    ///    - 去向一律用 `→ 系统剪贴板` / `→ 内部剪贴板` 表达，**不再**用括号备注；
    ///      括号只承载固定备注：`（粘贴时移动）`、`（可在废纸篓找回）`、
    ///      `（不可撤销）`、`（revision N）`、`（<role>）`。
    /// 3. **对象描述槽位**：单条 = `「<名称>」`；多条 = `<N> 项：<name_list>`，
    ///    `name_list` **最多 3 个名称**、超出以「…」收尾（见 `name_list`）。
    ///    拷贝 / 剪切 / 移动 / 导入 / Finder 显示一律同构，不允许「有的列名有的不列」。
    /// 4. **术语与量词**：`Finder`（禁「访达」）、`拷贝`（禁「复制」）；条目 = 项、
    ///    路径 = 条、目录 = 个、分支 = 个；名称与路径一律 `「」` 包裹；
    ///    数字与中文之间保留**一个半角空格**（`已移动 3 项`），`「」` 两侧不加空格。
    /// 5. **禁止内部实现术语**：canonical、写回、sha256、`.str.toml`、fs / IO 细节
    ///    （`revision` 属 STR 规范概念，可保留）。
    ///
    /// 取标题 / 名称一律复用 `visit_title` / `title_of_dir` / `name_list`
    /// —— 状态栏文案里**不新增磁盘 IO**。
    fn show_status(app: &AppWindow, msg: SharedString) {
        if debug_on() { eprintln!("[str-gui] show_status {:?}", msg); }
        app.set_status(msg);
        STATUS_CLEAR_TIMER.with(|cell| {
            let mut slot = cell.borrow_mut();
            let timer = slot.get_or_insert_with(|| {
                let weak = app.as_weak();
                let t = slint::Timer::default();
                t.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_secs(6),
                    move || {
                        if let Some(a) = weak.upgrade() {
                            // 批量进行中：进度正在**直写**状态栏（`spawn_gui_batch` 的 tick
                            // 不走 `show_status`），这条旧定时器若清空会让进度闪断到下一个
                            // tick。批量结束时的 `show_status`（终态摘要）会重启定时器，
                            // 因此正常自动清空不受影响。
                            if a.get_batch_busy() {
                                return;
                            }
                            if debug_on() { eprintln!("[str-gui] status auto-clear"); }
                            a.set_status(SharedString::from(""));
                        }
                    },
                );
                t
            });
            timer.restart();
        });
    }

/// 硬链接视图只读守卫（规范 §4.6.1：链接是只读视图，内容所有权唯一在目标分支）。
/// 返回 true = 已拦截（调用方直接 return）。
fn hard_view_guard(app: &AppWindow, e: &Editor) -> bool {
    if e.sel_hard {
        show_status(app, "硬链接视图只读：内容请到目标分支操作。".into());
        return true;
    }
    false
}


    /// 批量执行期间的**写入互斥**：批量在后台线程写 `.str.toml` / 移动文件，
    /// 此时任何新的写盘入口都必须被拒绝（此前靠模态进度遮罩挡住点击，
    /// 遮罩移除后改为显式门禁）。返回 `true` = 已提示、调用方应立即 `return`。
    ///
    /// 只查 `batch-busy`（真正的并发窗口）；汇总对话框是模态的、点击被吞，
    /// 不属于并发风险。
    fn batch_busy_guard(app: &AppWindow) -> bool {
        if app.get_batch_busy() {
            show_status(app, "请先等待批量操作完成。".into());
            true
        } else {
            false
        }
    }

    fn sync_ui(app: &AppWindow, e: &Editor) {
        // 选中驱动的两套标记：refs 关联项（强）/ 父级链（淡）。
        let ref_marks = sel_ref_marked(e);
        let spine_marks = sel_spine_marks(e);
        let row_model: Vec<BranchRow> = e
            .rows
            .iter()
            .map(|r| branch_row_of(e, r, false, &ref_marks, &spine_marks))
            .collect();
        // 横向滚动：行内容最大自然宽（viewport-width = max(视口, 该值)）。
        let content_px = row_model
            .iter()
            .map(|r| r.content_px)
            .fold(0.0f32, f32::max);
        app.set_tree_content_px(content_px);
        // 行模型尽量原地更新：替换模型会让 ListView 重建所有行组件，
        // 正在显示右键菜单的那一行被销毁 → 菜单项点击失效（选中分支即触发）。
        // 行数变化时（展开/收起等）才整体替换。
        let existing = e.rows_model.borrow().clone();
        match existing {
            Some(handle) if handle.row_count() == row_model.len() => {
                for (i, r) in row_model.into_iter().enumerate() {
                    handle.set_row_data(i, r);
                }
            }
            _ => {
                let handle = Rc::new(VecModel::from(row_model));
                app.set_rows(ModelRc::from(handle.clone()));
                *e.rows_model.borrow_mut() = Some(handle);
            }
        }
        // 挂载选择器的**独立**行模型：按 `pick_expanded` 建的 `pick_rows`，与树
        // 互不影响；仅在对话框可见时维护（行内无右键菜单，直接整体换模型即可）。
        if app.get_mount_dialog_visible() {
            let pickables = mount_pickables(e, app.get_m_mode_index(), &e.pick_rows);
            let pick_model: Vec<BranchRow> = e
                .pick_rows
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let empty = std::collections::HashSet::new();
                    branch_row_of(e, r, pickables.get(i).copied().unwrap_or(false), &empty, &empty)
                })
                .collect();
            app.set_pick_rows(ModelRc::from(Rc::new(VecModel::from(pick_model))));
        }
        match e.bundle.as_ref() {
            Some(b) => app.set_bundle_name(b.name().into()),
            None => app.set_bundle_name("未打开 bundle".into()),
        }
        app.set_has_bundle(e.bundle.is_some());
        sync_detail(app, e);
    }

    // ── 打开 ──
    {
        let editor = editor.clone();
        let app_weak: Weak<AppWindow> = app.as_weak();
        app.on_open_bundle(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let Some(path) = rfd::FileDialog::new()
                .set_title("选择 .str bundle 目录")
                .pick_folder()
            else {
                return;
            };
            // 对话框返回的一般已是绝对路径，仍统一规范化（解析符号链接 / 去尾点）。
            let path = canonical_bundle_path(&path);
            match with_editor(&editor, |e| e.open(&path)) {
                Ok(()) => {
                    push_recent(&path);
                    refresh_recent(&app);
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, format!("已打开 bundle：「{}」。", path.display()).into());
                }
                Err(msg) => show_status(&app, format!("打开失败：{msg}").into()),
            }
        });
    }

    // ── 最近打开 / 欢迎引导 ──
    {
        let editor = editor.clone();
        let app_weak: Weak<AppWindow> = app.as_weak();
        app.on_open_recent(move |path| {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            // 历史遗留的相对路径条目在此升为规范形式（与 recent_paths 的自愈一致）。
            let path = canonical_bundle_path(&PathBuf::from(path.to_string()));
            match with_editor(&editor, |e| e.open(&path)) {
                Ok(()) => {
                    push_recent(&path);
                    refresh_recent(&app);
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, format!("已打开 bundle：「{}」。", path.display()).into());
                }
                Err(msg) => show_status(&app, format!("打开失败：{msg}").into()),
            }
        });
    }
    {
        let app_weak: Weak<AppWindow> = app.as_weak();
        app.on_clear_recent(move || {
            let app = app_weak.upgrade().unwrap();
            clear_recent();
            refresh_recent(&app);
            show_status(&app, "已清除最近打开列表。".into());
        });
    }

    // ── 刷新 ──
    {
        // 右键菜单「刷新」（条目行 / 面板空白）与菜单栏「刷新」同一逻辑；
        // 克隆必须放在 on_refresh 注册**之前**（其闭包会 move 原句柄）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        let editor2 = editor.clone();
        let app_weak2 = app_weak.clone();
        app.on_refresh(move || {
            let app = app_weak.upgrade().unwrap();
            match with_editor(&editor, |e| e.rescan()) {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, "已刷新 bundle。".into());
                }
                Err(msg) => show_status(&app, format!("刷新失败：{msg}").into()),
            }
        });
        app.global::<EntryApi>().on_refresh_request(move || {
            let app = app_weak2.upgrade().unwrap();
            match with_editor(&editor2, |e| e.rescan()) {
                Ok(()) => {
                    sync_ui(&app, &editor2.borrow());
                    show_status(&app, "已刷新 bundle。".into());
                }
                Err(msg) => show_status(&app, format!("刷新失败：{msg}").into()),
            }
        });
    }

    // ── 树 / 导图交互 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_row_click(move |row| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let visit = e.rows.get(row as usize).map(|r| r.visit);
            if let Some(v) = visit {
                e.select_visit(v);
                // 硬链接行选中 = 进入只读内容视图（写操作被守卫拦下）；
                // 软挂载行选中 = 目标真身，记下挂载点供删除入口改道移除挂载。
                e.sel_hard = e.rows.get(row as usize).map(|r| r.hard).unwrap_or(false);
                e.sel_mount_parent = e
                    .rows
                    .get(row as usize)
                    .and_then(|r| r.mount_parent);
                e.sel_expand_key =
                    e.rows.get(row as usize).map(|r| r.expand_key.to_string());
                // 原地更新选中标记（不重建模型，保留双击手势状态）。
                let sel = e.selected_idx_in_visits();
                let ref_marks = sel_ref_marked(&e);
                let spine_marks = sel_spine_marks(&e);
                if let Some(model) = &*e.rows_model.borrow() {
                    for i in 0..model.row_count() {
                        if let Some(mut r) = model.row_data(i) {
                            let new_sel = e
                                .rows
                                .get(i)
                                .map(|vr| Some(vr.visit) == sel)
                                .unwrap_or(false);
                            // 关联项 / 父级链高亮同为选中驱动的渲染数据：
                            // 选中变化必须同步刷新（ref 优先于 parent）。
                            let visit_now =
                                e.rows.get(i).map(|vr| vr.visit).unwrap_or(usize::MAX);
                            let new_ref = ref_marks.contains(&visit_now);
                            let new_parent = !new_ref && spine_marks.contains(&visit_now);
                            if r.is_selected != new_sel
                                || r.ref_mark != new_ref
                                || r.parent_mark != new_parent
                            {
                                r.is_selected = new_sel;
                                r.ref_mark = new_ref;
                                r.parent_mark = new_parent;
                                model.set_row_data(i, r);
                            }
                        }
                    }
                }
                sync_detail(&app, &e);
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_expand(move |row| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let Some(r) = e.rows.get(row as usize) else {
                return;
            };
            let key = r.expand_key.clone();
            let visit = r.visit;
            let is_link_view = r.mounted || r.hard;
            if e.expanded.remove(&key) {
                // 仅**真实行**收起时清理子树状态；挂载行 / 硬链接行的展开态是
                // 实例独立的，其渲染的孩子属于其它上下文，不得连带清除。
                if !is_link_view {
                    collapse_subtree_state(&mut e, visit);
                }
            } else {
                e.expanded.insert(key);
            }
            e.rebuild();
            sync_ui(&app, &e);
        });
    }
    {
        // 挂载选择器的展开 / 收起：与结构树**互相独立**（`pick_expanded`）。
        // 收起只清选择器自己的状态，**不**调 `collapse_subtree_state` ——
        // 那会挪走主窗口选中、清空内容展开（用户没碰过树，树却变了）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_pick_toggle_expand(move |row| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            // 行下标来自 pick-rows 模型，必须查 pick_rows（查 e.rows 会在两份
            // 模型错位后展开到错误的分支——孙级展不开正是这么来的）。
            let Some(r) = e.pick_rows.get(row as usize) else {
                return;
            };
            let key = r.expand_key.clone();
            let visit = r.visit;
            let is_link_view = r.mounted || r.hard;
            // 先用不可变借用量算好本键与子树键，再动 pick_expanded（写时
            // scan / kids 的借用必须已结束）。
            let sub_keys = {
                let Some(scan) = e.scan.as_ref() else {
                    return;
                };
                let mut sub_keys: HashSet<String> = HashSet::new();
                if !is_link_view {
                    // 仅真实行收起连带清子树键；挂载行的展开态实例独立。
                    let mut stack = vec![visit];
                    while let Some(i) = stack.pop() {
                        for &c in e.kids.get(i).map(|v| v.as_slice()).unwrap_or_default() {
                            stack.push(c);
                            sub_keys.insert(visit_key(scan, c).to_string());
                        }
                    }
                }
                sub_keys
            };
            if e.pick_expanded.remove(&key) {
                for k in sub_keys {
                    e.pick_expanded.remove(&k);
                }
            } else {
                e.pick_expanded.insert(key);
            }
            e.rebuild();
            sync_ui(&app, &e);
        });
    }
    {
        // 导图节点双击 / 右键「展开·收起子树」：展开或收起该分支的子分支
        // （与列表树共用 expanded 集合）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_visit_expand(move |key| {
            let app = app_weak.upgrade().unwrap();
            let key = key.to_string();
            let mut e = editor.borrow_mut();
            if key.contains('\u{1f}') {
                // 挂载节点实例键：只切自己（渲染的孩子属于其它上下文，不清子树状态）。
                if e.expanded.remove(&key) {
                    e.rebuild();
                    sync_ui(&app, &e);
                } else {
                    e.expanded.insert(key);
                    e.rebuild();
                    sync_ui(&app, &e);
                }
                return;
            }
            // 真实位置 rel：走既有 toggle_subtree（保留子树状态清理语义）。
            let visit = e
                .scan
                .as_ref()
                .and_then(|s| s.visits.iter().position(|v| v.rel == key));
            if let Some(visit) = visit {
                toggle_subtree(&mut e, &app, visit);
            }
        });
    }
    {
        // 导图节点 ▶ / 右键「展开·收起内容」：展开或收起节点内的内容条目。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_node_content(move |visit| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            toggle_branch_content(&mut e, &app, visit as usize);
        });
    }
    {
        // 菜单栏「分支」：同样的两个开关，作用于**当前选中分支**
        // （菜单项拿不到行下标 / visit，故由宿主按选中态解析）。
        // 选中经由**挂载行 / 硬链接行**进入时，展开/收起只作用于该挂载实例
        // （实例键），不连带真身及其它挂载视图。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_sel_subtree(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            if let Some(idx) = e.selected_idx_in_visits() {
                // 选中行的展开键（含实例上下文前缀）。与真身 rel 键不同 ⇒ 选中
                // 经由挂载行 / 实例上下文进入：展开/收起只作用于该键（该实例），
                // 不连带真身、其它挂载视图或外层实例。
                let sel_key = e.sel_expand_key.clone();
                let rel_key = e
                    .scan
                    .as_ref()
                    .map(|s| visit_key(s, idx).to_string());
                if let Some(key) = sel_key {
                    if rel_key.as_deref() != Some(key.as_str()) {
                        if e.expanded.remove(&key) {
                            e.rebuild();
                        } else {
                            e.expanded.insert(key);
                            e.rebuild();
                        }
                        sync_ui(&app, &e);
                        return;
                    }
                }
                toggle_subtree(&mut e, &app, idx);
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_sel_content(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            if let Some(idx) = e.selected_idx_in_visits() {
                toggle_branch_content(&mut e, &app, idx);
            }
        });
    }
    {
        // 视图菜单：展开 / 收起**全部子树**（纯视图状态，不写盘）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_expand_all_subtrees(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let ids = all_branch_keys(&e);
            if ids.is_empty() {
                show_status(&app, "没有可展开的分支。".into());
                return;
            }
            let n = ids.len();
            e.expanded.extend(ids);
            e.rebuild();
            sync_ui(&app, &e);
            show_status(&app, format!("已展开全部子树（{n} 个分支）。").into());
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_collapse_all_subtrees(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            // 清空展开集合即收起到只剩 ROOT 一行（与逐级收起语义一致）。
            if e.expanded.is_empty() {
                show_status(&app, "已收起全部子树。".into());
                return;
            }
            e.expanded.clear();
            e.selected_entry_path = None;
            e.rebuild();
            sync_ui(&app, &e);
            show_status(&app, "已收起全部子树。".into());
        });
    }
    {
        // 视图菜单：展开**当前可见分支**的内容面板（导图内嵌内容列表随之显隐）。
        // 基准是「已展开的分支」：子树未展开时不越界展开其内容。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_expand_all_contents(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let ids = visible_branch_keys(&e);
            if ids.is_empty() {
                show_status(&app, "没有可展开内容的分支。".into());
                return;
            }
            let n = ids.len();
            e.content_expanded.extend(ids);
            sync_detail(&app, &e);
            // 内容面板只出现在导图节点里，故菜单项仅在导图视图可用（见 app.slint）；
            // 这里给出条数反馈，便于确认作用范围。
            show_status(&app, format!("已展开可见分支的内容（{n} 个分支）。").into());
        });
    }
    {
        // 「收起全部内容」则清空全部内容展开态（含被折叠子树内的），
        // 作为「展开」的逆操作不设可见性门槛 —— 只收回、不越界展开。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_collapse_all_contents(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            if e.content_expanded.is_empty() {
                show_status(&app, "已收起全部内容。".into());
                return;
            }
            e.content_expanded.clear();
            sync_detail(&app, &e);
            show_status(&app, "已收起全部内容。".into());
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_select_visit(move |visit| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let target = e
                .scan
                .as_ref()
                .map(|s| visit_key(s, visit as usize).to_string());
            // 已选中该节点：跳过重建，保住正在弹出的右键菜单。
            if target.is_some() && e.selected == target {
                return;
            }
            e.select_visit(visit as usize);
            // Slint 侧（导图节点 / 树行右键）会在调用前置好 sel-hard、
            // sel-mount-parent 与 sel-expand-key；其它路径默认 false / None / 空。
            e.sel_hard = app.get_sel_hard();
            e.sel_mount_parent = (app.get_sel_mount_parent() >= 0)
                .then(|| app.get_sel_mount_parent() as usize);
            let sek = app.get_sel_expand_key();
            e.sel_expand_key = (!sek.is_empty()).then(|| sek.to_string());
            sync_ui(&app, &e);
            app.set_info_tab(0);
        });
    }
    {
        // EntryApi 面板交互前调用：确保目标分支为当前选中分支（与 select-visit 同逻辑）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_activate_visit(move |visit| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let target = e
                .scan
                .as_ref()
                .map(|s| visit_key(s, visit as usize).to_string());
            // 已选中该分支：跳过重建——否则模型替换会销毁正在弹出右键菜单的元素。
            if target.is_some() && e.selected == target {
                return;
            }
            e.select_visit(visit as usize);
            sync_ui(&app, &e);
            app.set_info_tab(0);
        });
    }
    {
        let app_weak = app.as_weak();
        let editor = editor.clone();
        app.on_set_view(move |mind| {
            let app = app_weak.upgrade().unwrap();
            // 切视图后 mind 区域重建，flick 的 changed width 会触发 center-canvas 居中。
            app.set_view_mind(mind);
            // 列表视图不构建导图模型（见 sync_detail：省掉每次动作的整树布局与
            // 模型churn）——切回导图视图时补一次同步，否则画布用的是旧尺寸。
            if mind {
                sync_ui(&app, &editor.borrow());
            }
        });
    }
    // 拖拽收尾（含取消）：把 src-visit 复位到当前激活分支 —— 拖拽期间它被面板
    // 改写为「拖拽源」，若不复位，取消后仍指向旧拖拽源（激活态与源分支不一致）。
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_drag_ended(move || {
            let e = editor.borrow();
            if let Some(app) = app_weak.upgrade() {
                app.global::<DndApi>().set_src_visit(drag_source_visit(&e));
            }
        });
    }
    // 帮助菜单的在线入口：规范 / 仓库 / 问题反馈（顺序与 app.slint 的 open-doc 一致）。
    {
        let repo = env!("CARGO_PKG_REPOSITORY").to_string();
        let app_weak = app.as_weak();
        app.on_open_doc(move |which| {
            let url = doc_url(&repo, which);
            if let Some(app) = app_weak.upgrade() {
                show_status(&app, format!("已在浏览器打开链接：{url}。").into());
            }
            open_url(&url);
        });
    }
    // .slint 侧在 init 与生效态变更时各调用一次：经 apply_appearance 同时设定
    // 进程级配色（驱动 Widget/菜单主题）与 macOS 原生窗口装饰。
    let app_weak = app.as_weak();
    app.on_report_app_appearance(move |mode| {
        if let Some(app) = app_weak.upgrade() {
            apply_appearance(&app, mode);
        }
    });
    {
        // 点击导图空白：取消分支选中与条目选中（无变化时跳过重建）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_clear_activation(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            // 无变化才跳过（多选也算变化 —— 早前只看 selected/selected_entry_path，
            // 「只有多选、没有主选中」时直接 return，导致点空白后多选高亮不消失）。
            if e.selected.is_none()
                && e.selected_entry_path.is_none()
                && e.entry_multi.is_empty()
            {
                return;
            }
            e.selected = None;
            e.selected_entry_path = None;
            // 分支级「取消激活」= 连同内容选区一起清空（含多选与锚点）。
            e.entry_multi.clear();
            e.entry_anchor = None;
            sync_ui(&app, &e);
        });
    }

    // ── 保存详情 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_save_details(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            // 硬链接视图：信息页显示的是目标分支的元信息（与内容同源）——
            // 保存落点若仍是自身 meta 会与显示不一致，故整体只读。
            if editor.borrow().sel_hard {
                show_status(
                    &app,
                    "硬链接视图的信息与目标分支关联：请到目标分支编辑。".into(),
                );
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let is_root = visit.depth == 0;
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.set_str_or_remove("title", &app.get_d_title());
                if !is_root {
                    meta.set_str_or_remove("type", app.get_d_type().trim());
                }
                meta.set_str_or_remove("summary", &app.get_d_summary());
                meta.set_str_array("tags", &parse_tags(&app.get_d_tags()));
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(())
            });
            match result {
                Ok(()) => {
                    // 分支名与 revision 取自已 rescan 的 scan（无额外 IO）。
                    let (title, rev) = {
                        let e = editor.borrow();
                        match e.selected_visit() {
                            Some(v) => (
                                visit_title(v),
                                v.meta.as_ref().and_then(|m| m.revision).unwrap_or(0),
                            ),
                            None => (String::new(), 0),
                        }
                    };
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已保存分支「{title}」的信息（revision {rev}）。").into(),
                    );
                }
                Err(msg) => show_status(&app, format!("保存失败：{msg}").into()),
            }
        });
    }

    // ── 重命名分支（只改 `.str.toml.title`；分支目录名是 UUID，不可变）──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_branch_rename_open(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = e.selected_visit() else {
                show_status(&app, "请先选择一个分支。".into());
                return;
            };
            let title = visit
                .meta
                .as_ref()
                .and_then(|m| m.title.clone())
                .unwrap_or_default();
            app.set_branch_rename_name(title.into());
            app.set_branch_rename_visible(true);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_branch_rename_cancel(move || {
            app_weak.upgrade().unwrap().set_branch_rename_visible(false);
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_branch_rename_confirm(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            app.set_branch_rename_visible(false);
            let name = app.get_branch_rename_name().trim().to_string();
            // 旧标题（重命名提示要写「旧 → 新」，与条目重命名同构）。
            let old_title = {
                let e = editor.borrow();
                e.selected_visit().map(visit_title).unwrap_or_default()
            };
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[idx];
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.set_str_or_remove("title", &name);
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(())
            });
            match result {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    let msg = if name.is_empty() {
                        format!("已清除分支「{old_title}」的标题。")
                    } else {
                        format!("已重命名分支「{old_title}」→「{name}」。")
                    };
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("重命名失败：{msg}").into()),
            }
        });
    }

    // ── 结构树右键：新建子分支对话框 / 在 Finder 中显示 ──
    {
        let app_weak = app.as_weak();
        app.on_child_dialog_open(move || {
            let app = app_weak.upgrade().unwrap();
            app.set_bc_title("".into());
            app.set_bc_type("".into());
            app.set_branch_dialog_visible(true);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_child_dialog_cancel(move || {
            let app = app_weak.upgrade().unwrap();
            // 取消即丢弃菜单传入的落点（避免残留到下一次「信息页 → 新建子分支」）。
            app.set_child_parent_visit(-1);
            app.set_branch_dialog_visible(false);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_child_dialog_create(move || {
            let app = app_weak.upgrade().unwrap();
            app.set_branch_dialog_visible(false);
            // 复用「信息」页的新建子分支流程。
            app.set_child_title(app.get_bc_title());
            app.set_child_type(app.get_bc_type());
            app.invoke_add_child();
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_reveal_branch(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = e.selected_visit() else {
                show_status(&app, "请先选择一个分支。".into());
                return;
            };
            if !visit.dir.exists() {
                show_status(&app, "在 Finder 中显示失败：分支目录不存在。".into());
                return;
            }
            reveal_in_file_manager(&visit.dir);
            show_status(
                &app,
                format!("已在 Finder 中显示分支「{}」。", visit_title(visit)).into(),
            );
        });
    }
    {
        // 分支「拷贝路径」：当前选中分支目录的绝对路径写入剪贴板。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_branch_copy_path(move || {
            let app = app_weak.upgrade().unwrap();
            let (text, title) = {
                let e = editor.borrow();
                let Some(visit) = e.selected_visit() else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                (
                    visit.dir.to_string_lossy().to_string(),
                    visit_title(visit),
                )
            };
            match copy_text_to_clipboard(&text) {
                Ok(()) => show_status(
                    &app,
                    format!("已拷贝 1 条路径 → 系统剪贴板：分支「{title}」的目录。").into(),
                ),
                Err(msg) => show_status(&app, format!("拷贝路径失败：{msg}").into()),
            }
        });
    }
    {
        // 分支「在终端中显示」：在当前选中分支目录打开终端。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_branch_reveal_terminal(move || {
            let app = app_weak.upgrade().unwrap();
            let (dir, title) = {
                let e = editor.borrow();
                let Some(visit) = e.selected_visit() else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                (visit.dir.clone(), visit_title(visit))
            };
            if !dir.exists() {
                show_status(&app, "打开终端失败：分支目录不存在。".into());
                return;
            }
            match open_terminal_at(&dir) {
                Ok(()) => show_status(
                    &app,
                    format!("已在终端中打开分支「{title}」的目录。").into(),
                ),
                Err(msg) => show_status(&app, format!("打开终端失败：{msg}").into()),
            }
        });
    }

    // ── 新建子分支 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_add_child(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            // child-parent-visit ≥ 0：右键菜单对**硬链接行**新建子分支时传入自身
            // 分支 visit（此时选中是目标真身）——子分支落硬链接自有结构；
            // -1 = 落当前选中分支（常规流程，含软链接视图 = 落目标）。
            let child_parent = app.get_child_parent_visit();
            app.set_child_parent_visit(-1);
            let title = app.get_child_title().trim().to_string();
            let type_ = app.get_child_type().trim().to_string();
            if title.is_empty() {
                show_status(&app, "新建子分支失败：标题不能为空。".into());
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未选择分支")?;
                let parent_idx = if child_parent >= 0 {
                    child_parent as usize
                } else {
                    e.selected_idx_in_visits().ok_or("未选择分支")?
                };
                let parent = &scan.visits[parent_idx];
                // 来源（父级）标题：rescan 后 selected 会指向新分支，故此时先取。
                let parent_title = visit_title(parent);
                let id_version = scan
                    .visits
                    .first()
                    .and_then(|v| v.meta.as_ref())
                    .map(|m| m.policies.id_version)
                    .unwrap_or(7);
                let id = util::new_uuid(id_version);
                let dir = parent.dir.join(&id);
                std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
                let kind = if parent.depth == 0 {
                    Kind::Node
                } else {
                    Kind::Branch
                };
                let text = meta_edit::render_branch_meta(
                    kind,
                    &id,
                    if type_.is_empty() { None } else { Some(&type_) },
                    Some(&title),
                    None,
                    &util::now_rfc3339(),
                );
                std::fs::write(bundle.meta_path(&dir), text).map_err(|err| err.to_string())?;
                let mut parent_meta = read_meta(bundle, &parent.dir)?;
                let entry = Entry {
                    path: id.clone(),
                    role: kind.as_str().to_string(),
                    id: Some(id.clone()),
                    r#type: if type_.is_empty() {
                        None
                    } else {
                        Some(type_.clone())
                    },
                    title: Some(title.clone()),
                    ..Default::default()
                };
                parent_meta.upsert_entry(&entry);
                parent_meta.touch();
                parent_meta
                    .save(&bundle.meta_path(&parent.dir))
                    .map_err(|err| err.to_string())?;
                // 展开父级并选中新分支。身份键 = rel（见 `visit_key`）：rescan 后
                // 按目录反查下标，**不按 id** —— id 可由同名复制而来、并不唯一。
                e.expanded.insert(parent.rel.clone());
                // 父级是硬链接的自有分支：再点亮硬链接行的实例展开键，否则
                // 新子分支藏在收起的硬链接行下不可见。
                let l_visit = &scan.visits[parent_idx];
                if let (Some(t), Some(mp)) = (
                    l_visit.hard_link_to.clone(),
                    l_visit.parent,
                ) {
                    // 键 = 挂载点 rel ␟ mount-hard ␟ 目标 rel（hard_link_to 即目标
                    // rel；硬链接实例键带 -hard 维度，与 mount_expand_key 同式）。
                    e.expanded.insert(format!(
                        "{}\u{1f}mount-hard\u{1f}{}",
                        visit_key(scan, mp),
                        t
                    ));
                }
                e.rescan()?;
                if let Some(key) = e
                    .scan
                    .as_ref()
                    .and_then(|s| s.visits.iter().find(|v| v.dir == dir))
                    .map(|v| v.rel.clone())
                {
                    e.selected = Some(key);
                }
                e.rebuild();
                Ok(parent_title)
            });
            match result {
                Ok(parent_title) => {
                    app.set_child_title("".into());
                    app.set_child_type("".into());
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已在「{parent_title}」下新建子分支「{title}」。").into(),
                    );
                }
                Err(msg) => show_status(&app, format!("新建子分支失败：{msg}").into()),
            }
        });
    }

    // ── 删除分支 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_delete_branch(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            // op-visit：右键菜单对**硬链接行**删除时传入自身分支 visit（此时选中
            // 的是目标真身）；-1 = 删除当前选中分支。硬链接移除 = 摘引用 + 删自有
            // 目录（含自有子分支），目标分支与数据不动。
            let mut op_visit = app.get_op_visit();
            app.set_op_visit(-1);
            // 菜单栏 ⌘⇧⌫ / 信息页「删除分支」作用于**当前选中分支**，而挂载行 /
            // 硬链接行的选中身份 = 目标真身 —— 此前会直接删掉目标分支（指向它的
            // 挂载再被悬空清理一并摘除），表现为「移除软链接把目标也删了」。现与
            // 右键菜单同语义改道：软挂载 → 只摘引用；硬链接 → 移除挂载。
            if op_visit < 0 {
                let sel_mount_parent = editor.borrow().sel_mount_parent;
                let sel_hard_view = editor.borrow().sel_hard;
                if let Some(mount_parent) = sel_mount_parent {
                    let result = with_editor(&editor, |e| {
                        let target_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                        unmount_link(e, mount_parent, target_idx)
                    });
                    match result {
                        Ok((parent_title, dst_title)) => {
                            sync_ui(&app, &editor.borrow());
                            show_status(
                                &app,
                                format!(
                                    "已移除挂载：「{dst_title}」不再显示在「{parent_title}」之下。"
                                )
                                .into(),
                            );
                        }
                        Err(msg) => show_status(&app, format!("移除挂载失败：{msg}").into()),
                    }
                    return;
                }
                if sel_hard_view {
                    // 解析指向选中目标的硬链接自身分支；解析不到（选中态残留）时
                    // 拒绝执行，绝不回落成「删目标真身」。
                    let resolved = with_editor(&editor, |e| {
                        let sel = e.selected_idx_in_visits().ok_or("未选择分支")?;
                        selected_hard_link_idx(e, sel)
                    });
                    match resolved {
                        Ok(Some(link_idx)) => op_visit = link_idx as i32,
                        Ok(None) => {
                            show_status(
                                &app,
                                "当前选中经由硬链接进入，但未找到对应挂载：请到目标真实位置删除，或右键硬链接行选择「移除挂载」。".into(),
                            );
                            return;
                        }
                        Err(msg) => {
                            show_status(&app, format!("移除挂载失败：{msg}").into());
                            return;
                        }
                    }
                }
            }
            let hard_unlink = op_visit >= 0;
            let confirmed = rfd::MessageDialog::new()
                .set_title(if hard_unlink { "移除挂载" } else { "删除分支" })
                .set_description(if hard_unlink {
                    "确定移除该硬链接？将摘除引用并删除其自有目录（含自有子分支）；目标分支与数据不变。此操作不可撤销。"
                } else {
                    "确定删除该分支及其全部子分支与文件？此操作不可撤销。"
                })
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if confirmed != rfd::MessageDialogResult::Yes {
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未选择分支")?;
                let visit_idx = if hard_unlink {
                    op_visit as usize
                } else {
                    e.selected_idx_in_visits().ok_or("未选择分支")?
                };
                let visit = &scan.visits[visit_idx];
                if visit.depth == 0 {
                    return Err("ROOT 不可删除".into());
                }
                let parent_idx = visit.parent.ok_or("缺少父分支")?;
                let parent = &scan.visits[parent_idx];
                let id = visit
                    .dir
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .ok_or("无法取得分支 id")?;
                // 软链接清理（删除前先算）：分支连同后代一起消失后，全 bundle 内指向
                // 它们的 `role = "link"` 会变成悬空（规范 §4.6.1 规则 6 要求 MUST 摘除）。
                let doomed: HashSet<usize> = subtree_indices(e, visit_idx).into_iter().collect();
                let doomed_ids: HashSet<String> = doomed
                    .iter()
                    .filter_map(|&i| scan.visits.get(i)?.meta.as_ref()?.id.clone())
                    .collect();
                let mut cleaned = 0usize;
                for (i, v) in scan.visits.iter().enumerate() {
                    if doomed.contains(&i) {
                        continue;
                    }
                    let Some(meta) = v.meta.as_ref() else {
                        continue;
                    };
                    let hits: Vec<String> = meta
                        .entries
                        .iter()
                        .filter(|en| {
                            en.is_link() && en.target.as_deref().is_some_and(|t| doomed_ids.contains(t))
                        })
                        .map(|en| en.path.clone())
                        .collect();
                    if hits.is_empty() {
                        continue;
                    }
                    let mut m = read_meta(bundle, &v.dir)?;
                    for p in &hits {
                        m.remove_entry_path(p);
                    }
                    m.touch();
                    m.save(&bundle.meta_path(&v.dir))
                        .map_err(|err| err.to_string())?;
                    cleaned += hits.len();
                }
                std::fs::remove_dir_all(&visit.dir).map_err(|err| err.to_string())?;
                let mut parent_meta = read_meta(bundle, &parent.dir)?;
                parent_meta.remove_entry_path(&id);
                parent_meta.touch();
                parent_meta
                    .save(&bundle.meta_path(&parent.dir))
                    .map_err(|err| err.to_string())?;
                e.selected = Some(parent.rel.clone());
                let title = visit_title(visit);
                e.rescan()?;
                Ok((title, cleaned))
            });
            match result {
                Ok((title, cleaned)) => {
                    sync_ui(&app, &editor.borrow());
                    // 「来源 → 目标」类提示的同类句式：清理了就写明清理了几条。
                    let tail = if cleaned == 0 {
                        String::new()
                    } else {
                        format!("，并移除 {cleaned} 条指向它的关联")
                    };
                    let msg = if hard_unlink {
                        format!("已移除硬链接「{title}」（含自有子结构，目标分支数据不变）{tail}。")
                    } else {
                        format!("已删除分支「{title}」及其全部子分支（不可撤销）{tail}。")
                    };
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("删除失败：{msg}").into()),
            }
        });
    }

    // ── entries[] 管理 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_select_entry(move |index| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            select_entry_index(&app, &mut e, index as usize);
        });
    }
    {
        // ↑↓ 移动选中（Finder 语义：边界夹紧，无选中时从第一行起）；
        // **Shift+↑↓ = 以锚点为基准的范围选区**（与鼠标 Shift+点击同一锚点，
        // 锚点不动：Shift+↓ 延伸、Shift+↑ 收回，越过锚点后反向延伸）。
        // Shift 标志 = Slint 事件 modifiers ∥ NSEvent 系统真值（native_toggle_shift）：
        // 前者在部分环境/合成事件下不填充，后者对真实键盘可靠，取或最稳。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_step(move |delta, shift_flag| {
            let app = app_weak.upgrade().unwrap();
            // NSEvent 真值只有 macOS 有；其它平台（Windows / Linux）没有该
            // 通路，Shift 判定完全交给 Slint 传入的 shift_flag。
            let (native_shift, _cmd) = {
                #[cfg(target_os = "macos")]
                {
                    native_toggle_shift()
                }
                #[cfg(not(target_os = "macos"))]
                {
                    (false, false)
                }
            };
            let shift_down = shift_flag || native_shift;
            let mut e = editor.borrow_mut();
            let rows = build_entry_rows(&e);
            if rows.is_empty() {
                return;
            }
            if shift_down {
                let cur = e
                    .selected_entry_path
                    .as_ref()
                    .and_then(|p| rows.iter().position(|r| &r.path == p))
                    .unwrap_or_else(|| selected_row_index(&app).unwrap_or(0));
                let anchor = e.entry_anchor.unwrap_or(cur);
                let new = (cur as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize;
                let (lo, hi) = (anchor.min(new), anchor.max(new));
                e.entry_multi.clear();
                for r in &rows[lo..=hi] {
                    e.entry_multi.insert(r.path.clone());
                }
                e.selected_entry_path = Some(rows[new].path.clone());
                // 锚点保持不动（下一次延伸的基准）。
                app.global::<EntryApi>()
                    .set_multi_count(e.entry_multi.len() as i32);
                sync_detail(&app, &e);
                app.set_info_tab(1);
                sync_quick_look_preview(&app, &e);
            } else {
                let next = match selected_row_index(&app) {
                    None => 0,
                    Some(cur) => (cur as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize,
                };
                select_entry_index(&app, &mut e, next);
            }
        });
    }
    {
        // ← 在非展开行上 = 选中父目录行（Finder 语义）；已展开的文件夹由
        // Slint 侧先收起（toggle-dir），走不到这里。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_select_parent(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let rows = build_entry_rows(&e);
            let Some(cur) = selected_row_index(&app) else {
                return;
            };
            if cur == 0 || cur >= rows.len() {
                return;
            }
            let depth = rows[cur].depth;
            let mut i = cur - 1;
            loop {
                if rows[i].depth < depth {
                    break;
                }
                if i == 0 {
                    return; // 已在顶层且上方无更浅的行：不动
                }
                i -= 1;
            }
            select_entry_index(&app, &mut e, i);
        });
    }
    {
        // 展开/收起内容文件夹（仅视图状态，从磁盘列出子项）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_toggle_dir(move |index| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            let vis = build_entry_rows(&e);
            if let Some(row) = vis.get(index as usize) {
                if !row.is_dir {
                    return;
                }
                let key = row.fs_path.to_string_lossy().to_string();
                if !e.expanded_dirs.remove(&key) {
                    e.expanded_dirs.insert(key);
                }
                sync_detail(&app, &e);
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_save_entry(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let path = e.selected_entry_path.clone().ok_or("未选择条目")?;
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.set_entry_str(&path, "title", app.get_e_title().trim());
                meta.set_entry_str(&path, "note", app.get_e_note().trim());
                meta.set_entry_int(&path, "order", app.get_e_order().trim().parse::<i64>().ok());
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(path)
            });
            match result {
                Ok(path) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, format!("已保存条目「{path}」的信息。").into());
                }
                Err(msg) => show_status(&app, format!("保存条目失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_add_entry(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let path = app.get_a_path().trim().to_string();
            if path.is_empty() {
                show_status(&app, "添加条目失败：路径不能为空。".into());
                return;
            }
            let role = if app.get_a_role_index() == 1 {
                "asset"
            } else {
                "payload"
            }
            .to_string();
            let result = with_editor(&editor, |e| {
                if path.contains('/') || path.starts_with('.') || path == ".str.toml" {
                    return Err("路径必须是单段文件名，且不得以 . 开头".into());
                }
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let file = visit.dir.join(&path);
                if !file.exists() {
                    std::fs::write(&file, b"").map_err(|err| err.to_string())?;
                }
                let bytes = std::fs::read(&file).map_err(|err| err.to_string())?;
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.upsert_entry(&Entry {
                    path: path.clone(),
                    role: role.clone(),
                    size: Some(bytes.len() as i64),
                    sha256: Some(util::sha256_bytes(&bytes)),
                    ..Default::default()
                });
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(())
            });
            match result {
                Ok(()) => {
                    app.set_a_path("".into());
                    let branch_title = {
                        let e = editor.borrow();
                        e.selected_visit().map(visit_title).unwrap_or_default()
                    };
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已向「{branch_title}」添加条目「{path}」（{role}）。").into(),
                    );
                }
                Err(msg) => show_status(&app, format!("添加条目失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        let slot = std::sync::Arc::clone(&batch_results);
        app.global::<EntryApi>().on_delete_entry(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            // 多选（≥2）→ 批量删除：Finder 的「移到废纸篓」作用于整个选区；
            // 批量属破坏性场景，按用户要求先确认（单项保持免确认，与 Finder 一致）。
            let (multi, visit_idx): (Vec<(String, String)>, Option<usize>) = {
                let e = editor.borrow();
                let targets = menu_targets(&app);
                let vi = match app.global::<EntryApi>().get_menu_target_visit() {
                    v if v >= 0 => Some(v as usize),
                    _ => e.selected_idx_in_visits(),
                };
                if debug_on() {
                    eprintln!(
                        "[str-gui] delete-entry visit={vi:?} targets={}",
                        targets.len()
                    );
                }
                (
                    targets
                        .into_iter()
                        .map(|p| (p.clone(), basename(&p)))
                        .collect(),
                    vi,
                )
            };
            if multi.len() >= 2 {
                if app.get_batch_busy() || app.get_summary_visible() {
                    show_status(&app, "请先等待批量操作完成。".into());
                    return;
                }
                let n = multi.len();
                let confirmed = rfd::MessageDialog::new()
                    .set_title("批量删除")
                    .set_description(format!("确定把选中的 {n} 个条目移到废纸篓？"))
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show();
                if confirmed != rfd::MessageDialogResult::Yes {
                    return;
                }
                let branch_dir = {
                    let e = editor.borrow();
                    match visit_idx {
                        Some(vi) => e.scan.as_ref().unwrap().visits[vi].dir.clone(),
                        None => return,
                    }
                };
                app.set_summary_visible(false);
                app.set_batch_busy(true);
                if let Ok(mut s) = slot.lock() {
                    s.0 = ContentOp::Trash { branch_dir };
                    s.1.clear();
                }
                spawn_gui_batch(
                    app.as_weak(),
                    std::sync::Arc::clone(&slot),
                    &format!("批量删除 · {n} 项"),
                    "批量删除中",
                    multi,
                );
                return;
            }
            // 移到废纸篓可找回，无需确认。
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                // 目标分支 = 右键记录（跨节点删除）或激活分支。
                let visit_idx = match visit_idx {
                    Some(vi) => vi,
                    None => e.selected_idx_in_visits().ok_or("未选择分支")?,
                };
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                // 右键目标优先；否则主选中 / 单元素选区（统一选择模型）。
                let targets = menu_targets(&app);
                let path = match targets.first().cloned() {
                    Some(p) => p,
                    None => match e.selected_entry_path.clone() {
                        Some(p) => p,
                        None if e.entry_multi.len() == 1 => {
                            e.entry_multi.iter().next().cloned().unwrap()
                        }
                        _ => return Err("未选择条目".into()),
                    },
                };
                let file = visit.dir.join(&path);
                if file.exists() {
                    trash::delete(&file).map_err(|err| err.to_string())?;
                }
                e.selected_entry_path = None;
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.remove_entry_path(&path);
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(path)
            });
            match result {
                Ok(path) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已删除条目「{path}」（可在废纸篓找回）。").into(),
                    );
                }
                Err(msg) => show_status(&app, format!("删除条目失败：{msg}").into()),
            }
        });
    }

    // ── 内容条目批量操作（Finder 语义）：多选 / 全选 / 批量重命名 / 汇总 ──
    {
        let editor = editor.clone();
        // 覆盖外层的 app_weak（早前已被 move 进别的闭包），本块内统一用它克隆。
        let app_weak = app.as_weak();
        // 条目行左键点击（统一入口）：修饰键决策在 Rust 侧。Slint 传入的
        // ctrl/shift 是其内部键盘状态推出的**提示值**（非 macOS 平台直接采用）；
        // macOS 上改用 NSEvent 实时查询系统真值 —— vendor 的修饰符跟踪依赖
        // ⌘ 键盘事件登记，⌘+点击场景下不可靠，曾使 ⌘ 点击退化为单选替换
        // （选区永远单条 → 复制/粘贴/删除/拖拽的数量全部错误）。
        {
            let ed = editor.clone();
            let aw = app_weak.clone();
            app.global::<EntryApi>().on_entry_click(
                move |src_visit, index, ctrl_hint, shift_hint| {
                    let app = aw.upgrade().unwrap();
                    let (toggle, range) = {
                        #[cfg(target_os = "macos")]
                        {
                            native_toggle_shift()
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            (ctrl_hint, shift_hint)
                        }
                    };
                    if debug_on() {
                        let e = ed.borrow();
                        eprintln!(
                            "[str-gui] entry-click visit={src_visit} index={index} \
                             toggle={toggle} range={range} (hint ctrl={ctrl_hint} \
                             shift={shift_hint}) multi={} primary={:?}",
                            e.entry_multi.len(),
                            e.selected_entry_path
                        );
                    }
                    let mut e = ed.borrow_mut();
                    // 跨分支点击（导图里点「未激活节点」的条目；列表视图不会出现）：
                    // 先把该面板所属分支激活，再按 index 取行。旧行为在这里直接
                    // return，于是「只有激活分支的内容才能被选中」，点其它节点的
                    // 条目完全没反应 —— 与面板「先 activate 再操作」的约定相反。
                    let mut switched = false;
                    if e.selected_idx_in_visits() != Some(src_visit as usize) {
                        let known = e
                            .scan
                            .as_ref()
                            .and_then(|s| s.visits.get(src_visit as usize))
                            .and_then(|v| v.meta.as_ref())
                            .and_then(|m| m.id.clone())
                            .is_some();
                        if !known {
                            return;
                        }
                        e.select_visit(src_visit as usize);
                        switched = true;
                    }
                    let vis = build_entry_rows(&e);
                    let Some(vr) = vis.get(index as usize) else {
                        return;
                    };
                    let path = vr.path.clone();
                    let eligible = entry_multi_eligible(&vr);
                    if range {
                        // Finder 语义：Shift 点击 = 用「锚点…当前行」**替换**整个选区
                        // （不是并入——此前多次 Shift 点击只会越选越多）。锚点保持
                        // 不变：继续 Shift 点击别处，仍以同一锚点重新划范围。
                        let anchor = e.entry_anchor.unwrap_or(index as usize);
                        let (lo, hi) = (anchor.min(index as usize), anchor.max(index as usize));
                        e.entry_multi.clear();
                        for v in vis.iter().take(hi + 1).skip(lo) {
                            if entry_multi_eligible(v) {
                                e.entry_multi.insert(v.path.clone());
                            }
                        }
                        // 主选中跟随最后点击的条目（详情面板联动）；锚点不动。
                        e.selected_entry_path = Some(path);
                    } else if toggle {
                        // ⌘ 点击文件夹子行等不可批量行：不改变选区。
                        if !eligible {
                            return;
                        }
                        // Finder 语义：把当前主选中并入选区（若尚未在），再切换点击行。
                        if e.entry_multi.is_empty() {
                            if let Some(p) = e.selected_entry_path.clone() {
                                if p != path
                                    && vis.iter().any(|v| v.path == p && entry_multi_eligible(&v))
                                {
                                    e.entry_multi.insert(p);
                                }
                            }
                        }
                        if !e.entry_multi.remove(&path) {
                            // 加入：主选中跟随最新加入的条目。
                            e.entry_multi.insert(path.clone());
                            e.selected_entry_path = Some(path);
                        } else {
                            // 切换移除：主选中必须退出（否则 col-sel 高亮残留，
                            // 表现为「反选无效果」）。若选区因此只剩 1 条，
                            // 让它成为主选中——否则切回单选视图时右侧栏空白。
                            e.selected_entry_path = None;
                            if e.entry_multi.len() == 1 {
                                e.selected_entry_path = e.entry_multi.iter().next().cloned();
                            }
                        }
                        // Finder 语义：⌘ 点击（加入或移除）都重设 Shift 锚点。
                        e.entry_anchor = Some(index as usize);
                    } else {
                        // 无修饰键 = 单选：选区 = {该条目}。单选即单元素选区，
                        // 与多选使用**同一高亮**（条目行的 in-multi 渲染）。
                        e.entry_multi.clear();
                        if eligible {
                            e.entry_multi.insert(path.clone());
                        }
                        e.entry_anchor = Some(index as usize);
                        e.selected_entry_path = Some(path);
                    }
                    // 单选 / 多选都激活「条目」tab（多选时展示多选视图）。
                    app.set_info_tab(1);
                    app.global::<EntryApi>()
                        .set_multi_count(e.entry_multi.len() as i32);
                    // 换过分支：树选中行 / 导图节点高亮 / 行模型都要跟着刷新
                    // （sync_ui 内含 sync_detail）；同分支内只刷新条目高亮即可。
                    if switched {
                        sync_ui(&app, &e);
                    } else {
                        sync_detail(&app, &e);
                    }
                },
            );
        }

        // ⌘A 全选：仅选中分支的**顶层登记条目**（文件夹子行 / 分支行不参与批量）。
        let ed = editor.clone();
        let aw = app_weak.clone();
        app.global::<EntryApi>().on_entries_select_all(move || {
            let app = aw.upgrade().unwrap();
            let mut e = ed.borrow_mut();
            for v in build_entry_rows(&e)
                .iter()
                .filter(|v| entry_multi_eligible(v))
            {
                e.entry_multi.insert(v.path.clone());
            }
            // 全选即退出主选中（详情面板回到未选条目态）。
            e.selected_entry_path = None;
            if debug_on() {
                eprintln!("[str-gui] select-all: {} 项", e.entry_multi.len());
            }
            app.set_info_tab(1);
            app.global::<EntryApi>()
                .set_multi_count(e.entry_multi.len() as i32);
            sync_detail(&app, &e);
        });

        // 批量重新命名：打开对话框（Finder「批量重新命名…」）。
        let aw = app_weak.clone();
        app.global::<EntryApi>().on_entries_batch_rename(move || {
            let app = aw.upgrade().unwrap();
            if app.get_batch_busy() || app.get_summary_visible() {
                show_status(&app, "请先等待批量操作完成。".into());
                return;
            }
            app.set_br_mode(0);
            app.set_br_find("".into());
            app.set_br_replace("".into());
            app.set_br_prefix("".into());
            app.set_br_suffix("".into());
            app.set_br_visible(true);
        });
        let aw = app_weak.clone();
        app.on_br_cancel(move || {
            aw.upgrade().unwrap().set_br_visible(false);
        });
        // 应用：按模式计算新名并校验（非法名 / 冲突即整体拒绝），交后台线程执行。
        let ed = editor.clone();
        let aw = app_weak.clone();
        let slot = std::sync::Arc::clone(&batch_results);
        app.on_br_apply(move || {
            let app = aw.upgrade().unwrap();
            if app.get_batch_busy() {
                show_status(&app, "请先等待批量操作完成。".into());
                return;
            }
            // 硬链接视图只读：批量重命名是内容写操作。
            if hard_view_guard(&app, &ed.borrow()) {
                return;
            }
            app.set_br_visible(false);
            let (branch_dir, items, renames) = {
                let e = ed.borrow();
                if e.bundle.is_none() {
                    show_status(&app, "请先打开一个 bundle。".into());
                    return;
                }
                let Some(vi) = e.selected_idx_in_visits() else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                let visit = &e.scan.as_ref().unwrap().visits[vi];
                let mode = app.get_br_mode();
                let find = app.get_br_find().trim().to_string();
                let replace = app.get_br_replace().to_string();
                let prefix = app.get_br_prefix().to_string();
                let suffix = app.get_br_suffix().to_string();
                if mode == 0 && find.is_empty() {
                    show_status(&app, "查找内容不能为空。".into());
                    return;
                }
                if mode == 1 && prefix.is_empty() && suffix.is_empty() {
                    show_status(&app, "前缀与后缀不能同时为空。".into());
                    return;
                }
                let items = collect_entry_multi(&e);
                if items.len() < 2 {
                    show_status(&app, "批量重新命名需要选中至少 2 个条目。".into());
                    return;
                }
                // 目标名集合 = 全部登记条目名 - 旧名 + 新名（逐个校验冲突）。
                let mut taken: HashSet<String> = visit
                    .meta
                    .as_ref()
                    .map(|m| m.entries.iter().map(|x| x.path.clone()).collect())
                    .unwrap_or_default();
                let mut renames: Vec<(String, String)> = Vec::with_capacity(items.len());
                for (path, _) in &items {
                    let name = basename(path);
                    let stem_ext = name.rsplit_once('.');
                    let new_name = if mode == 0 {
                        name.replace(&find, &replace)
                    } else {
                        match stem_ext {
                            // 后缀加在扩展名前：`a.txt` + 后缀 `-草案` → `a-草案.txt`
                            Some((stem, ext)) if !ext.is_empty() => {
                                format!("{prefix}{stem}{suffix}.{ext}")
                            }
                            _ => format!("{prefix}{name}{suffix}"),
                        }
                    };
                    if new_name.is_empty()
                        || new_name.contains('/')
                        || new_name.starts_with('.')
                        || new_name == ".str.toml"
                    {
                        show_status(&app, format!("名称无效：「{new_name}」（{path}）。").into());
                        return;
                    }
                    if new_name == *path {
                        renames.push((path.clone(), new_name));
                        continue;
                    }
                    taken.remove(path);
                    if taken.contains(&new_name) {
                        show_status(&app, format!("名称冲突：「{new_name}」已存在。").into());
                        return;
                    }
                    taken.insert(new_name.clone());
                    renames.push((path.clone(), new_name));
                }
                let renames: Vec<(String, String)> =
                    renames.into_iter().filter(|(o, n)| o != n).collect();
                if renames.is_empty() {
                    show_status(&app, "没有需要重命名的条目。".into());
                    return;
                }
                (visit.dir.clone(), items, renames)
            };
            let n = renames.len();
            app.set_summary_visible(false);
            app.set_batch_busy(true);
            if let Ok(mut s) = slot.lock() {
                s.0 = ContentOp::Rename {
                    branch_dir,
                    renames,
                };
                s.1.clear();
            }
            spawn_gui_batch(
                app.as_weak(),
                std::sync::Arc::clone(&slot),
                &format!("批量重新命名 · {n} 项"),
                "批量重命名中",
                items,
            );
        });

        // 汇总对话框：关闭 / 重试失败项（按上一轮的**操作类型**重跑 ✗ 的那些）。
        let aw = app_weak.clone();
        app.on_summary_close(move || {
            aw.upgrade().unwrap().set_summary_visible(false);
        });
        let aw = app_weak.clone();
        let retry_slot = std::sync::Arc::clone(&batch_results);
        app.on_summary_retry(move || {
            let app = aw.upgrade().unwrap();
            if app.get_batch_busy() {
                show_status(&app, "请先等待批量操作完成。".into());
                return;
            }
            let failed: Vec<(String, String)> = match retry_slot.lock() {
                Ok(slot) => slot
                    .1
                    .iter()
                    .filter(|r| r.status == "✗")
                    .map(|r| (r.id.clone(), r.title.clone()))
                    .collect(),
                Err(_) => return,
            };
            if failed.is_empty() {
                app.set_summary_visible(false);
                return;
            }
            let n = failed.len();
            app.set_summary_visible(false);
            app.set_batch_busy(true);
            if let Ok(mut s) = retry_slot.lock() {
                s.1.clear();
            }
            spawn_gui_batch(
                app.as_weak(),
                std::sync::Arc::clone(&retry_slot),
                &format!("重试失败项 · {n}"),
                "重试失败项中",
                failed,
            );
        });

        // 批量完成（主线程）：重扫磁盘、刷新多选集合与计数。
        let ed = editor.clone();
        let aw = app_weak.clone();
        // macOS 的剪切失效走系统剪贴板清空，内部剪贴板仅非 macOS 需要。
        #[cfg(not(target_os = "macos"))]
        let clipboard = clipboard.clone();
        let slot = std::sync::Arc::clone(&batch_results);
        app.on_batch_finished(move || {
            let app = aw.upgrade().unwrap();
            // 批量重命名：先把选区 / 主选中里的旧路径映射为新路径，
            // 否则随后的 rescan 会按「磁盘已不存在」把整个选区剪空。
            if let Ok(s) = slot.lock() {
                if let ContentOp::Rename { renames, .. } = &s.0 {
                    let mut e = ed.borrow_mut();
                    for (old, new) in renames {
                        if e.entry_multi.remove(old) {
                            e.entry_multi.insert(new.clone());
                        }
                        if e.selected_entry_path.as_deref() == Some(old.as_str()) {
                            e.selected_entry_path = Some(new.clone());
                        }
                    }
                }
            }
            // 批量粘贴到**内容文件夹**：管线不逐项登记，这里统一维护该文件夹
            // 条目的 count（必须在 rescan 之前写 meta，重扫才能读到新值）。
            if let Ok(s) = slot.lock() {
                if let ContentOp::Paste {
                    bundle_root,
                    visit_dir,
                    into_folder: Some(rel),
                    ..
                } = &s.0
                {
                    if let Ok(bundle) = Bundle::new(bundle_root.clone()) {
                        if let Ok(mut meta) = read_meta(&bundle, visit_dir) {
                            if let Some(mut den) =
                                meta.entries.iter().find(|x| x.path == *rel).cloned()
                            {
                                let folder_fs = visit_dir.join(rel);
                                den.count = Some(
                                    std::fs::read_dir(&folder_fs)
                                        .map(|rd| rd.count() as i64)
                                        .unwrap_or(0),
                                );
                                meta.upsert_entry(&den);
                                meta.touch();
                                let _ = meta.save(&bundle.meta_path(visit_dir));
                            }
                        }
                    }
                }
            }
            // 展开目标文件夹沿途父级（建行模型在 rescan 时读取展开状态）。
            if let Ok(s) = slot.lock() {
                if let ContentOp::Paste {
                    visit_dir,
                    into_folder: Some(rel),
                    ..
                } = &s.0
                {
                    if let Ok(mut e) = ed.try_borrow_mut() {
                        expand_dir_chain(&mut e, visit_dir, rel);
                    }
                }
            }
            if let Err(msg) = with_editor(&ed, |e| e.rescan()) {
                show_status(&app, format!("刷新失败：{msg}").into());
            }
            // 批量粘贴：激活被粘贴条目（选区 = 落地成功的新路径，主选中 = 第一个）。
            // 必须在 rescan 之后做，否则新路径会被「磁盘不存在」剪除。
            if let Ok(s) = slot.lock() {
                let paste_clips: Option<&Vec<ClipItem>> = match &s.0 {
                    ContentOp::Paste { clips, .. } => Some(clips),
                    _ => None,
                };
                if let Some(clips) = paste_clips {
                    let pasted: Vec<String> =
                        s.1.iter()
                            .filter(|r| r.status == "✓" && !r.new_path.is_empty())
                            .map(|r| r.new_path.clone())
                            .collect();
                    if !pasted.is_empty() {
                        let mut e = ed.borrow_mut();
                        e.entry_multi = pasted.iter().cloned().collect();
                        e.entry_anchor = None;
                        e.selected_entry_path = pasted.first().cloned();
                    }
                    // 全部成功且来自剪切 → 剪贴板失效（Finder 语义：剪切粘贴
                    // 一次后剪贴板清空，避免二次粘贴移动已移走的文件）。
                    let all_ok = s.1.iter().all(|r| r.status == "✓");
                    if all_ok && clips.iter().any(|c| c.is_cut) {
                        #[cfg(target_os = "macos")]
                        let _ = mac_clear_clipboard();
                        #[cfg(not(target_os = "macos"))]
                        clipboard.borrow_mut().clear();
                    }
                }
            }
            // 全部成功时汇总框不弹（见 `spawn_gui_batch`），由状态栏给一句摘要：
            // 这是「批量进度 / 结果统一到状态栏」的第一步；有失败时汇总框已承载
            // 逐项原因与重试入口，状态栏不再重复报同一件事。
            if let Ok(s) = slot.lock() {
                let ok = s.1.iter().filter(|r| r.status == "✓").count();
                let failed = s.1.iter().filter(|r| r.status == "✗").count();
                if ok > 0 && failed == 0 {
                    let msg = match &s.0 {
                        ContentOp::Trash { branch_dir } => format!(
                            "已删除 {ok} 项 → 分支「{}」（可在废纸篓找回）。",
                            title_of_dir(&ed.borrow(), branch_dir)
                        ),
                        ContentOp::Rename { branch_dir, .. } => format!(
                            "已在「{}」内重命名 {ok} 项。",
                            title_of_dir(&ed.borrow(), branch_dir)
                        ),
                        ContentOp::Paste {
                            visit_dir,
                            into_folder,
                            ..
                        } => {
                            // 「制作副本」与「粘贴」在上游共用 `ContentOp::Paste`
                            // （同为 is_cut=false 的同分支落地），只能按发起时我们自己
                            // 写入的对话框标题区分 —— 若日后拆出独立 op，可删此判断。
                            let verb = if app.get_summary_title().starts_with("批量制作副本") {
                                "创建副本"
                            } else {
                                "粘贴"
                            };
                            match into_folder {
                                Some(rel) => format!(
                                    "已{verb} {ok} 项 →「{}」/「{rel}」。",
                                    title_of_dir(&ed.borrow(), visit_dir)
                                ),
                                None => format!(
                                    "已{verb} {ok} 项 →「{}」。",
                                    title_of_dir(&ed.borrow(), visit_dir)
                                ),
                            }
                        }
                    };
                    show_status(&app, msg.into());
                }
            }
            let e = ed.borrow();
            app.global::<EntryApi>()
                .set_multi_count(e.entry_multi.len() as i32);
            sync_ui(&app, &e);
        });
    }

    // ── 软 / 硬链接（挂载）管理 ──
    // 候选以分支树展示；软 / 硬链接的候选禁选集合见 `mount_banned`。
    // ROOT 不允许当目标（规范 §4.6.1 规则 3）。
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_mount_dialog_open(move || {
            let app = app_weak.upgrade().unwrap();
            // 选择器展开状态以树现状为**起点**（方便按熟悉的形状找目标），
            // 之后对话框内自由展开收起、不回写树。
            // 默认选中第一个可选行；记账用 visit（行下标会随展开收起漂移）。
            {
                let mut e = editor.borrow_mut();
                e.pick_expanded = e.expanded.clone();
                e.rebuild();
                let picks = mount_pickables(&e, app.get_m_mode_index(), &e.pick_rows);
                let first = e
                    .pick_rows
                    .iter()
                    .enumerate()
                    .find(|(i, _)| picks[*i])
                    .map(|(_, r)| r.visit as i32)
                    .unwrap_or(-1);
                app.set_branch_pick_ok(picks.iter().any(|p| *p));
                app.set_branch_pick_visit(first);
            }
            app.set_m_title("".into());
            // 先置可见再 sync：sync_ui 只在对话框可见时维护选择器行模型。
            app.set_mount_dialog_visible(true);
            sync_ui(&app, &editor.borrow());
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_mount_mode_changed(move |mode| {
            let app = app_weak.upgrade().unwrap();
            // 软 / 硬切换会改变禁选集合（硬链接多排除后代）——重置默认选中，
            // 并让 sync_ui 按新形态重算行上的 pickable。
            {
                let e = editor.borrow();
                let picks = mount_pickables(&e, mode, &e.pick_rows);
                let first = e
                    .pick_rows
                    .iter()
                    .enumerate()
                    .find(|(i, _)| picks[*i])
                    .map(|(_, r)| r.visit as i32)
                    .unwrap_or(-1);
                app.set_branch_pick_ok(picks.iter().any(|p| *p));
                app.set_branch_pick_visit(first);
            }
            sync_ui(&app, &editor.borrow());
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_mount_dialog_close(move || {
            let app = app_weak.upgrade().unwrap();
            // 丢弃菜单传入的挂载落点（避免残留到下一次「菜单栏 → 挂载已有分支」）。
            app.set_mount_parent_visit(-1);
            app.set_mount_dialog_visible(false);
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_mount_create(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let pick_visit = app.get_branch_pick_visit();
            if pick_visit < 0 {
                show_status(&app, "挂载失败：请先选择一个可选的分支。".into());
                return;
            }
            // mount-parent-visit ≥ 0：右键菜单对**硬链接行 / 节点**挂载时传入自身
            // 分支 visit（此时选中是目标真身）——链接落硬链接自有 entries，而不是
            // 目标分支（否则目标分支与硬链接行两处都出现该链接）；-1 = 常规流程。
            let mount_parent = app.get_mount_parent_visit();
            app.set_mount_parent_visit(-1);
            let mode = app.get_m_mode_index();
            let hard = mode == 1;
            // 关联线语义（规范 §5.3）：与 CLI `str ref add` 的 `--rel` 同一枚举。
            let rel = match app.get_m_rel_index() {
                1 => "depends_on",
                2 => "instance_of",
                3 => "derived_from",
                4 => "ref",
                _ => "related",
            };
            let alias = app.get_m_title().trim().to_string();
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未选择分支")?;
                let src_idx = if mount_parent >= 0 {
                    if mount_parent as usize >= scan.visits.len() {
                        return Err("挂载落点分支已不存在（树已变化）".into());
                    }
                    mount_parent as usize
                } else {
                    let sel = e.selected_idx_in_visits().ok_or("未选择分支")?;
                    // 菜单栏路径兜底：选中经由硬链接行 / 节点进入（sel_hard）时，
                    // 落点解析到该硬链接自身分支，而非其目标真身。
                    if e.sel_hard {
                        match selected_hard_link_idx(e, sel)? {
                            Some(idx) => idx,
                            None => sel,
                        }
                    } else {
                        sel
                    }
                };
                let dst_visit = pick_visit as usize;
                // 打开对话框后树可能已变（展开收起 / 其他窗口改动）：写前按
                // **当下**禁选集合复核一次，不可选即拒绝。
                if mount_banned(e, mode).contains(&dst_visit) {
                    return Err("目标分支当前不可挂载（自身 / 祖先 / ROOT / id 重复等）".into());
                }
                let target_id = scan
                    .visits
                    .get(dst_visit)
                    .and_then(|v| v.meta.as_ref())
                    .and_then(|m| m.id.clone())
                    .ok_or_else(|| "目标分支缺少 id".to_string())?;
                let dst_idx = scan.resolve(&target_id).ok_or("目标分支无法解析")?;
                // 目标 id 必须唯一存在：id 会被 Finder 复制而重复（E_ID_DUP），
                // 重复时任何解析都只能命中首次 ⇒ 拒绝挂载。
                let hits = scan.by_id.get(&target_id).map(|v| v.len()).unwrap_or(0);
                if hits == 0 {
                    return Err(format!("目标 id 不在本 bundle 内：{target_id}"));
                }
                if hits > 1 {
                    return Err(format!(
                        "目标 id 在 {hits} 个分支上重复，无法唯一指向：{target_id}"
                    ));
                }
                if scan.visits[dst_idx].depth == 0 {
                    return Err("关联目标不得是 ROOT".into());
                }
                if dst_idx == src_idx {
                    return Err("不能把分支挂载到它自己下面".into());
                }
                // 成环预检（与 CLI `link_add` / 校验器 `check_link_cycles` 同源）：
                // 沿真实父子边 + 挂载边，从目标出发能回到自己 ⇒ 挂进祖先都拒绝。
                if link_would_cycle(scan, src_idx, dst_idx) {
                    return Err("该挂载会形成环（挂进自己的祖先会无限递归）".into());
                }
                let src = &scan.visits[src_idx];
                let src_title = visit_title(src);
                let dst_title = visit_title(&scan.visits[dst_idx]);
                // 关联线（refs，规范 §4.5）：只画一条跨树关联线，不进结构树、
                // 不写 `entries` —— 与挂载（§4.6.1）是两种正交意图，按此处分流。
                // 自关联已被禁选集合挡住；指向祖先 / 后代合法（环判定只沿 `refs`
                // 边，由校验器 `E_REF_CYCLE` 兜底）；同一目标多条关联线允许。
                if mode == 2 {
                    let id_version = scan
                        .visits
                        .first()
                        .and_then(|v| v.meta.as_ref())
                        .map(|m| m.policies.id_version)
                        .unwrap_or(7);
                    let mut meta = read_meta(bundle, &src.dir)?;
                    meta.push_ref(&RefItem {
                        id: util::new_uuid(id_version),
                        target: target_id.clone(),
                        rel: rel.into(),
                        title: if alias.is_empty() { None } else { Some(alias) },
                        order: Some(meta.refs.len() as i64 + 1),
                        note: None,
                    });
                    meta.touch();
                    meta.save(&bundle.meta_path(&src.dir))
                        .map_err(|err| err.to_string())?;
                    e.rescan()?;
                    return Ok((src_title, dst_title, format!("关联线（{rel}）")));
                }
                let mut meta = read_meta(bundle, &src.dir)?;
                // 重复挂载检查仅对软链接（path = target）；硬链接 path = 新生成的
                // 自身 id，天然不重复。**任何角色**的同 path 条目都冲突：目标若是
                // 本分支的真实子分支，upsert 会把真身的登记替换成链接行 —— 拒绝。
                if !hard
                    && meta
                        .entries
                        .iter()
                        .any(|en| en.path == target_id)
                {
                    return Err(format!(
                        "「{dst_title}」已是本分支的真实子分支或挂载，无法重复挂载"
                    ));
                }
                // 软链接：`path` = 目标 id，**不带 id**（身份由 target 给出，声明式，
                // 磁盘不新建目录）。硬链接：有身份的真实分支 —— `path` = `id` =
                // 自身目录名，现在创建目录与 `.str.toml`；内容所有权仍在目标分支
                // （自有 entries 只允许子分支，内容面板显示目标内容且只读）。
                let (entry_path, entry_id, entry_title) = if hard {
                    let id_version = scan
                        .visits
                        .first()
                        .and_then(|v| v.meta.as_ref())
                        .map(|m| m.policies.id_version)
                        .unwrap_or(7);
                    let id = util::new_uuid(id_version);
                    let dir = src.dir.join(&id);
                    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
                    let kind = if src.depth == 0 {
                        Kind::Node
                    } else {
                        Kind::Branch
                    };
                    // 别名留空时，硬链接自身的标题默认取**目标标题**（显示语义与
                    // 软链接一致：不填别名就显示目标的名字）。
                    let own_title = if alias.is_empty() {
                        dst_title.clone()
                    } else {
                        alias.clone()
                    };
                    let text = meta_edit::render_branch_meta(
                        kind,
                        &id,
                        None,
                        Some(&own_title),
                        None,
                        &util::now_rfc3339(),
                    );
                    std::fs::write(bundle.meta_path(&dir), text).map_err(|err| err.to_string())?;
                    (id.clone(), Some(id), None)
                } else {
                    (
                        target_id.clone(),
                        None,
                        if alias.is_empty() { None } else { Some(alias) },
                    )
                };
                meta.upsert_entry(&Entry {
                    path: entry_path,
                    role: "link".into(),
                    id: entry_id,
                    target: Some(target_id),
                    mode: Some(if hard { "hard" } else { "soft" }.into()),
                    title: entry_title,
                    order: Some(meta.entries.len() as i64 + 1),
                    ..Default::default()
                });
                meta.touch();
                meta.save(&bundle.meta_path(&src.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok((
                    src_title,
                    dst_title,
                    if hard { "硬链接" } else { "软链接" }.to_string(),
                ))
            });
            match result {
                Ok((src_title, dst_title, kind)) => {
                    app.set_m_title("".into());
                    app.set_mount_dialog_visible(false);
                    sync_ui(&app, &editor.borrow());
                    let msg = if mode == 2 {
                        format!("已创建关联线（{kind}）：「{src_title}」→「{dst_title}」。")
                    } else {
                        format!("已挂载（{kind}）：「{dst_title}」→「{src_title}」之下。")
                    };
                    show_status(&app, msg.into());
                }
                Err(msg) => {
                    let noun = if mode == 2 { "关联" } else { "挂载" };
                    show_status(&app, format!("{noun}失败：{msg}").into())
                }
            }
        });
    }
    {
        // 摘除挂载：`parent_visit` = 挂载点所在分支，目标 = 当前选中分支
        // （挂载行的身份就是目标，点它即选中真身）。只摘引用，真身与数据不动。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_unmount(move |parent_visit| {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let result = with_editor(&editor, |e| {
                let dst_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let parent_idx = parent_visit.max(0) as usize;
                unmount_link(e, parent_idx, dst_idx)
            });
            match result {
                Ok((parent_title, dst_title)) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已移除挂载：「{dst_title}」不再显示在「{parent_title}」之下。")
                            .into(),
                    );
                }
                Err(msg) => show_status(&app, format!("移除挂载失败：{msg}").into()),
            }
        });
    }
    {
        // 关联线管理（refs[]，规范 §4.5，双向）：查看 / 修改 / 删除当前选中
        // 分支的关联线 —— 正向（本分支 refs）与反向（其它分支指向本分支）；
        // 反向行的修改与删除都落在**源分支**的 meta 上。创建走「创建链接…」
        // 对话框「关联线」形态。
        let editor = editor.clone();
        let editor_delete = editor.clone();
        let editor_edit = editor.clone();
        let editor_edit_save = editor.clone();
        let app_weak = app.as_weak();
        let app_weak_close = app_weak.clone();
        let app_weak_delete = app_weak.clone();
        let app_weak_edit_open = app_weak.clone();
        let app_weak_edit_save = app_weak.clone();
        let app_weak_edit_cancel = app_weak.clone();
        // 重建行模型（对话框停留时复用）：rescan 后 visit 下标漂移，统一按
        // e.selected 的 rel 键重新解析。
        fn refresh_refs_dialog(app: &AppWindow, e: &Editor) {
            let (rows, branch_title) = collect_ref_rows(e);
            app.set_refs_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
            app.set_refs_branch_title(branch_title.into());
        }
        app.on_refs_dialog_open(move || {
            let app = app_weak.upgrade().unwrap();
            let (rows, branch_title) = {
                let e = editor.borrow();
                collect_ref_rows(&e)
            };
            app.set_refs_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
            app.set_refs_branch_title(branch_title.into());
            app.set_refs_dialog_visible(true);
        });
        app.on_refs_dialog_close(move || {
            let app = app_weak_close.upgrade().unwrap();
            app.set_refs_dialog_visible(false);
        });
        // 按关联线 id 在全 bundle 定位持有它的源分支（对话框行可以是反向的 ——
        // 此时源分支不是当前选中分支，与 CLI `str ref rm` 的全 bundle 定位同口径）。
        fn locate_ref_source(e: &Editor, ref_id: &str) -> Result<PathBuf, String> {
            let scan = e.scan.as_ref().ok_or("未选择分支")?;
            e.bundle.as_ref().ok_or("未打开 bundle")?;
            for v in &scan.visits {
                if let Some(m) = v.meta.as_ref() {
                    if m.refs.iter().any(|r| r.id == ref_id) {
                        return Ok(v.dir.clone());
                    }
                }
            }
            Err("关联线不存在（可能已被删除）".into())
        }
        app.on_ref_delete(move |ref_id| {
            let app = app_weak_delete.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let ref_id = ref_id.to_string();
            let result = with_editor(&editor_delete, |e| {
                let dir = locate_ref_source(e, &ref_id)?;
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
                let mut meta = read_meta(&bundle, &dir)?;
                if !meta.remove_ref(&ref_id) {
                    return Err("关联线不存在（可能已被删除）".into());
                }
                meta.touch();
                meta.save(&bundle.meta_path(&dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(())
            });
            match result {
                Ok(()) => {
                    // 停留在此对话框：按当前选中分支重建行模型（rescan 后重新解析）。
                    refresh_refs_dialog(&app, &editor_delete.borrow());
                    sync_ui(&app, &editor_delete.borrow());
                    show_status(&app, "已删除关联线（只摘引用，目标分支与数据不动）。".into());
                }
                Err(msg) => show_status(&app, format!("删除关联线失败：{msg}").into()),
            }
        });
        app.on_ref_edit_open(move |ref_id, peer, incoming| {
            let app = app_weak_edit_open.upgrade().unwrap();
            let ref_id = ref_id.to_string();
            {
                let e = editor_edit.borrow();
                // 从源分支 meta 读当前值（扫描快照里有 refs，直接取，无需再读盘）。
                let item = e.scan.as_ref().and_then(|scan| {
                    scan.visits.iter().find_map(|v| {
                        v.meta
                            .as_ref()
                            .and_then(|m| m.refs.iter().find(|r| r.id == ref_id))
                    })
                });
                let Some(r) = item else {
                    show_status(&app, "关联线不存在（可能已被删除）".into());
                    return;
                };
                app.set_ref_edit_id(r.id.clone().into());
                app.set_ref_edit_peer(peer);
                app.set_ref_edit_incoming(incoming);
                app.set_ref_edit_rel_index(match r.rel.as_str() {
                    "depends_on" => 1,
                    "instance_of" => 2,
                    "derived_from" => 3,
                    "ref" => 4,
                    "related" => 0,
                    // x-* 等自定义 rel：下拉「自定义…」，保存时保持原值。
                    _ => 5,
                });
                app.set_ref_edit_label(r.title.clone().unwrap_or_default().into());
            }
            app.set_ref_edit_visible(true);
        });
        app.on_ref_edit_cancel({
            let app_weak = app_weak_edit_cancel.clone();
            move || {
                let app = app_weak.upgrade().unwrap();
                app.set_ref_edit_visible(false);
            }
        });
        app.on_ref_edit_save({
            let app_weak = app_weak_edit_save.clone();
            move || {
                let app = app_weak.upgrade().unwrap();
                if batch_busy_guard(&app) {
                    return;
                }
                let ref_id = app.get_ref_edit_id().to_string();
                let label = app.get_ref_edit_label().trim().to_string();
                let rel_index = app.get_ref_edit_rel_index();
                let result = with_editor(&editor_edit_save, |e| {
                    let dir = locate_ref_source(e, &ref_id)?;
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
                    let mut meta = read_meta(&bundle, &dir)?;
                    {
                        let Some(r) = meta.refs.iter_mut().find(|r| r.id == ref_id) else {
                            return Err("关联线不存在（可能已被删除）".into());
                        };
                        // rel-index 5 = 自定义 rel（x-* 等）：下拉框无法表达，保持原值。
                        if rel_index != 5 {
                            r.rel = match rel_index {
                                0 => "related",
                                1 => "depends_on",
                                2 => "instance_of",
                                3 => "derived_from",
                                _ => "ref",
                            }
                            .into();
                        }
                        r.title = if label.is_empty() { None } else { Some(label) };
                    }
                    meta.touch();
                    meta.save(&bundle.meta_path(&dir))
                        .map_err(|err| err.to_string())?;
                    e.rescan()?;
                    Ok(())
                });
                match result {
                    Ok(()) => {
                        app.set_ref_edit_visible(false);
                        // 管理对话框仍开着：按当前选中分支重建行模型。
                        refresh_refs_dialog(&app, &editor_edit_save.borrow());
                        sync_ui(&app, &editor_edit_save.borrow());
                        show_status(&app, "已更新关联线（只改声明，目标分支与数据不动）。".into());
                    }
                    Err(msg) => show_status(&app, format!("修改关联线失败：{msg}").into()),
                }
            }
        });
    }

    // ── entries[]：右键菜单 / 复制剪切粘贴 / 拖拽 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_clear_selection(move || {
            let app = app_weak.upgrade().unwrap();
            {
                let mut e = editor.borrow_mut();
                e.selected_entry_path = None;
                // 点空白 = 取消选中（Finder 语义）：清空整个选区（含多选）。
                e.entry_multi.clear();
                e.entry_anchor = None;
            }
            app.global::<EntryApi>().set_multi_count(0);
            // 必须重建条目模型：in-multi 高亮烧在模型行里，仅改状态不会刷新高亮。
            sync_detail(&app, &editor.borrow());
            app.set_info_tab(0);
        });
    }
    // 由行号解析条目路径（行号对应过滤后的内容条目列表）。
    // （选区解析已收敛到 menu-targets：fill_entry_fields 随每次选中状态变化刷新。）
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_reveal(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = action_visit(&app, &e) else {
                show_status(&app, "请先选择一个分支。".into());
                return;
            };
            let targets = menu_targets(&app);
            if targets.is_empty() {
                show_status(&app, "请先选择一个条目。".into());
                return;
            }
            let title = visit_title(visit);
            let full: Vec<PathBuf> = targets.iter().map(|p| visit.dir.join(p)).collect();
            let n = full.len();
            let reveal_ok = reveal_in_file_manager_multi(&full).is_ok();
            drop(e);
            // **无条件提示**：此前只有「多条或失败」才提示，单条成功静默 ——
            // 同一动作两种可见性，属覆盖缺口。
            if reveal_ok {
                show_status(
                    &app,
                    if n > 1 {
                        format!(
                            "已在 Finder 中显示 {n} 项 → 分支「{title}」：{}。",
                            name_list(&targets)
                        )
                        .into()
                    } else {
                        format!(
                            "已在 Finder 中显示「{}」→ 分支「{title}」。",
                            targets.first().map(String::as_str).unwrap_or("")
                        )
                        .into()
                    },
                );
            } else {
                show_status(
                    &app,
                    format!("在 Finder 中显示失败：分支「{title}」。").into(),
                );
            }
        });
    }
    {
        // 拷贝路径：整个选区的绝对路径（\n 分隔）写入系统剪贴板。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_copy_path(move || {
            let app = app_weak.upgrade().unwrap();
            let (text, title, names) = {
                let e = editor.borrow();
                let Some(visit) = action_visit(&app, &e) else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                let names: Vec<String> = menu_targets(&app);
                let text = names
                    .iter()
                    .map(|p| visit.dir.join(p).to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                (text, visit_title(visit), names)
            };
            if text.is_empty() {
                return;
            }
            match copy_text_to_clipboard(&text) {
                Ok(()) => {
                    let n = names.len();
                    show_status(
                        &app,
                        if n > 1 {
                            format!(
                                "已拷贝 {n} 条路径 → 系统剪贴板：{}。",
                                name_list(&names)
                            )
                            .into()
                        } else {
                            format!("已拷贝 1 条路径 → 系统剪贴板：分支「{title}」下的「{}」。", names.first().map(String::as_str).unwrap_or("")).into()
                        },
                    );
                }
                Err(msg) => show_status(&app, format!("拷贝路径失败：{msg}").into()),
            }
        });
    }
    {
        // 在终端中显示：目录条目 → 该目录；文件 → 所在文件夹；多选去重后逐个打开。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_reveal_terminal(move || {
            let app = app_weak.upgrade().unwrap();
            let (dirs, title): (Vec<PathBuf>, String) = {
                let e = editor.borrow();
                let Some(visit) = action_visit(&app, &e) else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                let mut dirs: Vec<PathBuf> = Vec::new();
                for p in menu_targets(&app) {
                    let full = visit.dir.join(&p);
                    let dir = if full.is_dir() {
                        full
                    } else {
                        full.parent().unwrap_or(&visit.dir).to_path_buf()
                    };
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }
                (dirs, visit_title(visit))
            };
            let mut ok = 0usize;
            for d in &dirs {
                if open_terminal_at(d).is_ok() {
                    ok += 1;
                }
            }
            show_status(&app, if ok > 0 {
                format!("已在终端中打开分支「{title}」下的 {ok} 个目录。").into()
            } else {
                format!("在终端中打开失败：分支「{title}」。").into()
            });
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_open(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = action_visit(&app, &e) else {
                show_status(&app, "请先选择一个分支。".into());
                return;
            };
            let targets = menu_targets(&app);
            if targets.is_empty() {
                show_status(&app, "请先选择一个条目。".into());
                return;
            }
            let title = visit_title(visit);
            for p in &targets {
                let full = visit.dir.join(p);
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(&full).spawn();
                #[cfg(windows)]
                let _ = std::process::Command::new("explorer")
                    .arg(format!("/select,{}", full.display()))
                    .spawn();
                #[cfg(all(unix, not(target_os = "macos")))]
                let _ = std::process::Command::new("xdg-open").arg(&full).spawn();
            }
            // 交给系统应用打开后不返回结果：**改为无条件提示**（此前完全静默 ——
            // 多选时用户无法确认到底打开了几项）。
            show_status(
                &app,
                if targets.len() > 1 {
                    format!(
                        "已用默认应用打开 {} 项 → 分支「{title}」：{}。",
                        targets.len(),
                        name_list(&targets)
                    )
                    .into()
                } else {
                    format!("已用默认应用打开「{}」→ 分支「{title}」。", targets[0]).into()
                },
            );
        });
    }
    // 快速查看是否开着。面板**不抢 key window**（见 quicklook 模块），方向键
    // 始终归内容列表；开 / 关与 Esc 都由应用这边驱动。
    let quicklook_open: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    // 本地键盘监视器：面板虽不 makeKey，但 QLPreviewPanel 在应用激活时会自动
    // 成为 key window、把方向键吃掉（拿去翻页）。预览开着时只拦 **↑↓ / 空格 /
    // Esc** —— ↑↓ 转交 Slint 窗口（列表移动选中，面板实时跟着换预览），
    // **←→ 留给预览面板翻页**（不占用它的左右键），其余按键照常放行。
    // 仅 macOS：QLPreviewPanel / NSEvent 都是 AppKit 对象，其它平台无此通路
    //（quicklook 模块在非 macOS 是返回 Err 的桩，面板根本打不开）。
    // 注：这里必须把 #[cfg] 标在**语句**上而非套一层 `{}` —— 套块会让
    // `_ql_monitor` 在块尾即刻析构，监视器当场失效。绑定活到本函数末尾。
    #[cfg(target_os = "macos")]
    let _ql_monitor: Option<objc2::rc::Retained<objc2::runtime::AnyObject>> = unsafe {
        let app_weak = app.as_weak();
        let open = quicklook_open.clone();
        let block = block2::RcBlock::new(
            move |evt: std::ptr::NonNull<objc2_app_kit::NSEvent>| -> *mut objc2_app_kit::NSEvent {
                if !open.get() {
                    return evt.as_ptr();
                }
                let e = evt.as_ref();
                const LEFT: u16 = 123;
                const RIGHT: u16 = 124;
                const DOWN: u16 = 125;
                const UP: u16 = 126;
                const SPACE: u16 = 49;
                const ESC: u16 = 53;
                let code = e.keyCode();
                if !matches!(code, LEFT | RIGHT | DOWN | UP | SPACE | ESC) {
                    return evt.as_ptr();
                }
                // ←→ 的归属看多选状态：多选预览时归面板翻页（放行）；
                // 单选预览时仍归列表（→ 展开 / ← 收起跳父），照常转交 Slint。
                let Some(app) = app_weak.upgrade() else {
                    return evt.as_ptr();
                };
                let multi = app.global::<EntryApi>().get_multi_count() > 1;
                if multi && matches!(code, LEFT | RIGHT) {
                    return evt.as_ptr();
                }
                let text = e.characters().map(|s| s.to_string()).unwrap_or_default();
                app.window()
                    .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                        text: text.into(),
                    });
                // 已转交 Slint：返回 nil，事件不再分发给面板（不翻页）。
                std::ptr::null_mut()
            },
        );
        objc2_app_kit::NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            objc2_app_kit::NSEventMask::KeyDown,
            &block,
        )
    };
    {
        // 快速查看（空格键 / 右键菜单「快速查看」）：macOS 系统 QLPreviewPanel。
        // **预览集 = 选区本身**（Finder 严格语义）：单选 1 项、多选这几项。
        // 面板打开期间方向键照常作用于列表，选中走到哪条就预览哪条（见
        // `select_entry_index` 末尾的跟随）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        let open = quicklook_open.clone();
        app.global::<EntryApi>().on_entry_quick_look(move || {
            let app = app_weak.upgrade().unwrap();
            if open.get() {
                // 再按一次空格 = 关闭（Finder 的切换语义）。
                open.set(false);
                app.global::<EntryApi>().set_quick_look_open(false);
                quicklook::close();
                show_status(&app, "已关闭快速查看。".into());
                return;
            }
            let (dir, rels) = {
                let e = editor.borrow();
                let Some(visit) = action_visit(&app, &e) else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                (visit.dir.clone(), menu_targets(&app))
            };
            if rels.is_empty() {
                show_status(&app, "请先选择一个条目。".into());
                return;
            }
            let n = rels.len();
            let full: Vec<std::path::PathBuf> = rels.iter().map(|p| dir.join(p)).collect();
            match quicklook::show(&full) {
                Ok(()) => {
                    open.set(true);
                    app.global::<EntryApi>().set_quick_look_open(true);
                    show_status(
                        &app,
                        if n > 1 {
                            format!("已打开快速查看（{n} 项）：{}。", name_list(&rels)).into()
                        } else {
                            format!("已打开快速查看：「{}」。", rels[0]).into()
                        },
                    );
                }
                Err(msg) => show_status(&app, format!("快速查看失败：{msg}").into()),
            }
        });
    }
    {
        // Esc 关闭预览：面板不抢 key，Esc 由列表 FocusScope 接住后转到这里。
        let app_weak = app.as_weak();
        let open = quicklook_open.clone();
        app.global::<EntryApi>().on_entry_quick_look_close(move || {
            let app = app_weak.upgrade().unwrap();
            open.set(false);
            app.global::<EntryApi>().set_quick_look_open(false);
            quicklook::close();
            show_status(&app, "已关闭快速查看。".into());
        });
    }
    {
        let editor = editor.clone();
        let clipboard = clipboard.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_copy(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = action_visit(&app, &e) else {
                show_status(&app, "请先选择一个分支。".into());
                return;
            };
            let paths = menu_targets(&app);
            if paths.is_empty() {
                return;
            }
            // 未登记的子行按文件系统推断角色（payload / dir）。
            let items: Vec<ClipItem> = paths
                .iter()
                .filter_map(|p| {
                    entry_role_for(visit, p).map(|role| ClipItem {
                        src_path: visit.dir.join(p),
                        name: p.clone(),
                        role,
                        is_cut: false,
                    })
                })
                .collect();
            let n = items.len();
            // 来源分支标题（「已从「X」拷贝…」）。
            let src_title = visit_title(visit);
            let names: Vec<String> = items.iter().map(|c| c.name.clone()).collect();
            *clipboard.borrow_mut() = items;
            // 同步到系统剪贴板（无剪切标记）：Finder 中 ⌘V 即可粘贴。
            #[cfg(target_os = "macos")]
            let sys_ok = mac_write_clipboard_files(
                &clipboard
                    .borrow()
                    .iter()
                    .map(|c| c.src_path.clone())
                    .collect::<Vec<_>>(),
                false,
            )
            .is_ok();
            #[cfg(not(target_os = "macos"))]
            let sys_ok = false;
            // 去向 = 剪贴板类型（与「来源 → 目标」模板一致，不再用括号备注表达）。
            let dst = if sys_ok { "系统剪贴板" } else { "内部剪贴板" };
            show_status(&app, 
                if n > 1 {
                    format!(
                        "已从「{src_title}」拷贝 {} 项 → {dst}：{}。",
                        n,
                        name_list(&names)
                    )
                } else {
                    format!(
                        "已从「{src_title}」拷贝「{}」→ {dst}。",
                        names.first().map(String::as_str).unwrap_or("")
                    )
                }
                .into(),
            );
        });
    }
    {
        // 剪切：记录到内部剪贴板，粘贴时执行移动。
        let editor = editor.clone();
        let clipboard = clipboard.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_cut(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = action_visit(&app, &e) else {
                show_status(&app, "请先选择一个分支。".into());
                return;
            };
            let paths = menu_targets(&app);
            if paths.is_empty() {
                return;
            }
            let items: Vec<ClipItem> = paths
                .iter()
                .filter_map(|p| {
                    entry_role_for(visit, p).map(|role| ClipItem {
                        src_path: visit.dir.join(p),
                        name: p.clone(),
                        role,
                        is_cut: true,
                    })
                })
                .collect();
            let n = items.len();
            // 来源分支标题（「已从「X」剪切…」）。
            let src_title = visit_title(visit);
            let single_name = items.first().map(|c| c.name.clone()).unwrap_or_default();
            let names: Vec<String> = items.iter().map(|c| c.name.clone()).collect();
            // macOS：系统剪贴板是粘贴的唯一事实来源——剪切也要写入源文件 URL，
            // 并随写入打上剪切标记（粘贴时据此执行移动而非复制）。
            #[cfg(target_os = "macos")]
            {
                let src_paths: Vec<PathBuf> = items.iter().map(|c| c.src_path.clone()).collect();
                let _ = mac_write_clipboard_files(&src_paths, true);
            }
            *clipboard.borrow_mut() = items;
            // 去向 = 剪贴板类型；「粘贴时移动」为固定备注，置于名称列表之前。
            #[cfg(target_os = "macos")]
            let dst = "系统剪贴板";
            #[cfg(not(target_os = "macos"))]
            let dst = "内部剪贴板";
            show_status(&app, if n > 1 {
                format!(
                    "已从「{src_title}」剪切 {} 项 → {dst}（粘贴时移动）：{}。",
                    n,
                    name_list(&names)
                )
                .into()
            } else {
                format!(
                    "已从「{src_title}」剪切「{}」→ {dst}（粘贴时移动）。",
                    single_name
                )
                .into()
            });
        });
    }
    {
        // 制作副本：在当前分支内复制一份（重名自动「副本」后缀）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        let slot_dupe = std::sync::Arc::clone(&batch_results);
        app.global::<EntryApi>().on_entry_duplicate(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            if hard_view_guard(&app, &editor.borrow()) {
                return;
            }
            // 制作副本 = 同分支拷贝（is_cut=false）。
            let (clips, visit_dir, depth, id_version, bundle_root) = {
                let e = editor.borrow();
                let Some(visit) = action_visit(&app, &e) else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                let paths = menu_targets(&app);
                if paths.is_empty() {
                    show_status(&app, "请先选择一个条目。".into());
                    return;
                }
                let clips: Vec<ClipItem> = paths
                    .iter()
                    .filter_map(|p| {
                        entry_role_for(visit, p).map(|role| ClipItem {
                            src_path: visit.dir.join(p),
                            name: p.clone(),
                            role,
                            is_cut: false,
                        })
                    })
                    .collect();
                let idv = e
                    .scan
                    .as_ref()
                    .unwrap()
                    .visits
                    .first()
                    .and_then(|v| v.meta.as_ref())
                    .map(|m| m.policies.id_version)
                    .unwrap_or(7);
                let root = e
                    .bundle
                    .as_ref()
                    .map(|b| b.root.clone())
                    .unwrap_or_default();
                (clips, visit.dir.clone(), visit.depth, idv, root)
            };
            if clips.is_empty() {
                show_status(&app, "请先选择一个条目。".into());
                return;
            }
            if clips.len() == 1 {
                // 单条：同步执行。
                let result = with_editor(&editor, |e| {
                    let src = clips[0].src_path.clone();
                    let visit_dir = e
                        .selected_idx_in_visits()
                        .and_then(|i| e.scan.as_ref().map(|s| s.visits[i].dir.clone()))
                        .ok_or("未选择分支")?;
                    // 源在**内容文件夹内**（未登记子行）⇒ 副本必须落在**源所在目录**
                    // （Finder 语义）。旧路径经 apply_clip 永远落到分支根，且未登记项
                    // 在 entries 里查不到 —— 表现为「制作副本不正常」。
                    let parent = src.parent().ok_or("无法定位源目录")?.to_path_buf();
                    if parent != visit_dir {
                        let name = src
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .ok_or("无法取得文件名")?;
                        let existing: HashSet<String> = std::fs::read_dir(&parent)
                            .map(|rd| {
                                rd.filter_map(|x| x.ok())
                                    .map(|x| x.file_name().to_string_lossy().to_string())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let new_name = dedup_name(&existing, &name);
                        let dst = parent.join(&new_name);
                        if src.is_dir() {
                            copy_dir_recursive(&src, &dst)?;
                        } else {
                            std::fs::copy(&src, &dst).map_err(|err| err.to_string())?;
                        }
                        // 未登记子行不登记（与内容文件夹语义一致），只更新选中。
                        let rel = dst
                            .strip_prefix(&visit_dir)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|_| new_name.clone());
                        e.entry_multi = [rel.clone()].into_iter().collect();
                        e.entry_anchor = None;
                        e.selected_entry_path = Some(rel);
                        return Ok(format!(
                            "已在「{}」内创建副本「{new_name}」。",
                            title_of_dir(e, &parent)
                        ));
                    }
                    apply_clip(e, &clips[0]).map(|(_m, pasted)| {
                        e.entry_multi = [pasted.clone()].into_iter().collect();
                        e.entry_anchor = None;
                        e.selected_entry_path = Some(pasted.clone());
                        // `apply_clip_at` 的文案面向「粘贴 / 移动」，制作副本须自述动作
                        // （否则会显示「已从剪贴板粘贴…」，与用户操作不符）。
                        format!(
                            "已在「{}」内创建副本「{pasted}」。",
                            title_of_dir(e, &visit_dir)
                        )
                    })
                });
                match result {
                    Ok(msg) => {
                        sync_ui(&app, &editor.borrow());
                        show_status(&app, msg.into());
                    }
                    Err(msg) => show_status(&app, format!("制作副本失败：{msg}").into()),
                }
                return;
            }
            // 多条：批量管线（进度 + 汇总 + 重试）；激活在 batch_finished 完成。
            app.set_summary_visible(false);
            app.set_batch_busy(true);
            if let Ok(mut s) = slot_dupe.lock() {
                s.0 = ContentOp::Paste {
                    clips: clips.clone(),
                    bundle_root,
                    visit_dir,
                    depth,
                    id_version,
                    into_folder: None,
                };
                s.1.clear();
            }
            let items: Vec<(String, String)> = clips
                .iter()
                .enumerate()
                .map(|(i, c)| (i.to_string(), c.name.clone()))
                .collect();
            spawn_gui_batch(
                app.as_weak(),
                std::sync::Arc::clone(&slot_dupe),
                &format!("批量制作副本 · {} 项", clips.len()),
                "批量创建副本中",
                items,
            );
        });
    }
    {
        // 重命名：对话框确认后 rename 文件/目录 + 更新 entries 登记。
        let pending: Rc<RefCell<Option<(PathBuf, String)>>> = Rc::new(RefCell::new(None));
        {
            let editor = editor.clone();
            let pending = pending.clone();
            let app_weak = app.as_weak();
            app.global::<EntryApi>().on_entry_rename_open(move || {
                let app = app_weak.upgrade().unwrap();
                let e = editor.borrow();
                let Some(visit) = action_visit(&app, &e) else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                // 多选（≥2）→ 一律批量重命名（无论有无主选中）：菜单与右键的
                // 「重命名条目…」在整组选区上等价于 Finder 的批量重新命名。
                if collect_entry_multi(&e).len() >= 2 {
                    app.set_br_mode(0);
                    app.set_br_find("".into());
                    app.set_br_replace("".into());
                    app.set_br_prefix("".into());
                    app.set_br_suffix("".into());
                    app.set_br_visible(true);
                    return;
                }
                let Some(path) = menu_targets(&app).into_iter().next() else {
                    show_status(&app, "请先选择一个条目。".into());
                    return;
                };
                *pending.borrow_mut() = Some((visit.dir.clone(), path.clone()));
                // 预填**文件名**（不含目录）：path 可能是子目录内的相对路径
                // （内容文件夹的子行），带目录会让用户连带改掉路径。
                app.set_rename_name(basename(&path).into());
                app.set_rename_visible(true);
                // 镜像显隐：面板据此在弹窗关闭时夺回焦点（空格预览等键依赖）。
                app.global::<EntryApi>().set_rename_overlay(true);
            });
        }
        {
            let editor = editor.clone();
            let pending = pending.clone();
            let app_weak = app.as_weak();
            app.on_rename_confirm(move || {
                let app = app_weak.upgrade().unwrap();
                if batch_busy_guard(&app) {
                    return;
                }
                app.set_rename_visible(false);
                app.global::<EntryApi>().set_rename_overlay(false);
                let Some((dir, old_path)) = pending.borrow_mut().take() else {
                    return;
                };
                let new_name = app.get_rename_name().trim().to_string();
                let result = with_editor(&editor, |e| {
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                    // old_path 可能是**子目录内**的相对路径（内容文件夹的子行）：
                    // 重命名只改最后一段文件名、发生在**文件所在目录**内 —— 此前拿
                    // 分支根当目录且预填全路径，会把文件移出其所在文件夹。
                    let parent_rel = std::path::Path::new(&old_path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let base_dir = if parent_rel.is_empty() {
                        dir.clone()
                    } else {
                        dir.join(&parent_rel)
                    };
                    let new_full = if parent_rel.is_empty() {
                        new_name.clone()
                    } else {
                        format!("{parent_rel}/{new_name}")
                    };
                    if new_name.is_empty()
                        || new_name.contains('/')
                        || new_name.starts_with('.')
                        || new_name == ".str.toml"
                    {
                        return Err(format!("无效名称：{new_name}"));
                    }
                    if new_full == old_path {
                        return Ok("未重命名：名称未变化。".to_string());
                    }
                    let dst = base_dir.join(&new_name);
                    if dst.exists() {
                        return Err(format!("名称已存在：{new_full}"));
                    }
                    let mut meta = read_meta(bundle, &dir)?;
                    match meta
                        .entries
                        .iter()
                        .find(|x| x.path == old_path)
                        .cloned()
                    {
                        // 已登记：换名并保留其余登记字段（role/size/sha256…）。
                        Some(mut ne) => {
                            std::fs::rename(dir.join(&old_path), &dst)
                                .map_err(|err| err.to_string())?;
                            meta.remove_entry_path(&old_path);
                            ne.path = new_full.clone();
                            meta.upsert_entry(&ne);
                            meta.touch();
                            meta.save(&bundle.meta_path(&dir))
                                .map_err(|err| err.to_string())?;
                        }
                        // 未登记（内容文件夹子行）：只改文件系统，无登记可更新。
                        None => {
                            std::fs::rename(dir.join(&old_path), &dst)
                                .map_err(|err| err.to_string())?;
                        }
                    }
                    if e.selected_entry_path.as_deref() == Some(old_path.as_str()) {
                        e.selected_entry_path = Some(new_full.clone());
                    }
                    e.rescan()?;
                    Ok(format!("已重命名「{old_path}」→「{new_full}」。"))
                });
                match result {
                    Ok(msg) => {
                        if !msg.is_empty() {
                            sync_ui(&app, &editor.borrow());
                            show_status(&app, msg.into());
                        }
                    }
                    Err(msg) => show_status(&app, format!("重命名失败：{msg}").into()),
                }
            });
        }
        {
            let app_weak = app.as_weak();
            app.on_rename_cancel(move || {
                let app = app_weak.upgrade().unwrap();
                app.set_rename_visible(false);
                app.global::<EntryApi>().set_rename_overlay(false);
            });
        }
    }
    {
        // 新建文件夹：创建目录 + 登记 role = dir。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_new_folder(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            if hard_view_guard(&app, &editor.borrow()) {
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let branch_title = visit_title(visit);
                let mut meta = read_meta(bundle, &visit.dir)?;
                let existing: HashSet<String> =
                    meta.entries.iter().map(|x| x.path.clone()).collect();
                let name = dedup_name(&existing, "未命名文件夹");
                std::fs::create_dir_all(visit.dir.join(&name)).map_err(|err| err.to_string())?;
                meta.upsert_entry(&Entry {
                    path: name.clone(),
                    role: "dir".to_string(),
                    count: Some(0),
                    ..Default::default()
                });
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.selected_entry_path = Some(name.clone());
                e.rescan()?;
                Ok(format!("已在「{branch_title}」下新建文件夹「{name}」。"))
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("新建文件夹失败：{msg}").into()),
            }
        });
    }
    {
        // 新建文件夹（内容文件夹行右键）：在目标文件夹**内部**创建目录。
        // 内容文件夹子项不登记 —— 仅 mkdir + 维护该 dir 条目的 count。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_new_folder_into(move |dir_rel| {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            if hard_view_guard(&app, &editor.borrow()) {
                return;
            }
            let dir_rel = dir_rel.to_string();
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit_dir = e.scan.as_ref().unwrap().visits[visit_idx].dir.clone();
                let folder_fs = visit_dir.join(&dir_rel);
                if !folder_fs.is_dir() {
                    return Err(format!("目标文件夹不存在：{dir_rel}"));
                }
                let existing: HashSet<String> = std::fs::read_dir(&folder_fs)
                    .map(|rd| {
                        rd.filter_map(|x| x.ok())
                            .map(|x| x.file_name().to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let name = dedup_name(&existing, "未命名文件夹");
                std::fs::create_dir_all(folder_fs.join(&name)).map_err(|err| err.to_string())?;
                // 维护该文件夹条目的 count（分支 meta 里的 dir 条目）。
                let mut meta = read_meta(bundle, &visit_dir)?;
                if let Some(mut den) = meta.entries.iter().find(|x| x.path == dir_rel).cloned() {
                    den.count = Some(
                        std::fs::read_dir(&folder_fs)
                            .map(|rd| rd.count() as i64)
                            .unwrap_or(0),
                    );
                    meta.upsert_entry(&den);
                    meta.touch();
                    meta.save(&bundle.meta_path(&visit_dir))
                        .map_err(|err| err.to_string())?;
                }
                e.selected_entry_path = Some(format!("{dir_rel}/{name}"));
                // 展开目标文件夹沿途父级（必须在 rescan 前设置）。
                expand_dir_chain(e, &visit_dir, &dir_rel);
                e.rescan()?;
                Ok(format!("已在「{dir_rel}/」新建文件夹「{name}」。"))
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("新建文件夹失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let clipboard = clipboard.clone();
        let app_weak = app.as_weak();
        let slot_paste = std::sync::Arc::clone(&batch_results);
        // macOS 粘贴主流程（嵌套函数：需调用 main 内的 with_editor / sync_ui）。
        /// macOS 粘贴主流程：**始终**读系统剪贴板（唯一事实来源——应用内拷贝/剪切
        /// 与 Finder 拷贝都经过它；若读内部剪贴板，Finder 里的新拷贝会被应用内的
        /// 旧内容遮蔽）。bundle 内的路径携带登记信息走应用内粘贴；bundle 外的
        /// （Finder 文件）走导入。返回 true = 已处理（含「剪贴板为空」提示）。
        #[cfg(target_os = "macos")]
        fn paste_from_system(
            app: &AppWindow,
            editor: &Rc<RefCell<Editor>>,
            slot: &std::sync::Arc<std::sync::Mutex<(ContentOp, Vec<BatchOutcome>)>>,
        ) -> bool {
            let sys_paths = mac_read_clipboard_files().unwrap_or_default();
            if sys_paths.is_empty() {
                show_status(&app, "剪贴板为空，请先拷贝或剪切条目。".into());
                return true;
            }
            let is_cut = mac_read_clipboard_cut();
            // 反推条目：bundle 内的路径携带登记信息；bundle 外（Finder 文件）走导入。
            let (in_bundle, external): (Vec<ClipItem>, Vec<PathBuf>) = {
                let e = editor.borrow();
                match (e.bundle.as_ref(), e.scan.as_ref()) {
                    (Some(_bundle), Some(scan)) => {
                        let mut inb = Vec::new();
                        let mut ext = Vec::new();
                        for p in &sys_paths {
                            match derive_clip(scan, p, is_cut) {
                                Some(c) => inb.push(c),
                                None => ext.push(p.clone()),
                            }
                        }
                        (inb, ext)
                    }
                    _ => (Vec::new(), sys_paths.clone()),
                }
            };
            if !external.is_empty() {
                // 外部文件（Finder 拷贝）→ 导入当前分支（一次导入完成，无进度）。
                let result = with_editor(editor, |e| {
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                    let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                    let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                    let names = import_paths_into(bundle, &visit.dir, &visit.dir, true, &external)?;
                    e.rescan()?;
                    // 目标分支标题：rescan 之后重新借用 scan（上面 visit 的借用已结束）。
                    let dst_title = e
                        .scan
                        .as_ref()
                        .and_then(|s| s.visits.get(visit_idx))
                        .map(visit_title)
                        .unwrap_or_default();
                    Ok(format!(
                        "已从系统剪贴板粘贴 {} 项 →「{dst_title}」：{}。",
                        names.len(),
                        name_list(&names)
                    ))
                });
                match result {
                    Ok(msg) => {
                        sync_ui(app, &editor.borrow());
                        show_status(&app, msg.into());
                    }
                    Err(msg) => show_status(&app, format!("粘贴失败：{msg}").into()),
                }
                return true;
            }
            // 目标 = 当前选中分支。
            let (visit_dir, depth, id_version) = {
                let e = editor.borrow();
                let Some(visit_idx) = e.selected_idx_in_visits() else {
                    show_status(&app, "请先选择一个分支。".into());
                    return true;
                };
                let scan = e.scan.as_ref().unwrap();
                let v = &scan.visits[visit_idx];
                let idv = scan
                    .visits
                    .first()
                    .and_then(|v| v.meta.as_ref())
                    .map(|m| m.policies.id_version)
                    .unwrap_or(7);
                (v.dir.clone(), v.depth, idv)
            };
            let bundle_root = {
                let e = editor.borrow();
                match e.bundle.as_ref() {
                    Some(b) => b.root.clone(),
                    None => {
                        show_status(&app, "请先打开一个 bundle。".into());
                        return true;
                    }
                }
            };
            if in_bundle.len() == 1 {
                // 单条：同步执行（与单条删除免进度一致）。apply_clip_at 不含重扫，
                // 这里必须补 —— 否则磁盘已变而界面不刷新，表现为「粘贴无效」。
                let clip = &in_bundle[0];
                let result = with_editor(editor, |e| {
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                    let r = apply_clip_at(bundle, &visit_dir, depth, id_version, clip);
                    if r.is_ok() {
                        e.rescan()?;
                    }
                    r
                });
                match result {
                    Ok((msg, pasted)) => {
                        if is_cut {
                            let _ = mac_clear_clipboard();
                        }
                        {
                            let mut e = editor.borrow_mut();
                            e.entry_multi = [pasted.clone()].into_iter().collect();
                            e.entry_anchor = None;
                            e.selected_entry_path = Some(pasted);
                        }
                        sync_ui(app, &editor.borrow());
                        show_status(&app, msg.into());
                    }
                    Err(msg) => show_status(&app, format!("粘贴失败：{msg}").into()),
                }
                return true;
            }
            // 多条：批量管线（进度 + 汇总 + 重试）；激活在 batch_finished 完成。
            app.set_summary_visible(false);
            app.set_batch_busy(true);
            if let Ok(mut s) = slot.lock() {
                s.0 = ContentOp::Paste {
                    clips: in_bundle.clone(),
                    bundle_root: bundle_root.clone(),
                    visit_dir,
                    depth,
                    id_version,
                    into_folder: None,
                };
                s.1.clear();
            }
            let items: Vec<(String, String)> = in_bundle
                .iter()
                .enumerate()
                .map(|(i, c)| (i.to_string(), c.name.clone()))
                .collect();
            spawn_gui_batch(
                app.as_weak(),
                std::sync::Arc::clone(slot),
                &format!("批量粘贴 · {} 项", in_bundle.len()),
                "批量粘贴中",
                items,
            );
            true
        }

        // 文件夹行右键「粘贴」：把剪贴板条目放进该内容文件夹（Finder 语义）。
        // 剪贴板读取与 on_paste 同源（macOS = 系统剪贴板唯一事实来源）；落地走
        // paste_into_folder（不登记、只维护 count），外部文件 = 导入该文件夹。
        // 注册在 on_paste **之前**：后者会把 editor / app_weak / slot_paste move 进闭包。
        let editor_pi = editor.clone();
        let app_weak_pi = app_weak.clone();
        let slot_pi = std::sync::Arc::clone(&batch_results);
        #[cfg(not(target_os = "macos"))]
        let clipboard_pi = clipboard.clone();
        app.global::<EntryApi>().on_paste_into(move |dir_rel| {
            let app = app_weak_pi.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            if hard_view_guard(&app, &editor_pi.borrow()) {
                return;
            }
            let dir_rel = dir_rel.to_string();
            let mut clips: Vec<ClipItem> = Vec::new();
            let mut external: Vec<PathBuf> = Vec::new();
            let is_cut;
            // 剪贴板来源（状态栏「已从<来源>粘贴…」用）。
            let source;
            #[cfg(target_os = "macos")]
            {
                let sys_paths = mac_read_clipboard_files().unwrap_or_default();
                if sys_paths.is_empty() {
                    show_status(&app, "剪贴板为空，请先拷贝或剪切条目。".into());
                    return;
                }
                is_cut = mac_read_clipboard_cut();
                source = "系统剪贴板";
                let e = editor_pi.borrow();
                if let (Some(_b), Some(scan)) = (e.bundle.as_ref(), e.scan.as_ref()) {
                    for p in &sys_paths {
                        match derive_clip(scan, p, is_cut) {
                            Some(c) => clips.push(c),
                            None => external.push(p.clone()),
                        }
                    }
                } else {
                    external = sys_paths.clone();
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                clips = clipboard_pi.borrow().clone();
                is_cut = clips.first().map(|c| c.is_cut).unwrap_or(false);
                source = "内部剪贴板";
                if clips.is_empty() {
                    show_status(&app, "剪贴板为空，请先拷贝或剪切条目。".into());
                    return;
                }
            }
            // 多条（bundle 内）→ 批量管线（进度 + 汇总 + 失败重试），与 on_paste 同流程；
            // into_folder 交给 ContentOp::Paste（子项不登记，count 由 batch_finished
            // 统一维护）。外部文件混入时保持同步路径（导入本就是一次性调用）。
            if clips.len() > 1 && external.is_empty() {
                let (visit_dir, depth, id_version) = {
                    let e = editor_pi.borrow();
                    let Some(visit_idx) = e.selected_idx_in_visits() else {
                        show_status(&app, "请先选择一个分支。".into());
                        return;
                    };
                    let scan = e.scan.as_ref().unwrap();
                    let idv = scan
                        .visits
                        .first()
                        .and_then(|v| v.meta.as_ref())
                        .map(|m| m.policies.id_version)
                        .unwrap_or(7);
                    let v = &scan.visits[visit_idx];
                    (v.dir.clone(), v.depth, idv)
                };
                app.set_summary_visible(false);
                app.set_batch_busy(true);
                if let Ok(mut s) = slot_pi.lock() {
                    s.0 = ContentOp::Paste {
                        clips: clips.clone(),
                        bundle_root: e_bundle_root(&editor_pi).unwrap_or_default(),
                        visit_dir,
                        depth,
                        id_version,
                        into_folder: Some(dir_rel.clone()),
                    };
                    s.1.clear();
                }
                let items: Vec<(String, String)> = clips
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i.to_string(), c.name.clone()))
                    .collect();
                spawn_gui_batch(
                    app.as_weak(),
                    std::sync::Arc::clone(&slot_pi),
                    &format!("批量粘贴 · {} 项", clips.len()),
                    "批量粘贴中",
                    items,
                );
                return;
            }
            let result = with_editor(&editor_pi, |e| {
                // 外部文件（Finder 拷贝）→ 导入到该文件夹（不登记顶层）。
                if !external.is_empty() {
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                    let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                    let visit_dir = e.scan.as_ref().unwrap().visits[visit_idx].dir.clone();
                    let folder_fs = visit_dir.join(&dir_rel);
                    let names =
                        import_paths_into(bundle, &visit_dir, &folder_fs, false, &external)?;
                    // 展开目标文件夹沿途父级（必须在 rescan 前设置）。
                    expand_dir_chain(e, &visit_dir, &dir_rel);
                    e.rescan()?;
                    return Ok(names
                        .iter()
                        .map(|n| in_dir_child_path(&visit_dir, &folder_fs, n))
                        .collect::<Vec<String>>());
                }
                paste_into_folder(e, &clips, &dir_rel)
            });
            match result {
                Ok(pasted) if !pasted.is_empty() => {
                    if is_cut {
                        #[cfg(target_os = "macos")]
                        let _ = mac_clear_clipboard();
                        #[cfg(not(target_os = "macos"))]
                        clipboard_pi.borrow_mut().clear();
                    }
                    {
                        let mut e = editor_pi.borrow_mut();
                        // 与粘贴 / 拖拽同语义：高亮落地的目标项（主选中 = 第一个）。
                        e.entry_multi = pasted.iter().cloned().collect();
                        e.entry_anchor = None;
                        e.selected_entry_path = pasted.first().cloned();
                    }
                    sync_ui(&app, &editor_pi.borrow());
                    let branch_title = {
                        let e = editor_pi.borrow();
                        e.selected_visit().map(visit_title).unwrap_or_default()
                    };
                    show_status(
                        &app,
                        format!(
                            "已从{source}粘贴 {} 项 →「{branch_title}」/「{dir_rel}」。",
                            pasted.len()
                        )
                        .into(),
                    );
                }
                Ok(_) => {}
                Err(msg) => show_status(&app, format!("粘贴失败：{msg}").into()),
            }
        });

        app.global::<EntryApi>().on_paste(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            if hard_view_guard(&app, &editor.borrow()) {
                return;
            }
            #[cfg(target_os = "macos")]
            {
                // macOS：**始终**走系统剪贴板（唯一事实来源——应用内拷贝/剪切与
                // Finder 拷贝都经过它；读内部剪贴板会让 Finder 里的新拷贝被
                // 应用内的旧内容遮蔽，导致跨来源粘贴全部错乱）。
                if paste_from_system(&app, &editor, &slot_paste) {
                    return;
                }
            }
            // 非 macOS：无系统文件剪贴板实现 → 内部剪贴板（单条同步 / 多条批量管线）。
            let clips = clipboard.borrow().clone();
            if clips.is_empty() {
                show_status(&app, "剪贴板为空，请先拷贝或剪切条目。".into());
                return;
            }
            if clips.len() == 1 {
                // 单条粘贴：保持同步执行（与单条删除免确认/免进度一致）。
                let result = with_editor(&editor, |e| apply_clip(e, &clips[0]));
                match result {
                    Ok((msg, pasted)) => {
                        if clips[0].is_cut {
                            clipboard.borrow_mut().clear();
                        }
                        {
                            let mut e = editor.borrow_mut();
                            e.entry_multi = [pasted.clone()].into_iter().collect();
                            e.entry_anchor = None;
                            e.selected_entry_path = Some(pasted);
                        }
                        sync_ui(&app, &editor.borrow());
                        show_status(&app, msg.into());
                    }
                    Err(msg) => show_status(&app, format!("粘贴失败：{msg}").into()),
                }
                return;
            }
            // 多条目粘贴：与批量删除同一管线——进度对话框 + 汇总 + 失败项重试。
            // 粘贴后激活被粘贴条目：在 batch_finished（主线程）按落地路径恢复选区。
            let (visit_dir, depth, id_version) = {
                let e = editor.borrow();
                let Some(visit_idx) = e.selected_idx_in_visits() else {
                    show_status(&app, "请先选择一个分支。".into());
                    return;
                };
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                (
                    visit.dir.clone(),
                    visit.depth,
                    e.scan
                        .as_ref()
                        .unwrap()
                        .visits
                        .first()
                        .and_then(|v| v.meta.as_ref())
                        .map(|m| m.policies.id_version)
                        .unwrap_or(7),
                )
            };
            app.set_summary_visible(false);
            app.set_batch_busy(true);
            if let Ok(mut s) = slot_paste.lock() {
                s.0 = ContentOp::Paste {
                    clips: clips.clone(),
                    bundle_root: e_bundle_root(&editor).unwrap_or_default(),
                    visit_dir,
                    depth,
                    id_version,
                    into_folder: None,
                };
                s.1.clear();
            }
            let items: Vec<(String, String)> = clips
                .iter()
                .enumerate()
                .map(|(i, c)| (i.to_string(), c.name.clone()))
                .collect();
            spawn_gui_batch(
                app.as_weak(),
                std::sync::Arc::clone(&slot_paste),
                &format!("批量粘贴 · {} 项", clips.len()),
                "批量粘贴中",
                items,
            );
        });
    }
    {
        // 拖到列表某行 = 在本分支内重排（写入显式 order）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>()
            .on_drop_row(move |transfer, files, row_index, into_dir, after| {
                let app = app_weak.upgrade().unwrap();
                if batch_busy_guard(&app) {
                    return;
                }
                // **以拖拽开始时的整组快照为准**（app.slint 在 `changed dragging` 里同步
                // 调 payload-for 写入 `DndApi.payload`）。不能反过来优先 data-transfer
                // 文本：行的 `data` 绑定只在重渲染时求值，抓「选中组里的一行」时它往往是
                // **选中之前**的陈旧值（实测只带单条 ⇒ 批量移动只动一个）。仅当载荷缺失
                // （外部拖入 / 跨应用）时才用事件里的文本。
                let payload = app.global::<DndApi>().get_payload();
                let from_payload = parse_entry_transfer_multi(&payload).is_some();
                let transfer = if from_payload {
                    payload
                } else {
                    transfer
                };
                // 打印**解析后**的落点载荷与来源：此前打印的是事件原文（恰是那份陈旧
                // 单条值），会让人误判成「修复没生效」。
                if debug_on() {
                    let d = app.global::<DndApi>();
                    eprintln!(
                        "[str-gui] drop-row row={row_index} into_dir={into_dir} after={after} \
                     files={files:?} transfer={transfer:?} src={} hover-row={} half={} panel={} \
                     hover-files={} hover-into-dir={} sortable={} src-mixed={}",
                        if from_payload { "payload" } else { "event" },
                        d.get_hover_row(),
                        d.get_hover_half(),
                        d.get_hover_panel(),
                        d.get_hover_files(),
                        d.get_hover_into_dir(),
                        d.get_hover_sortable(),
                        d.get_src_mixed()
                    );
                }
                let result = with_editor(&editor, |e| {
                    let vis = build_entry_rows(e);
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
                    let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                    let visit_dir = e.scan.as_ref().ok_or("未选择分支")?.visits[visit_idx]
                        .dir
                        .clone();
                    // row_index = 悬停行（-1 = 列表末尾）；into_dir = 手势指向文件夹内部；
                    // after = 插到该行之后（否则之前）。不再用 index+1 表达"插到行后"，
                    // 避免"下一行是文件夹"被误判成移入文件夹。
                    let at_end = row_index < 0;
                    let to_index = if at_end {
                        vis.len()
                    } else {
                        (row_index as usize).min(vis.len().saturating_sub(1))
                    };
                    // 目标行解析：文件夹行 → 其内部（不逐文件登记）；文件行 → 分支目录（登记）。
                    // target_dir 仅用于外部文件导入；内部拖拽改用 internal_target_dir。
                    let (target_dir, register, _target_is_dir) = match vis.get(to_index) {
                        Some(row) => (
                            row.drop_dir.clone(),
                            row.entries_idx.is_some() && !row.is_dir,
                            into_dir && row.is_dir,
                        ),
                        None => (visit_dir.clone(), true, false),
                    };
                    // 外部文件拖到行上 → 导入对应目录（文件夹行 = 拖进文件夹）。
                    if !files.is_empty() {
                        let paths: Vec<PathBuf> = files.lines().map(PathBuf::from).collect();
                        let names =
                            import_paths_into(&bundle, &visit_dir, &target_dir, register, &paths)?;
                        // 落到根路径的根层级插入位：按该位置排序插入（与应用内重排表现一致）。
                        if register
                            && !at_end
                            && vis.get(to_index).map(|v| v.depth).unwrap_or(0) == 0
                        {
                            let top_positions: Vec<usize> = vis
                                .iter()
                                .enumerate()
                                .filter(|(_, v)| v.entries_idx.is_some())
                                .map(|(i, _)| i)
                                .collect();
                            let to_row = (to_index + usize::from(after)).min(vis.len());
                            let insert_at = if to_row >= vis.len() {
                                top_positions.len()
                            } else {
                                top_positions.iter().take_while(|&&i| i < to_row).count()
                            };
                            let mut meta = read_meta(&bundle, &visit_dir)?;
                            let mut seq: Vec<Entry> = meta
                                .entries
                                .iter()
                                .filter(|en| !en.is_branch())
                                .cloned()
                                .collect();
                            let mut news: Vec<Entry> = Vec::new();
                            for n in &names {
                                if let Some(p) = seq.iter().position(|en| en.path == *n) {
                                    news.push(seq.remove(p));
                                }
                            }
                            if !news.is_empty() {
                                // 逆序插在同一位置，最终顺序与 names 一致。
                                let idx = insert_at.min(seq.len());
                                while let Some(en) = news.pop() {
                                    seq.insert(idx, en);
                                }
                                let snapshot = meta.entries.clone();
                                for en in snapshot.iter().filter(|en| !en.is_branch()) {
                                    meta.remove_entry_path(&en.path);
                                }
                                for (i, mut en) in seq.into_iter().enumerate() {
                                    en.order = Some(i as i64);
                                    meta.upsert_entry(&en);
                                }
                                meta.touch();
                                meta.save(&bundle.meta_path(&visit_dir))
                                    .map_err(|err| err.to_string())?;
                            }
                        }
                        e.rescan()?;
                        // 目标位置：落到分支根 = 该**分支标题**；落到内容文件夹 = 该文件夹相对路径。
                        let where_ = if register {
                            format!("「{}」", title_of_dir(e, &visit_dir))
                        } else {
                            format!(
                                "「{}/」",
                                target_dir
                                    .strip_prefix(&visit_dir)
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_default()
                            )
                        };
                        // 导入到文件夹时自动展开，保证新条目可见。
                        if !register {
                            e.expanded_dirs
                                .insert(target_dir.to_string_lossy().to_string());
                        }
                        // 高亮新导入的第一项（子路径 = "所在目录/文件名"）。
                        if let Some(first) = names.first() {
                            e.selected_entry_path =
                                Some(in_dir_child_path(&visit_dir, &target_dir, first));
                        }
                        return Ok(format!(
                            "已从外部拖入导入 {} 项 → {where_}：{}。",
                            names.len(),
                            name_list(&names)
                        ));
                    }
                    // 多选拖拽（Finder 语义：拖一个选中项 = 拖全部）。行级落点的
                    // 整组行为：跨分支 = 整组移到目标分支（按悬停位插入，与单条
                    // move_entry_across 同规则）；同分支拖入文件夹 =
                    // 整组移入；同分支根层级行间 = 整组重排（保持组内相对顺序）。
                    // 单条载荷（拖非选中行）仍走下方既有单条逻辑。
                    if let Some((g_visit, g_paths)) =
                        parse_entry_transfer_multi(&transfer).filter(|(_, ps)| ps.len() > 1)
                    {
                        if g_visit != visit_idx {
                            return move_group_to_branch(
                                e,
                                &bundle,
                                g_visit,
                                &g_paths,
                                visit_idx,
                                &vis,
                                at_end,
                                to_index,
                                after,
                                into_dir,
                            );
                        }
                        // 组里含**未登记**成员（内容文件夹的子行 —— 它们没有 `order`
                        // 顺序语义）时「重排」不可表达：旧行为会直接报
                        // 「组内含未登记条目，无法重排」，整组什么都不发生。按 Finder
                        // 语义改为移入**目标行所在目录**（顶层文件行 ⇒ 分支根）。
                        let registered: HashSet<&str> = e
                            .selected_visit()
                            .and_then(|v| v.meta.as_ref())
                            .map(|m| m.entries.iter().map(|en| en.path.as_str()).collect())
                            .unwrap_or_default();
                        let has_unregistered = g_paths.iter().any(|p| !registered.contains(p.as_str()));
                        let into_folder = !at_end
                            && (into_dir || vis[to_index].depth > 0 || has_unregistered);
                        if into_folder {
                            // 整组移入目标文件夹（仅 payload/asset；dir 逐项报错跳过）。
                            let target = if vis[to_index].is_dir {
                                vis[to_index].drop_dir.clone()
                            } else {
                                vis[to_index]
                                    .fs_path
                                    .parent()
                                    .unwrap_or(&visit_dir)
                                    .to_path_buf()
                            };
                            let mut meta = read_meta(&bundle, &visit_dir)?;
                            let mut moved: Vec<String> = Vec::new();
                            let mut errs: Vec<String> = Vec::new();
                            for p in &g_paths {
                                let file_like = meta
                                    .entries
                                    .iter()
                                    .find(|x| x.path == *p)
                                    .map(|en| en.is_file_like());
                                // **文件与文件夹都可移入文件夹**：登记过的 dir 与磁盘上真实
                                // 存在的未登记项一律放行 —— 用户报告「文件夹无法移到文件夹中」。
                                // （旧版对 dir 直接报「仅文件可移入文件夹」。）
                                let src_path = visit_dir.join(p);
                                match file_like {
                                    Some(_) => {}
                                    None if src_path.exists() => {}
                                    None => {
                                        errs.push(format!("{p}：未登记条目"));
                                        continue;
                                    }
                                }
                                // 防自吞：文件夹不得移入自身或其子孙内部（会把整棵子树搬进
                                // 自己，等于毁数据）。
                                if src_path.is_dir() && target.starts_with(&src_path) {
                                    errs.push(format!("{p}：不能把文件夹移入其自身内部"));
                                    continue;
                                }
                                let name = basename(p);
                                let dst = target.join(&name);
                                if dst.exists() {
                                    errs.push(format!("{p}：目标已存在同名项"));
                                    continue;
                                }
                                // 已在目标目录内：等价于「顺序微调」，不移动文件 ——
                                // 否则等于把文件移到自己身上（失败/空提示）。子目录行没有
                                // `order` 语义，这里直接跳过即可。
                                if visit_dir.join(p).parent() == Some(target.as_path()) {
                                    continue;
                                }
                                move_file(&visit_dir.join(p), &dst)?;
                                meta.remove_entry_path(p);
                                // 移回**分支根** = 顶层清单来自 meta.entries，必须登记 ——
                                // 与「拖到分支 / 节点」的既有路径同规则（role=payload +
                                // size + sha256，见同文件上方那段）。少了这步文件在磁盘上、
                                // 列表里却查不到（用户报告：「已移动但未登记到 .str.toml
                                // 导致无法显示」）。落到已登记的**内容文件夹**内不单独登记，
                                // 只由下方的 count 维护负责。
                                if target == visit_dir {
                                    // 顺序由**循环之后**的「按拖放位重排」统一写入
                                    // （见下方：重建有序 seq → 插到插入位 → 全体重写 order）。
                                    // 规则与「拖到分支」那条路径一致：目录登记为 dir（带
                                    // count），文件登记为 payload（带 size + sha256）。
                                    if target.join(&name).is_dir() {
                                        let count = std::fs::read_dir(target.join(&name))
                                            .map(|rd| rd.count())
                                            .unwrap_or(0);
                                        meta.upsert_entry(&Entry {
                                            path: name.clone(),
                                            role: "dir".to_string(),
                                            count: Some(count as i64),
                                            ..Default::default()
                                        });
                                    } else {
                                        let bytes = std::fs::read(target.join(&name))
                                            .map_err(|err| err.to_string())?;
                                        meta.upsert_entry(&Entry {
                                            path: name.clone(),
                                            role: "payload".to_string(),
                                            size: Some(bytes.len() as i64),
                                            sha256: Some(util::sha256_bytes(&bytes)),
                                            ..Default::default()
                                        });
                                    }
                                }
                                moved.push(name);
                            }
                            if moved.is_empty() {
                                // 全组已在目标目录内（上面的 skip 命中）：不是错误，
                                // 给一句明确反馈，避免弹出空字符串状态。
                                if errs.is_empty() {
                                    return Ok("已在目标目录内，位置未变。".into());
                                }
                                return Err(errs.join("；"));
                            }
                            // 同步目标文件夹条目的 count。
                            let dir_rel = target
                                .strip_prefix(&visit_dir)
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_default();
                            if let Some(den) =
                                meta.entries.iter().find(|x| x.path == dir_rel).cloned()
                            {
                                let count =
                                    std::fs::read_dir(&target).map(|rd| rd.count()).unwrap_or(0);
                                let mut ne = den;
                                ne.count = Some(count as i64);
                                meta.upsert_entry(&ne);
                            }
                            // 落点 = **分支根**：按拖放行的位置重排顶层序号，让移入的条目
                            // 落在松手处（与同分支重排同一语义、同一惯用法：重建有序 seq
                            // → 摘出组成员 → 插到插入位 → 全体重写 order 0..n）。
                            // 不这么做就只能追加到末尾（上一版行为），用户期望的是「拖哪里
                            // 落在哪里」。
                            if target == visit_dir {
                                let mut seq: Vec<Entry> = meta
                                    .entries
                                    .iter()
                                    .filter(|en| !en.is_branch())
                                    .cloned()
                                    .collect();
                                let insert_at_raw = if at_end {
                                    seq.len()
                                } else {
                                    let tp = &vis[to_index].path;
                                    seq.iter()
                                        .position(|en| en.path == *tp)
                                        .map(|i| i + usize::from(after))
                                        .unwrap_or(seq.len())
                                };
                                let mut group: Vec<Entry> = Vec::new();
                                for n in &moved {
                                    if let Some(p) = seq.iter().position(|en| en.path == *n) {
                                        group.push(seq.remove(p));
                                    }
                                }
                                let insert_at = insert_at_raw.min(seq.len());
                                for (k, en) in group.into_iter().enumerate() {
                                    seq.insert(insert_at + k, en);
                                }
                                let snapshot = meta.entries.clone();
                                for en in snapshot.iter().filter(|en| !en.is_branch()) {
                                    meta.remove_entry_path(&en.path);
                                }
                                for (i, mut en) in seq.into_iter().enumerate() {
                                    en.order = Some(i as i64);
                                    meta.upsert_entry(&en);
                                }
                            }
                            meta.touch();
                            meta.save(&bundle.meta_path(&visit_dir))
                                .map_err(|err| err.to_string())?;
                            if target != visit_dir {
                                e.expanded_dirs.insert(target.to_string_lossy().to_string());
                            }
                            e.rescan()?;
                            // 与粘贴同语义：高亮落地的目标项（选区 = 移动后的新路径，
                            // 主选中 = 第一个）。必须在 rescan 之后设置。
                            let moved_paths: Vec<String> = moved
                                .iter()
                                .map(|n| in_dir_child_path(&visit_dir, &target, n))
                                .collect();
                            if !moved_paths.is_empty() {
                                e.entry_multi = moved_paths.iter().cloned().collect();
                                e.entry_anchor = None;
                                e.selected_entry_path = moved_paths.first().cloned();
                            }
                            let err_note = if errs.is_empty() {
                                String::new()
                            } else {
                                format!("（{} 项失败：{}）", errs.len(), errs.join("；"))
                            };
                            return Ok(format!(
                                "已在「{}」内移动 {} 项 →「{dir_rel}/」{err_note}",
                                title_of_dir(e, &visit_dir),
                                moved.len()
                            ));
                        }
                        // 整组重排：根层级行之间；组成员按原相对顺序插到落点位
                        // （先扣除落在插入位之前的组成员数，再移除-回插）。
                        let mut meta = read_meta(&bundle, &visit_dir)?;
                        let mut seq: Vec<Entry> = meta
                            .entries
                            .iter()
                            .filter(|en| !en.is_branch())
                            .cloned()
                            .collect();
                        let group_set: HashSet<&String> = g_paths.iter().collect();
                        let group_pos: Vec<usize> = (0..seq.len())
                            .filter(|i| group_set.contains(&seq[*i].path))
                            .collect();
                        if group_pos.len() != g_paths.len() {
                            return Err("组内含未登记条目，无法重排".into());
                        }
                        let insert_at_raw = if at_end {
                            seq.len()
                        } else {
                            let target_path = &vis[to_index].path;
                            seq.iter()
                                .position(|en| en.path == *target_path)
                                .map(|i| i + usize::from(after))
                                .unwrap_or(seq.len())
                        };
                        let members_before =
                            group_pos.iter().filter(|&&i| i < insert_at_raw).count();
                        let insert_at =
                            (insert_at_raw - members_before).min(seq.len() - g_paths.len());
                        // 落点 == 组原位置 → 顺序未变。
                        if group_pos.first() == Some(&insert_at)
                            && group_pos
                                .iter()
                                .enumerate()
                                .all(|(k, &i)| i == insert_at + k)
                        {
                            return Ok("顺序未变化。".into());
                        }
                        let members: Vec<Entry> = g_paths
                            .iter()
                            .filter_map(|p| {
                                seq.iter()
                                    .position(|en| en.path == *p)
                                    .map(|i| seq.remove(i))
                            })
                            .collect();
                        for (offset, item) in members.into_iter().enumerate() {
                            seq.insert(insert_at + offset, item);
                        }
                        let snapshot = meta.entries.clone();
                        for en in snapshot.iter().filter(|en| !en.is_branch()) {
                            meta.remove_entry_path(&en.path);
                        }
                        for (i, mut en) in seq.into_iter().enumerate() {
                            en.order = Some(i as i64);
                            meta.upsert_entry(&en);
                        }
                        meta.touch();
                        meta.save(&bundle.meta_path(&visit_dir))
                            .map_err(|err| err.to_string())?;
                        e.rescan()?;
                        return Ok(format!(
                            "已在「{}」内调整 {} 项的顺序。",
                            title_of_dir(e, &visit_dir),
                            g_paths.len()
                        ));
                    }
                    let (src_visit, path) = parse_entry_transfer(&transfer)
                        .ok_or_else(|| format!("拖拽载荷无效：{transfer}"))?;
                    if src_visit != visit_idx {
                        // 跨节点拖放 = 移动到目标节点，并按悬停位置插入目标序列。
                        return move_entry_across(
                            e, src_visit, &path, visit_idx, &vis, at_end, to_index, after,
                            into_dir,
                        );
                    }
                    let src_row = vis
                        .iter()
                        .position(|v| v.path == path)
                        .ok_or("未找到源条目")?;
                    // 落点解析（内部拖拽）：**排序只发生在根层级行之间**；
                    // 落在非根层级行 = 移入该行所在的文件夹（文件夹行 = 移入其内部）。
                    let into_folder = !at_end && (into_dir || vis[to_index].depth > 0);
                    let internal_target_dir = if at_end {
                        visit_dir.clone()
                    } else if into_folder {
                        if vis[to_index].is_dir {
                            vis[to_index].drop_dir.clone()
                        } else {
                            vis[to_index]
                                .fs_path
                                .parent()
                                .unwrap_or(&visit_dir)
                                .to_path_buf()
                        }
                    } else {
                        visit_dir.clone()
                    };
                    // 文件夹子行拖拽：无 entries 顺序概念，一律按文件系统移动。
                    // 拖到文件夹行 = 移入其内部；拖到文件行 = 移到该文件所在文件夹；末尾 = 移回分支根。
                    if vis[src_row].entries_idx.is_none() {
                        let src = &vis[src_row];
                        // into_dir = 移入文件夹内部；否则插到该行所在的目录（根列表）。
                        let dst_dir = if at_end {
                            visit_dir.clone()
                        } else if into_dir {
                            vis[to_index].drop_dir.clone()
                        } else {
                            vis[to_index]
                                .fs_path
                                .parent()
                                .unwrap_or(&visit_dir)
                                .to_path_buf()
                        };
                        if dst_dir.starts_with(&src.fs_path) {
                            return Err("不能把文件夹移入其自身内部".into());
                        }
                        let parents = src.fs_path.parent().unwrap_or(Path::new(""));
                        if dst_dir == parents {
                            return Ok("位置未变化。".to_string());
                        }
                        // 落点在根路径时计算插入顺序位（拖到根层级行列之间 = 可排序）。
                        let root_insert_pos = if dst_dir == visit_dir {
                            let top_positions: Vec<usize> = vis
                                .iter()
                                .enumerate()
                                .filter(|(_, v)| v.entries_idx.is_some())
                                .map(|(i, _)| i)
                                .collect();
                            let to_row = if at_end {
                                vis.len()
                            } else {
                                (to_index + usize::from(after)).min(vis.len())
                            };
                            Some(if to_row >= vis.len() {
                                top_positions.len()
                            } else {
                                top_positions.iter().take_while(|&&i| i < to_row).count()
                            })
                        } else {
                            None
                        };
                        let existing: HashSet<String> = std::fs::read_dir(&dst_dir)
                            .map(|rd| {
                                rd.filter_map(|x| x.ok())
                                    .map(|x| x.file_name().to_string_lossy().to_string())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        let name = dedup_name(&existing, &name);
                        move_file(&src.fs_path, &dst_dir.join(&name))?;
                        let mut meta = read_meta(&bundle, &visit_dir)?;
                        // 移回分支根 = 顶层清单来自 meta.entries，必须登记。
                        if dst_dir == visit_dir {
                            if src.is_dir {
                                let count = std::fs::read_dir(dst_dir.join(&name))
                                    .map(|rd| rd.count())
                                    .unwrap_or(0);
                                meta.upsert_entry(&Entry {
                                    path: name.clone(),
                                    role: "dir".to_string(),
                                    count: Some(count as i64),
                                    ..Default::default()
                                });
                            } else {
                                let bytes = std::fs::read(dst_dir.join(&name))
                                    .map_err(|err| err.to_string())?;
                                meta.upsert_entry(&Entry {
                                    path: name.clone(),
                                    role: "payload".to_string(),
                                    size: Some(bytes.len() as i64),
                                    sha256: Some(util::sha256_bytes(&bytes)),
                                    ..Default::default()
                                });
                            }
                        }
                        // 根路径落点：把新登记的条目插到指定顺序位。
                        if let Some(pos) = root_insert_pos {
                            let mut seq: Vec<Entry> = meta
                                .entries
                                .iter()
                                .filter(|en| !en.is_branch())
                                .cloned()
                                .collect();
                            if let Some(old) = seq.iter().position(|en| en.path == name) {
                                let item = seq.remove(old);
                                seq.insert(pos.min(seq.len()), item);
                                let snapshot = meta.entries.clone();
                                for en in snapshot.iter().filter(|en| !en.is_branch()) {
                                    meta.remove_entry_path(&en.path);
                                }
                                for (i, mut en) in seq.into_iter().enumerate() {
                                    en.order = Some(i as i64);
                                    meta.upsert_entry(&en);
                                }
                            }
                        }
                        // 源/目标若是 meta 文件夹条目（role=dir），同步 count。
                        for dir_fs in [
                            src.fs_path.parent().unwrap_or(Path::new("")).to_path_buf(),
                            dst_dir.clone(),
                        ] {
                            if dir_fs == visit_dir {
                                continue;
                            }
                            if let Ok(rel) = dir_fs.strip_prefix(&visit_dir) {
                                let rel = rel.to_string_lossy().to_string();
                                if let Some(den) = meta
                                    .entries
                                    .iter()
                                    .find(|x| x.path == rel && x.role == "dir")
                                    .cloned()
                                {
                                    let count = std::fs::read_dir(&dir_fs)
                                        .map(|rd| rd.count())
                                        .unwrap_or(0);
                                    let mut ne = den;
                                    ne.count = Some(count as i64);
                                    meta.upsert_entry(&ne);
                                }
                            }
                        }
                        meta.touch();
                        meta.save(&bundle.meta_path(&visit_dir))
                            .map_err(|err| err.to_string())?;
                        // 展开目标文件夹沿途父级（必须在 rescan 前设置）。
                        if dst_dir != visit_dir {
                            if let Ok(rel) = dst_dir.strip_prefix(&visit_dir) {
                                expand_dir_chain(e, &visit_dir, &rel.to_string_lossy());
                            }
                        }
                        e.rescan()?;
                        let rel = dst_dir
                            .strip_prefix(&visit_dir)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let where_ = if rel.is_empty() {
                            "分支根目录".to_string()
                        } else {
                            format!("{rel}/")
                        };
                        // 高亮移动后的新位置。
                        e.selected_entry_path =
                            Some(in_dir_child_path(&visit_dir, &dst_dir, &name));
                        return Ok(format!(
                            "已在「{}」内移动「{path}」→「{where_}」",
                            title_of_dir(e, &visit_dir)
                        ));
                    }
                    let mut meta = read_meta(&bundle, &visit_dir)?;
                    // 落到文件夹（含拖到子行 = 移入其所在文件夹）= 把条目移入该文件夹。
                    if into_folder {
                        // 源可能是**未登记**项（内容文件夹的子行，entries 里没有）或
                        // **目录** —— 都允许移入文件夹；只有**分支**（node/branch，只在
                        // 结构树中呈现）不允许。目录移入时加防自吞护栏（不得移入自身或
                        // 其子孙内部，否则整棵子树会被搬进自己）。
                        let src_fs = visit_dir.join(&path);
                        if !src_fs.exists() {
                            return Err(format!("源文件不存在：{path}"));
                        }
                        if let Some(entry) = meta.entries.iter().find(|x| x.path == path) {
                            if entry.is_branch() {
                                return Err("分支不能移入文件夹".into());
                            }
                        }
                        if src_fs.is_dir() && internal_target_dir.starts_with(&src_fs) {
                            return Err("不能把文件夹移入其自身内部".into());
                        }
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone());
                        let dst_dir = internal_target_dir.join(&name);
                        move_file(&visit_dir.join(&path), &dst_dir)?;
                        meta.remove_entry_path(&path);
                        let dir_rel = internal_target_dir
                            .strip_prefix(&visit_dir)
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if let Some(den) = meta.entries.iter().find(|x| x.path == dir_rel).cloned()
                        {
                            let count = std::fs::read_dir(&internal_target_dir)
                                .map(|rd| rd.count())
                                .unwrap_or(0);
                            let mut ne = den;
                            ne.count = Some(count as i64);
                            meta.upsert_entry(&ne);
                        }
                        meta.touch();
                        meta.save(&bundle.meta_path(&visit_dir))
                            .map_err(|err| err.to_string())?;
                        let msg = format!(
                            "已在「{}」内移动「{path}」→「{dir_rel}/」",
                            title_of_dir(e, &visit_dir)
                        );
                        // 自动展开目标文件夹，保证新条目可见并被高亮。
                        if internal_target_dir != visit_dir {
                            e.expanded_dirs
                                .insert(internal_target_dir.to_string_lossy().to_string());
                        }
                        e.rescan()?;
                        // 高亮移动后的新位置（文件夹子行路径 = "目录/文件名"）。
                        e.selected_entry_path =
                            Some(in_dir_child_path(&visit_dir, &internal_target_dir, &name));
                        return Ok(msg);
                    }
                    // 重排：仅在顶层内容条目之间进行；分支与文件夹子行顺序不动。
                    let top_positions: Vec<usize> = vis
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| v.entries_idx.is_some())
                        .map(|(i, _)| i)
                        .collect();
                    let from_row = src_row;
                    let from_pos = top_positions.iter().take_while(|&&i| i < from_row).count();
                    let to_row = (to_index + usize::from(after)).min(vis.len());
                    let to_pos = if to_row >= vis.len() {
                        top_positions.len()
                    } else {
                        top_positions.iter().take_while(|&&i| i < to_row).count()
                    };
                    if from_pos == to_pos || from_pos + 1 == to_pos {
                        return Ok("顺序未变化。".into());
                    }
                    let mut meta = read_meta(&bundle, &visit_dir)?;
                    let mut seq: Vec<Entry> = meta
                        .entries
                        .iter()
                        .filter(|en| !en.is_branch())
                        .cloned()
                        .collect();
                    let item = seq.remove(from_pos.min(seq.len()));
                    let insert_at = if from_pos < to_pos {
                        to_pos - 1
                    } else {
                        to_pos
                    };
                    seq.insert(insert_at.min(seq.len()), item);
                    let snapshot = meta.entries.clone();
                    for en in snapshot.iter().filter(|en| !en.is_branch()) {
                        meta.remove_entry_path(&en.path);
                    }
                    for (i, mut en) in seq.into_iter().enumerate() {
                        en.order = Some(i as i64);
                        meta.upsert_entry(&en);
                    }
                    meta.touch();
                    meta.save(&bundle.meta_path(&visit_dir))
                        .map_err(|err| err.to_string())?;
                    e.rescan()?;
                    // 高亮被重排的条目。
                    e.selected_entry_path = Some(path.clone());
                    Ok(format!(
                        "已在「{}」内调整条目顺序。",
                        title_of_dir(e, &visit_dir)
                    ))
                });
                match result {
                    Ok(msg) => {
                        sync_ui(&app, &editor.borrow());
                        if !msg.is_empty() {
                            show_status(&app, msg.into());
                        }
                    }
                    Err(msg) => show_status(&app, format!("重排失败：{msg}").into()),
                }
            });
    }
    {
        // 拖到左侧树某分支 = 移动 payload/asset 文件过去。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_entry_drop_to_branch(move |transfer, files, to_visit| {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            // data-transfer 文本不可靠时回退到拖拽开始时记录的全局载荷。
            let transfer = if parse_entry_transfer(&transfer).is_some() {
                transfer
            } else {
                app.global::<DndApi>().get_payload()
            };
            let result = with_editor(&editor, |e| {
                let Ok(to_visit) = usize::try_from(to_visit) else {
                    return Err("目标分支无效".into());
                };
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未打开 bundle")?;
                if to_visit >= scan.visits.len() {
                    return Err("目标分支无效".into());
                }
                // 外部文件拖到树分支 → 导入该分支。
                if !files.is_empty() {
                    let paths: Vec<PathBuf> = files.lines().map(PathBuf::from).collect();
                    let dst_dir = scan.visits[to_visit].dir.clone();
                    let names = import_paths_into(bundle, &dst_dir, &dst_dir, true, &paths)?;
                    e.rescan()?;
                    // 目的地分支名（Tier B：只读 rescan 后的 scan，不新增 IO）。
                    let dst_title = e
                        .scan
                        .as_ref()
                        .and_then(|s| s.visits.get(to_visit))
                        .map(visit_title)
                        .unwrap_or_default();
                    return Ok(format!(
                        "已从外部拖入导入 {} 项 →「{}」：{}。",
                        names.len(),
                        dst_title,
                        name_list(&names)
                    ));
                }
                // 多选拖拽（Finder 语义：拖一个选中项 = 拖全部）→ 逐项移动；
                // 单目标保持既有行为与文案不变。
                let (src_visit, paths): (usize, Vec<String>) =
                    match parse_entry_transfer_multi(&transfer) {
                        Some((v, paths)) if paths.len() > 1 => (v, paths),
                        _ => {
                            let (v, p) = parse_entry_transfer(&transfer)
                                .ok_or_else(|| format!("拖拽载荷无效：{transfer}"))?;
                            (v, vec![p])
                        }
                    };
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
                // 逐项移动 + 重扫 + 汇总文案（拖到树分支与行级跨分支落点共用）。
                // 树分支落点无行概念：传空行表 = 登记到目标顶层序列末尾。
                move_group_to_branch(
                    e, &bundle, src_visit, &paths, to_visit, &[], true, 0, false, false,
                )
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("移动失败：{msg}").into()),
            }
        });
    }

    // ── 分支拖拽重排（结构树同级分支之间）──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_drop_branch_row(move |src, target, nest| {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            if debug_on() {
                eprintln!("[str-gui] drop-branch-row src={src} target={target} nest={nest}");
            }
            let (Ok(src), Ok(target)) = (usize::try_from(src), usize::try_from(target)) else {
                show_status(&app, "移动分支失败：分支无效。".into());
                return;
            };
            let result = with_editor(&editor, |e| {
                if nest {
                    // 上半区 = 插入为子分支（可跨父级结构移动）。
                    return branch_move_into_child(e, src, target);
                }
                // ROOT 下半区：「after ROOT」无意义 → 成为 ROOT 的第一个子分支。
                if e.scan
                    .as_ref()
                    .ok_or("未打开 bundle")?
                    .visits
                    .get(target)
                    .map(|v| v.depth == 0)
                    .unwrap_or(false)
                {
                    return branch_move_into_child(e, src, target);
                }
                // 下半区 = 排序：插到 target 之后 **所在层级**（同父 = 重排；
                // 跨层级 = 结构移动，fs 目录移动 + 两侧登记转移）。
                let Some(op) = branch_move_plan(e, src, target, true)? else {
                    return Ok("顺序未变化。".to_string());
                };
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
                match op {
                    BranchMoveOp::Reorder { parent_dir, seq } => {
                        let mut meta = read_meta(&bundle, &parent_dir)?;
                        let snapshot = meta.entries.clone();
                        for en in snapshot.iter().filter(|en| en.is_branch()) {
                            meta.remove_entry_path(&en.path);
                        }
                        for (i, mut en) in seq.into_iter().enumerate() {
                            en.order = Some(i as i64);
                            meta.upsert_entry(&en);
                        }
                        meta.touch();
                        meta.save(&bundle.meta_path(&parent_dir))
                            .map_err(|err| err.to_string())?;
                        e.rescan()?;
                        Ok(format!(
                            "已在「{}」内调整分支顺序。",
                            title_of_dir(e, &parent_dir)
                        ))
                    }
                    BranchMoveOp::Move {
                        src_dir,
                        src_parent_dir,
                        new_parent_dir,
                        name,
                        entry,
                        pos,
                    } => {
                        move_file(&src_dir, &new_parent_dir.join(&name))?;
                        // 深度变化 → 同步被移动分支自身 _meta 的 kind。
                        sync_moved_branch_kind(&bundle, &new_parent_dir.join(&name), &entry.role)?;
                        // 源父级撤登记。
                        let mut pm = read_meta(&bundle, &src_parent_dir)?;
                        pm.remove_entry_path(&name);
                        pm.touch();
                        pm.save(&bundle.meta_path(&src_parent_dir))
                            .map_err(|err| err.to_string())?;
                        // 新父级按位登记（重建分支序列 order）。
                        let mut nm = read_meta(&bundle, &new_parent_dir)?;
                        let mut seq: Vec<Entry> = nm
                            .entries
                            .iter()
                            .filter(|en| en.is_branch())
                            .cloned()
                            .collect();
                        seq.insert(pos.min(seq.len()), entry);
                        let snapshot = nm.entries.clone();
                        for en in snapshot.iter().filter(|en| en.is_branch()) {
                            nm.remove_entry_path(&en.path);
                        }
                        for (i, mut en) in seq.into_iter().enumerate() {
                            en.order = Some(i as i64);
                            nm.upsert_entry(&en);
                        }
                        nm.touch();
                        nm.save(&bundle.meta_path(&new_parent_dir))
                            .map_err(|err| err.to_string())?;
                        // 展开新父级（必须在 rescan 前设置：rescan 内部的 rebuild
                        // 按展开集合建行，之后插入要等下一次操作才体现）。rel 键
                        // 跨 rescan 稳定，可取自移动前的 scan。
                        if let Some(scan) = e.scan.as_ref() {
                            if let Some(i) = scan
                                .visits
                                .iter()
                                .position(|v| v.dir == new_parent_dir)
                            {
                                e.expanded.insert(visit_key(scan, i).to_string());
                            }
                        }
                        e.rescan()?;
                        // 与内容策略对齐：高亮落地的分支。
                        if let Some(scan) = e.scan.as_ref() {
                            let new_dir = new_parent_dir.join(&name);
                            if let Some(i) = scan.visits.iter().position(|v| v.dir == new_dir) {
                                e.selected = Some(visit_key(scan, i).to_string());
                            }
                        }
                        Ok(format!(
                            "已把分支「{name}」从「{}」移动到「{}」。",
                            title_of_dir(e, &src_parent_dir),
                            title_of_dir(e, &new_parent_dir)
                        ))
                    }
                }
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("移动失败：{msg}").into()),
            }
        });
    }
    {
        // 挂载行拖拽落点（软 / 硬链接）：只动挂载声明。src = DndApi 拖拽源状态
        // （挂载点 + 目标 visit）；落点 = 悬停状态（nest = 重挂载到该行之下，
        // 否则 = 同挂载点内重排，目标行 path 由 slint 侧 mount-row-path 求值写入）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_drop_mount_row(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let result = with_editor(&editor, |e| {
                let dnd = app.global::<DndApi>();
                let mp = dnd.get_tree_src_mount_parent();
                let tv = dnd.get_tree_src_mount_target();
                let hover = dnd.get_tree_hover_visit();
                let nest = dnd.get_tree_hover_nest();
                let action = dnd.get_tree_hover_mount_action();
                let (Ok(mp), Ok(tv)) = (usize::try_from(mp), usize::try_from(tv)) else {
                    return Err("挂载拖拽源无效。".into());
                };
                let Ok(hover) = usize::try_from(hover) else {
                    return Err("挂载落点无效。".into());
                };
                if nest || action == 2 {
                    // 上半区 / 下半区非兄弟真实行 = 重挂载为该行的第一个子分支
                    //（落到挂载父节点下半区 = 移到最前）。
                    mount_move_into_child(e, mp, tv, hover)
                } else if action == 1 {
                    // 同挂载点内重排：插到目标结构行之后。
                    let dst_path = dnd.get_tree_hover_mount_path().to_string();
                    if dst_path.is_empty() {
                        return Err("挂载落点无效。".into());
                    }
                    let src_entry = mount_link_entry(e, mp, tv)?;
                    mount_reorder(e, mp, &src_entry.path, &dst_path, true)
                } else {
                    Err("挂载落点无效。".into())
                }
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("移动失败：{msg}").into()),
            }
        });
    }
    {
        // 整窗兜底：外部文件拖到窗口任意位置 → 导入当前选中分支。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_external_files_dropped(move |files| {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            // 整窗兜底 / 画布空白 / 节点级落点都会走到这里：打印悬停状态即可判断
            // 拖拽期间有没有收到 DragMove（can-drop 有没有跑过）。
            if debug_on() {
                let d = app.global::<DndApi>();
                eprintln!(
                    "[str-gui] external-files-dropped files={:?} hover-row={} half={} \
                     panel={} node={} src-visit={} mind-drag={}",
                    files,
                    d.get_hover_row(),
                    d.get_hover_half(),
                    d.get_hover_panel(),
                    d.get_hover_node(),
                    d.get_src_visit(),
                    d.get_mind_drag()
                );
            }
            // 拖入 .str 目录直接打开时记录打开路径（供 push_recent / 关闭欢迎引导）。
            let mut drop_opened: Option<PathBuf> = None;
            let result = with_editor(&editor, |e| {
                if files.is_empty() {
                    return Err("拖拽载荷中没有文件".into());
                }
                let paths: Vec<PathBuf> = files.lines().map(PathBuf::from).collect();
                // 未打开 bundle 时：拖入 .str 目录（含 .str.toml 的目录）= 直接打开，而非导入报错。
                if e.bundle.is_none() {
                    match paths.iter().find(|p| p.is_dir() && p.join(".str.toml").is_file()) {
                        Some(first) => {
                            e.open(first)?;
                            drop_opened = Some(canonical_bundle_path(first));
                            return Ok(format!("已打开：{}", first.display()));
                        }
                        None => {
                            return Err(
                                "未打开 bundle —— 拖入 .str 目录可直接打开；也可用「文件 → 打开文件…」选择"
                                    .into(),
                            );
                        }
                    }
                }
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let (dir, title) = {
                    let v = &e.scan.as_ref().unwrap().visits[visit_idx];
                    (v.dir.clone(), visit_title(v))
                };
                let names = import_paths_into(bundle, &dir, &dir, true, &paths)?;
                e.rescan()?;
                Ok(format!(
                    "已从外部拖入导入 {} 项 →「{title}」：{}。",
                    names.len(),
                    name_list(&names)
                ))
            });
            match result {
                Ok(msg) => {
                    if let Some(p) = drop_opened.take() {
                        push_recent(&p);
                        refresh_recent(&app);
                    }
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, msg.into());
                }
                Err(msg) => show_status(&app, format!("导入失败：{msg}").into()),
            }
        });
    }

    // ── 校验 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_run_validate(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(bundle) = e.bundle.as_ref() else {
                show_status(&app, "请先打开一个 bundle。".into());
                return;
            };
            match validate::validate(bundle) {
                Ok(report) => {
                    let (errs, warns) = (report.error_count(), report.warning_count());
                    let summary = format!("校验完成：{errs} 个错误、{warns} 个警告。");
                    let detail = if errs + warns > 0 {
                        report
                            .to_text()
                            .lines()
                            .take(4)
                            .collect::<Vec<_>>()
                            .join(" ｜ ")
                    } else {
                        String::new()
                    };
                    show_status(&app, if detail.is_empty() {
                        summary.into()
                    } else {
                        format!("{summary} ｜ {detail}").into()
                    });
                }
                Err(err) => show_status(&app, format!("校验执行失败：{err}").into()),
            }
        });
    }

    // ── 新建 bundle 对话框 ──
    {
        let app_weak = app.as_weak();
        app.on_new_bundle(move || {
            let app = app_weak.upgrade().unwrap();
            app.set_new_name("".into());
            app.set_new_dir("".into());
            app.set_dir_picked(false);
            app.set_dialog_visible(true);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_dialog_cancel(move || {
            app_weak.upgrade().unwrap().set_dialog_visible(false);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_dialog_browse(move || {
            let app = app_weak.upgrade().unwrap();
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("选择 bundle 存放位置")
                .pick_folder()
            {
                app.set_new_dir(dir.display().to_string().into());
                app.set_dir_picked(true);
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_dialog_create(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let name = app.get_new_name().trim().to_string();
            let dir = app.get_new_dir().to_string();
            if name.is_empty() || dir.is_empty() {
                show_status(&app, "新建 bundle 失败：名称与位置不能为空。".into());
                return;
            }
            app.set_dialog_visible(false);
            let bundle_dir = PathBuf::from(&dir).join(format!("{name}.str"));
            let result = (|| -> Result<(), String> {
                std::fs::create_dir_all(&bundle_dir).map_err(|err| err.to_string())?;
                let root_id = util::new_uuid_v7();
                let created = util::now_rfc3339();
                let text =
                    meta_edit::render_root_meta(&name, Some(&name), None, &root_id, &created, 7);
                std::fs::write(bundle_dir.join(".str.toml"), text).map_err(|err| err.to_string())?;
                Ok(())
            })();
            match result.and_then(|()| with_editor(&editor, |e| e.open(&bundle_dir))) {
                Ok(()) => {
                    // 新建即打开：与四个打开入口同规，记入最近列表（push_recent
                    // 内部统一规范化为绝对路径）。
                    push_recent(&bundle_dir);
                    refresh_recent(&app);
                    sync_ui(&app, &editor.borrow());
                    show_status(&app, format!("已新建并打开 bundle：「{}」。", bundle_dir.display()).into());
                }
                Err(msg) => show_status(&app, format!("创建失败：{msg}").into()),
            }
        });
    }

    // ── 命令行参数：直接打开指定 bundle（`str-gui <路径>` / 文件关联双击）──
    // 带路径参数启动 = 用户目标明确，**不弹**欢迎引导；仅在无参数时按需弹出。
    match std::env::args().nth(1) {
        Some(arg) => {
            // 规范化后再打开：`str-gui .` 的状态栏与最近列表都落成绝对路径。
            let path = canonical_bundle_path(&PathBuf::from(arg));
            match with_editor(&editor, |e| e.open(&path)) {
                Ok(()) => {
                    push_recent(&path);
                    refresh_recent(&app);
                    show_status(&app, format!("已打开 bundle：「{}」。", path.display()).into());
                }
                Err(msg) => show_status(&app, format!("打开失败：{msg}").into()),
            }
        }
        None => {}
    }

    sync_ui(&app, &editor.borrow());

    // ── 交互性能基准模式（env 门控，正常启动零影响）：`STR_GUI_BENCH=<bundle>`
    // 打开指定 bundle，在**真实窗口 + 真实事件循环 + 真渲染器**上回放脚本化
    // 操作（展开全部 / 收起全部 / 逐行展开收起 / 选中 / 导图实例展开），输出
    // 每类操作的延迟统计（均值 / 最大 / >16ms 掉帧数 / >100ms 卡顿数）。──
    if let Ok(bench_path) = std::env::var("STR_GUI_BENCH") {
        eprintln!("bench mode: 打开 {}", bench_path);
        let path = canonical_bundle_path(&PathBuf::from(bench_path));
        match with_editor(&editor, |e| e.open(&path)) {
            Ok(()) => {
                push_recent(&path);
                refresh_recent(&app);
                sync_ui(&app, &editor.borrow());
                eprintln!("bench: bundle opened + synced");
            }
            Err(msg) => show_status(&app, format!("bench 打开失败：{msg}").into()),
        }
        let app_weak = app.as_weak();
        // 驱动线程：本环境（macOS + winit 后台启动）里 slint::Timer 不触发，
        // 改用 std::thread + invoke_from_event_loop 逐个投递操作 —— UI 线程按序
        // 执行，驱动线程等待完成并统计延迟。测量范围 = 处理器 + sync + 布局 +
        // 模型更新全链路（用户可感的操作延迟）。
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            struct Stats {
                n: u32,
                total: f64,
                max: f64,
                over16: u32,
                over100: u32,
            }
            impl Stats {
                fn new() -> Self {
                    Self { n: 0, total: 0.0, max: 0.0, over16: 0, over100: 0 }
                }
                fn record(&mut self, ms: f64) {
                    self.n += 1;
                    self.total += ms;
                    self.max = self.max.max(ms);
                    if ms > 16.0 {
                        self.over16 += 1;
                    }
                    if ms > 100.0 {
                        self.over100 += 1;
                    }
                }
                fn report(&self, name: &str) {
                    eprintln!(
                        "bench[{name:>10}] n={:<3} avg={:>7.2}ms max={:>8.2}ms >16ms={:<3} >100ms={}",
                        self.n,
                        if self.n > 0 { self.total / self.n as f64 } else { 0.0 },
                        self.max,
                        self.over16,
                        self.over100
                    );
                }
            }
            let (tx, rx) = std::sync::mpsc::channel::<(&'static str, f64)>();
            let mut st_expand = Stats::new();
            let mut st_collapse = Stats::new();
            let mut st_toggle = Stats::new();
            let mut st_select = Stats::new();
            let mut st_mind = Stats::new();
            // 在 UI 线程执行一个操作并等待完成；返回是否成功（超时 = UI 卡死）。
            fn bench_op(
                name: &'static str,
                weak: &slint::Weak<AppWindow>,
                tx: &std::sync::mpsc::Sender<(&'static str, f64)>,
                rx: &std::sync::mpsc::Receiver<(&'static str, f64)>,
                stats: &mut Stats,
                op: impl FnOnce(&AppWindow) + Send + 'static,
            ) -> bool {
                let weak = weak.clone();
                let tx2 = tx.clone();
                let posted = slint::invoke_from_event_loop(move || {
                    let Some(app) = weak.upgrade() else {
                        let _ = tx2.send((name, -1.0));
                        return;
                    };
                    let t = std::time::Instant::now();
                    op(&app);
                    let _ = tx2.send((name, t.elapsed().as_secs_f64() * 1000.0));
                });
                if posted.is_err() {
                    eprintln!("bench op {name} 投递失败");
                    return false;
                }
                match rx.recv_timeout(std::time::Duration::from_secs(180)) {
                    Ok((_, ms)) => {
                        stats.record(ms);
                        true
                    }
                    Err(_) => {
                        eprintln!("bench op {name} 超时（>180s，UI 卡死）");
                        false
                    }
                }
            }
            eprintln!("bench started");
            // 0) 切到导图视图（用户场景：导图才是重布局路径）。
            if !bench_op("switch_mind", &app_weak, &tx, &rx, &mut st_select, |app| {
                app.set_view_mind(true);
            }) {
                return;
            }
            // 1) 展开 / 收起全部 × 5（重布局最重路径）。
            for i in 0..5u32 {
                if !bench_op("expand_all", &app_weak, &tx, &rx, &mut st_expand, move |app| {
                    app.invoke_expand_all_subtrees();
                }) {
                    return;
                }
                if !bench_op("collapse_all", &app_weak, &tx, &rx, &mut st_collapse, move |app| {
                    app.invoke_collapse_all_subtrees();
                }) {
                    return;
                }
                let _ = i;
            }
            // 2) 展开全部后：逐行展开 / 收起 + 选中（轻量操作在重状态下的延迟）。
            if !bench_op("expand_all", &app_weak, &tx, &rx, &mut st_expand, |app| {
                app.invoke_expand_all_subtrees();
            }) {
                return;
            }
            for i in 0..40u32 {
                let row = 2 + (i % 7);
                if !bench_op("row_toggle", &app_weak, &tx, &rx, &mut st_toggle, move |app| {
                    app.invoke_toggle_expand(row as i32);
                }) {
                    return;
                }
                if !bench_op("select", &app_weak, &tx, &rx, &mut st_select, move |app| {
                    let v = app
                        .get_mind_nodes()
                        .row_data(1)
                        .map(|n| n.visit)
                        .unwrap_or(0);
                    app.invoke_select_visit(v);
                }) {
                    return;
                }
            }
            // 3) 导图实例展开 / 收起 × 40（取导图模型里的实际节点键）。
            for i in 0..40u32 {
                let slot = 1 + (i % 5) as usize;
                if !bench_op("mind_toggle", &app_weak, &tx, &rx, &mut st_mind, move |app| {
                    if let Some(n) = app.get_mind_nodes().row_data(slot) {
                        app.invoke_toggle_visit_expand(n.expand_key.clone());
                    }
                }) {
                    return;
                }
            }
            st_expand.report("expand_all");
            st_collapse.report("collapse_all");
            st_toggle.report("row_toggle");
            st_select.report("select");
            st_mind.report("mind_toggle");
            eprintln!("bench done");
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        });
    }

    app.run()
}

#[cfg(test)]
mod collapse_tests {
    use super::*;

    fn demo_editor() -> Editor {
        let mut e = Editor::new();
        let demo =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/客户运营.str");
        e.open(&demo).expect("打开示例 bundle");
        e
    }

    /// 小地图连线几何：两端各留半个子树按钮占位的间距（不接到两端节点上，
    /// 左右一致），中段按 1:2 划分（左段短、右段长）；大画布连线仍为按钮全让位。
    #[test]
    fn mini_edges_split_one_to_two() {
        let mut e = demo_editor();
        let root_id = e
            .scan
            .as_ref()
            .and_then(|s| s.visits.first())
            .and_then(|v| v.meta.as_ref())
            .and_then(|m| m.id.clone())
            .expect("根分支有 id");
        e.expanded.insert(root_id);
        let out = mind_layout(&e);
        assert!(!out.mini_edges.is_empty(), "展开根子树后应有缩略图连线");

        let root = out.nodes.iter().find(|n| n.is_root).expect("有根节点");
        let gap = SUBTREE_BTN_EXTENT / 2.0; // 两端各留的呼吸间距
        let mini_start = root.x + root.w + gap; // 缩略图左端
        let canvas_start = root.x + root.w + SUBTREE_BTN_EXTENT; // 画布左端
        let eps = 0.01;

        // 水平段判据：宽度明显大于线宽（借此排除竖线段）。
        let left = out
            .mini_edges
            .iter()
            .find(|s| (s.x - mini_start).abs() < eps && s.w > EDGE_W + eps)
            .expect("找到缩略图左段");
        let mid = left.x + left.w;
        let right = out
            .mini_edges
            .iter()
            .find(|s| (s.x - mid).abs() < eps && s.w > EDGE_W + eps)
            .expect("找到缩略图右段");

        assert!(left.w > 0.0 && right.w > 0.0, "两段都应可见");
        let ratio = right.w / left.w;
        assert!(
            (ratio - 2.0).abs() < 0.05,
            "右段应为左段的 2 倍，实际 左 {:.2} / 右 {:.2}",
            left.w,
            right.w
        );

        // 右端不接到分支上：与子节点左缘之间留与左端相同的间距。
        // 子节点左缘 = 对应画布右段的终点。
        let canvas_right = out
            .edges
            .iter()
            .find(|s| (s.x - (canvas_start + NODE_HGAP / 2.0)).abs() < eps && s.w > EDGE_W + eps)
            .expect("找到画布右段");
        let child_left = canvas_right.x + canvas_right.w;
        let mini_right_end = right.x + right.w;
        assert!(
            (mini_right_end - (child_left - gap)).abs() < eps,
            "右端应距子节点左缘 {}px（与左端一致），实际 右端 {:.2} / 子左缘 {:.2}",
            gap,
            mini_right_end,
            child_left
        );
        // 画布连线起点仍越过按钮：同一父节点在画布侧的左段起点更靠右。
        assert!(
            out.edges
                .iter()
                .any(|s| (s.x - canvas_start).abs() < eps && s.w > EDGE_W + eps),
            "画布连线起点应为父右缘 + 整个按钮占位"
        );
    }

    #[test]
    fn collapse_clears_descendant_expand_and_activation() {
        let mut e = demo_editor();
        // 找「客户档案」分支（rel / dir 路径含名字）。
        let idx = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .position(|v| v.rel.contains("客户") || v.dir.to_string_lossy().contains("客户"))
            .expect("找到客户档案分支");
        let scan = e.scan.as_ref().unwrap();
        let kids = e.kids.get(idx).cloned().unwrap_or_default();
        assert!(!kids.is_empty(), "客户档案应有子分支");
        let root_key = scan.visits[idx].rel.clone();
        let kid_keys: Vec<String> = kids.iter().map(|&i| scan.visits[i].rel.clone()).collect();
        // 展开客户档案与全部子分支，并激活第一个子分支（身份键 = rel）。
        e.expanded.insert(root_key.clone());
        for key in &kid_keys {
            e.expanded.insert(key.clone());
        }
        e.selected = Some(kid_keys[0].clone());
        e.content_expanded.insert(kid_keys[0].clone());
        e.rebuild();
        assert!(
            e.rows.iter().any(|r| r.visit == kids[0]),
            "展开后子分支行可见"
        );

        // 收缩客户档案 → 子树展开态与激活态应全部清除。
        e.expanded.remove(&root_key);
        collapse_subtree_state(&mut e, idx);
        e.rebuild();

        for key in &kid_keys {
            assert!(!e.expanded.contains(key), "子分支展开态应被移除");
            assert!(!e.content_expanded.contains(key), "子分支内容展开态应被移除");
        }
        // 收回包含选中分支的子树：选中转移到**收回的分支本身**（保持可见焦点，
        // Finder/VSCode 同语义），而不是凭空消失；内容选区随之清空。
        assert_eq!(
            e.selected.as_deref(),
            Some(root_key.as_str()),
            "焦点应落到被收回的分支上"
        );
        assert_eq!(e.selected_entry_path, None);
        assert!(e.entry_multi.is_empty(), "内容多选区应随焦点转移清空");
        assert!(
            e.rows.iter().all(|r| r.visit != kids[0]),
            "收缩后子分支行不应可见"
        );
        assert!(
            e.rows.iter().any(|r| r.visit == idx),
            "被收回的分支行本身应可见且持有焦点"
        );
    }
}

#[cfg(test)]
mod test_support {
    use super::*;

    /// 展开全树的示例 bundle（连线几何最完整：肘形三段 + 缩略图 1:2 划分）。
    pub fn expanded_demo() -> Editor {
        let mut e = Editor::new();
        let demo =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/客户运营.str");
        e.open(&demo).expect("打开示例 bundle");
        // 展开集合按身份键（rel）记账，见 `visit_key`。
        let keys: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.rel.clone())
            .collect();
        e.expanded = keys.into_iter().collect();
        e.rebuild();
        e
    }
}

#[cfg(test)]
mod edge_chunk_tests {
    use super::test_support::expanded_demo;
    use super::*;

    /// 从 Path 命令串里取全部 `M/L x y` 坐标点。
    fn cmd_points(commands: &str) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        let mut it = commands.split_whitespace();
        while let Some(tok) = it.next() {
            if tok == "M" || tok == "L" {
                let x: f32 = it.next().expect("x 坐标").parse().expect("float");
                let y: f32 = it.next().expect("y 坐标").parse().expect("float");
                out.push((x, y));
            }
        }
        out
    }

    /// 矩形段 → 居中描边：中轴取矩形中线，描边宽度 = 段厚 ⇒ 覆盖范围与矩形一致。
    #[test]
    fn spine_matches_rect_coverage() {
        let h = MindEdge {
            x: 10.0,
            y: 20.0,
            w: 50.0,
            h: EDGE_W,
        };
        assert_eq!(edge_spine(&h), (10.0, 21.0, 60.0, 21.0));
        let v = MindEdge {
            x: 10.0,
            y: 20.0,
            w: EDGE_W,
            h: 50.0,
        };
        assert_eq!(edge_spine(&v), (11.0, 20.0, 11.0, 70.0));
    }

    /// 画布连线：段数守恒、合并为少量块、坐标相对块原点且落在包络盒内
    /// （包络盒是 Slint 侧裁剪判定与坐标系换算的唯一依据）。
    #[test]
    fn canvas_chunks_cover_every_segment() {
        let out = mind_layout(&expanded_demo());
        assert!(!out.edge_chunks.is_empty(), "应有画布连线块");
        assert!(
            out.edge_chunks.len() * 2 <= out.edges.len(),
            "连线应合并为少量块：{} 块 / {} 段",
            out.edge_chunks.len(),
            out.edges.len()
        );

        let mut pts = 0;
        for c in &out.edge_chunks {
            let p = cmd_points(&c.commands);
            pts += p.len();
            assert!(c.w > 0.0 && c.h > 0.0, "块包络盒不能退化");
            for (x, y) in p {
                assert!(
                    x >= -0.01 && y >= -0.01 && x <= c.w + 0.01 && y <= c.h + 0.01,
                    "命令串坐标须为块内相对坐标且落在包络盒内：({x}, {y}) vs {}×{}",
                    c.w,
                    c.h
                );
            }
        }
        assert_eq!(pts, out.edges.len() * 2, "每段恰好一条描边线段（两个端点）");
    }

    /// 小地图连线：整图一块、保留内容绝对坐标（Slint 侧靠 viewbox 缩放），
    /// 包络盒覆盖全部端点。
    #[test]
    fn mini_chunk_keeps_absolute_coordinates() {
        let out = mind_layout(&expanded_demo());
        assert_eq!(out.mini_edge_chunks.len(), 1, "小地图连线应合并为一块");
        let c = &out.mini_edge_chunks[0];
        let p = cmd_points(&c.commands);
        assert_eq!(p.len(), out.mini_edges.len() * 2);

        let (ax, ay, _, _) = edge_spine(&out.mini_edges[0]);
        assert!(
            (p[0].0 - ax).abs() < 0.01 && (p[0].1 - ay).abs() < 0.01,
            "小地图命令串须为内容绝对坐标，首个点 ({}, {}) 应等于首段中轴起点 ({ax}, {ay})",
            p[0].0,
            p[0].1
        );
        for (x, y) in &p {
            assert!(
                *x >= c.x - 0.01
                    && *y >= c.y - 0.01
                    && *x <= c.x + c.w + 0.01
                    && *y <= c.y + c.h + 0.01,
                "端点须落在包络盒内：({x}, {y})"
            );
        }
    }
}

#[cfg(test)]
mod dnd_lint_tests {
    /// 每个 `can-drop` 都必须**先** `DndApi.clear-hover()` 再置本区域状态。
    ///
    /// 这是「拖拽离开某区域后指示残留」的唯一防线：早前 8 个 can-drop 各写一份字段
    /// 子集、彼此漂移 —— 画布空白处漏了 `hover-node`，拖拽从分支移到画布空白后该
    /// 分支的激活高亮一直留着；另有几处漏 `mind-drag`，边缘滚动停不下来。
    /// 唯一例外：条目面板的整行接收层（它故意不写状态，语义由上半区/下半区决定）。
    #[test]
    fn every_can_drop_clears_previous_hover() {
        let src = include_str!("../ui/app.slint");
        let lines: Vec<&str> = src.lines().collect();
        let mut checked = 0;
        for (i, ln) in lines.iter().enumerate() {
            if !ln.trim().starts_with("can-drop(event) => {") {
                continue;
            }
            let body = lines[(i + 1).min(lines.len())..(i + 8).min(lines.len())].join("\n");
            if body.contains("整行区域只作为 Drop 的接收层") {
                continue; // 接收层例外：不写状态，只兜底
            }
            assert!(
                body.contains("DndApi.clear-hover();"),
                "can-drop 未先清理上一个区域的悬停指示（app.slint:{}）",
                i + 1
            );
            checked += 1;
        }
        assert!(checked >= 5, "至少应覆盖 5 个 can-drop（实际 {checked}）");
    }
}

#[cfg(test)]
mod latin_only_tests {
    use super::*;

    /// 行内容的光学下移只对「全拉丁」行生效：满高字形（CJK / 假名 / 谚文 / 全角 /
    /// emoji）必须判为 false，否则中文行会被压得偏低（实测差 1.5px）。
    #[test]
    fn latin_only_matches_full_height_script_boundary() {
        // 拉丁 / ASCII / 空串 → 补偿
        assert!(is_latin_only("README.md"));
        assert!(is_latin_only("examples"));
        assert!(is_latin_only(""), "空串按拉丁处理（补偿无害）");
        assert!(is_latin_only("Café 2026"));
        // 满高字形 → 不补偿
        assert!(!is_latin_only("客户档案"));
        assert!(!is_latin_only("abc客"), "混排只要含一个满高字形就不补偿");
        assert!(!is_latin_only("日本語"), "假名 U+3040+");
        assert!(!is_latin_only("한글"), "谚文 U+AC00");
        assert!(!is_latin_only("全角ＡＢＣ"), "全角 U+FF00+");
        assert!(!is_latin_only("emoji🚀"), "emoji U+1F680");
        assert!(!is_latin_only("、。"), "CJK 标点 U+3000+");
    }
}

#[cfg(test)]
mod dnd_tests {
    use super::test_support::expanded_demo;
    use super::*;

    /// `src-visit` 的不变量：等于当前激活分支（拖拽收尾时据此复位 —— 拖拽期间
    /// 它被面板改写为拖拽源，取消后不复位就会指向旧拖拽源）。
    #[test]
    fn drag_source_follows_active_branch() {
        let mut e = expanded_demo();
        // 打开后默认选中根分支 → src-visit 应等于该分支下标。
        let root_idx = e.selected_idx_in_visits().expect("默认选中根分支");
        assert_eq!(drag_source_visit(&e), root_idx as i32);
        // 换一个带 id 的分支：src-visit 跟着走。
        let target = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .enumerate()
            .filter(|(_, v)| v.meta.as_ref().and_then(|m| m.id.as_deref()).is_some())
            .map(|(i, _)| i)
            .find(|&i| i != root_idx)
            .expect("至少还有另一个带 id 的分支");
        e.select_visit(target);
        assert_eq!(drag_source_visit(&e), target as i32);
        // 无选中 → -1（拖拽收尾也走这条复位路径）。
        e.selected = None;
        assert_eq!(drag_source_visit(&e), -1);
    }
}

#[cfg(test)]
mod help_tests {
    use super::*;

    /// 快捷键表：分组连续（组标题只画一次）、每组都有内容、修饰键字形随平台切换。
    #[test]
    fn shortcut_table_is_grouped_and_platform_aware() {
        for mac in [true, false] {
            let rows = shortcut_table(mac);
            assert!(rows.len() > 15, "条目数应覆盖各菜单");
            assert!(rows[0].head, "首行必为组首行");
            let mut groups: Vec<&str> = Vec::new();
            for (i, r) in rows.iter().enumerate() {
                assert!(!r.name.is_empty() && !r.keys.is_empty());
                if r.head {
                    assert!(
                        !groups.contains(&r.group.as_str()),
                        "同一分组不能出现两次组首行：{}",
                        r.group
                    );
                    groups.push(&r.group);
                } else {
                    assert_eq!(r.group, rows[i - 1].group, "组内行必须与上一行同组");
                }
            }
            assert!(groups.contains(&"文件") && groups.contains(&"视图"));
            let all = rows
                .iter()
                .map(|r| r.keys.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if mac {
                assert!(
                    all.contains('⌘') && !all.contains("Ctrl+"),
                    "macOS 用 ⌘ 字形"
                );
            } else {
                assert!(
                    all.contains("Ctrl+") && !all.contains('⌘'),
                    "其它平台用 Ctrl+"
                );
            }
        }
    }

    /// 帮助外链：由仓库地址派生，且三类入口各不相同。
    #[test]
    fn doc_urls_derive_from_repository() {
        let repo = "https://github.com/frowhy/str.str/";
        assert_eq!(
            doc_url(repo, 0),
            "https://github.com/frowhy/str.str/blob/main/SPEC.md"
        );
        assert_eq!(doc_url(repo, 1), "https://github.com/frowhy/str.str");
        assert_eq!(doc_url(repo, 2), "https://github.com/frowhy/str.str/issues");
        // 未预期的下标也落到「问题反馈」，不会 panic。
        assert_eq!(doc_url(repo, 9), doc_url(repo, 2));
    }
}

#[cfg(test)]
mod mini_density_tests {
    use super::test_support::expanded_demo;
    use super::*;

    /// 合成布局：两块 100×20 节点（右块为选中）+ 一条连线，内容 300×20。
    fn synthetic() -> MindOut {
        let node = |x: f32, selected: bool| MindNode {
            visit: 0,
            x,
            y: 0.0,
            w: 100.0,
            h: 20.0,
            title: "n".into(),
            type_str: "t".into(),
            is_root: false,
            is_selected: selected,
            expanded: false,
            children_expanded: false,
            has_children: false,
            mounted: false,
            hard: false,
            mount_parent: -1,
            link_visit: -1,
            expand_key: String::new().into(),
            depth: 0,
            has_entries: false,
            ref_mark: false,
            parent_mark: false,
            entry_rows: ModelRc::from(Rc::new(VecModel::<EntryRow>::default())),
        };
        MindOut {
            nodes: vec![node(0.0, false), node(200.0, true)],
            edges: vec![MindEdge {
                x: 100.0,
                y: 9.0,
                w: 100.0,
                h: EDGE_W,
            }],
            mini_edges: Vec::new(),
            edge_chunks: Vec::new(),
            mini_edge_chunks: Vec::new(),
            ref_edges: Vec::new(),
            w: 300.0,
            h: 20.0,
            edge_offset: 0.0,
        }
    }

    /// 密度位图：尺寸 = 盒 × 设备像素比；节点处着墨、空白处透明、
    /// 选中链节点用强调色，连线（骨架）也留痕。
    #[test]
    fn raster_covers_nodes_edges_and_spine() {
        let out = synthetic();
        let ink = [10, 20, 30];
        let accent = [200, 0, 0];
        let r = mini_density_raster(
            &out,
            (100.0, 20.0),
            (100.0 / 300.0, 20.0 / 20.0),
            2.0,
            &[1],
            ink,
            accent,
        );
        assert_eq!((r.w, r.h), (200, 40), "位图尺寸 = 盒 × 设备像素比");
        let px = |x: u32, y: u32| {
            let i = ((y * r.w + x) * 4) as usize;
            (r.px[i], r.px[i + 1], r.px[i + 2], r.px[i + 3])
        };
        // 左节点：内容 x∈[0,100] → 位图 x∈[0, 66.7]；y 铺满整幅高度。
        let (rr, gg, bb, aa) = px(4, 20);
        assert!(aa > 0 && [rr, gg, bb] == ink, "左节点应着墨：{aa}");
        // 右节点（选中）：内容 x∈[200,300] → 位图 x∈[133,200]。
        let (rr, gg, bb, aa) = px(160, 20);
        assert!(
            aa > 200 && [rr, gg, bb] == accent,
            "选中节点应为强调色且更实：{rr},{gg},{bb},{aa}"
        );
        // 两节点之间的空白（y 远离连线）：透明。
        assert_eq!(px(100, 0).3, 0, "空白处应透明");
        // 连线：取矩形段中轴（内容 y = 9 + 2/2 = 10）→ 位图 y = 20。
        assert!(px(100, 20).3 > 0, "连线应留痕");
    }

    /// 亚像素节点也要留痕（不足 1px 的块不能消失）。
    #[test]
    fn tiny_node_still_leaves_a_mark() {
        let mut out = synthetic();
        out.nodes.truncate(1);
        out.edges.clear();
        out.nodes[0].w = 0.2;
        out.nodes[0].h = 0.2;
        let r = mini_density_raster(
            &out,
            (100.0, 20.0),
            (100.0 / 300.0, 20.0 / 20.0),
            1.0,
            &[],
            [10, 20, 30],
            [200, 0, 0],
        );
        let inked = r.px.chunks(4).filter(|p| p[3] > 0).count();
        assert_eq!(inked, 1, "亚像素节点应恰好留下 1px 痕迹");
    }

    /// 选中链：从选中节点逐级回溯到根，长度 = 深度 + 1。
    #[test]
    fn selection_spine_reaches_root() {
        let mut e = expanded_demo();
        let scan = e.scan.as_ref().unwrap();
        let (idx, depth) = scan
            .visits
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| v.depth)
            .map(|(i, v)| (i, v.depth))
            .expect("示例 bundle 有节点");
        e.selected = Some(scan.visits[idx].rel.clone());
        e.rebuild();

        let out = mind_layout(&e);
        let spine = selection_spine(&out.nodes);
        assert_eq!(spine.len(), depth + 1, "链长应为深度 + 1");
        assert!(out.nodes[spine[0]].is_selected, "链首是选中节点");
        assert!(
            out.nodes[*spine.last().unwrap()].is_root,
            "链尾（回溯终点）应是根节点"
        );
    }

    /// 布局缓存签名：只随**几何输入**变化。选中分支 / 改多选集合都必须保持
    /// 同一签名（否则每次点选都会整树重排 —— 这正是性能问题的根源）。
    #[test]
    fn mind_sig_ignores_selection_only_changes() {
        let mut e = expanded_demo();
        let base = mind_sig(&e);
        let keys: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.rel.clone())
            .collect();
        // 换选中分支 + 改主选中条目 + 加多选条目：纯渲染数据
        e.selected = keys.get(1).cloned();
        e.selected_entry_path = Some("a.md".into());
        e.entry_multi.insert("a.md".into());
        assert_eq!(base, mind_sig(&e), "选中 / 多选变化不得使布局缓存失效");

        // 缓存命中路径仍要能把选中标记落到正确节点上
        let target = e.visit_idx(keys.get(1).unwrap()).unwrap();
        assert!(node_is_selected(&e, target), "新选中节点应判为选中");
        assert!(
            !e.scan
                .as_ref()
                .unwrap()
                .visits
                .iter()
                .enumerate()
                .any(|(i, _)| i != target && node_is_selected(&e, i)),
            "其余节点不应判为选中"
        );
    }

    /// 节点内嵌面板的条目多选高亮只跟**当前激活分支**走：各分支常有同名条目
    /// （都叫 README.md / payload 之类），若把 `entry_multi` 无条件交给所有节点
    /// 面板，非激活节点会因相对路径相同而串台点亮。
    #[test]
    fn node_entry_multi_is_scoped_to_active_branch() {
        let mut e = expanded_demo();
        let keys: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.rel.clone())
            .collect();
        e.selected = keys.get(1).cloned();
        let sel = e.visit_idx(keys.get(1).unwrap()).unwrap();
        e.entry_multi.insert("README.md".into());

        let none = HashSet::new();
        assert_eq!(
            node_multi(&e, sel, &none).len(),
            1,
            "激活分支的面板应带多选高亮"
        );
        assert!(
            (0..e.scan.as_ref().unwrap().visits.len())
                .filter(|&i| i != sel)
                .all(|i| node_multi(&e, i, &none).is_empty()),
            "非激活分支的面板不得带多选高亮（同名条目会串台）"
        );
    }

    /// 子目录行（含其文件）必须能进批量选区：用户报告「子目录中的文件无法多选」。
    /// 分支行仍不参与内容批量；未登记的**顶层**项保持原语义。选择 / 高亮 / 操作共用此判定。
    #[test]
    fn folder_children_are_multi_selectable() {
        let row = |depth: usize, role: &str, entries_idx: Option<usize>| VisibleEntry {
            entries_idx,
            path: "dir/file.md".into(),
            role: role.into(),
            depth,
            is_dir: false,
            expanded: false,
            drop_dir: PathBuf::from("/tmp"),
            fs_path: PathBuf::from("/tmp/dir/file.md"),
            size: None,
            title: None,
        };
        assert!(
            entry_multi_eligible(&row(0, "payload", Some(1))),
            "顶层登记条目"
        );
        assert!(
            entry_multi_eligible(&row(1, "file", None)),
            "子目录文件行（子目录行无登记）"
        );
        assert!(
            entry_multi_eligible(&row(2, "dir", None)),
            "更深层目录行"
        );
        assert!(
            !entry_multi_eligible(&row(0, "dir", None)),
            "未登记的顶层项保持原语义（不参与批量）"
        );
        assert!(!entry_multi_eligible(&row(0, "node", Some(2))), "分支行不参与");
        assert!(
            !entry_multi_eligible(&row(0, "branch", Some(3))),
            "分支行不参与"
        );
    }

    /// 几何输入变化必须使签名失效：展开集合、内容展开集合、可见行结构、scan 代次。
    #[test]
    fn mind_sig_tracks_geometry_inputs() {
        let mut e = expanded_demo();
        let base = mind_sig(&e);
        let some_key = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .get(1)
            .map(|v| v.rel.clone())
            .expect("示例 bundle 有多个分支");

        e.expanded.remove(&some_key);
        e.rebuild();
        assert_ne!(base, mind_sig(&e), "收起子树必须使缓存失效");
        e.expanded.insert(some_key.clone());
        e.rebuild();
        assert_eq!(base, mind_sig(&e), "恢复展开后签名应回到原值");

        e.content_expanded.insert(some_key);
        assert_ne!(base, mind_sig(&e), "内容展开必须使缓存失效");

        e.scan_gen += 1;
        assert_ne!(base, mind_sig(&e), "scan 代次变化必须使缓存失效");
    }

    /// 节点内嵌内容行的句柄复用：行数不变时二次写入必须复用同一句柄
    /// （句柄被换掉 = 节点内 ListView 重建 = 正在弹出的右键菜单失效）。
    #[test]
    fn node_entry_rows_handle_reused_when_count_matches() {
        let e = expanded_demo();
        let rows = visible_to_rows(&build_entry_rows_for(&e, 0), &HashSet::new());
        let first = apply_node_entry_rows(&e, 0, rows.clone());
        let second = apply_node_entry_rows(&e, 0, rows.clone());
        // ModelRc 的相等即底层模型指针相等（vendor/slint model.rs）。
        assert!(first == second, "行数不变时必须复用同一行模型句柄");
        if !rows.is_empty() {
            let mut shorter = rows;
            shorter.pop();
            let third = apply_node_entry_rows(&e, 0, shorter);
            assert!(first != third, "行数变化时应换句柄（内嵌列表结构变了）");
        }
    }
}

/// `.command` 默认处理程序解析：LSHandlers XML 的块扫描（含嵌套
/// PreferredVersions 的 `-` 占位与相邻条目干扰）。
#[cfg(all(test, target_os = "macos"))]
mod command_handler_tests {
    use super::*;

    /// 真实 plist 的缩进 / 嵌套形状：顶层 dict 包住 LSHandlers 数组，数组里
    /// 若干条目（前面的条目同样带 `LSHandlerRoleShell`，用于验证不会串档），
    /// `.command` 条目的角色值在内层 `-` 占位之后。
    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>LSHandlers</key>
	<array>
		<dict>
			<key>LSHandlerContentType</key>
			<string>public.unix-executable</string>
			<key>LSHandlerPreferredVersions</key>
			<dict>
				<key>LSHandlerRoleShell</key>
				<string>-</string>
			</dict>
			<key>LSHandlerRoleShell</key>
			<string>com.apple.dt.xcode</string>
		</dict>
		<dict>
			<key>LSHandlerContentType</key>
			<string>com.apple.terminal.shell-script</string>
			<key>LSHandlerPreferredVersions</key>
			<dict>
				<key>LSHandlerRoleAll</key>
				<string>-</string>
			</dict>
			<key>LSHandlerRoleAll</key>
			<string>com.googlecode.iterm2</string>
		</dict>
		<dict>
			<key>LSHandlerContentTag</key>
			<string>torrent</string>
			<key>LSHandlerRoleAll</key>
			<string>com.aone.keka</string>
		</dict>
	</array>
</dict>
</plist>
"#;

    #[test]
    fn picks_command_handler_and_skips_placeholders() {
        assert_eq!(
            parse_command_handler_from_ls_handlers_xml(XML).as_deref(),
            Some("com.googlecode.iterm2"),
            "应取 .command 条目里非 \"-\" 的角色值，且不串到相邻条目"
        );
    }

    /// 没有 `.command` 覆盖项时返回 None（由候选链兜底 Terminal）。
    #[test]
    fn missing_entry_yields_none() {
        let xml = XML.replace("com.apple.terminal.shell-script", "com.apple.terminal.nope");
        assert_eq!(parse_command_handler_from_ls_handlers_xml(&xml), None);
    }
}

#[cfg(test)]
mod branch_move_tests {
    use super::*;

    fn demo_editor() -> Editor {
        let mut e = Editor::new();
        let demo =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/客户运营.str");
        e.open(&demo).expect("打开示例 bundle");
        e
    }

    fn find(e: &Editor, title: &str) -> usize {
        e.scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .position(|v| visit_title(v) == title)
            .unwrap_or_else(|| panic!("分支不存在：{title}"))
    }

    #[test]
    fn same_parent_reorder_yields_reorder_op() {
        let e = demo_editor();
        let src = find(&e, "订单数据集");
        let tgt = find(&e, "客户档案 · 张伟");
        match branch_move_plan(&e, src, tgt, false) {
            Ok(Some(BranchMoveOp::Reorder { seq, .. })) => {
                let titles: Vec<String> =
                    seq.iter().map(|en| en.title.clone().unwrap_or_default()).collect();
                assert_eq!(titles, vec!["订单数据集", "客户档案 · 张伟", "标签体系"]);
            }
            _ => panic!("应为 Reorder"),
        }
    }

    #[test]
    fn same_parent_no_op_boundaries_yield_none() {
        let e = demo_editor();
        let zw = find(&e, "客户档案 · 张伟");
        let dd = find(&e, "订单数据集");
        // 张伟 已在 订单 之前：拖到 订单 上半区 = 原位。
        assert!(matches!(branch_move_plan(&e, zw, dd, false), Ok(None)));
        // 订单 的上一行 = 张伟：拖到 张伟 下半区 = 原位。
        assert!(matches!(branch_move_plan(&e, dd, zw, true), Ok(None)));
    }

    #[test]
    fn cross_level_move_yields_move_op_with_node_role() {
        let e = demo_editor();
        // 跟进记录(深度2) 拖到 标签体系 下半区 → 移出父级到根层级。
        let src = find(&e, "跟进记录");
        let tgt = find(&e, "标签体系");
        match branch_move_plan(&e, src, tgt, true) {
            Ok(Some(BranchMoveOp::Move { new_parent_dir, entry, pos, .. })) => {
                // 新父级 = ROOT（depth 0）⇒ 角色应为 node。
                assert_eq!(entry.role, "node");
                assert!(new_parent_dir.ends_with("客户运营.str"));
                // 插到 标签体系 之后 = 根层级第 4 位（张伟/订单/标签 之后）。
                assert_eq!(pos, 3);
            }
            _ => panic!("应为 Move"),
        }
    }

    #[test]
    fn move_into_own_subtree_is_rejected() {
        let e = demo_editor();
        let zw = find(&e, "客户档案 · 张伟");
        let follow = find(&e, "跟进记录");
        // 张伟 拖到其子分支 跟进记录 的边界 = 移入自身子树 → 拒绝。
        assert!(matches!(
            branch_move_plan(&e, zw, follow, true),
            Err(msg) if msg.contains("子树")
        ));
        assert!(matches!(
            branch_move_plan(&e, zw, follow, false),
            Err(msg) if msg.contains("子树")
        ));
    }

    #[test]
    fn root_rows_are_rejected() {
        let e = demo_editor();
        let dd = find(&e, "订单数据集");
        // ROOT 作为源：不可拖动。
        assert!(matches!(
            branch_move_plan(&e, 0, dd, true),
            Err(msg) if msg.contains("ROOT")
        ));
        // ROOT 作为排序目标：不能插到 ROOT 之后。
        assert!(matches!(
            branch_move_plan(&e, dd, 0, true),
            Err(msg) if msg.contains("ROOT")
        ));
    }

    #[test]
    fn tree_drop_ok_mirrors_plan() {
        let e = demo_editor();
        let zw = find(&e, "客户档案 · 张伟");
        let dd = find(&e, "订单数据集");
        let bq = find(&e, "标签体系");
        // 订单 → 张伟 下半区 = 插到 张伟 之后 = 原位无效。
        assert!(!tree_drop_ok(&e, dd, zw, true));
        // 标签体系 → 张伟 下半区 = 插到 张伟 之后 = 有效移动（上移一位）。
        assert!(tree_drop_ok(&e, bq, zw, true));
    }
}

#[cfg(test)]
mod move_group_tests {
    use super::*;

    /// 临时复制的示例 bundle（写盘测试不能污染仓库里的示例）。
    /// `.str.cache` 是派生数据（revisions 基线），必须剔除 —— 手工 upsert 后的
    /// `.str.toml` 与陈旧基线放一起会报 `E_REVISION_STALE`。
    fn temp_editor() -> (PathBuf, Editor) {
        let src =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/客户运营.str");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dst = std::env::temp_dir()
            .join(format!("str-gui-move-group-{}-{nanos}.str", std::process::id()));
        copy_dir_recursive(&src, &dst).expect("复制示例 bundle");
        let _ = std::fs::remove_dir_all(dst.join(".str.cache"));
        let mut e = Editor::new();
        e.open(&dst).expect("打开临时 bundle");
        (dst, e)
    }

    /// 从 scan 里取「根分支 + 一个非根分支」的下标（visits 顺序不保证首项是根）。
    fn root_and_child(e: &Editor) -> (usize, usize) {
        let visits = &e.scan.as_ref().expect("有 scan").visits;
        let root = visits
            .iter()
            .position(|v| v.depth == 0)
            .expect("有根分支");
        let child = visits
            .iter()
            .enumerate()
            .find(|(i, v)| *i != root && v.depth > 0)
            .map(|(i, _)| i)
            .expect("有非根分支");
        (child, root)
    }

    /// 已登记的**内容文件夹**（`role = "dir"`）必须能整组跨分支移动：目标 = 分支
    /// 顶层 ⇒ 目标登记为 dir（带 count）、源登记移除、整棵子树在磁盘上搬迁。
    /// 此前该路径只放行 payload/asset（用户报告「无法拖拽移动文件夹」）。
    #[test]
    fn registered_dir_moves_across_branches() {
        let (tmp, mut e) = temp_editor();
        let bundle = e.bundle.as_ref().expect("有 bundle").clone();
        let (src_visit, dst_visit) = root_and_child(&e);
        let (src_dir, dst_dir) = {
            let scan = e.scan.as_ref().unwrap();
            (
                scan.visits[src_visit].dir.clone(),
                scan.visits[dst_visit].dir.clone(),
            )
        };
        // 造内容文件夹条目：目录 + 一个子文件 + `role = "dir"` 登记。
        let folder = "整组文件夹";
        let _ = std::fs::remove_dir_all(src_dir.join(folder));
        std::fs::create_dir_all(src_dir.join(folder)).unwrap();
        std::fs::write(src_dir.join(folder).join("note.txt"), b"hi").unwrap();
        let mut m = read_meta(&bundle, &src_dir).unwrap();
        m.upsert_entry(&Entry {
            path: folder.to_string(),
            role: "dir".to_string(),
            count: Some(1),
            ..Default::default()
        });
        m.touch();
        m.save(&bundle.meta_path(&src_dir)).unwrap();
        e.rescan().unwrap();

        // 结构树分支落点无行概念（`&[]` + at_end）= 登记到目标顶层序列末尾。
        let msg = move_group_to_branch(
            &mut e,
            &bundle,
            src_visit,
            &[folder.to_string()],
            dst_visit,
            &[],
            true,
            0,
            false,
            false,
        )
        .expect("内容文件夹应可跨分支整组移动");
        assert!(msg.contains(folder), "汇总文案应含移动项：{msg}");

        assert!(!src_dir.join(folder).exists(), "源目录不应再有该文件夹");
        assert!(
            dst_dir.join(folder).join("note.txt").exists(),
            "目标分支应收到整棵子树"
        );
        let sm = read_meta(&bundle, &src_dir).unwrap();
        assert!(
            sm.entries.iter().all(|x| x.path != folder),
            "源登记应移除"
        );
        let dm = read_meta(&bundle, &dst_dir).unwrap();
        let en = dm
            .entries
            .iter()
            .find(|x| x.path == folder)
            .expect("目标分支应登记该文件夹");
        assert_eq!(en.role, "dir", "目录应按 dir 角色登记");
        assert_eq!(en.count, Some(1), "count 应等于目录实际子项数");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 未登记的磁盘文件夹（内容文件夹里的子文件夹）同样可跨分支移动，落地按
    /// `dir` 角色登记。这条臂的「是不是目录」判定同样必须在 `move_file` **之前**
    /// —— 否则源路径已消失、`is_dir()` 恒 false，文件夹被当文件读（「读取失败」）。
    #[test]
    fn unregistered_dir_moves_across_branches() {
        let (tmp, mut e) = temp_editor();
        let bundle = e.bundle.as_ref().expect("有 bundle").clone();
        let (src_visit, dst_visit) = root_and_child(&e);
        let (src_dir, dst_dir) = {
            let scan = e.scan.as_ref().unwrap();
            (
                scan.visits[src_visit].dir.clone(),
                scan.visits[dst_visit].dir.clone(),
            )
        };
        // 只落磁盘、不写 `.str.toml`：模拟内容文件夹的子文件夹。
        let folder = "未登记文件夹";
        let _ = std::fs::remove_dir_all(src_dir.join(folder));
        std::fs::create_dir_all(src_dir.join(folder)).unwrap();
        std::fs::write(src_dir.join(folder).join("a.txt"), b"a").unwrap();
        e.rescan().unwrap();

        let msg = move_group_to_branch(
            &mut e,
            &bundle,
            src_visit,
            &[folder.to_string()],
            dst_visit,
            &[],
            true,
            0,
            false,
            false,
        )
        .expect("未登记文件夹应可跨分支移动");
        assert!(msg.contains(folder), "汇总文案应含移动项：{msg}");

        assert!(!src_dir.join(folder).exists(), "源目录不应再有该文件夹");
        assert!(
            dst_dir.join(folder).join("a.txt").exists(),
            "目标分支应收到整棵子树"
        );
        let dm = read_meta(&bundle, &dst_dir).unwrap();
        let en = dm
            .entries
            .iter()
            .find(|x| x.path == folder)
            .expect("目标分支应登记该文件夹");
        assert_eq!(en.role, "dir");
        assert_eq!(en.count, Some(1));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// 全部分支身份键（用于「展开全部子树」）：真实位置 rel + 挂载行实例键 +
/// **实例上下文内的嵌套键**（镜像 `emit_branch` 的键组合规则 —— 挂载实例
/// 展开后其子树内行的键带实例前缀，「展开全部」必须覆盖它们）。
fn all_branch_keys(e: &Editor) -> Vec<String> {
    let Some(scan) = e.scan.as_ref() else {
        return Vec::new();
    };
    let mut keys: Vec<String> = scan.visits.iter().map(|v| v.rel.clone()).collect();
    fn walk_instance(
        scan: &Scan,
        kids: &[Vec<usize>],
        mounts: &[Vec<MountChild>],
        // 实例挂载点下渲染子树的位置：软 = 目标分支；硬 = 自身 link 分支。
        root: usize,
        prefix: &str,
        // 渲染链上的 visit 集合（真实祖先 + 挂载链），与 emit_branch /
        // mind_dfs 的 `path` 守卫同式 —— 挂载环 / 自我挂载 / 挂向真实祖先
        // 的声明在这里跳过，保证**非法 bundle** 下展开全部也能终止。
        path: &mut Vec<usize>,
        out: &mut Vec<String>,
        ) {
        // 全局预算（见 EXPAND_ALL_KEY_CAP）。
        if out.len() >= EXPAND_ALL_KEY_CAP {
            return;
        }
        path.push(root);
        for &c in kids.get(root).map(|v| v.as_slice()).unwrap_or_default() {
            if scan.visits[c].hard_link_to.is_some() {
                continue; // 硬链接分支不以真实行渲染（同 emit_branch 守卫）。
            }
            out.push(compose_key(prefix, scan.visits[c].rel.clone()));
            // 真实子分支延续同一实例前缀。
            walk_instance(scan, kids, mounts, c, prefix, path, out);
        }
        for m in mounts.get(root).map(|v| v.as_slice()).unwrap_or_default() {
            if path.contains(&m.visit) {
                continue; // 挂进自己 / 自己的祖先 ⇒ 跳过（防环，同渲染）。
            }
            let mk = compose_key(prefix, mount_expand_key(scan, root, m.visit, m.hard));
            out.push(mk.clone());
            // 嵌套挂载：软以其目标为根；硬以其自身 link 分支为根（自有结构）。
            let sub_root = if m.hard { m.link_idx.unwrap_or(m.visit) } else { m.visit };
            walk_instance(scan, kids, mounts, sub_root, &mk, path, out);
        }
        path.pop();
    }
    let mut walk_path: Vec<usize> = Vec::new();
    for (p, ms) in e.mounts.iter().enumerate() {
        for m in ms {
            if walk_path.contains(&m.visit) {
                continue;
            }
            let mk = mount_expand_key(scan, p, m.visit, m.hard);
            keys.push(mk.clone());
            let sub_root = if m.hard { m.link_idx.unwrap_or(m.visit) } else { m.visit };
            walk_path.push(m.visit);
            walk_instance(scan, &e.kids, &e.mounts, sub_root, &mk, &mut walk_path, &mut keys);
            walk_path.pop();
        }
    }
    keys
}


#[cfg(test)]
mod mount_tests {
    use super::*;

    const RA: &str = "01928f3a-7c4b-4001-8a01-000000000004";
    const RB: &str = "01928f3a-7c4b-4002-8a02-000000000005";
    /// 硬链接分支自身的 id（= 目录名；C1 语义下硬链接是有身份的真实分支）。
    const RL: &str = "01928f3a-7c4b-4003-8a03-000000000006";

    fn meta_text(kind: &str, id: &str, tables: &str) -> String {
        format!(
            "str = 1\nspec = \"1.14.0\"\nkind = \"{kind}\"\nid = \"{id}\"\n\
             revision = 1\ncreated_at = 2026-09-01T09:00:00+08:00\n\
             updated_at = 2026-09-01T09:00:00+08:00\n{tables}"
        )
    }

    fn payload_entry(path: &str, body: &str) -> String {
        format!(
            "[[entries]]\npath = \"{path}\"\nrole = \"payload\"\nsize = {}\nsha256 = \"{}\"\n",
            body.len(),
            util::sha256_bytes(body.as_bytes())
        )
    }

    /// ROOT + 两个独立节点 A / B；`alias` 非空时在 A 下挂载 B（带挂载点别名），
    /// `hard = true` 时写 `mode = "hard"`（硬链接，仅内容关联）。
    fn bundle_with_mount(name: &str, alias: Option<&str>, hard: bool) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "strgui-mount-{name}-{}.str",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(RA)).unwrap();
        std::fs::create_dir_all(root.join(RB)).unwrap();
        let body_a = "{\"a\":1}\n";
        let body_b = "{\"b\":2}\n";
        let mode = if hard { "mode = \"hard\"\n" } else { "" };
        // 软链接：path = target、无 id、无目录；硬链接：path = id = 自身目录名
        //（目录真实存在，内含自有 `.str.toml`），内容来自 target。
        let link = if hard {
            format!(
                "[[entries]]\npath = \"{RL}\"\nrole = \"link\"\nid = \"{RL}\"\ntarget = \"{RB}\"\n{mode}"
            )
        } else {
            match alias {
                Some(a) => format!(
                    "[[entries]]\npath = \"{RB}\"\nrole = \"link\"\ntarget = \"{RB}\"\n{mode}title = \"{a}\"\n"
                ),
                None => format!(
                    "[[entries]]\npath = \"{RB}\"\nrole = \"link\"\ntarget = \"{RB}\"\n{mode}"
                ),
            }
        };
        std::fs::write(
            root.join(".str.toml"),
            meta_text(
                "root",
                "01928f3a-7c4b-4000-8000-000000000003",
                &format!(
                    "{}{}",
                    payload_entry(RA, ""),
                    payload_entry(RB, "")
                ),
            ),
        )
        .unwrap();
        // ROOT 的 payload 条目是占位（无磁盘文件、非 optional 会报 ghost），
        // 但 Editor::open 不做严格校验 —— 直接换成正确的登记。
        std::fs::write(
            root.join(".str.toml"),
            meta_text(
                "root",
                "01928f3a-7c4b-4000-8000-000000000003",
                &format!(
                    "[[entries]]\npath = \"{RA}\"\nrole = \"node\"\nid = \"{RA}\"\n\
                     [[entries]]\npath = \"{RB}\"\nrole = \"node\"\nid = \"{RB}\"\n"
                ),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(RA).join(".str.toml"),
            meta_text("node", RA, &format!("{}{}", payload_entry("a.json", body_a), link)),
        )
        .unwrap();
        std::fs::write(root.join(RA).join("a.json"), body_a).unwrap();
        if hard {
            // 硬链接分支的自有目录与 `.str.toml`（只有子分支登记，无内容条目）。
            std::fs::create_dir_all(root.join(RA).join(RL)).unwrap();
            std::fs::write(
                root.join(RA).join(RL).join(".str.toml"),
                meta_text("branch", RL, "title = \"硬链接视图\"\n"),
            )
            .unwrap();
        }
        std::fs::write(
            root.join(RB).join(".str.toml"),
            meta_text("node", RB, &payload_entry("b.json", body_b)),
        )
        .unwrap();
        std::fs::write(root.join(RB).join("b.json"), body_b).unwrap();
        root
    }

    fn opened(dir: &Path) -> Editor {
        let mut e = Editor::new();
        e.open(dir).expect("打开 bundle");
        let keys: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.rel.clone())
            .collect();
        e.expanded = keys.into_iter().collect();
        e.rebuild();
        e
    }

    /// 挂载行出现在结构树里：身份 = 目标分支（visit = B），`mounted = true`，
    /// `mount_parent` = 挂载点所在分支；别名生效时行标题用别名，且带「⤷」识别标识。
    #[test]
    fn mount_row_renders_with_target_identity() {
        let dir = bundle_with_mount("row", Some("标签（挂载）"), false);
        let e = opened(&dir);
        let a_idx = e.visit_idx(RA).expect("A");
        let b_idx = e.visit_idx(RB).expect("B");
        let mounted: Vec<&VisibleRow> = e.rows.iter().filter(|r| r.mounted).collect();
        assert_eq!(mounted.len(), 1, "应恰有一条挂载行：{:?}", e.rows);
        assert_eq!(mounted[0].visit, b_idx, "挂载行的身份 = 目标分支");
        assert_eq!(mounted[0].mount_parent, Some(a_idx));
        assert_eq!(mounted[0].title, "⤷ 标签（挂载）", "别名生效且带 ⤷ 标识");
        // 目标分支仍在真实位置（挂载不移动数据）。
        assert!(
            e.rows.iter().any(|r| r.visit == b_idx && !r.mounted),
            "真实位置的 B 行仍在"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 硬链接行：身份 = 目标分支（visit = B，与软链接一致）、带「≡」识别标识、
    /// 内容面板显示**目标**的内容条目（只读视图，写操作由 sel_hard 守卫拦截）；
    /// `mounted = false`（移除挂载走 delete-branch + op-visit），`link_visit` =
    /// 自身分支 visit（「新建子分支」落自有结构）。
    #[test]
    fn hard_mount_row_is_own_branch_with_marker() {
        let dir = bundle_with_mount("hardrow", None, true);
        let e = opened(&dir);
        let b_idx = e.visit_idx(RB).expect("B");
        let link_idx = e
            .visit_idx(&format!("{RA}/{RL}"))
            .expect("硬链接分支自身是 visit");
        let hard_rows: Vec<&VisibleRow> = e.rows.iter().filter(|r| r.hard).collect();
        assert_eq!(hard_rows.len(), 1, "应恰有一条硬链接行");
        assert_eq!(hard_rows[0].visit, b_idx, "硬链接行的身份 = 目标分支（A 语义）");
        assert_eq!(
            hard_rows[0].link_visit,
            Some(link_idx),
            "link_visit = 自身分支（新建子分支落点）"
        );
        assert!(
            hard_rows[0].title.starts_with("≡ "),
            "硬链接行带 → 识别标识：{:?}",
            hard_rows[0].title
        );
        assert!(!hard_rows[0].mounted, "mounted 标记只给软链接");
        // 内容面板 = 目标 B 的内容条目（b.json）。
        let rows = build_entry_rows_for(&e, link_idx);
        assert!(
            rows.iter().any(|v| v.path == "b.json"),
            "硬链接内容面板应显示目标内容（b.json）"
        );
        // 真实位置的 B 仍正常。
        assert!(e.rows.iter().any(|r| r.visit == b_idx && !r.hard && !r.mounted));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 挂载落点解析（菜单栏兜底路径）：选中经由硬链接行进入（sel_hard，选中 =
    /// 目标真身）时，`selected_hard_link_idx` 必须解析到硬链接**自身分支**；
    /// 软链接行不参与解析（返回 None）。
    #[test]
    fn selected_hard_link_resolves_to_own_branch() {
        // 硬链接：选中 = 目标 B ⇒ 落点 = 自身分支。
        let dir = bundle_with_mount("hardresolve", None, true);
        let e = opened(&dir);
        let b_idx = e.visit_idx(RB).expect("B");
        let link_idx = e
            .visit_idx(&format!("{RA}/{RL}"))
            .expect("硬链接分支自身是 visit");
        assert_eq!(
            selected_hard_link_idx(&e, b_idx),
            Ok(Some(link_idx)),
            "sel_hard 兜底落点 = 硬链接自身分支（不是目标真身）"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // 软链接：没有硬链接挂载指向 B ⇒ None（调用方按普通分支落点处理）。
        let dir = bundle_with_mount("softresolve", None, false);
        let e = opened(&dir);
        let b_idx = e.visit_idx(RB).expect("B");
        assert_eq!(
            selected_hard_link_idx(&e, b_idx),
            Ok(None),
            "软链接行不参与硬链接落点解析"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 摘引用（`unmount_link`）：软挂载只从挂载点 `entries[]` 移除 link 行 ——
    /// 目标分支目录、`.str.toml` 与内容完好（回归：菜单栏 / 信息页删除入口曾在
    /// 挂载行选中态下误删目标真身）。
    #[test]
    fn unmount_link_keeps_target_branch() {
        let dir = bundle_with_mount("softunmount", None, false);
        let mut e = opened(&dir);
        let a_idx = e.visit_idx(RA).expect("A");
        let b_idx = e.visit_idx(RB).expect("B");
        let b_dir = e.scan.as_ref().unwrap().visits[b_idx].dir.clone();
        let (_pt, _dt) = unmount_link(&mut e, a_idx, b_idx).expect("摘引用成功");
        // 目标分支完好：目录、`.str.toml` 与内容文件都在，且仍可解析。
        assert!(b_dir.join(".str.toml").exists(), "目标分支 `.str.toml` 仍在");
        assert!(b_dir.join("b.json").exists(), "目标内容仍在");
        assert!(
            e.visit_idx(RB).is_some(),
            "目标分支仍在 bundle 内可解析"
        );
        // 挂载点 entries[] 已无 link 行。
        let a_dir = e.scan.as_ref().unwrap().visits[e.visit_idx(RA).unwrap()]
            .dir
            .clone();
        let a_meta = read_meta(e.bundle.as_ref().unwrap(), &a_dir).unwrap();
        assert!(
            a_meta.entries.iter().all(|en| !en.is_link()),
            "挂载点的 link 行已摘除"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重挂载（软）：链接声明从旧挂载点摘除、登记到新挂载点 —— 目标分支目录 /
    /// 内容不动；自我挂载按环拒绝。
    #[test]
    fn mount_move_soft_remounts_without_touching_target() {
        let dir = bundle_with_mount("mmove-soft", None, false);
        let mut e = opened(&dir);
        let a_idx = e.visit_idx(RA).expect("A");
        let b_idx = e.visit_idx(RB).expect("B");
        let b_dir = e.scan.as_ref().unwrap().visits[b_idx].dir.clone();
        let root_idx = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .position(|v| v.depth == 0)
            .expect("ROOT");
        let root_dir = e.scan.as_ref().unwrap().visits[root_idx].dir.clone();
        // 自我挂载 = 环。
        assert!(mount_move_into_child(&mut e, a_idx, b_idx, b_idx).is_err());
        // 重挂载到 ROOT：B 已是 ROOT 的真实子分支（同 path）⇒ 冲突拒绝，
        // 真身登记不被链接行顶替。
        assert!(mount_move_into_child(&mut e, a_idx, b_idx, root_idx).is_err());
        // 造第三个分支 C（ROOT 下）作为无冲突重挂载目标。
        let bundle = e.bundle.as_ref().unwrap().clone();
        let c_id = "01928f3a-7c4b-4000-8000-000000000009";
        let c_dir = root_dir.join(c_id);
        std::fs::create_dir_all(&c_dir).unwrap();
        std::fs::write(
            bundle.meta_path(&c_dir),
            meta_edit::render_branch_meta(
                Kind::Node,
                c_id,
                None,
                Some("C"),
                None,
                &util::now_rfc3339(),
            ),
        )
        .unwrap();
        let mut rm = read_meta(&bundle, &root_dir).unwrap();
        rm.upsert_entry(&Entry {
            path: c_id.to_string(),
            role: "node".into(),
            id: Some(c_id.to_string()),
            title: Some("C".into()),
            ..Default::default()
        });
        rm.touch();
        rm.save(&bundle.meta_path(&root_dir)).unwrap();
        e.rescan().unwrap();
        // rescan 后 visit 下标漂移，全部重取。
        let a_idx = e.visit_idx(RA).expect("A");
        let b_idx = e.visit_idx(RB).expect("B");
        let c_idx = e.visit_idx(c_id).expect("C");
        // 重挂载到 C 之下。
        mount_move_into_child(&mut e, a_idx, b_idx, c_idx).expect("重挂载成功");
        let bundle = e.bundle.as_ref().unwrap();
        let c_meta = read_meta(bundle, &c_dir).unwrap();
        assert!(
            c_meta
                .entries
                .iter()
                .any(|en| en.is_link() && en.target.as_deref() == Some(RB)),
            "C 获得指向 B 的挂载声明"
        );
        let a_dir = e.scan.as_ref().unwrap().visits[e.visit_idx(RA).unwrap()]
            .dir
            .clone();
        let a_meta = read_meta(bundle, &a_dir).unwrap();
        assert!(
            a_meta.entries.iter().all(|en| !en.is_link()),
            "旧挂载点的声明已摘除"
        );
        assert!(b_dir.join(".str.toml").exists(), "目标分支完好");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重挂载（硬）：自有目录随挂载点搬家（`.str.toml` 的 target 不变），旧挂载点
    /// 摘登记、新挂载点登记；目标分支与数据不动。
    #[test]
    fn mount_move_hard_relocates_own_dir() {
        let dir = bundle_with_mount("mmove-hard", None, true);
        let mut e = opened(&dir);
        let a_idx = e.visit_idx(RA).expect("A");
        let root_idx = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .position(|v| v.depth == 0)
            .expect("ROOT");
        let root_dir = e.scan.as_ref().unwrap().visits[root_idx].dir.clone();
        let a_dir = e.scan.as_ref().unwrap().visits[a_idx].dir.clone();
        let b_idx = e.visit_idx(RB).unwrap();
        mount_move_into_child(&mut e, a_idx, b_idx, root_idx).expect("重挂载成功");
        let new_dir = root_dir.join(RL);
        assert!(new_dir.join(".str.toml").exists(), "硬链接自有目录已随迁");
        assert!(!a_dir.join(RL).exists(), "旧位置目录已移走");
        let bundle = e.bundle.as_ref().unwrap();
        // target 绑定在新挂载点（ROOT）的 link 声明行上，且 mode = hard 保留。
        let root_meta = read_meta(bundle, &root_dir).unwrap();
        let link_row = root_meta
            .entries
            .iter()
            .find(|en| en.is_link())
            .expect("ROOT 获得硬链接声明行");
        assert_eq!(link_row.target.as_deref(), Some(RB), "target 绑定不变");
        assert_eq!(link_row.mode.as_deref(), Some("hard"), "mode = hard 保留");
        assert_eq!(
            link_row.order,
            Some(0),
            "重挂载插入为第一个子分支（ROOT 现有结构行均无 order，连续编号从 0 起）"
        );
        assert!(e.visit_idx(RB).is_some(), "目标分支完好");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同挂载点内重排：链接声明行在结构行序列（branch ∪ link）中移位并重编
    /// order；目标分支与数据不动；跨挂载点的真实行不是合法重排目标。
    #[test]
    fn mount_reorder_moves_link_row_among_structure_rows() {
        let dir = bundle_with_mount("mreorder", None, false);
        let mut e = opened(&dir);
        let a_idx = e.visit_idx(RA).expect("A");
        let b_idx = e.visit_idx(RB).expect("B");
        // 在 A 下手工加一个真实子分支 C：A 的结构行 = [link B, branch C]。
        let bundle = e.bundle.as_ref().unwrap().clone();
        let a_dir = e.scan.as_ref().unwrap().visits[a_idx].dir.clone();
        let cid = "01928f3a-7c4b-4000-8000-000000000009";
        let c_dir = a_dir.join(cid);
        std::fs::create_dir_all(&c_dir).unwrap();
        std::fs::write(
            bundle.meta_path(&c_dir),
            meta_edit::render_branch_meta(
                Kind::Branch,
                cid,
                None,
                Some("C"),
                None,
                &util::now_rfc3339(),
            ),
        )
        .unwrap();
        let mut am = read_meta(&bundle, &a_dir).unwrap();
        am.upsert_entry(&Entry {
            path: cid.to_string(),
            role: "branch".into(),
            id: Some(cid.to_string()),
            title: Some("C".into()),
            ..Default::default()
        });
        am.touch();
        am.save(&bundle.meta_path(&a_dir)).unwrap();
        e.rescan().unwrap();
        let b_link_path = visit_dir_name(e.scan.as_ref().unwrap(), b_idx).unwrap();
        // 拖到自身 = 原位（顺序未变化）。
        assert!(mount_reorder(&mut e, a_idx, &b_link_path, &b_link_path, true).is_ok());
        // link B → C **之前**（after = false）：B 的 order 应小于 C。
        //（am.save 的 canonical 排序后 C 已排在 link B 之前，「插到 C 之后」= 原位。）
        mount_reorder(&mut e, a_idx, &b_link_path, cid, false).expect("重排成功");
        let a_meta = read_meta(&bundle, &a_dir).unwrap();
        let b_order = a_meta
            .entries
            .iter()
            .find(|en| en.is_link() && en.target.as_deref() == Some(RB))
            .and_then(|en| en.order)
            .expect("link B 仍有 order");
        let c_order = a_meta
            .entries
            .iter()
            .find(|en| en.path == cid)
            .and_then(|en| en.order)
            .expect("C 仍有 order");
        assert!(b_order < c_order, "link B 已排到 C 之前");
        // 下半区动作枚举：兄弟真实行 C = 1（重排到它之后）；ROOT 下半区 = 0
        //（B 已是 ROOT 的真实子分支，同 path 冲突）；目标自身 = 0（自我挂载成环）。
        let c_visit = e.visit_idx(&format!("{RA}/{cid}")).expect("C");
        assert_eq!(
            tree_mount_drop_ok(&e, a_idx, b_idx, c_visit, false, false, -1, -1, true, -1),
            1,
            "兄弟真实行下半区 = 同挂载点内重排"
        );
        let root_idx = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .position(|v| v.depth == 0)
            .unwrap();
        assert_eq!(
            tree_mount_drop_ok(&e, a_idx, b_idx, root_idx, false, false, -1, -1, true, -1),
            0,
            "同 path 冲突（目标已是 ROOT 真实子分支）拒绝"
        );
        assert_eq!(
            tree_mount_drop_ok(&e, a_idx, b_idx, b_idx, false, false, -1, -1, true, -1),
            0,
            "自我挂载拒绝"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 上半区嵌套 = 插入为**第一个子分支**：移入的子分支排最前（order 0），
    /// 既有结构行按相对顺序连续重排 —— 全区 `order` 均 ≥ 0（规范 §4.6）。
    #[test]
    fn branch_move_into_child_inserts_as_first_child() {
        let dir = bundle_with_mount("firstchild", None, true);
        let mut e = opened(&dir);
        let a_dir = e.scan.as_ref().unwrap().visits[e.visit_idx(RA).unwrap()]
            .dir
            .clone();
        // 给 A 的硬链接声明行设 order = 5，验证移入的子分支排到它前面。
        let bundle = e.bundle.as_ref().unwrap().clone();
        let mut am = read_meta(&bundle, &a_dir).unwrap();
        let mut link = am.entries.iter().find(|en| en.is_link()).cloned().unwrap();
        link.order = Some(5);
        am.upsert_entry(&link);
        am.touch();
        am.save(&bundle.meta_path(&a_dir)).unwrap();
        e.rescan().unwrap();
        let a_idx = e.visit_idx(RA).unwrap();
        let b_idx = e.visit_idx(RB).unwrap();
        branch_move_into_child(&mut e, b_idx, a_idx).expect("移入成功");
        let a_meta = read_meta(&bundle, &a_dir).unwrap();
        let b_order = a_meta
            .entries
            .iter()
            .find(|en| en.path == RB)
            .and_then(|en| en.order)
            .expect("移入的 B 已登记");
        assert_eq!(b_order, 0, "移入的子分支 = 第一个子分支（编号从 0 起）");
        let link_order = a_meta
            .entries
            .iter()
            .find(|en| en.is_link())
            .and_then(|en| en.order)
            .expect("既有声明行仍在");
        assert_eq!(link_order, 1, "既有结构行按相对顺序重排为 1");
        assert!(
            a_meta
                .entries
                .iter()
                .filter(|en| en.is_branch() || en.is_link())
                .all(|en| en.order.unwrap_or(0) >= 0),
            "全部 order ≥ 0（规范 §4.6）：{:?}",
            a_meta.entries
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归：反复「移为第一个子分支」不得让 `order` 走到负值 —— 旧实现用
    /// `min - 1` 抢位，四次操作即产生 -1（现场已出现）；新实现连续编号 0..n-1。
    #[test]
    fn make_first_branch_never_produces_negative_order() {
        let text = meta_text(
            "branch",
            RA,
            &format!(
                "[[entries]]\npath = \"x\"\nrole = \"branch\"\nid = \"x\"\norder = 3\n\
                 [[entries]]\npath = \"y\"\nrole = \"branch\"\nid = \"y\"\norder = 5\n\
                 [[entries]]\npath = \"z\"\nrole = \"branch\"\nid = \"z\"\n"
            ),
        );
        let mut m = meta_edit::meta_from_text(&text).unwrap();
        for _ in 0..4 {
            make_first_branch(&mut m, "z");
        }
        let mut orders: Vec<(String, i64)> = m
            .entries
            .iter()
            .filter(|en| en.is_branch())
            .map(|en| (en.path.clone(), en.order.unwrap_or(i64::MIN)))
            .collect();
        orders.sort_by_key(|(_, o)| *o);
        assert_eq!(
            orders,
            vec![
                ("z".to_string(), 0),
                ("x".to_string(), 1),
                ("y".to_string(), 2)
            ],
            "z 恒为第一个子分支，全区连续编号且 ≥ 0"
        );
    }

    /// ROOT 作为**关联线源**（ROOT 的 refs 指向 A），选中 A 时 ROOT 应入
    /// 强高亮集合（ref_mark → 导图节点 2px 粉边框）。
    #[test]
    fn root_as_ref_source_marks_root_node() {
        let dir = bundle_with_mount("rootrefdbg", None, false);
        let mut e = opened(&dir);
        let bundle = e.bundle.as_ref().unwrap().clone();
        let root_dir = e.scan.as_ref().unwrap().visits[0].dir.clone();
        let mut rm = read_meta(&bundle, &root_dir).unwrap();
        rm.push_ref(&RefItem {
            id: "01928f3a-7c4b-400b-8a0b-00000000000b".into(),
            target: RA.into(),
            rel: "related".into(),
            title: None,
            order: Some(1),
            note: None,
        });
        rm.touch();
        rm.save(&bundle.meta_path(&root_dir)).unwrap();
        e.rescan().unwrap();
        let a_idx = e.visit_idx(RA).unwrap();
        let rel_of = |i: usize| e.scan.as_ref().unwrap().visits[i].rel.clone();
        e.selected = Some(rel_of(a_idx));
        let marked = sel_ref_marked(&e);
        assert!(marked.contains(&0), "ROOT（关联线源）应高亮");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 导图 refs 关联线：A、B 都在布局内 → 画出一条虚线边，标签取 ref 的
    /// `title`（缺省回退 `rel`）；两端不同时可见则不画。
    #[test]
    fn mind_ref_edges_render_when_both_endpoints_visible() {
        let dir = bundle_with_mount("refedge", None, false);
        let mut e = opened(&dir);
        let a_dir = e.scan.as_ref().unwrap().visits[e.visit_idx(RA).unwrap()]
            .dir
            .clone();
        let bundle = e.bundle.as_ref().unwrap().clone();
        let mut am = read_meta(&bundle, &a_dir).unwrap();
        am.push_ref(&RefItem {
            id: "01928f3a-7c4b-4009-8a09-000000000009".into(),
            target: RB.into(),
            rel: "depends_on".into(),
            title: Some("关联标签".into()),
            order: Some(1),
            note: None,
        });
        am.touch();
        am.save(&bundle.meta_path(&a_dir)).unwrap();
        e.rescan().unwrap();

        let mind = mind_layout(&e);
        assert_eq!(mind.ref_edges.len(), 1, "{:?}", mind.ref_edges);
        assert_eq!(mind.ref_edges[0].label, "关联标签");
        assert!(mind.ref_edges[0].w > 0.0 && mind.ref_edges[0].h > 0.0);
        // 命令串含至少一段 M/L（虚线段）。
        assert!(
            mind.ref_edges[0].commands.contains("M ") && mind.ref_edges[0].commands.contains("L ")
        );
        // 高亮是**选中驱动**：选中 A（持有关联线）→ 高亮集 = {A, B}；无选中 → 空。
        // ref_mark 本身由 sync_ui 回填（布局缓存签名与选中无关），布局产物恒 false。
        let a_idx = e.visit_idx(RA).unwrap();
        let b_idx = e.visit_idx(RB).unwrap();
        assert!(
            mind.nodes.iter().all(|n| !n.ref_mark),
            "布局产物不携带选中高亮（由 sync_ui 回填）"
        );
        let rel_of = |i: usize| {
            e.scan
                .as_ref()
                .unwrap()
                .visits[i]
                .rel
                .clone()
        };
        e.selected = Some(rel_of(a_idx));
        let marked = sel_ref_marked(&e);
        assert!(
            marked.contains(&b_idx) && !marked.contains(&a_idx),
            "选中 A：被指向的 B 高亮（A 自身走选中态，不重复标记）"
        );
        e.selected = Some(rel_of(b_idx));
        let marked_b = sel_ref_marked(&e);
        assert!(marked_b.contains(&a_idx), "选中 B：指向 B 的源分支 A 应高亮");
        // 父级链（非常淡的一档）= 关联项的祖先链，但**整条排除选中分支自身的
        // 祖先链（含 ROOT）**：选中 A（refs 指向 B，B 在 ROOT 下）→
        // ancestors(B) = {ROOT, B}，其中 ROOT ∈ own（选中链）、B ∈ ref_set
        //（走强高亮）⇒ spine 为空。ROOT 仅在它**直接是关联项**（refs 的源）
        // 时才走强高亮（见 root_as_ref_source_marks_root_node）。
        e.selected = Some(rel_of(a_idx));
        let spine = sel_spine_marks(&e);
        assert!(!spine.contains(&0), "选中分支自身的父级链（含 ROOT）不高亮");
        assert!(!spine.contains(&b_idx), "关联项自身不入 spine（走 ref 强高亮）");
        e.selected = None;
        assert!(sel_ref_marked(&e).is_empty(), "无选中不高亮");
        assert!(sel_spine_marks(&e).is_empty(), "无选中无父级高亮");

        // 对话框行模型（双向）：选中被关联项 B → 应看到 A 指向 B 的**反向**
        // 关联线（incoming = true，对端 = A）；选中 A → 正向行。
        e.selected = Some(rel_of(b_idx));
        let (rows, _) = collect_ref_rows(&e);
        assert_eq!(rows.len(), 1, "被关联项应看到反向关联线：{rows:?}");
        assert!(rows[0].incoming, "该行应标记为反向");
        e.selected = Some(rel_of(a_idx));
        let (rows, _) = collect_ref_rows(&e);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].incoming, "选中 A 看到的是正向关联线");
        // 端点锚定在节点边界（而不是穿过节点的中心连线）：解码第一段虚线的起点，
        // 断言它落在源节点矩形的边界上（四边之一，容差 0.6px）。
        let a = mind.nodes.iter().find(|n| n.visit as usize == a_idx).unwrap();
        let re = &mind.ref_edges[0];
        let first: Vec<f32> = re
            .commands
            .split_whitespace()
            .filter_map(|tok| tok.parse::<f32>().ok())
            .take(2)
            .collect();
        assert_eq!(first.len(), 2, "命令串首段应为 `M x y`");
        let (p1x, p1y) = (re.x + first[0], re.y + first[1]);
        let on_border = (p1x - a.x).abs() < 0.6
            || (p1x - (a.x + a.w)).abs() < 0.6
            || (p1y - a.y).abs() < 0.6
            || (p1y - (a.y + a.h)).abs() < 0.6;
        assert!(on_border, "出锚点 ({p1x},{p1y}) 应在源节点边界上（A = {:?}）", (a.x, a.y, a.w, a.h));

        // 标签缺省回退 rel：清掉 title 再布局。
        let mut am2 = read_meta(&bundle, &a_dir).unwrap();
        am2.remove_ref("01928f3a-7c4b-4009-8a09-000000000009");
        am2.push_ref(&RefItem {
            id: "01928f3a-7c4b-400a-8a0a-00000000000a".into(),
            target: RB.into(),
            rel: "related".into(),
            title: None,
            order: Some(1),
            note: None,
        });
        am2.touch();
        am2.save(&bundle.meta_path(&a_dir)).unwrap();
        e.rescan().unwrap();
        let mind2 = mind_layout(&e);
        assert_eq!(mind2.ref_edges.len(), 1);
        assert_eq!(mind2.ref_edges[0].label, "related");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 落点补全：ROOT 下半区（真实分支）= 成为 ROOT 第一个子分支；挂载行落到
    /// 硬链接行上 = 重挂载到其**自有结构**首子位置（成环场景拒绝）。
    #[test]
    fn drop_targets_root_lower_and_hard_link_row() {
        let dir = bundle_with_mount("droptgt", None, true);
        let mut e = opened(&dir);
        let a_idx = e.visit_idx(RA).expect("A");
        let b_idx = e.visit_idx(RB).expect("B");
        let root_idx = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .position(|v| v.depth == 0)
            .expect("ROOT");
        // ① 真实分支 → ROOT 下半区：有效（after ROOT 无意义，改道首子插入）。
        assert!(tree_drop_ok(&e, b_idx, root_idx, true));
        branch_move_into_child(&mut e, b_idx, root_idx).expect("移入 ROOT 成功");
        let bundle = e.bundle.as_ref().unwrap().clone();
        let root_dir = e.scan.as_ref().unwrap().visits[root_idx].dir.clone();
        let b_order = read_meta(&bundle, &root_dir)
            .unwrap()
            .entries
            .iter()
            .find(|en| en.path == RB)
            .and_then(|en| en.order)
            .expect("B 已登记为 ROOT 子分支");
        let _ = b_order;
        // ② 挂载行 → 硬链接行下半区 = 重挂载到硬链接自有结构首子。
        // 手工造第三个分支 C（ROOT 下）+ C 的挂载（挂在 B 下）。
        let c_id = "01928f3a-7c4b-4000-8000-000000000009";
        let c_dir = root_dir.join(c_id);
        std::fs::create_dir_all(&c_dir).unwrap();
        std::fs::write(
            bundle.meta_path(&c_dir),
            meta_edit::render_branch_meta(
                Kind::Node,
                c_id,
                None,
                Some("C"),
                None,
                &util::now_rfc3339(),
            ),
        )
        .unwrap();
        let mut rm = read_meta(&bundle, &root_dir).unwrap();
        rm.upsert_entry(&Entry {
            path: c_id.to_string(),
            role: "node".into(),
            id: Some(c_id.to_string()),
            title: Some("C".into()),
            ..Default::default()
        });
        rm.touch();
        rm.save(&bundle.meta_path(&root_dir)).unwrap();
        e.rescan().unwrap();
        let b_idx = e.visit_idx(RB).unwrap();
        let b_dir = e.scan.as_ref().unwrap().visits[b_idx].dir.clone();
        let mut bm = read_meta(&bundle, &b_dir).unwrap();
        bm.upsert_entry(&Entry {
            path: c_id.to_string(),
            role: "link".into(),
            target: Some(c_id.to_string()),
            ..Default::default()
        });
        bm.touch();
        bm.save(&bundle.meta_path(&b_dir)).unwrap();
        e.rescan().unwrap();
        let c_idx = e.visit_idx(c_id).expect("C");
        let rl_idx = e
            .visit_idx(&format!("{RA}/{RL}"))
            .expect("硬链接自有分支");
        // 硬链接行（target = B，自有 = RL）：下半区 = 重挂载到 RL 自有结构首子。
        let hard_row_visit = b_idx; // 硬链接行身份 = 目标 B
        assert_eq!(
            tree_mount_drop_ok(
                &e,
                b_idx,  // 被拖挂载的挂载点 = B
                c_idx,  // 被拖挂载的目标 = C
                hard_row_visit,
                false,  // 硬链接行 mounted = false
                true,   // hard = true
                a_idx as i32,
                rl_idx as i32,
                true,
                -1,     // 被拖的是软挂载，无自有结构
            ),
            2,
            "硬链接行下半区 = 重挂载到自有结构首子"
        );
        // 拖硬链接行到**它自己**：落点解析为自有结构 = 自嵌套，拒绝（上半区
        // 不得点亮、落盘被守卫拦截）。
        assert!(!tree_mount_nest_ok(
            &e, b_idx, b_idx, true, rl_idx as i32, rl_idx as i32
        ));
        mount_move_into_child(&mut e, b_idx, c_idx, rl_idx).expect("重挂载成功");
        let rl_dir = e.scan.as_ref().unwrap().visits
            [e.visit_idx(&format!("{RA}/{RL}")).unwrap()]
        .dir
        .clone();
        let rl_meta = read_meta(&bundle, &rl_dir).unwrap();
        assert!(
            rl_meta
                .entries
                .iter()
                .any(|en| en.is_link() && en.target.as_deref() == Some(c_id)),
            "硬链接自有结构获得指向 C 的挂载声明"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 挂载行展开键实例独立：与真身行的展开键不同 —— 展开收起挂载行不再连带
    /// 真身与兄弟挂载视图（回归：展开态曾按目标 rel 共享）。
    #[test]
    fn mounted_row_expand_key_is_instance_scoped() {
        let dir = bundle_with_mount("expkeys", None, false);
        let e = opened(&dir);
        let b_idx = e.visit_idx(RB).expect("B");
        let mount_row = e.rows.iter().find(|r| r.mounted).expect("挂载行");
        let real_row = e
            .rows
            .iter()
            .find(|r| r.visit == b_idx && !r.mounted)
            .expect("真身行");
        assert_ne!(
            mount_row.expand_key, real_row.expand_key,
            "挂载行与真身行的展开键相互独立"
        );
        assert_eq!(real_row.expand_key, RB, "真实行展开键 = 自身 rel");
        assert!(
            mount_row.expand_key.contains("mount"),
            "挂载行展开键为实例键"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同一挂载点下指向同一目标的软 / 硬链接对：实例展开键互不相同（回归：
    /// 实例键未含软硬维度时，二者联动展开 / 收起）。
    #[test]
    fn soft_and_hard_pair_have_distinct_expand_keys() {
        let dir = bundle_with_mount("pairkeys", None, false);
        let mut e = opened(&dir);
        // 在 A 下再挂一条指向 B 的硬链接（path = 自身 id，与软链接 path 不同 ⇒ 合法）。
        let bundle = e.bundle.as_ref().unwrap().clone();
        let a_idx = e.visit_idx(RA).expect("A");
        let a_dir = e.scan.as_ref().unwrap().visits[a_idx].dir.clone();
        let hid = "01928f3a-7c4b-4000-8000-00000000000a";
        let h_dir = a_dir.join(hid);
        std::fs::create_dir_all(&h_dir).unwrap();
        std::fs::write(
            bundle.meta_path(&h_dir),
            meta_edit::render_branch_meta(
                Kind::Branch,
                hid,
                None,
                Some("B"),
                None,
                &util::now_rfc3339(),
            ),
        )
        .unwrap();
        let mut am = read_meta(&bundle, &a_dir).unwrap();
        am.upsert_entry(&Entry {
            path: hid.to_string(),
            role: "link".into(),
            id: Some(hid.to_string()),
            target: Some(RB.to_string()),
            mode: Some("hard".into()),
            title: Some("B".into()),
            ..Default::default()
        });
        am.touch();
        am.save(&bundle.meta_path(&a_dir)).unwrap();
        e.rescan().unwrap();
        let b_idx = e.visit_idx(RB).expect("B");
        let soft_key = e
            .rows
            .iter()
            .find(|r| r.mounted)
            .map(|r| r.expand_key.clone())
            .expect("软挂载行");
        let hard_key = e
            .rows
            .iter()
            .find(|r| r.hard)
            .map(|r| r.expand_key.clone())
            .expect("硬链接行");
        let real_key = e
            .rows
            .iter()
            .find(|r| r.visit == b_idx && !r.mounted && !r.hard)
            .map(|r| r.expand_key.clone())
            .expect("真身行");
        assert_ne!(soft_key, hard_key, "软 / 硬链接对实例键互不关联");
        assert_ne!(real_key, soft_key, "真身行与软挂载行实例键互不关联");
        assert_ne!(real_key, hard_key, "真身行与硬链接行实例键互不关联");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 挂载实例子树的展开态独立（回归：挂载行展开曾按 rel 键记账 —— 真身视图
    /// 已展开的孙代会随挂载行展开直接显示到子孙级；实例子树内收起也会连带真身）。
    #[test]
    fn mount_instance_subtree_expand_is_scoped() {
        let dir = bundle_with_mount("instscope", None, false);
        let mut e = opened(&dir);
        // 给 B 加一个真实子分支 C：真身视图里 C 是「已展开的孙代」。
        let bundle = e.bundle.as_ref().unwrap().clone();
        let b_idx = e.visit_idx(RB).expect("B");
        let b_dir = e.scan.as_ref().unwrap().visits[b_idx].dir.clone();
        let cid = "01928f3a-7c4b-4000-8000-00000000000b";
        let c_dir = b_dir.join(cid);
        std::fs::create_dir_all(&c_dir).unwrap();
        std::fs::write(
            bundle.meta_path(&c_dir),
            meta_edit::render_branch_meta(
                Kind::Branch,
                cid,
                None,
                Some("C"),
                None,
                &util::now_rfc3339(),
            ),
        )
        .unwrap();
        let mut bm = read_meta(&bundle, &b_dir).unwrap();
        bm.upsert_entry(&Entry {
            path: cid.to_string(),
            role: "branch".into(),
            id: Some(cid.to_string()),
            title: Some("C".into()),
            ..Default::default()
        });
        bm.touch();
        bm.save(&bundle.meta_path(&b_dir)).unwrap();
        e.rescan().unwrap();
        let c_rel = visit_key(
            e.scan.as_ref().unwrap(),
            e.visit_idx(&format!("{RB}/{cid}")).expect("C"),
        )
        .to_string();
        e.expanded.insert(c_rel.clone());
        e.rebuild();
        // 展开挂载实例（A 下的 ⤷B）。
        let a_idx = e.visit_idx(RA).expect("A");
        let inst_key = mount_expand_key(e.scan.as_ref().unwrap(), a_idx, b_idx, false);
        e.expanded.insert(inst_key.clone());
        e.rebuild();
        // 实例内的 C 行：键带实例前缀，**未展开**（不随真身视图联动到子孙级）。
        let inst_c_key = format!("{inst_key}\u{1f}{c_rel}");
        let inst_c_row = e
            .rows
            .iter()
            .find(|r| r.expand_key == inst_c_key)
            .expect("实例内的 C 行");
        assert!(
            !inst_c_row.expanded,
            "实例内 C 行收起：挂载行展开只显示一级"
        );
        // 真身 C 行保持展开，且与实例内 C 行的键互不相同。
        let real_c_row = e
            .rows
            .iter()
            .find(|r| r.expand_key == c_rel)
            .expect("真身 C 行");
        assert!(real_c_row.expanded, "真身视图的 C 行保持展开");
        // 实例内展开 C / 真身收起 C：互不影响。
        e.expanded.insert(inst_c_key.clone());
        e.expanded.remove(&c_rel);
        e.rebuild();
        assert!(
            e.rows
                .iter()
                .find(|r| r.expand_key == inst_c_key)
                .unwrap()
                .expanded,
            "实例内 C 独立展开"
        );
        assert!(
            !e.rows
                .iter()
                .find(|r| r.expand_key == c_rel)
                .unwrap()
                .expanded,
            "真身 C 独立收起"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 导图布局：多挂载实例 + 实例内展开的混合态下，节点矩形不得互相重叠
    /// （回归：实例上下文带高与渲染不一致时子带溢出、兄弟子树互相压盖）。
    #[test]
    fn mind_layout_no_overlap_with_instance_expansions() {
        // ROOT ─ R1 ─ A ─ A1；R1 下挂 ⤷A、⤷B；R2 下挂 ⤷A；B 下挂 ⤷A。
        let root = std::env::temp_dir().join(format!(
            "strgui-mindlay-{}.str",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let r1 = "01928f3a-7c4b-4000-8000-0000000000c1";
        let r2 = "01928f3a-7c4b-4000-8000-0000000000c2";
        let a1 = "01928f3a-7c4b-4000-8000-0000000000c3";
        let r3 = "01928f3a-7c4b-4000-8000-0000000000c4";
        let hl = "01928f3a-7c4b-4000-8000-0000000000c5";
        for d in [
            r1,
            &format!("{r1}/{RA}"),
            &format!("{r1}/{RA}/{a1}"),
            r2,
            RB,
            &format!("{RB}/{hl}"),
            r3,
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(
            root.join(".str.toml"),
            meta_text(
                "root",
                "01928f3a-7c4b-4000-8000-000000000003",
                &format!(
                    "[[entries]]\npath = \"{r1}\"\nrole = \"node\"\nid = \"{r1}\"\n\
                     [[entries]]\npath = \"{r2}\"\nrole = \"node\"\nid = \"{r2}\"\n\
                     [[entries]]\npath = \"{RB}\"\nrole = \"node\"\nid = \"{RB}\"\n\
                     [[entries]]\npath = \"{r3}\"\nrole = \"node\"\nid = \"{r3}\"\n"
                ),
            ),
        )
        .unwrap();
        let link_a = format!("[[entries]]\npath = \"{RA}\"\nrole = \"link\"\ntarget = \"{RA}\"\n");
        std::fs::write(
            root.join(r1).join(".str.toml"),
            meta_text(
                "node",
                r1,
                &format!(
                    "[[entries]]\npath = \"{RA}\"\nrole = \"branch\"\nid = \"{RA}\"\n\
                     [[entries]]\npath = \"{RB}\"\nrole = \"link\"\ntarget = \"{RB}\"\n"
                ),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(r1).join(RA).join(".str.toml"),
            meta_text(
                "branch",
                RA,
                &format!("[[entries]]\npath = \"{a1}\"\nrole = \"branch\"\nid = \"{a1}\"\n"),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(r1).join(RA).join(a1).join(".str.toml"),
            meta_text("branch", a1, "title = \"A1\"\n"),
        )
        .unwrap();
        std::fs::write(
            root.join(r2).join(".str.toml"),
            meta_text("node", r2, &link_a),
        )
        .unwrap();
        // B：硬链接 ≡A（自有目录 hl）+ 软挂载 ⤷A。
        std::fs::write(
            root.join(RB).join(".str.toml"),
            meta_text(
                "node",
                RB,
                &format!(
                    "{link_a}[[entries]]\npath = \"{hl}\"\nrole = \"link\"\nid = \"{hl}\"\n\
                     target = \"{RA}\"\nmode = \"hard\"\n"
                ),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(RB).join(hl).join(".str.toml"),
            meta_text("branch", hl, "title = \"硬A\"\n"),
        )
        .unwrap();
        std::fs::write(
            root.join(r3).join(".str.toml"),
            meta_text("node", r3, "title = \"R3\"\n"),
        )
        .unwrap();
        let mut e = opened(&root);
        let scan = e.scan.as_ref().unwrap();
        let idx = |rel: &str| scan.visits.iter().position(|v| v.rel == rel).unwrap();
        let (r1_i, r2_i, a_i, b_i) =
            (idx(r1), idx(r2), idx(&format!("{r1}/{RA}")), idx(RB));
        // 实例展开键 + 实例内嵌套展开（⤷B 实例内的 ⤷A 及其 A1 都展开）。
        let i_b_r1 = mount_expand_key(&scan, r1_i, b_i, false);
        let i_a_r2 = mount_expand_key(&scan, r2_i, a_i, false);
        let i_a_b = mount_expand_key(&scan, b_i, a_i, false);
        let a1_rel = visit_key(&scan, idx(&format!("{r1}/{RA}/{a1}"))).to_string();
        let nested_a_in_b = format!(
            "{i_b_r1}\u{1f}{}",
            mount_expand_key(&scan, b_i, a_i, false)
        );
        // 穷举所有展开键子集：任何组合下布局都不得出现节点矩形重叠。
        let hl_i = idx(&format!("{RB}/{hl}"));
        let i_hard_b = mount_expand_key(&scan, b_i, hl_i, true);
        let mut universe: Vec<String> = scan.visits.iter().map(|v| v.rel.clone()).collect();
        universe.push(i_b_r1.clone());
        universe.push(i_a_r2.clone());
        universe.push(i_a_b.clone());
        universe.push(nested_a_in_b.clone());
        universe.push(i_hard_b.clone());
        universe.push(format!("{i_a_r2}\u{1f}{a1_rel}"));
        universe.push(format!("{i_a_b}\u{1f}{a1_rel}"));
        universe.push(format!("{nested_a_in_b}\u{1f}{a1_rel}"));
        universe.sort();
        universe.dedup();
        assert!(universe.len() <= 20, "穷举规模失控");
        for mask in 0..(1u32 << universe.len()) {
            e.expanded = universe
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, k)| k.clone())
                .collect();
            e.rebuild();
            let out = mind_layout(&e);
            for (i, a) in out.nodes.iter().enumerate() {
                for b in &out.nodes[i + 1..] {
                    let ox = (a.x + a.w - b.x).min(b.x + b.w - a.x);
                    let oy = (a.y + a.h - b.y).min(b.y + b.h - a.y);
                    assert!(
                        ox <= 0.5 || oy <= 0.5,
                        "mask={mask:#06b} 节点矩形重叠：「{}」{:?} 与「{}」{:?}\nkeys={:?}",
                        a.title,
                        (a.x, a.y, a.w, a.h),
                        b.title,
                        (b.x, b.y, b.w, b.h),
                        e.expanded
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 导图布局模糊测试：随机真实树 + 随机无环挂载（软 / 硬）+ 随机展开组合
    /// （rel 键 + 实例键 + 嵌套实例键），断言节点矩形两两不重叠。
    #[test]
    fn mind_layout_fuzz_no_overlap() {
        // 简易可复现 RNG。
        let mut seed: u64 = 0x5EED_2026_0929;
        let mut rng = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        // 固定 id 池（合法 UUID 尾组）。
        let ids: Vec<String> = (0..24)
            .map(|i| format!("01928f3a-7c4b-4000-8000-{i:012}"))
            .collect();
        for case in 0..200u64 {
            let root = std::env::temp_dir().join(format!("strgui-fuzz-{}.str", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            // ── 随机真实树：ROOT 下挂 2-4 个一级，每个一级再随机挂 0-2 个二级 ──
            let mut tree: Vec<(String, Option<String>)> = Vec::new(); // (id, parent)
            let top_n = 2 + (rng() % 3) as usize;
            for i in 0..top_n {
                tree.push((ids[i].clone(), None));
            }
            for i in top_n..ids.len() {
                if rng() % 3 == 0 {
                    continue;
                }
                let parent = &ids[(rng() % top_n as u64) as usize];
                tree.push((ids[i].clone(), Some(parent.clone())));
            }
            for (id, parent) in &tree {
                let dir = match parent {
                    Some(p) => root.join(p).join(id),
                    None => root.join(id),
                };
                std::fs::create_dir_all(&dir).unwrap();
                let kind = if parent.is_none() { "node" } else { "branch" };
                std::fs::write(
                    dir.join(".str.toml"),
                    meta_text(kind, id, "title = \"N\"\n"),
                )
                .unwrap();
            }
            // ROOT meta：一级分支登记。
            let top_list: Vec<String> = tree
                .iter()
                .filter(|(_, p)| p.is_none())
                .map(|(id, _)| {
                    format!("[[entries]]\npath = \"{id}\"\nrole = \"node\"\nid = \"{id}\"\n")
                })
                .collect();
            std::fs::write(
                root.join(".str.toml"),
                meta_text("root", "01928f3a-7c4b-4000-8000-000000000000", &top_list.concat()),
            )
            .unwrap();
            // 各分支 meta：登记真实子分支。
            for (id, parent) in &tree {
                if parent.is_none() {
                    continue;
                }
                let kid_list: String = tree
                    .iter()
                    .filter(|(_, p)| p.as_deref() == Some(id.as_str()))
                    .map(|(kid, _)| {
                        format!("[[entries]]\npath = \"{kid}\"\nrole = \"branch\"\nid = \"{kid}\"\n")
                    })
                    .collect();
                let dir = match parent {
                    Some(p) => root.join(p).join(id),
                    None => root.join(id),
                };
                std::fs::write(
                    dir.join(".str.toml"),
                    meta_text("branch", id, &kid_list),
                )
                .unwrap();
            }
            // ── 随机挂载：软链接（无目录）。目标任取非自身 / 非祖先的分支 ──
            // 祖先判定走真实父子链（挂载环由构造避免：只挂到真实路径上不构成
            // 环的组合 —— 简化：目标与挂载点的真实路径不相交即挂）。
            let is_ancestor = |a: &str, b: &str| -> bool {
                // a 是否 b 的真实祖先（含相等）。
                let mut cur = Some(b.to_string());
                while let Some(c) = cur {
                    if c == a {
                        return true;
                    }
                    cur = tree
                        .iter()
                        .find(|(id, _)| *id == c)
                        .and_then(|(_, p)| p.clone());
                }
                false
            };
            let all_ids: Vec<String> = tree.iter().map(|(id, _)| id.clone()).collect();
            let dir_of = |id: &str| -> std::path::PathBuf {
                let mut parts = vec![id.to_string()];
                let mut cur = Some(id.to_string());
                loop {
                    let next = tree
                        .iter()
                        .find(|(id2, _)| *id2 == cur.as_deref().unwrap_or(""))
                        .and_then(|(_, p)| p.clone());
                    match next {
                        Some(p) => {
                            parts.push(p.clone());
                            cur = Some(p);
                        }
                        None => break,
                    }
                }
                parts.iter().rev().fold(root.clone(), |acc, p| acc.join(p))
            };
            let mut hl_seq = 90u64;
            for parent in &all_ids {
                let n_mounts = (rng() % 3) as usize;
                for _ in 0..n_mounts {
                    let target = &all_ids[(rng() % all_ids.len() as u64) as usize];
                    if is_ancestor(target, parent) || target == parent {
                        continue; // 环 / 自我挂载：跳过。
                    }
                    let meta_path = dir_of(parent).join(".str.toml");
                    let mut text = std::fs::read_to_string(&meta_path).unwrap();
                    // 同挂载点同目标只挂一次（软链接 path = 目标 id）。
                    if text.contains(&format!("path = \"{target}\"")) {
                        continue;
                    }
                    if rng() % 4 == 0 {
                        // 硬链接：path = id = 自有目录名（目录 + 自有 `_meta`，
                        // 可带自有子分支）。
                        hl_seq += 1;
                        let hl = format!("01928f3a-7c4b-4000-8000-{hl_seq:012}");
                        let hdir = dir_of(parent).join(&hl);
                        std::fs::create_dir_all(&hdir).unwrap();
                        let mut hl_tables = String::from("title = \"硬\"\n");
                        if rng() % 2 == 0 {
                            hl_seq += 1;
                            let hc = format!("01928f3a-7c4b-4000-8000-{hl_seq:012}");
                            std::fs::create_dir_all(hdir.join(&hc)).unwrap();
                            std::fs::write(
                                hdir.join(&hc).join(".str.toml"),
                                meta_text("branch", &hc, "title = \"硬子\"\n"),
                            )
                            .unwrap();
                            hl_tables.push_str(&format!(
                                "[[entries]]\npath = \"{hc}\"\nrole = \"branch\"\nid = \"{hc}\"\n"
                            ));
                        }
                        // 盲区覆盖：硬链接自有结构下的**软挂载**（回归：此处
                        // mount_parent 曾传错分支导致实例键不一致、子带溢出）。
                        if rng() % 2 == 0 {
                            let mtarget = &all_ids[(rng() % all_ids.len() as u64) as usize];
                            if mtarget != &hl && !is_ancestor(mtarget, parent) {
                                hl_tables.push_str(&format!(
                                    "[[entries]]\npath = \"{mtarget}\"\nrole = \"link\"\ntarget = \"{mtarget}\"\n"
                                ));
                            }
                        }
                        std::fs::write(hdir.join(".str.toml"), meta_text("branch", &hl, &hl_tables))
                            .unwrap();
                        text.push_str(&format!(
                            "[[entries]]\npath = \"{hl}\"\nrole = \"link\"\nid = \"{hl}\"\n\
                             target = \"{target}\"\nmode = \"hard\"\n"
                        ));
                    } else {
                        text.push_str(&format!(
                            "[[entries]]\npath = \"{target}\"\nrole = \"link\"\ntarget = \"{target}\"\n"
                        ));
                    }
                    std::fs::write(meta_path, text).unwrap();
                }
            }
            // ── 打开 + 随机展开 ──
            let mut e = opened(&root);
            let scan = e.scan.as_ref().unwrap();
            let mut universe: Vec<String> =
                scan.visits.iter().map(|v| v.rel.clone()).collect();
            // 一层实例键 + 一层嵌套（真实孩子 / 挂载孩子）。硬链接的渲染结构根
            // = **link 分支**（自有结构），不是挂载目标。
            for p in 0..scan.visits.len() {
                for m in e.mounts.get(p).map(|v| v.as_slice()).unwrap_or_default() {
                    let k = mount_expand_key(&scan, p, m.visit, m.hard);
                    universe.push(k.clone());
                    let structure_root = if m.hard {
                        m.link_idx.unwrap_or(m.visit)
                    } else {
                        m.visit
                    };
                    for &g in
                        e.kids.get(structure_root).map(|v| v.as_slice()).unwrap_or_default()
                    {
                        universe.push(format!("{k}\u{1f}{}", scan.visits[g].rel));
                    }
                    for m2 in
                        e.mounts.get(structure_root).map(|v| v.as_slice()).unwrap_or_default()
                    {
                        if m2.visit == structure_root {
                            continue;
                        }
                        let k2 = format!(
                            "{k}\u{1f}{}",
                            mount_expand_key(&scan, structure_root, m2.visit, m2.hard)
                        );
                        universe.push(k2.clone());
                        // 三层嵌套：实例里的实例里的真实孩子 / 挂载孩子。
                        for &g2 in
                            e.kids.get(m2.visit).map(|v| v.as_slice()).unwrap_or_default()
                        {
                            universe.push(format!("{k2}\u{1f}{}", scan.visits[g2].rel));
                        }
                        for m3 in
                            e.mounts.get(m2.visit).map(|v| v.as_slice()).unwrap_or_default()
                        {
                            if m3.visit == m2.visit {
                                continue;
                            }
                            universe.push(format!(
                                "{k2}\u{1f}{}",
                                mount_expand_key(&scan, m2.visit, m3.visit, m3.hard)
                            ));
                        }
                    }
                }
            }
            universe.sort();
            universe.dedup();
            for k in &universe {
                if rng() % 2 == 0 {
                    e.expanded.insert(k.clone());
                }
            }
            // 随机内容面板（影响节点高度，验证高度表与渲染一致）。
            for v in scan.visits.iter() {
                if rng() % 4 == 0 {
                    e.content_expanded.insert(v.rel.clone());
                }
            }
            e.rebuild();
            let out = mind_layout(&e);
            for (i, a) in out.nodes.iter().enumerate() {
                for b in &out.nodes[i + 1..] {
                    let ox = (a.x + a.w - b.x).min(b.x + b.w - a.x);
                    let oy = (a.y + a.h - b.y).min(b.y + b.h - a.y);
                    assert!(
                        ox <= 0.5 || oy <= 0.5,
                        "case={case} 节点矩形重叠：「{}」{:?} 与「{}」{:?}\nexpanded={:?}",
                        a.title,
                        (a.x, a.y, a.w, a.h),
                        b.title,
                        (b.x, b.y, b.w, b.h),
                        e.expanded
                    );
                }
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// 硬链接节点**自有结构下的软挂载**展开（回归：mind_dfs 曾把孩子的
    /// mount_parent 传成硬链接的挂载目标而非 link 分支，渲染的实例展开键与
    /// 带高计算用的键不一致 → 带高按收起、渲染按展开 → 子带溢出压盖兄弟）。
    #[test]
    fn mind_layout_soft_mount_under_hard_link_no_overflow() {
        // ROOT ─ R1 ─ { T(real), T2(real)→T2A, ≡HL→T(硬链接，自有结构挂 ⤷T2), S(real) }。
        let root = std::env::temp_dir().join(format!("strgui-hardmnt-{}.str", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let r1 = "01928f3a-7c4b-4000-8000-0000000000d1";
        let t = "01928f3a-7c4b-4000-8000-0000000000d2";
        let t2 = "01928f3a-7c4b-4000-8000-0000000000d3";
        let t2a = "01928f3a-7c4b-4000-8000-0000000000d4";
        let hl = "01928f3a-7c4b-4000-8000-0000000000d5";
        let s = "01928f3a-7c4b-4000-8000-0000000000d6";
        for d in [r1, &format!("{r1}/{t}"), &format!("{r1}/{t2}"), &format!("{r1}/{t2}/{t2a}"), &format!("{r1}/{hl}"), r1.to_string().as_str(), &format!("{r1}/{s}")] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(
            root.join(".str.toml"),
            meta_text(
                "root",
                "01928f3a-7c4b-4000-8000-000000000000",
                &format!("[[entries]]\npath = \"{r1}\"\nrole = \"node\"\nid = \"{r1}\"\n"),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(r1).join(".str.toml"),
            meta_text(
                "node",
                r1,
                &format!(
                    "[[entries]]\npath = \"{t}\"\nrole = \"branch\"\nid = \"{t}\"\n\
                     [[entries]]\npath = \"{t2}\"\nrole = \"branch\"\nid = \"{t2}\"\n\
                     [[entries]]\npath = \"{hl}\"\nrole = \"link\"\nid = \"{hl}\"\n\
                     target = \"{t}\"\nmode = \"hard\"\n\
                     [[entries]]\npath = \"{s}\"\nrole = \"branch\"\nid = \"{s}\"\n"
                ),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(r1).join(t).join(".str.toml"),
            meta_text("branch", t, "title = \"T\"\n"),
        )
        .unwrap();
        std::fs::write(
            root.join(r1).join(t2).join(".str.toml"),
            meta_text(
                "branch",
                t2,
                &format!("[[entries]]\npath = \"{t2a}\"\nrole = \"branch\"\nid = \"{t2a}\"\n"),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(r1).join(t2).join(t2a).join(".str.toml"),
            meta_text("branch", t2a, "title = \"T2A\"\n"),
        )
        .unwrap();
        std::fs::write(
            root.join(r1).join(s).join(".str.toml"),
            meta_text("branch", s, "title = \"S\"\n"),
        )
        .unwrap();
        // HL 自有结构：软挂载 ⤷T2。
        std::fs::write(
            root.join(r1).join(hl).join(".str.toml"),
            meta_text(
                "branch",
                hl,
                &format!("[[entries]]\npath = \"{t2}\"\nrole = \"link\"\ntarget = \"{t2}\"\n"),
            ),
        )
        .unwrap();
        let mut e = opened(&root);
        let scan = e.scan.as_ref().unwrap();
        let pos = |rel: &str| scan.visits.iter().position(|v| v.rel == rel).unwrap();
        let (r1_i, t_i, hl_i) = (pos(r1), pos(&format!("{r1}/{t}")), pos(&format!("{r1}/{hl}")));
        // 展开硬实例 + 其自有结构下的 ⤷T2（**link 分支** 为挂载点的正确键）。
        let hard_key = mount_expand_key(scan, r1_i, t_i, true);
        e.expanded.insert(hard_key.clone());
        let soft_key = format!(
            "{hard_key}\u{1f}{}",
            mount_expand_key(scan, hl_i, pos(&format!("{r1}/{t2}")), false)
        );
        e.expanded.insert(soft_key);
        e.rebuild();
        let out = mind_layout(&e);
        // 硬链接节点下应渲染出 ⤷T2，且 ⤷T2 展开渲染出 T2A。
        assert!(
            out.nodes.iter().any(|n| n.title.contains("⤷") && n.mount_parent == hl_i as i32),
            "硬链接自有结构下的 ⤷T2 应以 link 分支为 mount_parent 渲染"
        );
        for (i, a) in out.nodes.iter().enumerate() {
            for b in &out.nodes[i + 1..] {
                let ox = (a.x + a.w - b.x).min(b.x + b.w - a.x);
                let oy = (a.y + a.h - b.y).min(b.y + b.h - a.y);
                assert!(
                    ox <= 0.5 || oy <= 0.5,
                    "节点矩形重叠：「{}」{:?} 与「{}」{:?}",
                    a.title,
                    (a.x, a.y, a.w, a.h),
                    b.title,
                    (b.x, b.y, b.w, b.h)
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 性能基准（手动 --ignored 运行）：生成超大规模合成 bundle（约 500 真实
    /// 分支 + 300 软挂载 + 30 硬挂载，跨子树挂载），测量 open / rebuild /
    /// mind_layout（收起、真实全展开、含实例全展开）/ 展开全部 全链路耗时。
    #[test]
    #[ignore]
    fn probe_perf_large_bundle() {
        use std::collections::HashMap;
        use std::time::Instant;
        let root =
            std::env::temp_dir().join(format!("strgui-perf-large-{}.str", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let t_gen = Instant::now();
        // ── 结构：10 个一级，各 3→3→2→1 层，约 490 真实分支 ──
        let mut nodes: Vec<(String, Option<String>)> = Vec::new();
        let mut seq = 0usize;
        let mut next_id = || {
            seq += 1;
            format!("01928f3a-7c4b-4000-8000-{seq:012}")
        };
        let top = 10usize;
        let mut level: Vec<String> = Vec::new();
        for _ in 0..top {
            let id = next_id();
            nodes.push((id.clone(), None));
            level.push(id);
        }
        let mut all: Vec<String> = level.clone();
        for width in [3usize, 3, 2, 1] {
            let mut next_level = Vec::new();
            for p in &level {
                for _ in 0..width {
                    let id = next_id();
                    nodes.push((id.clone(), Some(p.clone())));
                    next_level.push(id.clone());
                    all.push(id.clone());
                }
            }
            level = next_level;
        }
        let parent_of: HashMap<&String, &Option<String>> =
            nodes.iter().map(|(id, p)| (id, p)).collect();
        let mut dirs: HashMap<String, std::path::PathBuf> = HashMap::new();
        for (id, parent) in &nodes {
            let dir = match parent {
                Some(p) => dirs[p].join(id),
                None => root.join(id),
            };
            std::fs::create_dir_all(&dir).unwrap();
            dirs.insert(id.clone(), dir);
        }
        // ── 挂载：每个深度 ≥1 分支向**别的子树**挂 1 条；10% 硬链接
        //（HL 自有结构下再挂一条软链接，覆盖硬链接自有挂载路径）。──
        let top_of = |parent_of: &HashMap<&String, &Option<String>>, id: &String| -> String {
            let mut cur = id;
            loop {
                match parent_of.get(cur) {
                    Some(Some(p)) => cur = p,
                    _ => return cur.clone(),
                }
            }
        };
        let mut soft: Vec<(String, String)> = Vec::new(); // (挂载点, 目标)
        let mut hard: Vec<(String, String, String)> = Vec::new(); // (宿主, 目标, HL id)
        for (i, id) in all.iter().enumerate() {
            let target = &all[(i * 7 + 3) % all.len()];
            // 跨一级子树挂载 ⇒ 无环；目标 ≠ 自身 / 祖先由跨子树保证。
            if top_of(&parent_of, id) == top_of(&parent_of, target) || target == id {
                continue;
            }
            if i % 10 == 0 {
                let hl = format!("01928f3a-7c4b-4000-8000-9{i:011}");
                let hdir = dirs[id].join(&hl);
                std::fs::create_dir_all(&hdir).unwrap();
                dirs.insert(hl.clone(), hdir.clone());
                // HL 自有结构：一条软挂载 + 一个自有子分支。
                let t2 = &all[(i * 11 + 5) % all.len()];
                if top_of(&parent_of, t2) != top_of(&parent_of, &hl) {
                    std::fs::write(
                        hdir.join(".str.toml"),
                        meta_text(
                            "branch",
                            &hl,
                            &format!(
                                "title = \"硬\"\n[[entries]]\npath = \"{t2}\"\nrole = \"link\"\ntarget = \"{t2}\"\n"
                            ),
                        ),
                    )
                    .unwrap();
                } else {
                    std::fs::write(hdir.join(".str.toml"), meta_text("branch", &hl, "title = \"硬\"\n"))
                        .unwrap();
                }
                hard.push((id.clone(), target.clone(), hl));
            } else {
                soft.push((id.clone(), target.clone()));
            }
        }
        // ── 落盘 `_meta` 与载荷 ──
        std::fs::write(
            root.join(".str.toml"),
            meta_text(
                "root",
                "01928f3a-7c4b-4000-8000-000000000000",
                &nodes
                    .iter()
                    .filter(|(_, p)| p.is_none())
                    .map(|(id, _)| {
                        format!("[[entries]]\npath = \"{id}\"\nrole = \"node\"\nid = \"{id}\"\n")
                    })
                    .collect::<String>(),
            ),
        )
        .unwrap();
        for (id, parent) in &nodes {
            let dir = &dirs[id];
            let kind = if parent.is_none() { "node" } else { "branch" };
            let mut tables = String::new();
            for (kid, kp) in &nodes {
                if kp.as_deref() == Some(id.as_str()) {
                    tables.push_str(&format!(
                        "[[entries]]\npath = \"{kid}\"\nrole = \"branch\"\nid = \"{kid}\"\n"
                    ));
                }
            }
            for (p, target) in &soft {
                if p == id {
                    tables.push_str(&format!(
                        "[[entries]]\npath = \"{target}\"\nrole = \"link\"\ntarget = \"{target}\"\n"
                    ));
                }
            }
            for (p, target, hl) in &hard {
                if p == id {
                    tables.push_str(&format!(
                        "[[entries]]\npath = \"{hl}\"\nrole = \"link\"\nid = \"{hl}\"\ntarget = \"{target}\"\nmode = \"hard\"\n"
                    ));
                }
            }
            tables.push_str(&format!(
                "[[entries]]\npath = \"data.json\"\nrole = \"payload\"\nsize = 12\nsha256 = \"{}\"\n",
                "0".repeat(64)
            ));
            std::fs::write(dir.join(".str.toml"), meta_text(kind, id, &tables)).unwrap();
            std::fs::write(dir.join("data.json"), "{\"v\":1}\n").unwrap();
        }
        eprintln!(
            "生成：{} 分支 / {} 软挂载 / {} 硬挂载 / 耗时 {:.1?}",
            nodes.len(),
            soft.len(),
            hard.len(),
            t_gen.elapsed()
        );

        // ── 测量 ──
        let t = Instant::now();
        let mut e = opened(&root);
        eprintln!("open             {:>8.1?}", t.elapsed());
        e.rebuild();
        eprintln!("rows(收起)       {}", e.rows.len());
        let t = Instant::now();
        let out = mind_layout(&e);
        eprintln!(
            "layout(收起)     {:>8.1?} nodes={}",
            t.elapsed(),
            out.nodes.len()
        );

        let t = Instant::now();
        let rel_keys: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.rel.clone())
            .collect();
        for k in &rel_keys {
            e.expanded.insert(k.clone());
        }
        e.rebuild();
        eprintln!("rows(真实全开)   {}", e.rows.len());
        let t = Instant::now();
        let out = mind_layout(&e);
        eprintln!(
            "layout(真实全开) {:>8.1?} nodes={}",
            t.elapsed(),
            out.nodes.len()
        );

        // 一层实例键。
        let scan = e.scan.as_ref().unwrap();
        let mut inst: Vec<String> = Vec::new();
        for p in 0..scan.visits.len() {
            for m in e.mounts.get(p).map(|v| v.as_slice()).unwrap_or_default() {
                inst.push(mount_expand_key(scan, p, m.visit, m.hard));
            }
        }
        inst.sort();
        inst.dedup();
        for k in &inst {
            e.expanded.insert(k.clone());
        }
        e.rebuild();
        let t = Instant::now();
        let out = mind_layout(&e);
        eprintln!(
            "layout(一层实例) {:>8.1?} nodes={} rows={}",
            t.elapsed(),
            out.nodes.len(),
            e.rows.len()
        );
        // 展开全部（含嵌套实例键）。
        let t = Instant::now();
        let all_keys = all_branch_keys(&e);
        eprintln!(
            "all_branch_keys  {:>8.1?} keys={}",
            t.elapsed(),
            all_keys.len()
        );
        for k in &all_keys {
            e.expanded.insert(k.clone());
        }
        e.rebuild();
        let t = Instant::now();
        let out = mind_layout(&e);
        eprintln!(
            "layout(全展开)   {:>8.1?} nodes={} rows={}",
            t.elapsed(),
            out.nodes.len(),
            e.rows.len()
        );
        // 供 STR_GUI_BENCH 基准模式复用时保留 bundle。
        if std::env::var("STR_BENCH_KEEP").is_err() {
            let _ = std::fs::remove_dir_all(&root);
        } else {
            eprintln!("bundle kept: {}", root.display());
        }
    }

    /// 导图同样渲染挂载节点（mounted = true），且同一 visit 可同时出现在
    /// 真实位置与挂载位置（DAG）。
    #[test]
    fn mount_node_renders_in_mind_layout() {
        let dir = bundle_with_mount("mind", None, false);
        let e = opened(&dir);
        let b_idx = e.visit_idx(RB).expect("B");
        let out = mind_layout(&e);
        let mounted: Vec<&MindNode> = out.nodes.iter().filter(|n| n.mounted).collect();
        assert_eq!(mounted.len(), 1, "应恰有一个挂载节点");
        assert_eq!(mounted[0].visit, b_idx as i32);
        assert!(out.nodes.iter().any(|n| n.visit == b_idx as i32 && !n.mounted));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 挂载进自己的祖先（B 下挂 A）：渲染必须终止（防环守卫），不得无限展开。
    #[test]
    fn mount_into_ancestor_does_not_recurse_forever() {
        let dir = bundle_with_mount("cycle", None, false);
        let mut e = opened(&dir);
        // 在 B 的 `.str.toml` 里追加一条指向 A 的软链接（A 是 B 的「父级」方向的分支）。
        let b_dir = e.scan.as_ref().unwrap().visits[e.visit_idx(RB).unwrap()].dir.clone();
        let p = b_dir.join(".str.toml");
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(
            &p,
            format!("{text}[[entries]]\npath = \"{RA}\"\nrole = \"link\"\ntarget = \"{RA}\"\n"),
        )
        .unwrap();
        e.rescan().unwrap();
        let keys: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.rel.clone())
            .collect();
        e.expanded = keys.into_iter().collect();
        e.rebuild();
        // 渲染必须终止且行数有限（无环守卫生效：这条挂载不再展开）。
        assert!(e.rows.len() < 64, "渲染应终止：{} 行", e.rows.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归（真机 SIGABRT：`clamp` 收到 min > max）：把**浅层目标**挂到**深层
    /// 挂载点**下时，`sub_hs` 单趟深度序递推会用陈旧（偏小）的目标高度，渲染时
    /// `total > band_h`。修复 = 带高递推到不动点 + 防御式夹取。
    #[test]
    fn mount_shallow_target_under_deep_node_does_not_panic() {
        // 结构：ROOT / A(1) → C(2)；B(1) → D(2)；并把 B 挂载到 C 下
        //（挂载点 C 深度 2 ＞ 目标 B 深度 1，且 B 展开后带高会增长）。
        let dir = std::env::temp_dir().join(format!(
            "strgui-mount-deep-{}.str",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let ra = "01928f3a-7c4b-4001-8a01-000000000004";
        let rb = "01928f3a-7c4b-4002-8a02-000000000005";
        let rc = "01928f3a-7c4b-4003-8a03-000000000006";
        let rd = "01928f3a-7c4b-4004-8a04-000000000007";
        // 注意：相对段不能以 `/` 开头 —— `Path::join` 会把它当绝对路径替换整个前缀。
        for d in [
            "".to_string(),
            ra.to_string(),
            format!("{ra}/{rc}"),
            rb.to_string(),
            format!("{rb}/{rd}"),
        ] {
            std::fs::create_dir_all(dir.join(&d)).unwrap();
        }
        std::fs::write(
            dir.join(".str.toml"),
            meta_text(
                "root",
                "01928f3a-7c4b-4000-8000-000000000003",
                &format!(
                    "[[entries]]\npath = \"{ra}\"\nrole = \"node\"\nid = \"{ra}\"\n\
                     [[entries]]\npath = \"{rb}\"\nrole = \"node\"\nid = \"{rb}\"\n"
                ),
            ),
        )
        .unwrap();
        // A 下挂载 B（挂载点在 C 更深处的场景由 C 承载 link —— 这里把 link 放 C）。
        std::fs::write(
            dir.join(ra).join(".str.toml"),
            meta_text(
                "node",
                ra,
                &format!("[[entries]]\npath = \"{rc}\"\nrole = \"branch\"\nid = \"{rc}\"\n"),
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join(ra).join(rc).join(".str.toml"),
            meta_text(
                "branch",
                rc,
                &format!("[[entries]]\npath = \"{rb}\"\nrole = \"link\"\ntarget = \"{rb}\"\n"),
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join(rb).join(".str.toml"),
            meta_text(
                "node",
                rb,
                &format!("[[entries]]\npath = \"{rd}\"\nrole = \"branch\"\nid = \"{rd}\"\n"),
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join(rb).join(rd).join(".str.toml"),
            meta_text("branch", rd, ""),
        )
        .unwrap();

        let mut e = Editor::new();
        e.open(&dir).expect("打开 bundle");
        let keys: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.rel.clone())
            .collect();
        e.expanded = keys.into_iter().collect();
        e.rebuild();
        // 修复前：此处 clamp(min>max) panic；修复后正常出布局，且含挂载节点。
        let out = mind_layout(&e);
        assert!(
            out.nodes.iter().any(|n| n.mounted),
            "挂载节点应被渲染（防环守卫不得误杀合法 DAG）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod recent_tests {
    use super::*;

    /// 最近列表只存**规范绝对路径**：历史遗留的相对条目（修复前 `str-gui .`
    /// 的产物）读取时直接丢弃；同一路径的两种写法视为同一条目（去重）。
    #[test]
    fn recent_normalizes_relative_paths_and_dedupes() {
        let cfg = std::env::temp_dir().join(format!(
            "strgui-recent-cfg-{}-{}",
            std::process::id(),
            std::path::Path::new(&format!("{:?}", std::thread::current().id()))
                .display()
                .to_string()
                .replace(['(', ')', ' ', '"'], "")
        ));
        let _ = std::fs::remove_dir_all(&cfg);
        std::fs::create_dir_all(&cfg).unwrap();
        // 历史遗留：相对路径行（`str-gui .` 时代的产物）+ 一条不存在的路径。
        std::fs::write(cfg.join("recent.txt"), ".\n/some/nonexistent/bundle.str\n").unwrap();
        let list = recent_paths_in(&cfg);
        assert!(
            list.is_empty(),
            "相对条目（原意图不可考）与不存在条目都应被剔除：{list:?}"
        );
        // 入库时相对形式升为规范绝对路径（模拟 `str-gui .` 的入参）。
        push_recent_in(&cfg, Path::new("."));
        let list = recent_paths_in(&cfg);
        assert_eq!(list.len(), 1);
        assert!(
            list[0].is_absolute() && list[0].is_dir(),
            "入库后应为规范的绝对路径：{:?}",
            list[0]
        );
        // 落盘内容不得残留相对路径。
        let stored = std::fs::read_to_string(cfg.join("recent.txt")).unwrap();
        assert!(
            !stored.lines().any(|l| l.trim() == "."),
            "落盘内容不得残留相对路径：{stored:?}"
        );
        let _ = std::fs::remove_dir_all(&cfg);
    }
}

#[cfg(test)]
mod mind_button_tests {
    use super::test_support::expanded_demo;
    use super::*;

    /// 回归（用户报告「导图视图展开分支按钮不能正确显示」）：`has-children` 曾用
    /// **展开后才填充**的孩子列表判定 ⇒ 折叠节点的外置「展开子树」按钮消失。
    /// 折叠的节点必须保留按钮（`has-children` 与展开态无关）。
    #[test]
    fn mind_node_keeps_expand_button_when_collapsed() {
        let mut e = expanded_demo();
        let idx = {
            let scan = e.scan.as_ref().unwrap();
            (1..scan.visits.len())
                .find(|&i| scan.visits[i].depth >= 1 && !e.kids[i].is_empty())
                .expect("示例 bundle 应有带孩子的分支")
        };
        let key = visit_key(e.scan.as_ref().unwrap(), idx).to_string();
        e.expanded.remove(&key); // 收起该节点
        let out = mind_layout(&e);
        let node = out
            .nodes
            .iter()
            .find(|n| n.visit == idx as i32)
            .expect("折叠节点本身仍应被渲染");
        assert!(node.has_children, "折叠节点必须保留展开子树按钮");
        assert!(!node.children_expanded, "折叠态下 children-expanded 应为 false");
        // 展开后按钮仍在（且箭头方向翻转为展开态）。
        e.expanded.insert(key);
        let out = mind_layout(&e);
        let node = out.nodes.iter().find(|n| n.visit == idx as i32).unwrap();
        assert!(node.has_children && node.children_expanded);
    }
}

#[cfg(test)]
mod pick_expand_tests {
    use super::*;

    /// 从零开始逐层模拟选择器展开回调（等价 `on_pick_toggle_expand` 的展开
    /// 分支）：`pick_expanded` 只含 ROOT 键，每轮挑一个可见的可展开行插键后
    /// `rebuild` —— 断言每次都让行集增长，且 BFS 到底后最深行等于 bundle 里
    /// 最深的 visit（任意深度都能展开，玄孙级也不例外）。
    #[test]
    fn pick_expansion_reaches_any_depth() {
        let mut e = Editor::new();
        let demo =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/客户运营.str");
        e.open(&demo).expect("打开示例 bundle");
        let max_visit_depth = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .map(|v| v.depth)
            .max()
            .unwrap();
        eprintln!("demo 最深 visit depth = {max_visit_depth}");
        // 只展开 ROOT：与「对话框打开、用户从零往下点」等价。
        let root_key = visit_key(e.scan.as_ref().unwrap(), 0).to_string();
        e.pick_expanded.insert(root_key);
        e.rebuild();
        loop {
            let scan = e.scan.as_ref().unwrap();
            let target = e.pick_rows.iter().enumerate().find(|(_, r)| {
                r.has_children && !r.hard && !e.pick_expanded.contains(&scan.visits[r.visit].rel)
            });
            let Some((_, row)) = target else {
                break;
            };
            let visit = row.visit;
            let depth_before = row.depth;
            let before = e.pick_rows.len();
            e.pick_expanded.insert(scan.visits[visit].rel.clone());
            e.rebuild();
            assert!(
                e.pick_rows.len() > before,
                "展开 depth={depth_before} 的行后行数未增长"
            );
        }
        let max_row_depth = e.pick_rows.iter().map(|r| r.depth).max().unwrap();
        assert_eq!(
            max_row_depth, max_visit_depth,
            "全量展开后最深行应等于最深 visit（demo 最深 = {max_visit_depth}）"
        );
    }

    /// 合成 **五级链**（ROOT → a1 → a2 → a3 → a4）：demo 只有 3 代、测不到
    /// 「玄孙展不开」，这里直接验证任意深度的链都能逐层展开。
    #[test]
    fn pick_expansion_synthetic_depth4_chain() {
        let tmp = std::env::temp_dir().join(format!("pick_depth4_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let ids = [
            "01928f3a-7c4b-7d00-8a00-00000000000r",
            "01928f3a-7c4b-7d01-8a01-00000000000a",
            "01928f3a-7c4b-7d02-8a02-00000000000b",
            "01928f3a-7c4b-7d03-8a03-00000000000c",
            "01928f3a-7c4b-7d04-8a04-00000000000d",
        ];
        let names = ["", "a1", "a1/a2", "a1/a2/a3", "a1/a2/a3/a4"];
        for n in &names[1..] {
            std::fs::create_dir_all(tmp.join(n)).unwrap();
        }
        let ts = "2026-09-01T09:12:00+08:00";
        // ROOT → a1 → … → a4：父级 `[[entries]]` 逐级登记子目录。
        for (i, id) in ids.iter().enumerate() {
            let dir = if i == 0 { tmp.clone() } else { tmp.join(names[i]) };
            let mut meta = format!(
                "str = 1\nspec = \"1.14.0\"\nkind = \"{kind}\"\nid = \"{id}\"\n\
                 name = \"t{i}\"\ntitle = \"第{depth}代\"\nrevision = 1\n\
                 created_at = {ts}\nupdated_at = {ts}\n",
                kind = if i == 0 { "root" } else { "node" },
                depth = i,
            );
            if i + 1 < ids.len() {
                meta.push_str(&format!(
                    "\n[[entries]]\npath = \"{}\"\nrole = \"node\"\nid = \"{}\"\n\
                     type = \"t\"\ntitle = \"第{}代\"\norder = 1\n",
                    ids[i + 1],
                    ids[i + 1],
                    i + 1
                ));
            }
            std::fs::write(dir.join(".str.toml"), meta).unwrap();
        }
        let mut e = Editor::new();
        e.open(&tmp).expect("打开合成 bundle");
        {
            let scan = e.scan.as_ref().unwrap();
            assert_eq!(scan.visits.len(), 5, "ROOT + 4 代分支");
            assert_eq!(scan.visits[4].depth, 4, "a4 是第 4 代（玄孙）");
        }
        // 与对话框同一流程：只展开 ROOT，逐层把可见的可展开行插键。
        e.pick_expanded
            .insert(visit_key(e.scan.as_ref().unwrap(), 0).to_string());
        e.rebuild();
        loop {
            let scan = e.scan.as_ref().unwrap();
            let Some(row) = e.pick_rows.iter().find(|r| {
                r.has_children && !r.hard && !e.pick_expanded.contains(&scan.visits[r.visit].rel)
            }) else {
                break;
            };
            e.pick_expanded.insert(scan.visits[row.visit].rel.clone());
            e.rebuild();
        }
        let max_row_depth = e.pick_rows.iter().map(|r| r.depth).max().unwrap();
        assert_eq!(max_row_depth, 4, "玄孙（第 4 代）必须能展开出来");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
