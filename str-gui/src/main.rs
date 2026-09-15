//! STR bundle GUI 编辑器（Rust + Slint）。
//!
//! 所有 `._meta` 的读取与写回均经由 `str-format` 库（与 `str` CLI 同一代码路径），
//! 写出字节一律是规范 §4.9 的 canonical 形式，保证 Git diff 干净、校验器零告警。

use std::cell::RefCell;
use std::collections::HashSet;
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
const NODE_H: f32 = 40.0;
const NODE_VGAP: f32 = 10.0;
const NODE_HGAP: f32 = 70.0;
const COL_W: f32 = 300.0;
const MIND_PAD: f32 = 20.0;

/// 左侧列表 / 导图共用的可视行。
struct VisibleRow {
    /// 对应 `Scan::visits` 的下标。
    visit: usize,
    depth: usize,
    title: String,
    type_str: String,
    expanded: bool,
    has_children: bool,
}

/// 编辑器状态。
struct Editor {
    bundle: Option<Bundle>,
    scan: Option<Scan>,
    rows: Vec<VisibleRow>,
    /// 已展开分支的 id 集合（跨 rescan 稳定）。
    expanded: HashSet<String>,
    /// 当前选中分支 id。
    selected: Option<String>,
    /// 「内容」页选中的条目路径（None = 未选中；路径跨 rescan 稳定）。
    selected_entry_path: Option<String>,
    /// 「内容」页已展开的内容文件夹（以分支内绝对路径为键）。
    expanded_dirs: HashSet<String>,
    /// 结构树模型句柄：行点击时原地更新选中标记，避免重建模型破坏双击手势。
    rows_model: RefCell<Option<Rc<VecModel<BranchRow>>>>,
}

impl Editor {
    fn new() -> Self {
        Self {
            bundle: None,
            scan: None,
            rows: Vec::new(),
            expanded: HashSet::new(),
            selected: None,
            selected_entry_path: None,
            expanded_dirs: HashSet::new(),
            rows_model: RefCell::new(None),
        }
    }

    fn open(&mut self, path: &Path) -> Result<(), String> {
        let bundle = Bundle::new(path).map_err(|e| e.to_string())?;
        let scan = bundle.scan().map_err(|e| e.to_string())?;
        self.expanded = scan
            .visits
            .first()
            .and_then(|v| v.meta.as_ref())
            .and_then(|m| m.id.clone())
            .into_iter()
            .collect();
        self.selected = self.expanded.iter().next().cloned();
        self.bundle = Some(bundle);
        self.scan = Some(scan);
        self.selected_entry_path = None;
        self.rebuild();
        Ok(())
    }

    /// 重新扫描磁盘（按 id 保留展开与选中状态）。
    fn rescan(&mut self) -> Result<(), String> {
        let bundle = self.bundle.as_ref().ok_or("未打开 bundle")?;
        let scan = bundle.scan().map_err(|e| e.to_string())?;
        self.scan = Some(scan);
        if self
            .selected
            .as_deref()
            .is_some_and(|id| self.visit_idx(id).is_none())
        {
            self.selected = self
                .scan
                .as_ref()
                .unwrap()
                .visits
                .first()
                .and_then(|v| v.meta.as_ref().and_then(|m| m.id.clone()));
        }
        self.rebuild();
        Ok(())
    }

    /// 依据展开集合重建可视行。
    fn rebuild(&mut self) {
        self.rows = Vec::new();
        if let Some(scan) = self.scan.as_ref() {
            if scan.root_index.is_some() {
                dfs_rows(scan, 0, &self.expanded, &mut self.rows);
            }
        }
    }

    fn visit_idx(&self, id: &str) -> Option<usize> {
        self.scan.as_ref()?.resolve(id)
    }

    fn selected_visit(&self) -> Option<&Visit> {
        let id = self.selected.as_deref()?;
        let idx = self.visit_idx(id)?;
        self.scan.as_ref()?.visits.get(idx)
    }

    fn selected_idx_in_visits(&self) -> Option<usize> {
        self.visit_idx(self.selected.as_deref()?)
    }

    fn select_visit(&mut self, visit: usize) {
        self.selected = self
            .scan
            .as_ref()
            .and_then(|s| s.visits.get(visit))
            .and_then(|v| v.meta.as_ref())
            .and_then(|m| m.id.clone());
        self.selected_entry_path = None;
    }
}

/// 按父级 `entries[]` 顺序（目录名兜底）返回子分支 visits 下标。
fn ordered_children(scan: &Scan, idx: usize) -> Vec<usize> {
    let mut children: Vec<usize> = (0..scan.visits.len())
        .filter(|&i| scan.visits[i].parent == Some(idx))
        .collect();
    let order: Vec<String> = scan.visits[idx]
        .meta
        .as_ref()
        .map(|m| {
            m.entries
                .iter()
                .filter(|e| e.is_branch())
                .map(|e| e.path.clone())
                .collect()
        })
        .unwrap_or_default();
    children.sort_by_key(|&c| {
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
    children
}

fn dfs_rows(scan: &Scan, idx: usize, expanded: &HashSet<String>, out: &mut Vec<VisibleRow>) {
    let visit = &scan.visits[idx];
    let id = visit.meta.as_ref().and_then(|m| m.id.clone());
    let children = ordered_children(scan, idx);
    out.push(VisibleRow {
        visit: idx,
        depth: visit.depth,
        title: visit_title(visit),
        type_str: visit_type(visit),
        expanded: id.as_deref().is_some_and(|id| expanded.contains(id)),
        has_children: !children.is_empty(),
    });
    // ROOT 也受展开态控制：折叠根即隐藏全部一级分支。
    if !id.as_deref().is_some_and(|id| expanded.contains(id)) {
        return;
    }
    for c in children {
        dfs_rows(scan, c, expanded, out);
    }
}

fn visit_title(visit: &Visit) -> String {
    visit
        .meta
        .as_ref()
        .and_then(|m| m.title.clone())
        .unwrap_or_else(|| "（无标题）".into())
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

// ── 思维导图布局：水平树，叶子均分垂直空间，父节点居中 ──
struct MindOut {
    nodes: Vec<MindNode>,
    edges: Vec<MindEdge>,
    w: f32,
    h: f32,
}

fn mind_layout(scan: &Scan, expanded: &HashSet<String>, selected: Option<&str>) -> MindOut {
    let mut out = MindOut {
        nodes: Vec::new(),
        edges: Vec::new(),
        w: 0.0,
        h: 0.0,
    };
    if scan.root_index.is_none() {
        return out;
    }
    let mut top = NODE_VGAP;
    mind_dfs(scan, expanded, selected, 0, 0, &mut top, &mut out);
    out.h = top + NODE_VGAP;
    out.w = out.nodes.iter().map(|n| n.x + n.w).fold(0.0f32, f32::max) + MIND_PAD;
    out
}

fn mind_dfs(
    scan: &Scan,
    expanded: &HashSet<String>,
    selected: Option<&str>,
    idx: usize,
    depth: usize,
    top: &mut f32,
    out: &mut MindOut,
) -> f32 {
    let visit = &scan.visits[idx];
    let id = visit.meta.as_ref().and_then(|m| m.id.clone());
    let is_root = visit.depth == 0;
    let children: Vec<usize> = if id.as_deref().is_some_and(|id| expanded.contains(id)) {
        ordered_children(scan, idx)
    } else {
        Vec::new()
    };

    let x = MIND_PAD + depth as f32 * COL_W;
    let title = visit_title(visit);
    let w = est_node_w(&title);
    let y = if children.is_empty() {
        let c = *top + NODE_H / 2.0;
        *top += NODE_H + NODE_VGAP;
        c
    } else {
        let mut centers = Vec::new();
        for &c in &children {
            centers.push(mind_dfs(scan, expanded, selected, c, depth + 1, top, out));
        }
        (centers[0] + centers[centers.len() - 1]) / 2.0
    };

    let is_selected = selected.is_some_and(|s| id.as_deref() == Some(s));
    out.nodes.push(MindNode {
        visit: idx as i32,
        x,
        y: y - NODE_H / 2.0,
        w,
        h: NODE_H,
        title: title.into(),
        type_str: visit_type(visit).into(),
        is_root,
        is_selected,
    });

    if !children.is_empty() {
        for &c in &children {
            let cx = MIND_PAD + (visit.depth + 1) as f32 * COL_W;
            let cy = out
                .nodes
                .iter()
                .find(|n| n.visit == c as i32)
                .map(|n| n.y + n.h / 2.0)
                .unwrap_or(y);
            // 肘形连接线：父右缘 → 中线 → 子左缘（三段）。
            let x1 = x + w;
            let xm = x1 + NODE_HGAP / 2.0;
            out.edges.push(MindEdge {
                x: x1,
                y,
                w: xm - x1,
                h: 2.0,
            });
            let (vy, vh) = if cy >= y { (y, cy - y) } else { (cy, y - cy) };
            out.edges.push(MindEdge {
                x: xm,
                y: vy,
                w: 2.0,
                h: vh,
            });
            out.edges.push(MindEdge {
                x: xm,
                y: cy,
                w: cx - xm,
                h: 2.0,
            });
        }
    }
    y
}

/// 依据标题长度估算节点宽度（CJK 按双宽计）。
fn est_node_w(title: &str) -> f32 {
    let units: usize = title
        .chars()
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum();
    (units as f32 * 8.0 + 34.0).clamp(120.0, COL_W - 40.0)
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

/// 在系统文件管理器中显示（macOS：`open -R`）。
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

/// 构建可见内容行（展开的文件夹列出磁盘子项，按名排序）。
fn build_entry_rows(e: &Editor) -> Vec<VisibleEntry> {
    let Some(visit) = e.selected_visit() else {
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

/// 把「内容」页可见行转成 UI 模型行。
fn visible_to_rows(vis: &[VisibleEntry]) -> Vec<EntryRow> {
    vis.iter()
        .map(|v| EntryRow {
            path: v.path.clone().into(),
            // 显示名：文件夹子行只显示真实文件名（不带父路径前缀）。
            display: basename(&v.path).into(),
            role: v.role.clone().into(),
            title: v.title.clone().unwrap_or_default().into(),
            size: fmt_size(v.size).into(),
            is_branch: false,
            is_file: !v.is_dir && v.entries_idx.map(|_| true).unwrap_or(true),
            depth: v.depth as i32,
            is_dir: v.is_dir,
            expanded: v.expanded,
        })
        .collect()
}

/// 「内容」页可见条目的路径列表（与 UI 行一一对应）。
fn visible_entry_paths(e: &Editor) -> Vec<String> {
    e.selected_visit()
        .and_then(|v| v.meta.as_ref())
        .map(|m| {
            m.entries
                .iter()
                .filter(|en| !en.is_branch())
                .map(|en| en.path.clone())
                .collect()
        })
        .unwrap_or_default()
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

/// 文件名（去掉父路径前缀）。
fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Finder / 系统噪声文件：隐藏文件（`.DS_Store`、`._*`）、`__MACOSX`、`Thumbs.db`。
fn is_noise_name(name: &str) -> bool {
    name.starts_with('.') || name == "__MACOSX" || name.eq_ignore_ascii_case("Thumbs.db")
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

/// 把剪贴板条目落到当前选中分支：`is_cut` = 移动（剪切），否则复制。
/// 返回给状态栏显示的消息。
fn apply_clip(e: &mut Editor, clip: &ClipItem) -> Result<String, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
    let scan = e.scan.as_ref().ok_or("未选择分支")?;
    let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
    let visit = &scan.visits[visit_idx];
    let id_version = scan
        .visits
        .first()
        .and_then(|v| v.meta.as_ref())
        .map(|m| m.policies.id_version)
        .unwrap_or(7);
    if !clip.src_path.exists() {
        return Err("剪贴板中的源条目已不存在".into());
    }
    let cut = clip.is_cut;
    // 剪切时源、目标不能是同一目录。
    if cut {
        let src_parent = clip.src_path.parent().unwrap_or(Path::new(""));
        if src_parent == visit.dir {
            return Err("源与目标是同一分支".into());
        }
    }
    let mut meta = read_meta(bundle, &visit.dir)?;
    let existing: HashSet<String> = meta.entries.iter().map(|x| x.path.clone()).collect();
    let msg;
    match clip.role.as_str() {
        "payload" | "asset" => {
            let name = dedup_name(&existing, &clip.name);
            let dst = visit.dir.join(&name);
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
            msg = format!("已{} {name}", if cut { "移动" } else { "粘贴" });
        }
        "dir" => {
            let name = dedup_name(&existing, &clip.name);
            let dst = visit.dir.join(&name);
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
            msg = format!("已{} {name}/", if cut { "移动" } else { "粘贴" });
        }
        "node" | "branch" => {
            if cut {
                // 同 bundle 内移动分支：保持 id，仅目录换位 + 父级登记转移。
                let src_meta = read_meta(bundle, &clip.src_path)?;
                let name = clip.name.clone();
                std::fs::rename(&clip.src_path, visit.dir.join(&name))
                    .map_err(|err| err.to_string())?;
                let role = if visit.depth == 0 { "node" } else { "branch" };
                meta.upsert_entry(&Entry {
                    path: name.clone(),
                    role: role.to_string(),
                    id: Some(name.clone()),
                    r#type: src_meta.r#type.clone(),
                    title: src_meta.title.clone(),
                    ..Default::default()
                });
                msg = format!("已移动分支 {name}");
            } else {
                let mut mapping = std::collections::HashMap::new();
                let new_id = copy_branch_recursive(
                    bundle,
                    &clip.src_path,
                    &visit.dir,
                    visit.depth,
                    id_version,
                    &mut mapping,
                )?;
                let new_meta = read_meta(bundle, &visit.dir.join(&new_id))?;
                let role = if visit.depth == 0 { "node" } else { "branch" };
                meta.upsert_entry(&Entry {
                    path: new_id.clone(),
                    role: role.to_string(),
                    id: Some(new_id.clone()),
                    r#type: new_meta.r#type.clone(),
                    title: new_meta.title.clone(),
                    ..Default::default()
                });
                msg = format!("已粘贴分支 {}（新 id {new_id}）", clip.name);
            }
        }
        other => return Err(format!("暂不支持 role = {other} 的条目")),
    }
    // 剪切：移除源分支的 entries 登记。
    if cut {
        let src_parent = clip.src_path.parent().ok_or("无法定位源分支目录")?;
        if src_parent.starts_with(&bundle.root) {
            let mut sm = read_meta(bundle, src_parent)?;
            sm.remove_entry_path(&clip.name);
            sm.touch();
            sm.save(&bundle.meta_path(src_parent))
                .map_err(|err| err.to_string())?;
        }
    }
    meta.touch();
    meta.save(&bundle.meta_path(&visit.dir))
        .map_err(|err| err.to_string())?;
    e.rescan()?;
    Ok(msg)
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

/// 运行多行 AppleScript（每行一个 `-e`），返回 stdout。
#[cfg(target_os = "macos")]
fn run_applescript(lines: &[&str], args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("osascript");
    for line in lines {
        cmd.arg("-e").arg(line);
    }
    if !args.is_empty() {
        cmd.arg("--").args(args);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// 把文件/目录写入 macOS 系统剪贴板（NSPasteboard writeObjects:，与 Finder 拷贝同格式）。
#[cfg(target_os = "macos")]
fn mac_write_clipboard_files(paths: &[PathBuf]) -> Result<(), String> {
    let args: Vec<&str> = paths
        .iter()
        .map(|p| p.to_str().unwrap_or_default())
        .collect();
    run_applescript(
        &[
            "use framework \"Foundation\"",
            "use scripting additions",
            "on run argv",
            "    set pathList to current application's NSMutableArray's array()",
            "    repeat with p in argv",
            "        (pathList's addObject:(current application's NSURL's fileURLWithPath:p))",
            "    end repeat",
            "    set pb to current application's NSPasteboard's generalPasteboard()",
            "    pb's clearContents()",
            "    pb's writeObjects:pathList",
            "end run",
        ],
        &args,
    )
    .map(|_| ())
}

/// 读取 macOS 系统剪贴板中的文件路径（NSURL 对象），无文件类内容时返回空。
#[cfg(target_os = "macos")]
fn mac_read_clipboard_files() -> Result<Vec<PathBuf>, String> {
    let out = run_applescript(
        &[
            "use framework \"Foundation\"",
            "use scripting additions",
            "on run",
            "    set pb to current application's NSPasteboard's generalPasteboard()",
            "    set objs to pb's readObjectsForClasses:{current application's NSURL} options:(missing value)",
            "    set out to \"\"",
            "    if objs is not missing value then",
            "        repeat with u in objs",
            "            if (u's isFileURL() as boolean) then",
            "                set out to out & ((u's |path|()) as text) & linefeed",
            "            end if",
            "        end repeat",
            "    end if",
            "    return out",
            "end run",
        ],
        &[],
    )?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
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
    app.window().set_color_scheme(scheme);
    report_app_appearance_native(mode);
}

#[cfg(target_os = "macos")]
fn report_app_appearance_native(_mode: i32) {
    // 原生菜单（Slint 经 muda 生成、挂在系统菜单栏上）的外观必须且只能跟随系统：
    // 一旦显式设置 `NSApp.appearance` 或强制 `NSWindow.appearance`，菜单弹窗会继承「被强制的
    // 外观」而与系统外观冲突，出现「浅色↔深色」的闪烁；`NSApp.appearance` 设成深色时还会造成
    // 深底黑字（文字仍按系统浅色绘制）。
    // 因此这里**什么都不做**——`NSApp`/`NSWindow` 保持默认（nil → 跟随系统），菜单自然跟随
    // 系统。窗口内容区（Slint 控件）的明暗由 FluentPalette + `set_color_scheme` 控制，与本
    // 函数无关。appearance-mode 的「浅色/深色」仅作用于 Slint 自绘内容，不波及原生菜单。
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

fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    // 先让 UI 知道系统当前是否为深色：「跟随系统」模式下生效态与 Palette 都依赖它。
    // 此后 dark-mode 重算会触发 .slint 的 `changed dark-mode`，配色与上报随之刷新。
    app.set_system_dark(system_prefers_dark());

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
    let clipboard: Rc<RefCell<Option<ClipItem>>> = Rc::new(RefCell::new(None));

    // ── 拖拽载荷编解码（data-transfer 在 Slint 侧不透明）──
    {
        let dnd = app.global::<DndApi>();
        dnd.on_entry_to_transfer(slint::DataTransfer::from);
        dnd.on_transfer_to_entry(|d| d.plain_text().unwrap_or_default());
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

    // ── 选中分支 → 信息页表单 ──
    fn sync_detail(app: &AppWindow, e: &Editor) {
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
            app.set_mind_nodes(ModelRc::default());
            app.set_mind_edges(ModelRc::default());
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
        let entry_model: Vec<EntryRow> = visible_to_rows(&build_entry_rows(e));
        app.set_entries(ModelRc::from(Rc::new(VecModel::from(entry_model))));

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

        // 条目编辑区
        fill_entry_fields(app, e);

        // 思维导图
        let mind = mind_layout(e.scan.as_ref().unwrap(), &e.expanded, e.selected.as_deref());
        app.set_mind_nodes(ModelRc::from(Rc::new(VecModel::from(mind.nodes))));
        app.set_mind_edges(ModelRc::from(Rc::new(VecModel::from(mind.edges))));
        app.set_mind_w(mind.w);
        app.set_mind_h(mind.h);

        // 拖拽源分支（当前选中分支）。
        app.set_dnd_src_visit(e.selected_idx_in_visits().map(|i| i as i32).unwrap_or(-1));
    }

    fn fill_entry_fields(app: &AppWindow, e: &Editor) {
        // 选中条目按路径定位（跨 rescan 稳定）；文件夹子行不可编辑。
        let vis = build_entry_rows(e);
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
    }

    // ── 列表 / 导图 / 选中 → UI ──
    fn sync_ui(app: &AppWindow, e: &Editor) {
        let row_model: Vec<BranchRow> = e
            .rows
            .iter()
            .map(|r| BranchRow {
                visit: r.visit as i32,
                title: r.title.clone().into(),
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
                    .and_then(|(id, s)| s.resolve(id))
                    .is_some_and(|sel| sel == r.visit),
                expanded: r.expanded,
                has_children: r.has_children,
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
            let Some(path) = rfd::FileDialog::new()
                .set_title("选择 .str bundle 目录")
                .pick_folder()
            else {
                return;
            };
            match with_editor(&editor, |e| e.open(&path)) {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status(format!("已打开：{}", path.display()).into());
                }
                Err(msg) => app.set_status(format!("打开失败：{msg}").into()),
            }
        });
    }

    // ── 刷新 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_refresh(move || {
            let app = app_weak.upgrade().unwrap();
            match with_editor(&editor, |e| e.rescan()) {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status("已刷新。".into());
                }
                Err(msg) => app.set_status(format!("刷新失败：{msg}").into()),
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
            let id = e
                .scan
                .as_ref()
                .and_then(|s| s.visits.get(visit))
                .and_then(|v| v.meta.as_ref())
                .and_then(|m| m.id.clone());
            if let Some(id) = id {
                if !e.expanded.remove(&id) {
                    e.expanded.insert(id);
                }
                e.rebuild();
                sync_ui(&app, &e);
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_select_visit(move |visit| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            e.select_visit(visit as usize);
            sync_ui(&app, &e);
            app.set_info_tab(0);
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_set_view(move |mind| {
            app_weak.upgrade().unwrap().set_view_mind(mind);
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

    // ── 保存详情 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_save_details(move || {
            let app = app_weak.upgrade().unwrap();
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
                    sync_ui(&app, &editor.borrow());
                    app.set_status("已保存（canonical 写回，revision + 1）。".into());
                }
                Err(msg) => app.set_status(format!("保存失败：{msg}").into()),
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
                app.set_status("未选择分支".into());
                return;
            };
            if !visit.dir.exists() {
                app.set_status(format!("目录不存在：{}", visit.dir.display()).into());
                return;
            }
            reveal_in_file_manager(&visit.dir);
            app.set_status(format!("已在 Finder 中定位：{}", visit.dir.display()).into());
        });
    }

    // ── 新建子分支 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_add_child(move || {
            let app = app_weak.upgrade().unwrap();
            let title = app.get_child_title().trim().to_string();
            let type_ = app.get_child_type().trim().to_string();
            if title.is_empty() {
                return;
            }
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未选择分支")?;
                let parent_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let parent = &scan.visits[parent_idx];
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
                // 展开父级并选中新分支。
                if let Some(pid) = parent.meta.as_ref().and_then(|m| m.id.clone()) {
                    e.expanded.insert(pid);
                }
                e.rescan()?;
                e.selected = Some(id);
                e.rebuild();
                Ok(())
            });
            match result {
                Ok(()) => {
                    app.set_child_title("".into());
                    app.set_child_type("".into());
                    sync_ui(&app, &editor.borrow());
                    app.set_status(format!("已添加子分支「{title}」。").into());
                }
                Err(msg) => app.set_status(format!("添加失败：{msg}").into()),
            }
        });
    }

    // ── 删除分支 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_delete_branch(move || {
            let app = app_weak.upgrade().unwrap();
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
                e.selected = parent.meta.as_ref().and_then(|m| m.id.clone());
                e.rescan()?;
                Ok(())
            });
            match result {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status("已删除分支。".into());
                }
                Err(msg) => app.set_status(format!("删除失败：{msg}").into()),
            }
        });
    }

    // ── entries[] 管理 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_select_entry(move |index| {
            let app = app_weak.upgrade().unwrap();
            let mut e = editor.borrow_mut();
            e.selected_entry_path = build_entry_rows(&e)
                .get(index as usize)
                .map(|v| v.path.clone());
            fill_entry_fields(&app, &e);
            app.set_info_tab(1);
        });
    }
    {
        // 展开/收起内容文件夹（仅视图状态，从磁盘列出子项）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_toggle_dir(move |index| {
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
        app.on_save_entry(move || {
            let app = app_weak.upgrade().unwrap();
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
                Ok(())
            });
            match result {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status("条目已保存。".into());
                }
                Err(msg) => app.set_status(format!("保存条目失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_add_entry(move || {
            let app = app_weak.upgrade().unwrap();
            let path = app.get_a_path().trim().to_string();
            if path.is_empty() {
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
                    sync_ui(&app, &editor.borrow());
                    app.set_status(format!("已添加条目 {path}（{role}）。").into());
                }
                Err(msg) => app.set_status(format!("添加条目失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_delete_entry(move || {
            let app = app_weak.upgrade().unwrap();
            // 移到废纸篓可找回，无需确认。
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let path = e.selected_entry_path.clone().ok_or("未选择条目")?;
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
                Ok(())
            });
            match result {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status("条目已删除。".into());
                }
                Err(msg) => app.set_status(format!("删除条目失败：{msg}").into()),
            }
        });
    }

    // ── refs[] 管理 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_add_ref(move || {
            let app = app_weak.upgrade().unwrap();
            let target = app.get_r_target().trim().to_string();
            if target.is_empty() {
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
                if e.visit_idx(&target).is_none() {
                    return Err(format!("目标 id 不存在于本 bundle：{target}"));
                }
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
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
                Ok(())
            });
            match result {
                Ok(()) => {
                    app.set_r_target("".into());
                    app.set_r_rel("".into());
                    app.set_r_title("".into());
                    sync_ui(&app, &editor.borrow());
                    app.set_status("已添加关联。".into());
                }
                Err(msg) => app.set_status(format!("添加关联失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_delete_ref(move |index| {
            let app = app_weak.upgrade().unwrap();
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                let ref_id = e
                    .selected_visit()
                    .and_then(|v| v.meta.as_ref())
                    .and_then(|m| m.refs.get(index as usize))
                    .map(|r| r.id.clone())
                    .ok_or("未找到该关联")?;
                let mut meta = read_meta(bundle, &visit.dir)?;
                meta.remove_ref(&ref_id);
                meta.touch();
                meta.save(&bundle.meta_path(&visit.dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(())
            });
            match result {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status("已移除关联。".into());
                }
                Err(msg) => app.set_status(format!("移除关联失败：{msg}").into()),
            }
        });
    }

    // ── entries[]：右键菜单 / 复制剪切粘贴 / 拖拽 ──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_clear_entry_selection(move || {
            let app = app_weak.upgrade().unwrap();
            editor.borrow_mut().selected_entry_path = None;
            fill_entry_fields(&app, &editor.borrow());
            app.set_info_tab(0);
        });
    }
    // 由行号解析条目路径（行号对应过滤后的内容条目列表）。
    fn row_entry_path(e: &Editor, index: i32) -> Option<String> {
        visible_entry_paths(e).get(index as usize).cloned()
    }
    {
        let editor = editor.clone();
        app.on_entry_reveal(move |index| {
            let e = editor.borrow();
            let Some(visit) = e.selected_visit() else {
                return;
            };
            let Some(path) = row_entry_path(&e, index).map(|p| visit.dir.join(p)) else {
                return;
            };
            reveal_in_file_manager(&path);
        });
    }
    {
        let editor = editor.clone();
        app.on_entry_open(move |index| {
            let e = editor.borrow();
            let Some(path) = e
                .selected_visit()
                .zip(row_entry_path(&e, index))
                .map(|(v, p)| v.dir.join(p))
            else {
                return;
            };
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&path).spawn();
            #[cfg(windows)]
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start"])
                .arg(&path)
                .spawn();
            #[cfg(all(unix, not(target_os = "macos")))]
            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
        });
    }
    {
        let editor = editor.clone();
        let clipboard = clipboard.clone();
        let app_weak = app.as_weak();
        app.on_entry_copy(move |index| {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = e.selected_visit() else {
                return;
            };
            let Some(en) = row_entry_path(&e, index).and_then(|p| {
                visit
                    .meta
                    .as_ref()
                    .and_then(|m| m.entries.iter().find(|x| x.path == p).cloned())
            }) else {
                return;
            };
            let src_path = visit.dir.join(&en.path);
            *clipboard.borrow_mut() = Some(ClipItem {
                src_path: src_path.clone(),
                name: en.path.clone(),
                role: en.role.clone(),
                is_cut: false,
            });
            // 同步到系统剪贴板：Finder 中 ⌘V 即可粘贴。
            #[cfg(target_os = "macos")]
            let sys_ok = mac_write_clipboard_files(&[src_path]).is_ok();
            #[cfg(not(target_os = "macos"))]
            let sys_ok = false;
            app.set_status(
                if sys_ok {
                    format!("已复制 {}（已同步系统剪贴板）。", en.path)
                } else {
                    format!("已复制 {}（内部剪贴板）。", en.path)
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
        app.on_entry_cut(move |index| {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = e.selected_visit() else {
                return;
            };
            let Some(en) = row_entry_path(&e, index).and_then(|p| {
                visit
                    .meta
                    .as_ref()
                    .and_then(|m| m.entries.iter().find(|x| x.path == p).cloned())
            }) else {
                return;
            };
            *clipboard.borrow_mut() = Some(ClipItem {
                src_path: visit.dir.join(&en.path),
                name: en.path.clone(),
                role: en.role.clone(),
                is_cut: true,
            });
            app.set_status(format!("已剪切 {}（粘贴时移动到目标分支）。", en.path).into());
        });
    }
    {
        // 制作副本：在当前分支内复制一份（重名自动「副本」后缀）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_entry_duplicate(move |index| {
            let app = app_weak.upgrade().unwrap();
            let result = with_editor(&editor, |e| {
                let clip = {
                    let Some(visit) = e.selected_visit() else {
                        return Err("未选择分支".into());
                    };
                    let Some(path) = row_entry_path(e, index) else {
                        return Err("未选择条目".into());
                    };
                    let role = e
                        .selected_visit()
                        .and_then(|v| v.meta.as_ref())
                        .and_then(|m| m.entries.iter().find(|x| x.path == path))
                        .map(|en| en.role.clone())
                        .ok_or("未找到条目")?;
                    ClipItem {
                        src_path: visit.dir.join(&path),
                        name: path,
                        role,
                        is_cut: false,
                    }
                };
                apply_clip(e, &clip)
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status(msg.into());
                }
                Err(msg) => app.set_status(format!("制作副本失败：{msg}").into()),
            }
        });
    }
    {
        // 重命名：对话框确认后 rename 文件/目录 + 更新 entries 登记。
        let pending: Rc<RefCell<Option<(PathBuf, String)>>> = Rc::new(RefCell::new(None));
        {
            let editor = editor.clone();
            let pending = pending.clone();
            let app_weak = app.as_weak();
            app.on_entry_rename_open(move |index| {
                let app = app_weak.upgrade().unwrap();
                let e = editor.borrow();
                let Some(visit) = e.selected_visit() else {
                    return;
                };
                let Some(path) = row_entry_path(&e, index) else {
                    return;
                };
                *pending.borrow_mut() = Some((visit.dir.clone(), path.clone()));
                app.set_rename_name(path.into());
                app.set_rename_visible(true);
            });
        }
        {
            let editor = editor.clone();
            let pending = pending.clone();
            let app_weak = app.as_weak();
            app.on_rename_confirm(move || {
                let app = app_weak.upgrade().unwrap();
                app.set_rename_visible(false);
                let Some((dir, old_path)) = pending.borrow_mut().take() else {
                    return;
                };
                let new_name = app.get_rename_name().trim().to_string();
                let result = with_editor(&editor, |e| {
                    if new_name.is_empty()
                        || new_name.contains('/')
                        || new_name.starts_with('.')
                        || new_name == "._meta"
                    {
                        return Err(format!("无效名称：{new_name}"));
                    }
                    if new_name == old_path {
                        return Ok(String::new());
                    }
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                    let mut meta = read_meta(bundle, &dir)?;
                    let existing: HashSet<String> =
                        meta.entries.iter().map(|x| x.path.clone()).collect();
                    if existing.contains(&new_name) {
                        return Err(format!("名称已存在：{new_name}"));
                    }
                    let entry = meta
                        .entries
                        .iter()
                        .find(|x| x.path == old_path)
                        .cloned()
                        .ok_or("未找到条目")?;
                    std::fs::rename(dir.join(&old_path), dir.join(&new_name))
                        .map_err(|err| err.to_string())?;
                    meta.remove_entry_path(&old_path);
                    let mut ne = entry;
                    ne.path = new_name.clone();
                    meta.upsert_entry(&ne);
                    meta.touch();
                    meta.save(&bundle.meta_path(&dir))
                        .map_err(|err| err.to_string())?;
                    if e.selected_entry_path.as_deref() == Some(old_path.as_str()) {
                        e.selected_entry_path = Some(new_name.clone());
                    }
                    e.rescan()?;
                    Ok(format!("已重命名 {old_path} → {new_name}"))
                });
                match result {
                    Ok(msg) => {
                        if !msg.is_empty() {
                            sync_ui(&app, &editor.borrow());
                            app.set_status(msg.into());
                        }
                    }
                    Err(msg) => app.set_status(format!("重命名失败：{msg}").into()),
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
        app.on_entry_new_folder(move || {
            let app = app_weak.upgrade().unwrap();
            let result = with_editor(&editor, |e| {
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
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
                Ok(format!("已创建文件夹 {name}"))
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status(msg.into());
                }
                Err(msg) => app.set_status(format!("新建文件夹失败：{msg}").into()),
            }
        });
    }
    {
        let editor = editor.clone();
        let clipboard = clipboard.clone();
        let app_weak = app.as_weak();
        app.on_entry_paste(move || {
            let app = app_weak.upgrade().unwrap();
            let result = with_editor(&editor, |e| {
                // 优先读系统剪贴板（Finder 复制的文件 / 本应用复制的内容）。
                #[cfg(target_os = "macos")]
                let sys_paths = mac_read_clipboard_files().unwrap_or_default();
                #[cfg(not(target_os = "macos"))]
                let sys_paths: Vec<PathBuf> = Vec::new();
                if !sys_paths.is_empty() {
                    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                    let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                    let visit = &e.scan.as_ref().unwrap().visits[visit_idx];
                    let names =
                        import_paths_into(bundle, &visit.dir, &visit.dir, true, &sys_paths)?;
                    e.rescan()?;
                    return Ok(format!(
                        "已从系统剪贴板粘贴 {} 项：{}",
                        names.len(),
                        names.join("、")
                    ));
                }
                let clip = clipboard
                    .borrow()
                    .as_ref()
                    .cloned()
                    .ok_or("剪贴板为空：请先复制或剪切一个条目")?;
                apply_clip(e, &clip)
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status(msg.into());
                }
                Err(msg) => app.set_status(format!("粘贴失败：{msg}").into()),
            }
        });
    }
    {
        // 拖到列表某行 = 在本分支内重排（写入显式 order）。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_entry_dropped(move |transfer, files, row_index, into_dir, after| {
            let app = app_weak.upgrade().unwrap();
            // data-transfer 文本不可靠时回退到拖拽开始时记录的全局载荷。
            let transfer = if parse_entry_transfer(&transfer).is_some() {
                transfer
            } else {
                app.get_dnd_payload()
            };
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
                    if register && !at_end && vis.get(to_index).map(|v| v.depth).unwrap_or(0) == 0 {
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
                    let where_ = if register { "分支" } else { "文件夹" };
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
                        "已导入到{where_}（{} 项）：{}",
                        names.len(),
                        names.join("、")
                    ));
                }
                let (src_visit, path) = parse_entry_transfer(&transfer)
                    .ok_or_else(|| format!("拖拽载荷无效：{transfer}"))?;
                if src_visit != visit_idx {
                    return Err("重排仅限本分支内；跨分支请拖到左侧树".into());
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
                        return Ok(String::new());
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
                                let count =
                                    std::fs::read_dir(&dir_fs).map(|rd| rd.count()).unwrap_or(0);
                                let mut ne = den;
                                ne.count = Some(count as i64);
                                meta.upsert_entry(&ne);
                            }
                        }
                    }
                    meta.touch();
                    meta.save(&bundle.meta_path(&visit_dir))
                        .map_err(|err| err.to_string())?;
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
                    e.selected_entry_path = Some(in_dir_child_path(&visit_dir, &dst_dir, &name));
                    return Ok(format!("已移动 {path} → {where_}"));
                }
                let mut meta = read_meta(&bundle, &visit_dir)?;
                // 落到文件夹（含拖到子行 = 移入其所在文件夹）= 把条目移入该文件夹。
                if into_folder {
                    let entry = meta
                        .entries
                        .iter()
                        .find(|x| x.path == path)
                        .cloned()
                        .ok_or("未找到源条目")?;
                    if !entry.is_file_like() {
                        return Err("仅 payload/asset 文件可移入文件夹".into());
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
                    if let Some(den) = meta.entries.iter().find(|x| x.path == dir_rel).cloned() {
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
                    let msg = format!("已移动 {path} → {dir_rel}/");
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
                Ok("条目顺序已更新。".into())
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    if !msg.is_empty() {
                        app.set_status(msg.into());
                    }
                }
                Err(msg) => app.set_status(format!("重排失败：{msg}").into()),
            }
        });
    }
    {
        // 拖到左侧树某分支 = 移动 payload/asset 文件过去。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_entry_drop_to_branch(move |transfer, files, to_visit| {
            let app = app_weak.upgrade().unwrap();
            // data-transfer 文本不可靠时回退到拖拽开始时记录的全局载荷。
            let transfer = if parse_entry_transfer(&transfer).is_some() {
                transfer
            } else {
                app.get_dnd_payload()
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
                    return Ok(format!(
                        "已导入到分支（{} 项）：{}",
                        names.len(),
                        names.join("、")
                    ));
                }
                let (src_visit, path) = parse_entry_transfer(&transfer)
                    .ok_or_else(|| format!("拖拽载荷无效：{transfer}"))?;
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let scan = e.scan.as_ref().ok_or("未打开 bundle")?;
                if src_visit == to_visit {
                    return Err("源与目标是同一分支".into());
                }
                if src_visit >= scan.visits.len() {
                    return Err("源分支无效".into());
                }
                let (src_dir, dst_dir) = {
                    let src = &scan.visits[src_visit];
                    let dst = &scan.visits[to_visit];
                    (src.dir.clone(), dst.dir.clone())
                };
                let src_meta = read_meta(bundle, &src_dir)?;
                // 文件夹子行没有 meta 条目：直接按文件系统移动到目标分支根目录。
                if !src_meta.entries.iter().any(|x| x.path == path) {
                    let src_fs = src_dir.join(&path);
                    if !src_fs.exists() {
                        return Err("源文件不存在".into());
                    }
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
                    move_file(&src_fs, &dst_dir.join(&name))?;
                    // 目标分支顶层清单来自 meta.entries，必须登记。
                    let mut dst_meta = read_meta(bundle, &dst_dir)?;
                    if src_fs.is_dir() {
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
                        let bytes =
                            std::fs::read(dst_dir.join(&name)).map_err(|err| err.to_string())?;
                        dst_meta.upsert_entry(&Entry {
                            path: name.clone(),
                            role: "payload".to_string(),
                            size: Some(bytes.len() as i64),
                            sha256: Some(util::sha256_bytes(&bytes)),
                            ..Default::default()
                        });
                    }
                    dst_meta.touch();
                    dst_meta
                        .save(&bundle.meta_path(&dst_dir))
                        .map_err(|err| err.to_string())?;
                    e.rescan()?;
                    return Ok(format!("已移动 {path} → 目标分支"));
                }
                let entry = src_meta
                    .entries
                    .iter()
                    .find(|x| x.path == path)
                    .cloned()
                    .ok_or("未找到源条目")?;
                if !entry.is_file_like() {
                    return Err("仅支持移动 payload/asset 文件；分支请用「复制 + 粘贴」".into());
                }
                let dst_meta = read_meta(bundle, &dst_dir)?;
                let existing: HashSet<String> =
                    dst_meta.entries.iter().map(|x| x.path.clone()).collect();
                let name = dedup_name(&existing, &path);
                move_file(&src_dir.join(&path), &dst_dir.join(&name))?;
                let bytes = std::fs::read(dst_dir.join(&name)).map_err(|err| err.to_string())?;
                let mut sm = src_meta;
                sm.remove_entry_path(&path);
                sm.touch();
                sm.save(&bundle.meta_path(&src_dir))
                    .map_err(|err| err.to_string())?;
                let mut dm = dst_meta;
                dm.upsert_entry(&Entry {
                    path: name.clone(),
                    role: entry.role.clone(),
                    title: entry.title.clone(),
                    note: entry.note.clone(),
                    size: Some(bytes.len() as i64),
                    sha256: Some(util::sha256_bytes(&bytes)),
                    ..Default::default()
                });
                dm.touch();
                dm.save(&bundle.meta_path(&dst_dir))
                    .map_err(|err| err.to_string())?;
                e.rescan()?;
                Ok(format!("已移动 {path} → {name}"))
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status(msg.into());
                }
                Err(msg) => app.set_status(format!("移动失败：{msg}").into()),
            }
        });
    }
    {
        // 整窗兜底：外部文件拖到窗口任意位置 → 导入当前选中分支。
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_external_files_dropped(move |files| {
            let app = app_weak.upgrade().unwrap();
            let result = with_editor(&editor, |e| {
                if files.is_empty() {
                    return Err("拖拽载荷中没有文件".into());
                }
                let paths: Vec<PathBuf> = files.lines().map(PathBuf::from).collect();
                let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?;
                let visit_idx = e.selected_idx_in_visits().ok_or("未选择分支")?;
                let (dir, title) = {
                    let v = &e.scan.as_ref().unwrap().visits[visit_idx];
                    (v.dir.clone(), visit_title(v))
                };
                let names = import_paths_into(bundle, &dir, &dir, true, &paths)?;
                e.rescan()?;
                Ok(format!(
                    "已导入到「{title}」（{} 项）：{}",
                    names.len(),
                    names.join("、")
                ))
            });
            match result {
                Ok(msg) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status(msg.into());
                }
                Err(msg) => app.set_status(format!("导入失败：{msg}").into()),
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
                return;
            };
            match validate::validate(bundle) {
                Ok(report) => {
                    let (errs, warns) = (report.error_count(), report.warning_count());
                    let summary = format!("校验完成：{errs} errors / {warns} warnings");
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
                    app.set_status(if detail.is_empty() {
                        summary.into()
                    } else {
                        format!("{summary} ｜ {detail}").into()
                    });
                }
                Err(err) => app.set_status(format!("校验执行失败：{err}").into()),
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
            let name = app.get_new_name().trim().to_string();
            let dir = app.get_new_dir().to_string();
            if name.is_empty() || dir.is_empty() {
                return;
            }
            app.set_dialog_visible(false);
            let bundle_dir = PathBuf::from(&dir).join(format!("{name}.str"));
            let result = (|| -> Result<(), String> {
                std::fs::create_dir_all(&bundle_dir).map_err(|err| err.to_string())?;
                let root_id = util::new_uuid_v7();
                let created = util::now_rfc3339();
                let text =
                    meta_edit::render_root_meta(&name, Some(&name), None, &root_id, &created, 0, 7);
                std::fs::write(bundle_dir.join("._meta"), text).map_err(|err| err.to_string())?;
                Ok(())
            })();
            match result.and_then(|()| with_editor(&editor, |e| e.open(&bundle_dir))) {
                Ok(()) => {
                    sync_ui(&app, &editor.borrow());
                    app.set_status(format!("已创建并打开：{}", bundle_dir.display()).into());
                }
                Err(msg) => app.set_status(format!("创建失败：{msg}").into()),
            }
        });
    }

    // ── 命令行参数：直接打开指定 bundle ──
    if let Some(arg) = std::env::args().nth(1) {
        let path = PathBuf::from(arg);
        match with_editor(&editor, |e| e.open(&path)) {
            Ok(()) => app.set_status(format!("已打开：{}", path.display()).into()),
            Err(msg) => app.set_status(format!("打开失败：{msg}").into()),
        }
    }

    sync_ui(&app, &editor.borrow());
    app.run()
}
