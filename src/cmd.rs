//! CLI 子命令实现（规范第 9 章）。

use std::path::{Path, PathBuf};

use serde_json::Value as JValue;

use crate::bundle::{Bundle, Scan};
use crate::error::{Error, Result, code};
use crate::meta::{Entry, Kind, MetaLoad, RefItem};
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
) -> Result<()> {
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
    let root_id = util::new_uuid_v7();
    let text = render_root_meta(
        &bundle_name,
        title.as_deref(),
        summary.as_deref(),
        &root_id,
        &created,
        crate::EMBEDDED_SCHEMAS.len(),
    );
    let meta = meta_from_text(&text)?;
    meta.save(&target.join(util::META_FILE))?;

    println!("已创建 bundle：{}", target.display());
    println!("  spec = {SPEC_VERSION}  str = {STR_MAJOR}");
    println!(
        "  {} 内已写入 {} 份校验 Schema",
        SCHEMA_DIR,
        crate::EMBEDDED_SCHEMAS.len()
    );
    Ok(())
}

// ─────────────────────────── validate ───────────────────────────

/// 校验整个 bundle。
pub fn validate(dir: &Path, strict: bool, json: bool, fix_manifest: bool) -> Result<i32> {
    let bundle = open(dir)?;
    if fix_manifest {
        sync(dir, true)?;
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
pub fn tree(dir: &Path, max_depth: Option<usize>, show_refs: bool) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    println!("{}", bundle.name());
    if scan.root_index.is_none() {
        println!("  （缺少 `._meta`，无法渲染）");
        return Ok(());
    }
    render_children(&scan, 0, "", max_depth, show_refs);
    Ok(())
}

fn render_children(
    scan: &Scan,
    idx: usize,
    prefix: &str,
    max_depth: Option<usize>,
    show_refs: bool,
) {
    let Some(meta) = scan.visits[idx].meta.as_ref() else {
        return;
    };
    let children: Vec<usize> = scan
        .visits
        .iter()
        .enumerate()
        .filter(|(_, v)| v.parent == Some(idx))
        .map(|(i, _)| i)
        .collect();
    if show_refs {
        for r in &meta.refs {
            let target = scan
                .resolve(&r.target)
                .map(|i| scan.visits[i].rel.clone())
                .unwrap_or_else(|| format!("{}（未解析）", r.target));
            println!("{prefix}⇢ 关联: {target}  --{}--", r.rel);
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
        let mark = if last { "└─ " } else { "├─ " };
        let mut line = format!("{prefix}{mark}[{}] {}", n + 1, title);
        if !type_.is_empty() {
            line.push_str(&format!("  ({type_})"));
        }
        if v.meta.is_none() {
            line.push_str("  ⚠ 解析失败");
        }
        println!("{line}");
        if max_depth.map(|d| v.depth < d).unwrap_or(true) {
            let next_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
            render_children(scan, *ci, &next_prefix, max_depth, show_refs);
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
    let root_meta_path = bundle.meta_path(&bundle.root);
    let root = match bundle.read_meta(&bundle.root)? {
        MetaLoad::Ok(m, _) => m,
        MetaLoad::Failed(_) => {
            return Err(Error::BadArg("root `._meta` 解析失败，无法新增节点".into()));
        }
    };
    if root.kind != Some(Kind::Root) {
        return Err(Error::BadArg("root `._meta` 的 kind 不是 root".into()));
    }
    let id = util::new_uuid_v7();
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
    child.save(&bundle.meta_path(&child_dir))?;

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
    root.save(&root_meta_path)?;
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
    let anchor_meta_path = bundle.meta_path(&anchor_dir);
    let mut parent = match bundle.read_meta(&anchor_dir)? {
        MetaLoad::Ok(m, _) => m,
        MetaLoad::Failed(_) => return Err(Error::BadArg("锚点 `._meta` 解析失败".into())),
    };

    let id = util::new_uuid_v7();
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
    meta_from_text(&text)?.save(&bundle.meta_path(&child_dir))?;

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
    parent.save(&anchor_meta_path)?;
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
        parent.save(&bundle.meta_path(&parent_dir))?;
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
    meta.save(&bundle.meta_path(&src_dir))?;
    println!("已新增关联线 {ref_id}：{uuid} → {target}");
    Ok(())
}

/// 删除关联线。
pub fn ref_rm(dir: &Path, uuid: &str, ref_id: &str) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = locate(&scan, uuid).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{uuid}`")))?;
    let src_dir = scan.visits[idx].dir.clone();
    let mut meta = match bundle.read_meta(&src_dir)? {
        MetaLoad::Ok(m, _) => m,
        MetaLoad::Failed(_) => return Err(Error::BadArg("分支 `._meta` 解析失败".into())),
    };
    if !meta.remove_ref(ref_id) {
        return Err(Error::BadArg(format!("找不到关联线 `{ref_id}`")));
    }
    meta.touch();
    meta.save(&bundle.meta_path(&src_dir))?;
    println!("已删除关联线 {ref_id}");
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

/// 按规范键序 / 表序重写 `._meta`（保注释）。
pub fn fmt(dir: &Path, check: bool, strip_comments: bool) -> Result<i32> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let mut would_change = 0usize;
    for v in &scan.visits {
        if v.meta.is_none() {
            continue;
        }
        let mut meta = match bundle.read_meta(&v.dir)? {
            MetaLoad::Ok(m, _) => m,
            MetaLoad::Failed(_) => continue,
        };
        meta.sort_collections();
        let mut text = meta.doc.to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if strip_comments {
            text = strip_line_comments(&text);
        }
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

/// 尽力移除整行 `#` 注释（不处理字符串内的 `#`）。
fn strip_line_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ─────────────────────────── norm / context / export ───────────────────────────

/// 输出归一化 JSON。
pub fn norm(dir: &Path, uuid: Option<String>) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let idx = match uuid {
        Some(u) => locate(&scan, &u).ok_or_else(|| Error::BadArg(format!("找不到分支 id `{u}`")))?,
        None => scan
            .root_index
            .ok_or_else(|| Error::BadArg("bundle 缺少 `._meta`".into()))?,
    };
    let Some(meta) = scan.visits[idx].meta.as_ref() else {
        return Err(Error::BadArg("`._meta` 解析失败".into()));
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&meta.to_json()).unwrap_or_default()
    );
    Ok(())
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
    let children: Vec<usize> = scan
        .visits
        .iter()
        .enumerate()
        .filter(|(_, c)| c.parent == Some(idx))
        .map(|(i, _)| i)
        .collect();
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
pub fn export(dir: &Path, format: String, depth: Option<usize>) -> Result<()> {
    let bundle = open(dir)?;
    let scan = bundle.scan()?;
    let Some(root_idx) = scan.root_index else {
        return Err(Error::BadArg("bundle 缺少 `._meta`".into()));
    };
    let value = export_node(&scan, root_idx, depth);
    if format == "toml" {
        println!("{}", toml_from_json(&value, 0));
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
    }
    Ok(())
}

fn export_node(scan: &Scan, idx: usize, depth: Option<usize>) -> JValue {
    let v = &scan.visits[idx];
    let meta = match v.meta.as_ref() {
        Some(m) => m.to_json(),
        None => serde_json::json!({ "id": null, "error": "parse_failed" }),
    };
    let children: Vec<usize> = scan
        .visits
        .iter()
        .enumerate()
        .filter(|(_, c)| c.parent == Some(idx))
        .map(|(i, _)| i)
        .collect();
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
