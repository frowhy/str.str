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
/// 节点标题行高（需与 app.slint 节点标题行、EntryListPanel 高度扣减一致）。
const NODE_H: f32 = 28.0;
const NODE_VGAP: f32 = 10.0;
const NODE_HGAP: f32 = 70.0;
const MIND_PAD: f32 = 20.0;
/// 导图节点内嵌面板行高（对标 Finder 列表视图行高，需与 app.slint 中
/// 条目的 `height` 及上下半区放置区高度（各行高一半）保持一致）。
const ENTRY_ROW_H: f32 = 24.0;
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
    /// 已展开分支的 id 集合（跨 rescan 稳定）。
    expanded: HashSet<String>,
    /// 当前选中分支 id。
    selected: Option<String>,
    /// 「内容」页选中的条目路径（None = 未选中；路径跨 rescan 稳定）。
    selected_entry_path: Option<String>,
    /// 「内容」页已展开的内容文件夹（以分支内绝对路径为键）。
    expanded_dirs: HashSet<String>,
    /// 导图节点内已展开内容的分支 id 集合（跨 rescan 稳定）。
    content_expanded: HashSet<String>,
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
            kids: Vec::new(),
            selected: None,
            selected_entry_path: None,
            expanded_dirs: HashSet::new(),
            content_expanded: HashSet::new(),
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
        self.kids = children_index(&scan);
        self.scan = Some(scan);
        self.selected_entry_path = None;
        self.content_expanded.clear();
        self.rebuild();
        Ok(())
    }

    /// 重新扫描磁盘（按 id 保留展开与选中状态）。
    fn rescan(&mut self) -> Result<(), String> {
        let bundle = self.bundle.as_ref().ok_or("未打开 bundle")?;
        let scan = bundle.scan().map_err(|e| e.to_string())?;
        self.kids = children_index(&scan);
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
                dfs_rows(scan, &self.kids, 0, &self.expanded, &mut self.rows);
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
    let mut subtree_ids: Vec<String> = Vec::new();
    let mut selected_in_subtree = false;
    while let Some(i) = stack.pop() {
        let id_i = scan.visits[i].meta.as_ref().and_then(|m| m.id.clone());
        if e.selected.as_ref() == id_i.as_ref() {
            selected_in_subtree = true;
        }
        for &c in e.kids.get(i).map(|v| v.as_slice()).unwrap_or_default() {
            stack.push(c);
            if let Some(cid) = scan.visits[c].meta.as_ref().and_then(|m| m.id.clone()) {
                subtree_ids.push(cid);
            }
        }
    }
    for cid in &subtree_ids {
        e.expanded.remove(cid);
        e.content_expanded.remove(cid);
    }
    if selected_in_subtree {
        e.selected = None;
        e.selected_entry_path = None;
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

fn dfs_rows(
    scan: &Scan,
    kids: &[Vec<usize>],
    idx: usize,
    expanded: &HashSet<String>,
    out: &mut Vec<VisibleRow>,
) {
    let visit = &scan.visits[idx];
    let id = visit.meta.as_ref().and_then(|m| m.id.clone());
    let children: &[usize] = kids.get(idx).map(|v| v.as_slice()).unwrap_or_default();
    out.push(VisibleRow {
        visit: idx,
        depth: visit.depth,
        title: visit_title(visit),
        type_str: visit_type(visit),
        expanded: id.as_deref().is_some_and(|id| expanded.contains(id)),
        has_children: !children.is_empty(),
        has_entries: has_content_entries(visit),
    });
    // ROOT 也受展开态控制：折叠根即隐藏全部一级分支。
    if !id.as_deref().is_some_and(|id| expanded.contains(id)) {
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
        raster_line(
            &mut cov,
            w,
            h,
            (ax * kx, ay * ky),
            (bx * kx, by * ky),
            0.7,
        );
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
        let (rgb, a) = if hot[i] { (accent, a.max(0.9)) } else { (ink, a) };
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
        let id = v.meta.as_ref().and_then(|m| m.id.clone());
        let show = id.as_deref().is_some_and(|id| e.content_expanded.contains(id));
        let rows_n = if show { build_entry_rows_for(e, i).len() } else { 0 };
        let has_entries = has_content_entries(v);
        node_hs[i] = if rows_n == 0 {
            if has_entries { NODE_H_COMPACT } else { NODE_H }
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
        let id = scan.visits[i].meta.as_ref().and_then(|m| m.id.clone());
        if !id.as_deref().is_some_and(|id| e.expanded.contains(id)) {
            continue;
        }
        let children: &[usize] = e.kids.get(i).map(|v| v.as_slice()).unwrap_or_default();
        if children.is_empty() {
            continue;
        }
        let span: f32 = children.iter().map(|&c| sub_hs[c] + NODE_VGAP).sum::<f32>() - NODE_VGAP;
        sub_hs[i] = span.max(node_hs[i]);
    }
    mind_dfs(e, &node_hs, &sub_hs, 0, MIND_PAD, NODE_VGAP, sub_hs[0], &mut out);
    out.h = NODE_VGAP + sub_hs[0] + NODE_VGAP;
    // 缩略图的内容实际右缘（不含按钮占位）：用它换算面板宽度与缩放比例。
    let content_right = out.nodes.iter().map(|n| n.x + n.w).fold(0.0f32, f32::max);
    // 画布右缘需覆盖外置子树按钮的横向占位。
    out.w = out
        .nodes
        .iter()
        .map(|n| n.x + n.w + if n.has_children { SUBTREE_BTN_EXTENT } else { 0.0 })
        .fold(0.0f32, f32::max)
        + MIND_PAD;
    out.edge_offset = out.w - (content_right + MIND_PAD);
    // 连线几何一次算好后合并成 Path：Slint 侧只渲染少量 Path 元素（见 MindEdgeChunk）。
    out.edge_chunks = canvas_edge_chunks(&out.edges);
    out.mini_edge_chunks = mini_edge_chunks(&out.mini_edges);
    out
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
    let id = visit.meta.as_ref().and_then(|m| m.id.clone());
    let is_root = visit.depth == 0;
    let children: &[usize] = if id.as_deref().is_some_and(|id| e.expanded.contains(id)) {
        e.kids.get(idx).map(|v| v.as_slice()).unwrap_or_default()
    } else {
        &[]
    };

    // 节点内容展开时内嵌完整内容列表面板（与列表视图同一 EntryRow 模型）。
    let show_entries = id.as_deref().is_some_and(|id| e.content_expanded.contains(id));
    let rows: Vec<EntryRow> = if show_entries {
        visible_to_rows(&build_entry_rows_for(e, idx))
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
            centers.push(mind_dfs(e, node_hs, sub_hs, c, child_x, cursor, sub_hs[c], out));
            cursor += sub_hs[c] + NODE_VGAP;
        }
    }

    let is_selected = e.selected.as_deref().is_some_and(|s| id.as_deref() == Some(s));
    let children_expanded = id.as_deref().is_some_and(|id| e.expanded.contains(id));
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
        entry_rows: ModelRc::from(Rc::new(VecModel::from(rows))),
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

    push(&mut out, "编辑", "复制 / 剪切 / 粘贴", format!("{m}C / {m}X / {m}V"));
    push(&mut out, "编辑", "制作副本", format!("{m}D"));
    push(&mut out, "编辑", "删除条目", format!("{m}{del}"));
    push(&mut out, "编辑", "保存条目修改", format!("{m}S"));

    push(&mut out, "分支", "新建子分支…", format!("{s}{m}N"));
    push(&mut out, "分支", "保存分支信息", format!("{s}{m}S"));
    push(&mut out, "分支", "删除分支", format!("{s}{m}{del}"));

    push(
        &mut out,
        "工具",
        "在 Finder 中显示分支",
        format!("{s}{m}R"),
    );
    push(
        &mut out,
        "工具",
        "在 Finder 中显示内容",
        format!("{a}{m}R"),
    );
    push(&mut out, "工具", "校验 bundle", format!("{s}{m}V"));

    push(&mut out, "视图", "列表视图 / 导图视图", format!("{m}1 / {m}2"));
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
) -> Result<String, String> {
    let bundle = e.bundle.as_ref().ok_or("未打开 bundle")?.clone();
    let (src_dir, dst_dir) = {
        let scan = e.scan.as_ref().ok_or("未打开 bundle")?;
        if src_visit >= scan.visits.len() || dst_visit >= scan.visits.len() {
            return Err("源或目标分支无效".into());
        }
        (
            scan.visits[src_visit].dir.clone(),
            scan.visits[dst_visit].dir.clone(),
        )
    };
    let src_meta = read_meta(&bundle, &src_dir)?;
    let dst_meta = read_meta(&bundle, &dst_dir)?;

    // 目的地目录与插入位（None = 目标顶层序列末尾）。
    let hovered = if at_end || to_index >= dst_vis.len() {
        None
    } else {
        dst_vis.get(to_index)
    };
    let (dst_folder, insert_pos) = match hovered {
        Some(row) if row.is_dir => (Some(row.drop_dir.clone()), None),
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
                let count =
                    std::fs::read_dir(dest_dir.join(&name)).map(|rd| rd.count()).unwrap_or(0);
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
            if src_fs.is_dir() {
                let count =
                    std::fs::read_dir(dest_dir.join(&name)).map(|rd| rd.count()).unwrap_or(0);
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
    e.rescan()?;
    e.selected_entry_path = Some(highlight);
    Ok(format!("已移动 {path} → 目标分支"))
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
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
        NSAppearanceCustomization,
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
            format!("v{}（规范 v{}）", env!("CARGO_PKG_VERSION"), str_format::SPEC_VERSION),
        );
        std::env::set_var(
            "STR_ABOUT_COPYRIGHT",
            format!("{} License · {}", env!("CARGO_PKG_LICENSE"), env!("CARGO_PKG_REPOSITORY")),
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
        let id = e
            .scan
            .as_ref()
            .and_then(|s| s.visits.get(visit))
            .and_then(|v| v.meta.as_ref())
            .and_then(|m| m.id.clone());
        let Some(id) = id else { return };
        if e.expanded.remove(&id) {
            collapse_subtree_state(e, visit);
        } else {
            e.expanded.insert(id);
        }
        e.rebuild();
        sync_ui(app, e);
    }

    /// 展开 / 收起某分支的**内容条目**面板（仅视图状态，不写盘）。
    fn toggle_branch_content(e: &mut Editor, app: &AppWindow, visit: usize) {
        let id = e
            .scan
            .as_ref()
            .and_then(|s| s.visits.get(visit))
            .and_then(|v| v.meta.as_ref())
            .and_then(|m| m.id.clone());
        let Some(id) = id else { return };
        if !e.content_expanded.remove(&id) {
            e.content_expanded.insert(id);
        }
        sync_detail(app, e);
    }

    /// 全部分支 id（用于「展开全部子树」）。
    fn all_branch_ids(e: &Editor) -> Vec<String> {
        e.scan
            .as_ref()
            .map(|s| {
                s.visits
                    .iter()
                    .filter_map(|v| v.meta.as_ref().and_then(|m| m.id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **当前可见**的分支 id：与列表树 / 导图此刻渲染出的节点一致 ——
    /// 即 `e.rows`（沿「已展开链」可达的行）对应的分支，含 ROOT。
    ///
    /// 「展开全部内容」以此为基准：只为看得见的分支展开内容面板。
    /// 注意不能用 `expanded` 集合本身 —— 它是「谁的子分支被展开」，而不是
    /// 「谁被渲染」；用错会导致根节点没有内容条目时整项被误判为不可用
    /// （且只展开了一个节点，看起来像没反应）。
    fn visible_branch_ids(e: &Editor) -> Vec<String> {
        let Some(scan) = e.scan.as_ref() else {
            return Vec::new();
        };
        e.rows
            .iter()
            .filter_map(|r| scan.visits.get(r.visit))
            .filter_map(|v| v.meta.as_ref().and_then(|m| m.id.clone()))
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
        let mind = mind_layout(e);
        // 内容尺寸与节点数必须**先**写回：小地图的映射参数（比例 / 盒尺寸 / 密度
        // 判据）都是 Slint 侧按这几个值算的绑定，晚写就会读到上一轮布局的旧值。
        // 缩略图内容宽度需扣掉子树按钮占位（面板宽度与缩略比例都用它），否则右侧
        // 会多出一段空白——或反过来，比例基准不扣时内容会溢出面板。
        app.set_mind_edge_offset(mind.edge_offset);
        app.set_mind_w(mind.w);
        app.set_mind_h(mind.h);
        app.set_mind_node_count(mind.nodes.len() as i32);
        // 小地图密度形态：映射参数（盒尺寸 / 两轴比例 / 判据 / 用色）都由 Slint 侧
        // 计算，这里回读后烘焙位图 —— 两边用同一组数值，不会各算一套。
        if app.get_mini_dense() {
            // Slint 在窗口显示前只报 1.0，密度图固定按 2x 生成：位图很小（184×116
            // 量级），Retina 下不糊，1x 屏上略缩也看不出差别。
            let dpr = app.window().scale_factor().clamp(2.0, 3.0);
            let spine = selection_spine(&mind, e.scan.as_ref().expect("已打开 bundle"));
            let raster = mini_density_raster(
                &mind,
                (app.get_mini_box_w(), app.get_mini_box_h()),
                (app.get_mini_sx(), app.get_mini_sy()),
                dpr,
                &spine,
                rgb_of(app.get_mini_ink()),
                rgb_of(app.get_mini_accent()),
            );
            app.set_mind_mini_image(mini_density_image(&raster));
        }
        // 连线以「分块 Path」渲染（逐条矩形的 item 数在超大导图下不可接受）。
        app.set_mind_edge_chunks(ModelRc::from(Rc::new(VecModel::from(mind.edge_chunks))));
        app.set_mind_mini_edge_chunks(ModelRc::from(Rc::new(VecModel::from(
            mind.mini_edge_chunks,
        ))));
        app.set_mind_nodes(ModelRc::from(Rc::new(VecModel::from(mind.nodes))));

        // 内容尺寸变化时 slint 侧会经 flick 的 changed width 触发 center-canvas 居中。
        sync_selected_detail(app, e);
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

        // 拖拽源分支（当前选中分支）。
        app.global::<DndApi>().set_src_visit(drag_source_visit(e));
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
                    app.set_status(format!("已打开：{}", path.display()).into());
                }
                Err(msg) => app.set_status(format!("打开失败：{msg}").into()),
            }
        });
    }

    // ── 最近打开 / 欢迎引导 ──
    {
        let editor = editor.clone();
        let app_weak: Weak<AppWindow> = app.as_weak();
        app.on_open_recent(move |path| {
            let app = app_weak.upgrade().unwrap();
            let path = PathBuf::from(path.to_string());
            match with_editor(&editor, |e| e.open(&path)) {
                Ok(()) => {
                    push_recent(&path);
                    refresh_recent(&app);
                    sync_ui(&app, &editor.borrow());
                    app.set_status(format!("已打开：{}", path.display()).into());
                }
                Err(msg) => app.set_status(format!("打开失败：{msg}").into()),
            }
        });
    }
    {
        let app_weak: Weak<AppWindow> = app.as_weak();
        app.on_clear_recent(move || {
            let app = app_weak.upgrade().unwrap();
            clear_recent();
            refresh_recent(&app);
            app.set_status("已清除最近打开列表。".into());
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
                if e.expanded.remove(&id) {
                    collapse_subtree_state(&mut e, visit);
                } else {
                    e.expanded.insert(id);
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
            let ids = all_branch_ids(&e);
            if ids.is_empty() {
                app.set_status("没有可展开的分支。".into());
                return;
            }
            let n = ids.len();
            e.expanded.extend(ids);
            e.rebuild();
            sync_ui(&app, &e);
            app.set_status(format!("已展开全部子树（{n} 个分支）。").into());
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
                app.set_status("子树已全部收起。".into());
                return;
            }
            e.expanded.clear();
            e.selected_entry_path = None;
            e.rebuild();
            sync_ui(&app, &e);
            app.set_status("已收起全部子树。".into());
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
            let ids = visible_branch_ids(&e);
            if ids.is_empty() {
                app.set_status("没有可见分支可展开内容。".into());
                return;
            }
            let n = ids.len();
            e.content_expanded.extend(ids);
            sync_detail(&app, &e);
            // 内容面板只出现在导图节点里，故菜单项仅在导图视图可用（见 app.slint）；
            // 这里给出条数反馈，便于确认作用范围。
            app.set_status(format!("已展开 {n} 个可见分支的内容。").into());
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
                app.set_status("内容已全部收起。".into());
                return;
            }
            e.content_expanded.clear();
            sync_detail(&app, &e);
            app.set_status("已收起全部内容。".into());
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
                .and_then(|s| s.visits.get(visit as usize))
                .and_then(|v| v.meta.as_ref())
                .and_then(|m| m.id.clone());
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
                .and_then(|s| s.visits.get(visit as usize))
                .and_then(|v| v.meta.as_ref())
                .and_then(|m| m.id.clone());
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
                app.set_status(format!("已在浏览器打开：{url}").into());
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
            if e.selected.is_none() && e.selected_entry_path.is_none() {
                return;
            }
            e.selected = None;
            e.selected_entry_path = None;
            sync_ui(&app, &e);
        });
    }

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

    // ── 重命名分支（只改 `._meta.title`；分支目录名是 UUID，不可变）──
    {
        let editor = editor.clone();
        let app_weak = app.as_weak();
        app.on_branch_rename_open(move || {
            let app = app_weak.upgrade().unwrap();
            let e = editor.borrow();
            let Some(visit) = e.selected_visit() else {
                app.set_status("未选择分支".into());
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
            app.set_branch_rename_visible(false);
            let name = app.get_branch_rename_name().trim().to_string();
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
                        "已清除分支标题。".to_string()
                    } else {
                        format!("分支已重命名为「{name}」。")
                    };
                    app.set_status(msg.into());
                }
                Err(msg) => app.set_status(format!("重命名失败：{msg}").into()),
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
        app.global::<EntryApi>().on_select_entry(move |index| {
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
        app.global::<EntryApi>().on_delete_entry(move || {
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
        app.global::<EntryApi>().on_clear_selection(move || {
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
        app.global::<EntryApi>().on_entry_reveal(move |index| {
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
        app.global::<EntryApi>().on_entry_open(move |index| {
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
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .spawn();
            #[cfg(all(unix, not(target_os = "macos")))]
            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
        });
    }
    {
        let editor = editor.clone();
        let clipboard = clipboard.clone();
        let app_weak = app.as_weak();
        app.global::<EntryApi>().on_entry_copy(move |index| {
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
        app.global::<EntryApi>().on_entry_cut(move |index| {
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
        app.global::<EntryApi>().on_entry_duplicate(move |index| {
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
            app.global::<EntryApi>().on_entry_rename_open(move |index| {
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
        app.global::<EntryApi>().on_new_folder(move || {
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
        app.global::<EntryApi>().on_paste(move || {
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
        app.global::<EntryApi>().on_drop_row(move |transfer, files, row_index, into_dir, after| {
            let app = app_weak.upgrade().unwrap();
            // 行级落点：打印入参与悬停状态，判断落点行号是否与指针一致。
            if debug_on() {
                let d = app.global::<DndApi>();
                eprintln!(
                    "[str-gui] drop-row row={row_index} into_dir={into_dir} after={after} \
                     files={files:?} transfer={transfer:?} hover-row={} half={} panel={} \
                     hover-files={} hover-into-dir={}",
                    d.get_hover_row(),
                    d.get_hover_half(),
                    d.get_hover_panel(),
                    d.get_hover_files(),
                    d.get_hover_into_dir()
                );
            }
            // data-transfer 文本不可靠时回退到拖拽开始时记录的全局载荷。
            let transfer = if parse_entry_transfer(&transfer).is_some() {
                transfer
            } else {
                app.global::<DndApi>().get_payload()
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
                    // 跨节点拖放 = 移动到目标节点，并按悬停位置插入目标序列。
                    return move_entry_across(
                        e,
                        src_visit,
                        &path,
                        visit_idx,
                        &vis,
                        at_end,
                        to_index,
                        after,
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
                    "已导入到「{title}」（{} 项）：{}",
                    names.len(),
                    names.join("、")
                ))
            });
            match result {
                Ok(msg) => {
                    if let Some(p) = drop_opened.take() {
                        push_recent(&p);
                        refresh_recent(&app);
                    }
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
                    meta_edit::render_root_meta(&name, Some(&name), None, &root_id, &created, 7);
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

    // ── 命令行参数：直接打开指定 bundle（`str-gui <路径>` / 文件关联双击）──
    // 带路径参数启动 = 用户目标明确，**不弹**欢迎引导；仅在无参数时按需弹出。
    match std::env::args().nth(1) {
        Some(arg) => {
            let path = PathBuf::from(arg);
            match with_editor(&editor, |e| e.open(&path)) {
                Ok(()) => {
                    push_recent(&path);
                    refresh_recent(&app);
                    app.set_status(format!("已打开：{}", path.display()).into());
                }
                Err(msg) => app.set_status(format!("打开失败：{msg}").into()),
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
        let demo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/客户运营.str");
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
            .find(|s| {
                (s.x - (canvas_start + NODE_HGAP / 2.0)).abs() < eps && s.w > EDGE_W + eps
            })
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
            .position(|v| {
                v.rel.contains("客户") || v.dir.to_string_lossy().contains("客户")
            })
            .expect("找到客户档案分支");
        let scan = e.scan.as_ref().unwrap();
        let kids = e.kids.get(idx).cloned().unwrap_or_default();
        assert!(!kids.is_empty(), "客户档案应有子分支");
        let root_id = scan.visits[idx]
            .meta
            .as_ref()
            .and_then(|m| m.id.clone())
            .expect("客户档案有 id");
        let kid_ids: Vec<String> = kids
            .iter()
            .map(|&i| {
                scan.visits[i]
                    .meta
                    .as_ref()
                    .and_then(|m| m.id.clone())
                    .expect("子分支有 id")
            })
            .collect();
        // 展开客户档案与全部子分支，并激活第一个子分支。
        e.expanded.insert(root_id);
        for id in &kid_ids {
            e.expanded.insert(id.clone());
        }
        e.selected = Some(kid_ids[0].clone());
        e.content_expanded.insert(kid_ids[0].clone());
        e.rebuild();
        assert!(
            e.rows.iter().any(|r| r.visit == kids[0]),
            "展开后子分支行可见"
        );

        // 收缩客户档案 → 子树展开态与激活态应全部清除。
        e.expanded.remove(
            e.scan.as_ref().unwrap().visits[idx]
                .meta
                .as_ref()
                .and_then(|m| m.id.clone())
                .as_deref()
                .unwrap_or(""),
        );
        collapse_subtree_state(&mut e, idx);
        e.rebuild();

        for id in &kid_ids {
            assert!(!e.expanded.contains(id), "子分支展开态应被移除");
            assert!(!e.content_expanded.contains(id), "子分支内容展开态应被移除");
        }
        assert_eq!(e.selected, None, "子树内的激活分支应被移除");
        assert_eq!(e.selected_entry_path, None);
        assert!(
            e.rows.iter().all(|r| r.visit != kids[0]),
            "收缩后子分支行不应可见"
        );
    }
}

#[cfg(test)]
mod test_support {
    use super::*;

    /// 展开全树的示例 bundle（连线几何最完整：肘形三段 + 缩略图 1:2 划分）。
    pub fn expanded_demo() -> Editor {
        let mut e = Editor::new();
        let demo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/客户运营.str");
        e.open(&demo).expect("打开示例 bundle");
        let ids: Vec<String> = e
            .scan
            .as_ref()
            .unwrap()
            .visits
            .iter()
            .filter_map(|v| v.meta.as_ref().and_then(|m| m.id.clone()))
            .collect();
        e.expanded = ids.into_iter().collect();
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
                *x >= c.x - 0.01 && *y >= c.y - 0.01 && *x <= c.x + c.w + 0.01
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
                assert!(all.contains('⌘') && !all.contains("Ctrl+"), "macOS 用 ⌘ 字形");
            } else {
                assert!(all.contains("Ctrl+") && !all.contains('⌘'), "其它平台用 Ctrl+");
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
        assert_eq!(
            doc_url(repo, 2),
            "https://github.com/frowhy/str.str/issues"
        );
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
        let id = scan.visits[idx]
            .meta
            .as_ref()
            .and_then(|m| m.id.clone())
            .expect("最深分支有 id");
        e.selected = Some(id);
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
}
