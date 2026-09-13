//! CLI 子命令实现（规范第 9 章）。

use std::path::{Path, PathBuf};

use serde_json::Value as JValue;

use crate::bundle::{Bundle, Scan};
use crate::error::{Error, Result, code};
use crate::meta::{Author, Entry, Kind, Meta, MetaLoad, RefItem};
use crate::meta_edit::{meta_from_text, render_branch_meta, render_root_meta, toml_str};
use crate::util::{self, SCHEMA_DIR};
use crate::{SPEC_VERSION, STR_MAJOR};

// ─────────────────────────── 公共辅助 ───────────────────────────

/// 打开 bundle。
pub fn open(dir: &Path) -> Result<Bundle> {
    Bundle::new(dir.to_path_buf())
}

/// 定位某 uuid 对应的 visit 下标。
pub fn locate(scan: &Scan, uuid: &str) -> Option<usize> {
    scan.resolve(uuid)
}

/// 依据扩展名猜测媒体类型。
pub fn media_type_for(name: &str) -> Option<String> {
    let ext = Path::new(name)
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let s = match ext.as_str() {
        "json" => "application/json",
        "toml" => "application/toml",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "yaml" | "yml" => "application/yaml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => return None,
    };
    Some(s.to_string())
}

/// 目录直接子项数（排除 `._meta`）。
fn child_count(dir: &Path) -> Option<i64> {
    crate::validate::dir_child_count(dir)
}

// ─────────────────────────── init ───────────────────────────

/// 创建一个新的 `.str` bundle。
pub fn init(
    dir: &Path,
    name: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    id_version: usize,
) -> Result<()> {
    if !matches!(id_version, 4 | 7) {
        return Err(Error::BadArg(format!(
            "`--id-version` = {id_version} 不受支持（`policies.id_version` 只允许 4 或 7）"
        )));
    }
    let target = if dir.extension().map(|e| e == "str").unwrap_or(false) {
        dir.to_path_buf()
    } else {
        PathBuf::from(format!("{}.str", dir.display()))
    };
    if target.exists() {
        return Err(Error::BadArg(format!("{} 已存在", target.display())));
    }
    let bundle_name = name.clone().unwrap_or_else(|| {
        target
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "bundle".to_string())
    });

    std::fs::create_dir_all(target.join(SCHEMA_DIR)).map_err(|e| Error::io(&target, e))?;
    for (file, body) in crate::EMBEDDED_SCHEMAS {
        let p = target.join(SCHEMA_DIR).join(file);
        std::fs::write(&p, body).map_err(|e| Error::io(&p, e))?;
    }

    let created = util::now_rfc3339();
    let root_id = util::new_uuid(id_version);
    let text = render_root_meta(
        &bundle_name,
        title.as_deref(),
        summary.as_deref(),
        &root_id,
        &created,
        crate::EMBEDDED_SCHEMAS.len(),
        id_version,
    );
    let meta = meta_from_text(&text)?;
    let meta_path = target.join(util::META_FILE);
    meta.save(&meta_path)?;

    println!("已创建 bundle：{}", target.display());
    println!("  spec = {SPEC_VERSION}  str = {STR_MAJOR}");
    println!("  policies.id_version = {id_version}");
    println!(
        "  {} 内已写入 {} 份校验 Schema",
        SCHEMA_DIR,
        crate::EMBEDDED_SCHEMAS.len()
    );
    Ok(())
}

/// ROOT `policies.id_version`（缺省 7）—— 生成端必须产出同版本的 UUID，否则 `E_ID_VERSION`。
fn root_id_version(bundle: &Bundle) -> usize {
    match bundle.read_meta(&bundle.root) {
        Ok(MetaLoad::Ok(m, _)) => m.policies.id_version,
        _ => 7,
    }
}

/// 写回一份 `._meta` 并登记 `E_REVISION_STALE` 的历史基线（见 [`crate::baseline`]）。
fn save_meta(bundle: &Bundle, dir: &Path, meta: &Meta) -> Result<()> {
    meta.save(&bundle.meta_path(dir))?;
    crate::baseline::record(bundle, dir, meta);
    Ok(())
}

/// 取目标分支下标：给了 `uuid` 就解析，缺省为 ROOT。
fn target_branch(scan: &Scan, uuid: Option<&str>) -> Result<usize> {
    match uuid {
        Some(u) => {
            locate(scan, u).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{u}`")))
        }
        None => scan
            .root_index
            .ok_or_else(|| Error::BadArg("bundle 缺少 `._meta`".into())),
    }
}

/// `--out` 的统一出口：缺省或 `-` 走 stdout，其余路径写文件。
fn emit(text: &str, out: Option<&str>) -> Result<()> {
    match out {
        None | Some("-") => {
            print!("{text}");
            Ok(())
        }
        Some(p) => std::fs::write(p, text).map_err(|e| Error::io(p, e)),
    }
}

/// `str export` 的产物是派生数据，**不得**写回 bundle 内部（规范 §9）。
///
/// 比较前把目标路径的**最深已存在祖先**也 canonicalize：否则 `Bundle::new` 归一化过的
/// 根路径与未归一化的 `--out`（macOS `/var` → `/private/var` 这类符号链接）会对不上，
/// 判定形同虚设。
fn resolve_out_path(bundle: &Bundle, out: &str) -> Result<PathBuf> {
    let p = abs(out);
    let probe = match p.parent() {
        Some(dir) => canonical_ancestor(dir),
        None => p.clone(),
    };
    if probe.starts_with(&bundle.root) {
        return Err(Error::BadArg(format!(
            "`--out` 不得指向 bundle 内部（{} 在 {} 内）：export 的产物是派生数据",
            p.display(),
            bundle.root.display()
        )));
    }
    Ok(p)
}

/// `dir` 的最深已存在祖先（canonicalize 后）；一层都不存在时原样返回。
fn canonical_ancestor(dir: &Path) -> PathBuf {
    let mut cur = dir.to_path_buf();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(&cur) {
            return resolved;
        }
        match cur.parent() {
            Some(parent) if parent != cur => cur = parent.to_path_buf(),
            _ => return dir.to_path_buf(),
        }
    }
}

// ─────────────────────────── validate ───────────────────────────

/// 校验整个 bundle。
pub fn validate(dir: &Path, strict: bool, json: bool, fix_manifest: bool) -> Result<i32> {
    let bundle = open(dir)?;
    if fix_manifest {
        // 规范 §9：`--fix-manifest` 是「校验前先修正清单」——必须**真正写盘**。
        sync(dir, false)?;
    }
    let mut report = crate::validate::validate(&bundle)?;
    if strict {
        // `--strict`：把告警也视为失败（CI 用）
        for i in &mut report.issues {
            if i.level == crate::error::Level::Warn {
                i.level = crate::error::Level::Error;
            }
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.to_json()).unwrap_or_default()
        );
    } else {
        println!("{}", report.to_text());
    }
    Ok(report.exit_code())
}

// ─────────────────────────── tree ───────────────────────────

/// 渲染分支树（含 `refs` 关联线）。
pub fn tree(dir: &Path, max_depth: Option<usize>, show_refs: bool, ascii: bool) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    println!("{}", bundle.name());
    if scan.root_index.is_none() {
        println!("  （缺少 `._meta`，无法渲染）");
        return Ok(());
    }
    render_children(&scan, 0, "", max_depth, show_refs, ascii);
    Ok(())
}

/// 树形符号：`(false)` 为 Unicode 制表符，`(true)` 为纯 ASCII（`--ascii`）。
fn marks(ascii: bool) -> (&'static str, &'static str, &'static str, &'static str) {
    if ascii {
        ("|-- ", "`-- ", "|   ", "    ")
    } else {
        ("├─ ", "└─ ", "│  ", "   ")
    }
}

/// 子分支下标，顺序取父级 `entries[]` 的 `(order, path)`（规范 §4.6：`order` 为同层排序键）。
///
/// 与 §4.9 的落盘顺序同源，因此 `str tree` 的次序与 `._meta` 中的条目次序一致。
fn ordered_children(scan: &Scan, idx: usize) -> Vec<usize> {
    let parent = scan.visits[idx].meta.as_ref();
    let mut children: Vec<(i64, String, usize)> = Vec::new();
    for (i, v) in scan.visits.iter().enumerate() {
        if v.parent != Some(idx) {
            continue;
        }
        let name = v
            .dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let order = parent
            .and_then(|m| m.entries.iter().find(|e| e.path == name))
            .and_then(|e| e.order)
            .unwrap_or(i64::MAX);
        children.push((order, name, i));
    }
    children.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    children.into_iter().map(|(_, _, i)| i).collect()
}

fn render_children(
    scan: &Scan,
    idx: usize,
    prefix: &str,
    max_depth: Option<usize>,
    show_refs: bool,
    ascii: bool,
) {
    let Some(meta) = scan.visits[idx].meta.as_ref() else {
        return;
    };
    let children = ordered_children(scan, idx);
    let (mid, last_mark, pipe, blank) = marks(ascii);
    if show_refs {
        let arrow = if ascii { "->" } else { "⇢" };
        for r in &meta.refs {
            let target = scan
                .resolve(&r.target)
                .map(|i| scan.visits[i].rel.clone())
                .unwrap_or_else(|| format!("{}（未解析）", r.target));
            println!("{prefix}{arrow} 关联: {target}  --{}--", r.rel);
        }
    }
    for (n, ci) in children.iter().enumerate() {
        let last = n + 1 == children.len();
        let v = &scan.visits[*ci];
        let (title, type_) = match v.meta.as_ref() {
            Some(m) => (
                m.title.clone().unwrap_or_default(),
                m.r#type.clone().unwrap_or_default(),
            ),
            None => (String::new(), String::new()),
        };
        let mark = if last { last_mark } else { mid };
        let mut line = format!("{prefix}{mark}[{}] {}", n + 1, title);
        if !type_.is_empty() {
            line.push_str(&format!("  ({type_})"));
        }
        if v.meta.is_none() {
            line.push_str("  ! 解析失败");
        }
        println!("{line}");
        if max_depth.map(|d| v.depth < d).unwrap_or(true) {
            let next_prefix = format!("{prefix}{}", if last { blank } else { pipe });
            render_children(scan, *ci, &next_prefix, max_depth, show_refs, ascii);
        }
    }
}

// ─────────────────────────── ls / show ───────────────────────────

/// 列出某分支的清单（读 `._meta`）或磁盘原始内容。
pub fn ls(dir: &Path, uuid: Option<String>, raw: bool) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = match uuid {
        Some(u) => locate(&scan, &u)
            .ok_or_else(|| Error::BadArg(format!("找不到分支 id `{u}`")))?,
        None => scan
            .root_index
            .ok_or_else(|| Error::BadArg("bundle 缺少 `._meta`".into()))?,
    };
    let v = &scan.visits[idx];
    println!("{}  （{}）", v.rel, if raw { "磁盘原始" } else { "清单" });
    if raw {
        for (name, is_dir) in bundle.list_names(&v.dir)? {
            if util::is_meta_file(&name) {
                continue;
            }
            println!("  {}{}", if is_dir { "d " } else { "- " }, name);
        }
        return Ok(());
    }
    let Some(meta) = v.meta.as_ref() else {
        println!("  （`._meta` 解析失败）");
        return Ok(());
    };
    for e in &meta.entries {
        let mut line = format!("  {:<28} {}", e.path, e.role);
        if let Some(t) = &e.title {
            line.push_str(&format!("  {t}"));
        }
        if let Some(s) = e.size {
            line.push_str(&format!("  {s}B"));
        }
        if e.optional {
            line.push_str("  (optional)");
        }
        println!("{line}");
    }
    Ok(())
}

/// 打印某分支的 `._meta`（归一化 JSON）。
pub fn show(dir: &Path, uuid: &str, full: bool) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = locate(&scan, uuid).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{uuid}`")))?;
    let v = &scan.visits[idx];
    let Some(meta) = v.meta.as_ref() else {
        return Err(Error::BadArg(format!("{} 的 `._meta` 解析失败", v.rel)));
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&meta.to_json()).unwrap_or_default()
    );
    if full {
        for e in &meta.entries {
            if (e.role == "payload" || e.role == "asset") && !e.path.is_empty() {
                let p = v.dir.join(&e.path);
                if let Ok(text) = std::fs::read_to_string(&p) {
                    println!("\n── {} ──\n{}", e.path, text);
                }
            }
        }
    }
    Ok(())
}

// ─────────────────────────── node / branch ───────────────────────────

/// 新增独立节点（深度 1）。
pub fn node_add(
    dir: &Path,
    type_: Option<String>,
    title: Option<String>,
    summary: Option<String>,
) -> Result<()> {
    let bundle = open(dir)?;
    let root = match bundle.read_meta(&bundle.root)? {
        MetaLoad::Ok(m, _) => m,
        MetaLoad::Failed(_) => {
            return Err(Error::BadArg("root `._meta` 解析失败，无法新增节点".into()));
        }
    };
    if root.kind != Some(Kind::Root) {
        return Err(Error::BadArg("root `._meta` 的 kind 不是 root".into()));
    }
    let id = util::new_uuid(root.policies.id_version);
    let created = util::now_rfc3339();
    let child_dir = bundle.root.join(&id);
    std::fs::create_dir_all(&child_dir).map_err(|e| Error::io(&child_dir, e))?;
    let text = render_branch_meta(
        Kind::Node,
        &id,
        type_.as_deref(),
        title.as_deref(),
        summary.as_deref(),
        &created,
    );
    let child = meta_from_text(&text)?;
    save_meta(&bundle, &child_dir, &child)?;

    let mut root = root;
    let order = root
        .entries
        .iter()
        .filter(|e| e.role == "node")
        .filter_map(|e| e.order)
        .max()
        .unwrap_or(0)
        + 1;
    let entry = Entry {
        path: id.clone(),
        role: "node".into(),
        id: Some(id.clone()),
        r#type: type_,
        title,
        summary,
        order: Some(order),
        ..Default::default()
    };
    root.upsert_entry(&entry);
    root.sort_collections();
    root.touch();
    save_meta(&bundle, &bundle.root, &root)?;
    println!("已新增独立节点 {id}（深度 1）");
    Ok(())
}

/// 在指定分支下新增关联分支（任意深度）。
pub fn branch_add(
    dir: &Path,
    anchor: &str,
    type_: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    order: Option<i64>,
) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = locate(&scan, anchor)
        .ok_or_else(|| Error::BadArg(format!("找不到锚点分支 id `{anchor}`")))?;
    if scan.visits[idx].depth == 0 {
        return Err(Error::BadArg(
            "ROOT 的直接子分支应使用 `str node add`（role = node）".into(),
        ));
    }
    let anchor_dir = scan.visits[idx].dir.clone();
    let mut parent = match bundle.read_meta(&anchor_dir)? {
        MetaLoad::Ok(m, _) => m,
        MetaLoad::Failed(_) => return Err(Error::BadArg("锚点 `._meta` 解析失败".into())),
    };

    let id = util::new_uuid(root_id_version(&bundle));
    let created = util::now_rfc3339();
    let child_dir = anchor_dir.join(&id);
    std::fs::create_dir_all(&child_dir).map_err(|e| Error::io(&child_dir, e))?;
    let text = render_branch_meta(
        Kind::Branch,
        &id,
        type_.as_deref(),
        title.as_deref(),
        summary.as_deref(),
        &created,
    );
    let child = meta_from_text(&text)?;
    save_meta(&bundle, &child_dir, &child)?;

    let order = order.unwrap_or_else(|| {
        parent
            .entries
            .iter()
            .filter(|e| e.is_branch())
            .filter_map(|e| e.order)
            .max()
            .unwrap_or(0)
            + 1
    });
    let entry = Entry {
        path: id.clone(),
        role: "branch".into(),
        id: Some(id.clone()),
        r#type: type_,
        title,
        summary,
        order: Some(order),
        ..Default::default()
    };
    parent.upsert_entry(&entry);
    parent.sort_collections();
    parent.touch();
    save_meta(&bundle, &anchor_dir, &parent)?;
    println!("已在 {} 下新增关联分支 {id}（深度 {}）", scan.visits[idx].rel, scan.visits[idx].depth + 1);
    Ok(())
}

/// 删除关联分支（含其全部下级）。
pub fn branch_rm(dir: &Path, uuid: &str, force: bool) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = locate(&scan, uuid).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{uuid}`")))?;
    let v = &scan.visits[idx];
    if v.depth == 0 {
        return Err(Error::BadArg("不能删除 ROOT".into()));
    }
    if !force {
        return Err(Error::BadArg(format!(
            "删除 {} 会移除其全部下级，请加 `--force` 确认",
            v.rel
        )));
    }
    let target = v.dir.clone();
    let rel = v.rel.clone();
    let parent_idx = v.parent;
    std::fs::remove_dir_all(&target).map_err(|e| Error::io(&target, e))?;

    if let Some(pi) = parent_idx {
        let parent_dir = scan.visits[pi].dir.clone();
        let name = target
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut parent = match bundle.read_meta(&parent_dir)? {
            MetaLoad::Ok(m, _) => m,
            MetaLoad::Failed(_) => return Ok(()),
        };
        parent.remove_entry_path(&name);
        parent.touch();
        save_meta(&bundle, &parent_dir, &parent)?;
    }
    // 重扫一遍以丢弃被删分支的基线条目（否则 `E_REVISION_STALE` 基线会残留）
    if let Ok(after) = bundle.scan() {
        crate::baseline::record_scan(&bundle, &after);
    }
    println!("已删除 {rel}");
    Ok(())
}

// ─────────────────────────── ref ───────────────────────────

/// 新增跨枝关联线。
pub fn ref_add(
    dir: &Path,
    uuid: &str,
    target: &str,
    rel: String,
    title: Option<String>,
    note: Option<String>,
) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = locate(&scan, uuid).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{uuid}`")))?;
    if locate(&scan, target).is_none() {
        return Err(Error::BadArg(format!("找不到目标分支 id `{target}`")));
    }
    let src_dir = scan.visits[idx].dir.clone();
    let mut meta = match bundle.read_meta(&src_dir)? {
        MetaLoad::Ok(m, _) => m,
        MetaLoad::Failed(_) => return Err(Error::BadArg("源分支 `._meta` 解析失败".into())),
    };
    let ref_id = util::new_uuid_v7();
    let order = meta.refs.len() as i64 + 1;
    meta.push_ref(&RefItem {
        id: ref_id.clone(),
        target: target.to_string(),
        rel,
        title,
        order: Some(order),
        note,
    });
    meta.touch();
    save_meta(&bundle, &src_dir, &meta)?;
    println!("已新增关联线 {ref_id}：{uuid} → {target}");
    Ok(())
}

/// 删除关联线。
///
/// 规范 §9 的形式是 `str ref rm <dir> <ref-uuid>`：只给关联线 id，由 CLI 在全 bundle 内定位
/// 它所属的源分支。`--uuid <源分支>` 可把搜索范围钉死在一个分支上（旧版 `--ref` 形式等价）。
pub fn ref_rm(dir: &Path, uuid: Option<String>, ref_id: &str) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let has_ref = |i: usize| -> bool {
        scan.visits[i]
            .meta
            .as_ref()
            .map(|m| m.refs.iter().any(|r| r.id == ref_id))
            .unwrap_or(false)
    };
    let idx = match uuid {
        Some(u) => {
            let i = locate(&scan, &u).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{u}`")))?;
            if !has_ref(i) {
                return Err(Error::BadArg(format!("分支 `{u}` 内找不到关联线 `{ref_id}`")));
            }
            i
        }
        None => {
            let hits: Vec<usize> = (0..scan.visits.len()).filter(|i| has_ref(*i)).collect();
            match hits.as_slice() {
                [only] => *only,
                [] => return Err(Error::BadArg(format!("找不到关联线 `{ref_id}`"))),
                _ => {
                    let rels: Vec<String> =
                        hits.iter().map(|i| scan.visits[*i].rel.clone()).collect();
                    return Err(Error::BadArg(format!(
                        "关联线 `{ref_id}` 在多个分支中出现（{}），请用 `--uuid` 指定源分支",
                        rels.join("、")
                    )));
                }
            }
        }
    };
    let rel = scan.visits[idx].rel.clone();
    let src_dir = scan.visits[idx].dir.clone();
    let mut meta = match bundle.read_meta(&src_dir)? {
        MetaLoad::Ok(m, _) => m,
        MetaLoad::Failed(_) => return Err(Error::BadArg("分支 `._meta` 解析失败".into())),
    };
    if !meta.remove_ref(ref_id) {
        return Err(Error::BadArg(format!("找不到关联线 `{ref_id}`")));
    }
    meta.touch();
    save_meta(&bundle, &src_dir, &meta)?;
    println!("已删除关联线 {ref_id}（源分支 {rel}）");
    Ok(())
}

// ─────────────────── meta / entry / author（写入既有字段）───────────────────

/// 允许的 `authors[].role`（规范 §4.4 与 `._schema` 枚举一致）。
const AUTHOR_ROLES: &[&str] = &["owner", "editor", "viewer", "agent"];

/// 读取目标分支的 `._meta`（供字段写入类命令复用）。
fn read_target_meta(bundle: &Bundle, dir: &Path) -> Result<Meta> {
    match bundle.read_meta(dir)? {
        MetaLoad::Ok(m, _) => Ok(*m),
        MetaLoad::Failed(_) => Err(Error::BadArg(format!(
            "{} 的 `._meta` 解析失败",
            bundle.rel(dir)
        ))),
    }
}

/// 至少给出一个字段，否则拒绝执行（避免「无参数空写」把 `revision` 白白推进）。
fn need_one(given: bool, hint: &str) -> Result<()> {
    if given {
        Ok(())
    } else {
        Err(Error::BadArg(format!("至少需要指定一个字段（{hint}）")))
    }
}

/// `str meta set`：设置分支自身（`._meta` 顶层）的元信息字段。
///
/// 空串表示**移除**该字段。字段语义见规范 §4.3。
pub fn meta_set(
    dir: &Path,
    uuid: Option<String>,
    type_: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    name: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<()> {
    need_one(
        type_.is_some() || title.is_some() || summary.is_some() || name.is_some() || tags.is_some(),
        "--type / --title / --summary / --name / --tags",
    )?;
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = target_branch(&scan, uuid.as_deref())?;
    let target_dir = scan.visits[idx].dir.clone();
    let rel = scan.visits[idx].rel.clone();
    let mut meta = read_target_meta(&bundle, &target_dir)?;

    if let Some(v) = &type_ {
        meta.set_str_or_remove("type", v);
    }
    if let Some(v) = &title {
        meta.set_str_or_remove("title", v);
    }
    if let Some(v) = &summary {
        meta.set_str_or_remove("summary", v);
    }
    if let Some(v) = &name {
        meta.set_str_or_remove("name", v);
    }
    if let Some(v) = &tags {
        // `--tags ""` → 清空；顺带滤掉空项（否则写出 `tags = [""]` 会被 Schema 拒绝）
        let cleaned: Vec<String> = v.iter().filter(|s| !s.is_empty()).cloned().collect();
        meta.set_str_array("tags", &cleaned);
    }

    meta.touch();
    save_meta(&bundle, &target_dir, &meta)?;
    println!("已更新 {rel} 的元信息");
    Ok(())
}

/// `str entry set` 的字段补丁：`None` 表示不改动，`Some("")` 表示移除该键。
#[derive(Debug, Clone, Default)]
pub struct EntryPatch {
    /// 子分支类型。
    pub type_: Option<String>,
    /// 展示名。
    pub title: Option<String>,
    /// 子分支摘要。
    pub summary: Option<String>,
    /// 备注。
    pub note: Option<String>,
    /// 同层排序键。
    pub order: Option<i64>,
}

impl EntryPatch {
    /// 是否一个字段都没给。
    pub fn is_empty(&self) -> bool {
        self.type_.is_none()
            && self.title.is_none()
            && self.summary.is_none()
            && self.note.is_none()
            && self.order.is_none()
    }
}

/// `str entry set`：设置某分支 `entries[]` 中指定 `path` 条目的字段。
///
/// `str sync` 补登出来的行只有 `path` / `role` / `id`，其 `type` / `title` / `summary`
/// 由此命令补齐 —— 不再需要「手改 `._meta` 的唯一例外」。空串表示移除该字段。
pub fn entry_set(dir: &Path, uuid: Option<String>, path: &str, patch: &EntryPatch) -> Result<()> {
    need_one(
        !patch.is_empty(),
        "--type / --title / --summary / --note / --order",
    )?;
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = target_branch(&scan, uuid.as_deref())?;
    let target_dir = scan.visits[idx].dir.clone();
    let rel = scan.visits[idx].rel.clone();
    let mut meta = read_target_meta(&bundle, &target_dir)?;

    let mut applied = false;
    for (key, val) in [
        ("type", &patch.type_),
        ("title", &patch.title),
        ("summary", &patch.summary),
        ("note", &patch.note),
    ] {
        if let Some(v) = val {
            applied |= meta.set_entry_str(path, key, v);
        }
    }
    if let Some(n) = patch.order {
        applied |= meta.set_entry_int(path, "order", Some(n));
    }
    if !applied {
        return Err(Error::BadArg(format!(
            "{rel} 的 `entries[]` 内找不到 `path` = {path:?}"
        )));
    }

    meta.touch();
    save_meta(&bundle, &target_dir, &meta)?;
    println!("已更新 {rel} 的条目 {path}");
    Ok(())
}

/// `str author add`：按 `id` 新增 / 覆盖一条 `[[authors]]`（规范 §4.4）。
pub fn author_add(
    dir: &Path,
    uuid: Option<String>,
    id: String,
    name: Option<String>,
    role: String,
    at: Option<String>,
) -> Result<()> {
    if !AUTHOR_ROLES.contains(&role.as_str()) {
        return Err(Error::BadArg(format!(
            "`--role` = {role:?} 非法（owner / editor / viewer / agent）"
        )));
    }
    let at = match at {
        Some(v) => {
            let dt = v.parse::<toml_edit::Datetime>().map_err(|_| {
                Error::BadArg(format!(
                    "`--at` = {v:?} 不是合法 offset date-time（如 2026-09-14T10:03:11+08:00）"
                ))
            })?;
            if dt.offset.is_none() {
                return Err(Error::BadArg(format!("`--at` = {v:?} 缺少时区偏移（规范 4.1）")));
            }
            Some(dt.to_string())
        }
        None => Some(util::now_rfc3339()),
    };

    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = target_branch(&scan, uuid.as_deref())?;
    let target_dir = scan.visits[idx].dir.clone();
    let rel = scan.visits[idx].rel.clone();
    let mut meta = read_target_meta(&bundle, &target_dir)?;

    let added = meta.upsert_author(&Author {
        id: id.clone(),
        name,
        role: role.clone(),
        at,
    });
    meta.touch();
    save_meta(&bundle, &target_dir, &meta)?;
    println!(
        "已{} {rel} 的协作者 {id}（role = {role}）",
        if added { "新增" } else { "更新" }
    );
    Ok(())
}

/// `str author rm`：按 `id` 删除一条 `[[authors]]`。
pub fn author_rm(dir: &Path, uuid: Option<String>, id: &str) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = target_branch(&scan, uuid.as_deref())?;
    let target_dir = scan.visits[idx].dir.clone();
    let rel = scan.visits[idx].rel.clone();
    let mut meta = read_target_meta(&bundle, &target_dir)?;
    if !meta.remove_author(id) {
        return Err(Error::BadArg(format!("{rel} 内找不到协作者 `{id}`")));
    }
    meta.touch();
    save_meta(&bundle, &target_dir, &meta)?;
    println!("已删除 {rel} 的协作者 {id}");
    Ok(())
}

// ─────────────────────────── sync ───────────────────────────

/// 用磁盘实际状态修正全部 `entries`，并更新 `size` / `sha256`。
pub fn sync(dir: &Path, dry_run: bool) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let mut changed = 0usize;
    let mut planned: Vec<String> = Vec::new();

    for v in &scan.visits {
        if v.meta.is_none() {
            continue;
        }
        let dir_path = v.dir.clone();
        let rel = v.rel.clone();
        let mut work = match bundle.read_meta(&v.dir)? {
            MetaLoad::Ok(m, _) => m,
            MetaLoad::Failed(_) => continue,
        };
        let real = crate::validate::real_entries(&dir_path);
        let declared: Vec<Entry> = work.entries.clone();
        let mut touched = false;

        // 补登
        for (name, is_dir) in &real {
            if declared.iter().any(|e| e.path == *name) {
                continue;
            }
            let path = dir_path.join(name);
            let e = if *is_dir {
                if bundle.has_meta(&path) {
                    Entry {
                        path: name.clone(),
                        role: if v.depth == 0 { "node" } else { "branch" }.into(),
                        id: Some(name.clone()),
                        order: Some(declared.len() as i64 + 1),
                        ..Default::default()
                    }
                } else {
                    Entry {
                        path: name.clone(),
                        role: "dir".into(),
                        count: child_count(&path),
                        ..Default::default()
                    }
                }
            } else {
                let meta_info = std::fs::metadata(&path).ok();
                let size = meta_info.map(|m| m.len() as i64);
                let sha = util::sha256_file(&path).ok();
                Entry {
                    path: name.clone(),
                    role: guess_file_role(name).into(),
                    media_type: media_type_for(name),
                    size,
                    sha256: sha,
                    ..Default::default()
                }
            };
            planned.push(format!("+ {rel}/{name}  role={}", e.role));
            work.upsert_entry(&e);
            touched = true;
        }

        // 移除已消失的条目 + 刷新指纹
        for e in &declared {
            let path = dir_path.join(&e.path);
            if !path.exists() {
                if !e.optional {
                    planned.push(format!("- {rel}/{}", e.path));
                    work.remove_entry_path(&e.path);
                    touched = true;
                }
                continue;
            }
            if !e.is_file_like() {
                continue;
            }
            let size = std::fs::metadata(&path).ok().map(|m| m.len() as i64);
            let sha = util::sha256_file(&path).ok();
            if size != e.size || sha != e.sha256 {
                planned.push(format!("~ {rel}/{}  指纹更新", e.path));
                let mut ne = e.clone();
                ne.size = size;
                ne.sha256 = sha;
                if ne.media_type.is_none() {
                    ne.media_type = media_type_for(&e.path);
                }
                work.upsert_entry(&ne);
                touched = true;
            }
        }

        if touched {
            work.sort_collections();
            work.touch();
            changed += 1;
            if !dry_run {
                work.save(&bundle.meta_path(&dir_path))?;
            }
        }
    }

    for line in &planned {
        println!("{line}");
    }
    if dry_run {
        println!("（dry-run）将更新 {changed} 份 `._meta`");
    } else {
        // `sync` 是「与磁盘对齐」的总入口：顺手把 `E_REVISION_STALE` 的基线刷成当前状态。
        if let Ok(after) = bundle.scan() {
            crate::baseline::record_scan(&bundle, &after);
        }
        println!("已更新 {changed} 份 `._meta`");
    }
    Ok(())
}

fn guess_file_role(name: &str) -> &'static str {
    match media_type_for(name).as_deref() {
        Some("application/json")
        | Some("application/toml")
        | Some("application/yaml")
        | Some("text/csv")
        | Some("text/markdown")
        | Some("text/plain") => "payload",
        _ => "asset",
    }
}

// ─────────────────────────── fmt ───────────────────────────

/// 按规范 §4.9 键序 / 表序重写 `._meta`（保注释）。
///
/// 规范化**不触碰** `revision` / `updated_at`，因此不影响 `E_REVISION_STALE` 基线。
pub fn fmt(dir: &Path, check: bool, strip_comments: bool) -> Result<i32> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let mut would_change = 0usize;
    for v in &scan.visits {
        if v.meta.is_none() {
            continue;
        }
        let meta = match bundle.read_meta(&v.dir)? {
            MetaLoad::Ok(m, _) => m,
            MetaLoad::Failed(_) => continue,
        };
        let text = meta.canonical_text(strip_comments);
        let path = bundle.meta_path(&v.dir);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current != text {
            would_change += 1;
            if !check {
                std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
                println!("已规范化 {}", v.rel);
            }
        }
    }
    if check {
        if would_change == 0 {
            println!("全部 `._meta` 已是规范形式");
            return Ok(0);
        }
        println!("{would_change} 份 `._meta` 需要规范化");
        return Ok(1);
    }
    if would_change == 0 {
        println!("全部 `._meta` 已是规范形式");
    }
    Ok(0)
}

// ─────────────────────────── norm / context / export ───────────────────────────

/// 输出归一化 JSON。
///
/// `--out -`（或缺省）写 stdout；给路径则写文件（规范 §9）。
pub fn norm(dir: &Path, uuid: Option<String>, out: Option<String>) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = target_branch(&scan, uuid.as_deref())?;
    let Some(meta) = scan.visits[idx].meta.as_ref() else {
        return Err(Error::BadArg("`._meta` 解析失败".into()));
    };
    let mut text = serde_json::to_string_pretty(&meta.to_json()).unwrap_or_default();
    text.push('\n');
    emit(&text, out.as_deref())
}

/// 生成供 AI 使用的上下文片段。
pub fn context(dir: &Path, uuid: &str, depth: usize, budget: usize) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = locate(&scan, uuid).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{uuid}`")))?;
    let mut out = String::new();
    out.push_str(&format!("# STR 上下文：{}\n\n", bundle.name()));
    render_context(&scan, idx, depth, &mut out, 0);
    if out.len() > budget {
        out.truncate(floor_char_boundary(&out, budget));
        out.push_str("\n…（已按 --budget 截断）\n");
    }
    print!("{out}");
    Ok(())
}

fn render_context(scan: &Scan, idx: usize, depth: usize, out: &mut String, level: usize) {
    let v = &scan.visits[idx];
    let Some(meta) = v.meta.as_ref() else {
        return;
    };
    let indent = "  ".repeat(level);
    out.push_str(&format!(
        "{indent}- `{}` **{}** ({}) depth={}\n",
        meta.id.clone().unwrap_or_default(),
        meta.title.clone().unwrap_or_else(|| v.rel.clone()),
        meta.r#type.clone().unwrap_or_else(|| "-".into()),
        v.depth
    ));
    if let Some(s) = &meta.summary {
        out.push_str(&format!("{indent}  {s}\n"));
    }
    if !meta.tags.is_empty() {
        out.push_str(&format!("{indent}  tags: {}\n", meta.tags.join(", ")));
    }
    let files: Vec<String> = meta
        .entries
        .iter()
        .filter(|e| !e.is_branch())
        .map(|e| format!("{}({})", e.path, e.role))
        .collect();
    if !files.is_empty() {
        out.push_str(&format!("{indent}  内容：{}\n", files.join("、")));
    }
    for r in &meta.refs {
        let t = scan
            .resolve(&r.target)
            .map(|i| scan.visits[i].rel.clone())
            .unwrap_or_else(|| r.target.clone());
        out.push_str(&format!("{indent}  关联线 → {t} ({})\n", r.rel));
    }
    if level >= depth {
        return;
    }
    let children = ordered_children(scan, idx);
    for c in children {
        render_context(scan, c, depth, out, level + 1);
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 导出为单一 JSON / TOML（派生数据，只读）。
///
/// `--out -`（或缺省）写 stdout；给路径则写文件，且**不得**落在 bundle 内部（规范 §9）。
pub fn export(
    dir: &Path,
    format: String,
    depth: Option<usize>,
    out: Option<String>,
) -> Result<()> {
    if !matches!(format.as_str(), "json" | "toml") {
        return Err(Error::BadArg(format!(
            "`--format` = {format:?} 非法（只允许 json / toml）"
        )));
    }
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let Some(root_idx) = scan.root_index else {
        return Err(Error::BadArg("bundle 缺少 `._meta`".into()));
    };
    let value = export_node(&scan, root_idx, depth);
    let mut text = if format == "toml" {
        toml_from_json(&value, 0)
    } else {
        serde_json::to_string_pretty(&value).unwrap_or_default()
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    match out.as_deref() {
        None | Some("-") => emit(&text, None),
        Some(p) => {
            let path = resolve_out_path(&bundle, p)?;
            std::fs::write(&path, text).map_err(|e| Error::io(&path, e))
        }
    }
}

fn export_node(scan: &Scan, idx: usize, depth: Option<usize>) -> JValue {
    let v = &scan.visits[idx];
    let meta = match v.meta.as_ref() {
        Some(m) => m.to_json(),
        None => serde_json::json!({ "id": null, "error": "parse_failed" }),
    };
    let children = ordered_children(scan, idx);
    let descend = depth.map(|d| v.depth < d).unwrap_or(true);
    let kids: Vec<JValue> = if descend {
        children.iter().map(|c| export_node(scan, *c, depth)).collect()
    } else {
        Vec::new()
    };
    serde_json::json!({
        "path": v.rel,
        "depth": v.depth,
        "meta": meta,
        "children": kids,
    })
}

/// 简易 JSON → TOML（仅用于 `str export --format toml`）。
fn toml_from_json(v: &JValue, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    match v {
        JValue::Object(m) => {
            let mut scalars = String::new();
            let mut tables = String::new();
            for (k, val) in m {
                match val {
                    JValue::Object(_) | JValue::Array(_) => {
                        tables.push_str(&format!("\n{pad}[{k}]\n{}", toml_from_json(val, indent + 1)));
                    }
                    _ => scalars.push_str(&format!("{pad}{k} = {}\n", toml_from_json(val, 0))),
                }
            }
            format!("{scalars}{tables}")
        }
        JValue::Array(items) => {
            if items.iter().all(|i| matches!(i, JValue::Object(_))) {
                let mut s = String::new();
                for it in items {
                    s.push_str(&format!("{pad}[[_item]]\n{}", toml_from_json(it, indent + 1)));
                }
                s
            } else {
                let inner: Vec<String> = items.iter().map(|i| toml_from_json(i, 0)).collect();
                format!("[{}]", inner.join(", "))
            }
        }
        JValue::String(s) => toml_str(s),
        JValue::Bool(b) => b.to_string(),
        JValue::Number(n) => n.to_string(),
        JValue::Null => "\"\"".to_string(),
    }
}

// ─────────────────────────── reveal ───────────────────────────

/// 平台适配：macOS 上把 `.str` 目录标记为 bundle，并让 `._meta` 可见。
pub fn reveal(dir: &Path) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let root = bundle.root.display().to_string();
    if cfg!(target_os = "macos") {
        // Finder 里显示包内容的前提是 bundle 位（SetFile 属于 Xcode CLI 工具）
        let status = std::process::Command::new("SetFile")
            .args(["-a", "B", &root])
            .status();
        match status {
            Ok(s) if s.success() => println!("已设置 bundle 位：{root}"),
            _ => println!(
                "未找到 `SetFile`（需 Xcode Command Line Tools）。手动执行：SetFile -a B {}",
                root
            ),
        }
        let mut n = 0;
        for v in &scan.visits {
            let p = bundle.meta_path(&v.dir);
            let _ = std::process::Command::new("chflags")
                .args(["nohidden", &p.display().to_string()])
                .status();
            n += 1;
        }
        println!("已取消 {n} 个 `._meta` 的隐藏标记");
    } else {
        println!("当前平台无 bundle 概念：`.str` 就是普通目录，`._meta` 为点文件（可能默认隐藏）。");
    }
    Ok(())
}

// ─────────────────────────── codes ───────────────────────────

/// 列出全部错误码（测试矩阵用）。
pub fn list_codes() {
    for c in code::ALL {
        println!("{c}");
    }
}

/// 便捷：把相对路径映射为绝对路径。
pub fn abs(p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}
