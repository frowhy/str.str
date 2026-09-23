//! STR bundle GUI 编辑器（Rust + Slint）。
//!
//! 所有 `._meta` 的读取与写回均经由 `str-format` 库，
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
}

/// 编辑器状态。
struct Editor {
    bundle: Option<Bundle>,
    scan: Option<Scan>,
    /// 子分支索引（见 `children_index`）：随 scan 一起建立，供行构建 / 导图布局
    /// 按节点 O(1) 取用。
    kids: Vec<Vec<usize>>,
    rows: Vec<VisibleRow>,
    /// 已展开分支的身份键集合（见 `visit_key`；跨 rescan 稳定）。
    expanded: HashSet<String>,
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
            expanded: HashSet::new(),
            kids: Vec::new(),
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
    fn rebuild(&mut self) {
        self.rows = Vec::new();
        if let Some(scan) = self.scan.as_ref() {
            if scan.root_index.is_some() {
                dfs_rows(scan, &self.kids, 0, &self.expanded, &mut self.rows);
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
/// 2. `._meta` 的解析/重写是 O(bundle 内容)，逐项做即 O(n²) 写放大。
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
    /// 磁盘与 `._meta` 不会不一致），随后按需落盘。
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
        // 再次调用会让整批报错，进而让「已删文件」仍留在登记里（磁盘/`._meta` 不一致）。
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
    /// `._meta` 内存累积、按阈值与收尾落盘）；Paste 走 `apply_clip_at` 自管读写，忽略之。
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
                        // 源若是**分支**（有 ._meta）撤登记；内容文件夹子行本就不登记。
                        if let Some(sp) = clip.src_path.parent() {
                            if sp.starts_with(bundle_root) && sp.join("._meta").exists() {
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

/// 后台线程执行批量操作：顺序逐项（`._meta` 登记写回需要互斥），
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

/// 分支是否存在**内容条目**（`node` / `branch` 之外的登记项）：决定
/// 「展开 / 收起内容」菜单项是否可用。
fn has_content_entries(visit: &Visit) -> bool {
    visit
        .meta
        .as_ref()
        .map(|m| m.entries.iter().any(|en| !en.is_branch()))
        .unwrap_or(false)
}

/// 分支身份键 = `Visit::rel`（bundle 内相对路径）。
///
/// **不用 `meta.id`**：id 可被复制 —— 在 Finder 里把分支目录复制到别处（⌥ 拖拽）时
/// `._meta` 一并被复制，于是两个**同名**分支共享同一 id（规范侧由 `E_ID_DUP` 报错）。
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

fn dfs_rows(
    scan: &Scan,
    kids: &[Vec<usize>],
    idx: usize,
    expanded: &HashSet<String>,
    out: &mut Vec<VisibleRow>,
) {
    let visit = &scan.visits[idx];
    let key = visit_key(scan, idx);
    let children: &[usize] = kids.get(idx).map(|v| v.as_slice()).unwrap_or_default();
    out.push(VisibleRow {
        visit: idx,
        depth: visit.depth,
        title: visit_title(visit),
        type_str: visit_type(visit),
        expanded: expanded.contains(key),
        has_children: !children.is_empty(),
        has_entries: has_content_entries(visit),
    });
    // ROOT 也受展开态控制：折叠根即隐藏全部一级分支。
    if !expanded.contains(key) {
        return;
    }
    for &c in children {
        dfs_rows(scan, kids, c, expanded, out);
    }
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
        .unwrap_or_else(|| "（._meta 解析失败）".into())
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
/// 节点序列是**后序**（子节点先于父节点入列）：选中项之后第一个「深度更小」的
/// 节点即其父节点，继续回溯直到根 —— 一次顺序扫描即可（O(N)）。
fn selection_spine(out: &MindOut, scan: &Scan) -> Vec<usize> {
    let depth = |i: usize| scan.visits[out.nodes[i].visit as usize].depth;
    let Some(start) = out.nodes.iter().position(|n| n.is_selected) else {
        return Vec::new();
    };
    let mut chain = vec![start];
    let mut d = depth(start);
    for j in start + 1..out.nodes.len() {
        let dj = depth(j);
        if dj < d {
            chain.push(j);
            d = dj;
            if d == 0 {
                break;
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

fn mind_layout(e: &Editor) -> MindOut {
    let mut out = MindOut {
        nodes: Vec::new(),
        edges: Vec::new(),
        mini_edges: Vec::new(),
        edge_chunks: Vec::new(),
        mini_edge_chunks: Vec::new(),
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
        let has_entries = has_content_entries(v);
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
    // 子树带高自深向浅递推：子节点深度必然大于父节点。
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(scan.visits[i].depth));
    let mut sub_hs = node_hs.clone();
    for &i in &order {
        if !e.expanded.contains(visit_key(scan, i)) {
            continue;
        }
        let children: &[usize] = e.kids.get(i).map(|v| v.as_slice()).unwrap_or_default();
        if children.is_empty() {
            continue;
        }
        let span: f32 = children.iter().map(|&c| sub_hs[c] + NODE_VGAP).sum::<f32>() - NODE_VGAP;
        sub_hs[i] = span.max(node_hs[i]);
    }
    mind_dfs(
        e, &node_hs, &sub_hs, 0, MIND_PAD, NODE_VGAP, sub_hs[0], &mut out,
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
    out
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
        // 内容展开集合按身份键（rel）排序入哈希：集合里可能残留上一代 scan 的键，
        // 但 `scan_gen` 已入哈希，rescan 必然换签名，故无需再过滤存在性。
        let mut expanded_keys: Vec<&str> =
            e.content_expanded.iter().map(|s| s.as_str()).collect();
        expanded_keys.sort_unstable();
        expanded_keys.hash(&mut h);
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

/// 在垂直带 [band_top, band_top + band_h] 内布置节点及其子树，返回节点中心 y。
/// 带高由调用方保证 ≥ 节点自身高度：父节点中心 = 首/末子中心的中点并钳制在带内。
fn mind_dfs(
    e: &Editor,
    node_hs: &[f32],
    sub_hs: &[f32],
    idx: usize,
    x: f32,
    band_top: f32,
    band_h: f32,
    out: &mut MindOut,
) -> f32 {
    let scan = e.scan.as_ref().unwrap();
    let visit = &scan.visits[idx];
    let key = visit_key(scan, idx);
    let is_root = visit.depth == 0;
    let children: &[usize] = if e.expanded.contains(key) {
        e.kids.get(idx).map(|v| v.as_slice()).unwrap_or_default()
    } else {
        &[]
    };

    // 节点内容展开时内嵌完整内容列表面板（与列表视图同一 EntryRow 模型）。
    let show_entries = e.content_expanded.contains(key);
    let rows: Vec<EntryRow> = if show_entries {
        let none = HashSet::new();
        visible_to_rows(&build_entry_rows_for(e, idx), node_multi(e, idx, &none))
    } else {
        Vec::new()
    };

    let title = visit_title(visit);
    let type_str = visit_type(visit);
    // 是否存在内容条目（控制内容按钮可见性，与展开态无关）。
    let has_any_entries = visit
        .meta
        .as_ref()
        .map(|m| m.entries.iter().any(|en| !en.is_branch()))
        .unwrap_or(false);
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
        let total: f32 = children.iter().map(|&c| sub_hs[c] + NODE_VGAP).sum::<f32>() - NODE_VGAP;
        let mut cursor = (y - total / 2.0).clamp(band_top, band_top + band_h - total);
        for &c in children {
            centers.push(mind_dfs(
                e, node_hs, sub_hs, c, child_x, cursor, sub_hs[c], out,
            ));
            cursor += sub_hs[c] + NODE_VGAP;
        }
    }

    let is_selected = e.selected.as_deref() == Some(key);
    let children_expanded = e.expanded.contains(key);
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
        has_children: e.kids.get(idx).is_some_and(|v| !v.is_empty()),
        has_entries: has_any_entries,
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
            "._meta 解析失败：{}",
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

/// 递归复制目录（不含 `._meta` 语义，仅内容容器）。
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

/// 递归复制分支：重建 `._meta`（新 id、新 ref id、修正子分支指向），payload 按位拷贝。
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
    let Some(meta) = visit.meta.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, en) in meta.entries.iter().enumerate() {
        if en.is_branch() {
            continue;
        }
        let is_dir = en.role == "dir";
        let fs_path = visit.dir.join(&en.path);
        let dir_key = fs_path.to_string_lossy().to_string();
        let expanded = is_dir && e.expanded_dirs.contains(&dir_key);
        out.push(VisibleEntry {
            entries_idx: Some(idx),
            path: en.path.clone(),
            role: en.role.clone(),
            depth: 0,
            is_dir,
            expanded,
            // 文件夹行的拖放目标是文件夹内部；文件行是分支目录。
            drop_dir: if is_dir {
                fs_path.clone()
            } else {
                visit.dir.clone()
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

/// 从系统剪贴板的文件路径反推应用内条目：角色取自源分支 `._meta` 的登记
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
    // 复制/粘贴的来源是剪贴板本身，故不读源 `._meta`（避免在后台批量线程里多一次 IO）。
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
        // 仅当源目录是**分支**（有 ._meta）才需要撤登记 —— 内容文件夹的子行本就
        // 不登记；对无 ._meta 的目录做 read_meta+save 会凭空把该目录变成分支。
        if src_parent.starts_with(&bundle.root) && src_parent.join("._meta").exists() {
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
/// 内容文件夹不是分支、没有 `._meta`，其子项**不登记**（与拖拽移入同规则）：
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
        // 剪切：源若是**分支**（有 ._meta）撤登记；内容文件夹子行本就不登记。
        if clip.is_cut {
            if let Some(sp) = clip.src_path.parent() {
                if sp.starts_with(&bundle.root) && sp.join("._meta").exists() {
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
fn tree_drop_ok(e: &Editor, src: usize, target: usize, after: bool) -> bool {
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

/// 移动分支后按新深度同步其自身 `._meta` 的 `kind`（深度 1 = node，≥2 = branch）。
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

/// 分支结构移动：把 `src` 分支移入 `target` 分支作为**最后一个子分支**
/// （fs 目录移动 + 源父级撤登记 + 目标登记；角色随新深度 node/branch）。
/// 分支自身 `._meta` 随目录移动，内部条目相对路径不受影响。
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
    // 目标分支登记为最后一个子分支。
    let mut tm = read_meta(&bundle, &target_dir)?;
    tm.upsert_entry(&Entry {
        path: name.clone(),
        role: new_role.to_string(),
        id: src_meta.id.clone(),
        r#type: src_meta.r#type.clone(),
        title: src_meta.title.clone(),
        ..Default::default()
    });
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

/// 读取最近打开列表（新→旧；自动剔除已不存在的路径）。
fn recent_paths() -> Vec<PathBuf> {
    let Some(dir) = app_config_dir() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(dir.join("recent.txt")) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir() && seen.insert(p.display().to_string()))
        .take(RECENT_MAX)
        .collect()
}

/// 把路径提到最近列表最前（去重、截断到上限），并落盘。
fn push_recent(path: &Path) {
    let Some(dir) = app_config_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut list: Vec<PathBuf> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("recent.txt")) {
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let p = PathBuf::from(line);
            if !list.contains(&p) {
                list.push(p);
            }
        }
    }
    list.insert(0, path.to_path_buf());
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
        dnd.on_transfer_to_entry(|d| d.plain_text().unwrap_or_default());
        dnd.on_branch_to_transfer(|visit| {
            slint::DataTransfer::from(SharedString::from(format!("branch|{visit}")))
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

    /// 全部分支身份键（用于「展开全部子树」）。
    fn all_branch_keys(e: &Editor) -> Vec<String> {
        e.scan
            .as_ref()
            .map(|s| s.visits.iter().map(|v| v.rel.clone()).collect())
            .unwrap_or_default()
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
                s.visits.get(idx).is_some_and(has_content_entries),
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
                .any(|r| s.visits.get(r.visit).is_some_and(has_content_entries))
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
        // 小地图密度形态：映射参数（盒尺寸 / 两轴比例 / 判据 / 用色）都由 Slint 侧
        // 计算，这里回读后烘焙位图 —— 两边用同一组数值，不会各算一套。
        if app.get_mini_dense() {
            // Slint 在窗口显示前只报 1.0，密度图固定按 2x 生成：位图很小（184×116
            // 量级），Retina 下不糊，1x 屏上略缩也看不出差别。
            let dpr = app.window().scale_factor().clamp(2.0, 3.0);
            let (bw, bh) = (app.get_mini_box_w(), app.get_mini_box_h());
            let (sx, sy) = (app.get_mini_sx(), app.get_mini_sy());
            let ink = rgb_of(app.get_mini_ink());
            let accent = rgb_of(app.get_mini_accent());
            let selected = e.selected.clone().unwrap_or_default();
            // 键含布局签名 / 映射参数 / 用色 / 选中路径：只有这些变化才需要重烘焙。
            let key = format!(
                "{sig}|{dpr:.2}|{bw:.2}x{bh:.2}|{sx:.4}x{sy:.4}|{ink:?}{accent:?}|{selected}"
            );
            if e.mini_raster_key.borrow().as_deref() != Some(key.as_str()) {
                // 无 scan（未打开 bundle）时布局为空，跳过光栅即可：过去这里 expect 会 panic。
                if let Some(scan) = e.scan.as_ref() {
                    let spine = selection_spine(mind, scan);
                    let raster =
                        mini_density_raster(mind, (bw, bh), (sx, sy), dpr, &spine, ink, accent);
                    app.set_mind_mini_image(mini_density_image(&raster));
                }
                *e.mini_raster_key.borrow_mut() = Some(key);
            }
        }
        // 节点模型尽量原地更新：整体替换会重建所有节点组件，正在显示右键
        // 菜单的那个节点被销毁 → 菜单项点击失效（首次右键选中分支即触发，
        // 与结构树 rows_model 同款问题）。节点数变化时（展开/收起等）才整体替换。
        let existing_nodes = e.mind_nodes_model.borrow().clone();
        match existing_nodes {
            Some(handle) if handle.row_count() == mind.nodes.len() => {
                if fresh {
                    for (i, n) in mind.nodes.iter().enumerate() {
                        handle.set_row_data(i, n.clone());
                    }
                } else {
                    // 缓存命中：几何不变，只把选中标记与节点内容行数据刷新到位。
                    // 行模型句柄保持不动（内容行原地更新 → 节点内右键菜单 / 拖拽
                    // 所在组件不会被销毁）。
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
                let handle = Rc::new(VecModel::from(mind.nodes.clone()));
                app.set_mind_nodes(ModelRc::from(handle.clone()));
                *e.mind_nodes_model.borrow_mut() = Some(handle);
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
            app.set_refs(ModelRc::default());
            app.set_entry_selected(-1);
            // 动作目标必须随选中一起清空：否则无选中时触发条目动作（快捷键 /
            // 菜单未灰化路径）会读到上一个分支与上一批路径，操作落到错误位置。
            app.global::<EntryApi>().set_menu_target_visit(-1);
            app.global::<EntryApi>().set_menu_targets("".into());
            app.global::<EntryApi>().set_multi_count(0);
            *e.entries_model.borrow_mut() = None;
            app.global::<DndApi>().set_src_visit(-1);
            return;
        };
        let meta = v.meta.as_deref();
        let is_root = v.depth == 0;
        app.set_has_selection(true);
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

        // 关联 refs[]（目标标题经 id 解析）
        let ref_model: Vec<RefRow> = meta
            .map(|m| {
                m.refs
                    .iter()
                    .map(|r| {
                        let target_title = e
                            .scan
                            .as_ref()
                            .and_then(|s| s.resolve(&r.target).and_then(|i| s.visits.get(i)))
                            .map(visit_title)
                            .unwrap_or_else(|| "（未找到目标）".into());
                        RefRow {
                            ref_id: r.id.clone().into(),
                            target: r.target.clone().into(),
                            target_title: target_title.into(),
                            rel: r.rel.clone().into(),
                            title: r.title.clone().unwrap_or_default().into(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        app.set_refs(ModelRc::from(Rc::new(VecModel::from(ref_model))));

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
                e.selected_visit()
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
    /// 5. **禁止内部实现术语**：canonical、写回、sha256、`._meta`、fs / IO 细节
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

    /// 批量执行期间的**写入互斥**：批量在后台线程写 `._meta` / 移动文件，
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
        let row_model: Vec<BranchRow> = e
            .rows
            .iter()
            .map(|r| BranchRow {
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
                expanded: r.expanded,
                has_children: r.has_children,
                has_entries: r.has_entries,
            })
            .collect();
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
            let path = PathBuf::from(path.to_string());
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
                // 原地更新选中标记（不重建模型，保留双击手势状态）。
                let sel = e.selected_idx_in_visits();
                if let Some(model) = &*e.rows_model.borrow() {
                    for i in 0..model.row_count() {
                        if let Some(mut r) = model.row_data(i) {
                            let new_sel = e
                                .rows
                                .get(i)
                                .map(|vr| Some(vr.visit) == sel)
                                .unwrap_or(false);
                            if r.is_selected != new_sel {
                                r.is_selected = new_sel;
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
            let Some(visit) = e.rows.get(row as usize).map(|r| r.visit) else {
                return;
            };
            let key = e
                .scan
                .as_ref()
                .map(|s| visit_key(s, visit).to_string())
                .filter(|k| !k.is_empty());
            if let Some(key) = key {
                if e.expanded.remove(&key) {
                    collapse_subtree_state(&mut e, visit);
                } else {
                    e.expanded.insert(key);
                }
                e.rebuild();
                sync_ui(&app, &e);
            }
        });
    }
    {
        // 导图节点双击 / 右键「展开·收起子树」：展开或收起该分支的子分支
        // （与列表树共用 expanded 集合）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_visit_expand(move |visit| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            toggle_subtree(&mut e, &app, visit as usize);
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
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_sel_subtree(move || {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            if let Some(idx) = e.selected_idx_in_visits() {
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

    // ── 重命名分支（只改 `._meta.title`；分支目录名是 UUID，不可变）──
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
            app_weak.upgrade().unwrap().set_branch_dialog_visible(false);
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
            let title = app.get_child_title().trim().to_string();
            let type_ = app.get_child_type().trim().to_string();
            if title.is_empty() {
                show_status(&app, "新建子分支失败：标题不能为空。".into());
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未选择分支")?;
                let parent_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
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
            let confirmed = rfd::MessageDialog::new()
                .set_title("删除分支")
                .set_description("确定删除该分支及其全部子分支与文件？此操作不可撤销。")
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if confirmed != rfd::MessageDialogResult::Yes {
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未选择分支")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
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
                Ok(title)
            });
            match result {
                Ok(title) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已删除分支「{title}」及其全部子分支（不可撤销）。").into(),
                    );
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
            let (native_shift, _cmd) = native_toggle_shift();
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
                if path.contains('/') || path.starts_with('.') || path == "._meta" {
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
                        || new_name == "._meta"
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

    // ── refs[] 管理 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_add_ref(move || {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let target = app.get_r_target().trim().to_string();
            if target.is_empty() {
                show_status(&app, "添加关联失败：目标分支 id 不能为空。".into());
                return;
            }
            let rel = {
                let r = app.get_r_rel().trim().to_string();
                if r.is_empty() {
                    "related".to_string()
                } else {
                    r
                }
            };
            let title = app.get_r_title().trim().to_string();
            let result = with_editor(&editor, |e| {
                let Some(dst_idx) = e.visit_idx(&target) else {
                    return Err(format!("目标 id 不存在于本 bundle：{target}"));
                };
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                // 关联的「来源 → 目标」两端标题（目标按 id 反查，取不到则退回 id 本身）。
                let src_title = visit_title(&e.scan.as_ref().unwrap().visits[visit_idx]);
                let dst_title = e
                    .scan
                    .as_ref()
                    .map(|s| visit_title(&s.visits[dst_idx]))
                    .unwrap_or_else(|| target.clone());
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.push_ref(&RefItem {
                    id: util::new_uuid_v7(),
                    target,
                    rel,
                    title: if title.is_empty() { None } else { Some(title) },
                    order: None,
                    note: None,
                });
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok((src_title, dst_title))
            });
            match result {
                Ok((src_title, dst_title)) => {
                    app.set_r_target("".into());
                    app.set_r_rel("".into());
                    app.set_r_title("".into());
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已添加关联：「{src_title}」→「{dst_title}」。").into(),
                    );
                }
                Err(msg) => show_status(&app, format!("添加关联失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_delete_ref(move |index| {
            let app = app_weak.upgrade().unwrap();
            if batch_busy_guard(&app) {
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let (ref_id, ref_target) = e
                    .selected_visit()
                    .and_then(|v| v.meta.as_ref())
                    .and_then(|m| m.refs.get(index as usize))
                    .map(|r| (r.id.clone(), r.target.clone()))
                    .ok_or("未找到该关联")?;
                // 关联的「来源 → 目标」两端标题（目标按 id 反查）。
                let src_title = visit_title(visit);
                let dst_title = e
                    .visit_idx(&ref_target)
                    .and_then(|i| e.scan.as_ref().map(|s| visit_title(&s.visits[i])))
                    .unwrap_or_else(|| ref_target.clone());
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.remove_ref(&ref_id);
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok((src_title, dst_title))
            });
            match result {
                Ok((src_title, dst_title)) => {
                    sync_ui(&app, &editor.borrow());
                    show_status(
                        &app,
                        format!("已移除关联：「{src_title}」→「{dst_title}」。").into(),
                    );
                }
                Err(msg) => show_status(&app, format!("移除关联失败：{msg}").into()),
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
                        || new_name == "._meta"
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
                app_weak.upgrade().unwrap().set_rename_visible(false);
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
                                // 列表里却查不到（用户报告：「已移动但未登记到 ._meta
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
                // 未打开 bundle 时：拖入 .str 目录（含 ._meta 的目录）= 直接打开，而非导入报错。
                if e.bundle.is_none() {
                    match paths.iter().find(|p| p.is_dir() && p.join("._meta").is_file()) {
                        Some(first) => {
                            e.open(first)?;
                            drop_opened = Some(first.clone());
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
                std::fs::write(bundle_dir.join("._meta"), text).map_err(|err| err.to_string())?;
                Ok(())
            })();
            match result.and_then(|()| with_editor(&editor, |e| e.open(&bundle_dir))) {
                Ok(()) => {
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
            let path = PathBuf::from(arg);
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
            has_entries: false,
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
        let spine = selection_spine(&out, e.scan.as_ref().unwrap());
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
    /// `._cache` 是派生数据（revisions 基线），必须剔除 —— 手工 upsert 后的
    /// `._meta` 与陈旧基线放一起会报 `E_REVISION_STALE`。
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
        let _ = std::fs::remove_dir_all(dst.join("._cache"));
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
        // 只落磁盘、不写 `._meta`：模拟内容文件夹的子文件夹。
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
