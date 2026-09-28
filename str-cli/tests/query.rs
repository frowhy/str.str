//! 查询命令测试：`str find` / `str grep` / `str tags` / `str where` / `str get`。
//!
//! 「定位 → 精读」链路（规范 §9）：断言收集段函数的返回值（打印层由 CLI 集成覆盖）。

use std::path::PathBuf;

use str_format::bundle::Bundle;
use str_format::cmd;
use str_format::cmd::EntryNew;

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("strquery-{name}-{}.str", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

/// fixture：ROOT（tags: crm,demo）→ 节点 Alpha（a.one，tags: vip，summary 含「重点客户」）
/// → 子分支 Beta（b.two，tags: followup）内含 payload `notes.md`（正文含「张伟」与「跟进」）。
/// ROOT 另有 payload `root.md`（正文含「张伟」）。返回 bundle 路径与 Alpha / Beta 的 id。
fn fixture(name: &str) -> (PathBuf, String, String) {
    let raw = tmp(name);
    let parent = raw.parent().unwrap().to_path_buf();
    let stem = raw.file_stem().unwrap().to_string_lossy().to_string();
    cmd::init(
        &parent.join(&stem),
        None,
        Some("查询测试束".into()),
        None,
        7,
    )
    .unwrap();
    cmd::meta_set(&raw, None, None, None, None, None, Some(vec!["crm".into(), "demo".into()])).unwrap();

    cmd::node_add(&raw, Some("a.one".into()), Some("Alpha".into()), Some("重点客户档案".into())).unwrap();
    let bundle = Bundle::new(raw.clone()).unwrap();
    let scan = bundle.scan().unwrap();
    let alpha = scan
        .visits
        .iter()
        .find(|v| v.depth == 1)
        .map(|v| v.dir.file_name().unwrap().to_string_lossy().to_string())
        .unwrap();
    cmd::meta_set(
        &raw,
        Some(alpha.clone()),
        None,
        None,
        None,
        None,
        Some(vec!["vip".into()]),
    )
    .unwrap();

    cmd::branch_add(&raw, Some(alpha.clone()), Some("b.two".into()), Some("Beta".into()), Some("跟进记录分支".into()), None).unwrap();
    let scan = bundle.scan().unwrap();
    let beta = scan
        .visits
        .iter()
        .find(|v| v.depth == 2)
        .map(|v| v.dir.file_name().unwrap().to_string_lossy().to_string())
        .unwrap();
    cmd::meta_set(
        &raw,
        Some(beta.clone()),
        None,
        None,
        None,
        None,
        Some(vec!["followup".into()]),
    )
    .unwrap();

    let notes = raw.join(&alpha).join(&beta).join("notes.md");
    std::fs::write(&notes, "line1 普通\n客户：张伟\nline3 跟进计划\n").unwrap();
    cmd::entry_add(&raw, Some(beta.clone()), "notes.md", &EntryNew::default()).unwrap();

    let root_md = raw.join("root.md");
    std::fs::write(&root_md, "ROOT 提到张伟\n").unwrap();
    cmd::entry_add(&raw, None, "root.md", &EntryNew::default()).unwrap();

    (raw, alpha, beta)
}

// ───────────────────────── find ─────────────────────────

#[test]
fn find_matches_branch_and_entry_fields() {
    let (root, alpha, _beta) = fixture("find");
    // 关键词命中分支自身 summary
    let hits = cmd::find_hits(&root, Some("重点客户".into()), None, &[], None, None, None, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["path"].as_str(), Some(alpha.as_str()));
    assert!(hits[0]["matched"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m.as_str() == Some("summary")));

    // 关键词命中子分支标题（跟进记录分支）
    let hits = cmd::find_hits(&root, Some("跟进".into()), None, &[], None, None, None, None).unwrap();
    assert!(hits.iter().any(|h| h["title"].as_str() == Some("Beta")));

    // 不区分大小写
    let hits = cmd::find_hits(&root, Some("ALPHA".into()), None, &[], None, None, None, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["title"].as_str(), Some("Alpha"));
}

#[test]
fn find_filters_by_tag_type_and_field() {
    let (root, alpha, _beta) = fixture("findfilter");
    // 标签过滤：vip 只在 Alpha 上
    let hits = cmd::find_hits(&root, None, None, &["vip".into()], None, None, None, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["id"].as_str(), Some(alpha.as_str()));

    // 多标签 = 交集
    let hits = cmd::find_hits(
        &root,
        None,
        None,
        &["vip".into(), "crm".into()],
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(hits.is_empty(), "crm 在 ROOT、vip 在 Alpha，无交集");

    // type 精确过滤
    let hits = cmd::find_hits(&root, None, Some("b.two"), &[], None, None, None, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["title"].as_str(), Some("Beta"));

    // --field 限定：只搜标题时，summary 命中不再算
    let hits = cmd::find_hits(&root, Some("重点".into()), None, &[], Some("title"), None, None, None).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn find_respects_depth_and_limit() {
    let (root, _alpha, _beta) = fixture("finddepth");
    let all = cmd::find_hits(&root, None, None, &[], None, None, None, None).unwrap();
    assert_eq!(all.len(), 3, "ROOT + Alpha + Beta");
    let deep = cmd::find_hits(&root, None, None, &[], None, None, Some(1), None).unwrap();
    assert_eq!(deep.len(), 2, "depth ≤ 1：ROOT + Alpha");
    let limited = cmd::find_hits(&root, None, None, &[], None, None, None, Some(2)).unwrap();
    assert_eq!(limited.len(), 2);
    // 排序：depth 升序，ROOT 在前
    assert_eq!(all[0]["path"].as_str(), Some("."));
}

// ───────────────────────── grep ─────────────────────────

#[test]
fn grep_finds_payload_text_with_branch_context() {
    let (root, alpha, beta) = fixture("grep");
    let (hits, stopped) = cmd::grep_hits(&root, "张伟", None, false, false, None, None).unwrap();
    assert!(!stopped);
    assert_eq!(hits.len(), 2, "root.md 与 notes.md 各一处");
    assert_eq!(hits[0]["branch"]["path"].as_str(), Some("."));
    assert_eq!(hits[0]["entry"].as_str(), Some("root.md"));
    assert_eq!(hits[0]["line"].as_u64(), Some(1));
    let second = hits
        .iter()
        .find(|h| h["branch"]["path"].as_str() != Some("."))
        .unwrap();
    assert_eq!(
        second["branch"]["path"].as_str(),
        Some(format!("{alpha}/{beta}").as_str())
    );
    assert_eq!(second["entry"].as_str(), Some("notes.md"));
    assert_eq!(second["line"].as_u64(), Some(2));
}

#[test]
fn grep_ignore_case_and_glob() {
    let (root, _alpha, _beta) = fixture("grepcase");
    // fixture 里没有大写差异词，用「客户」/ 不存在的「LINE1」验证大小写开关
    assert!(cmd::grep_hits(&root, "LINE1", None, false, false, None, None).unwrap().0.is_empty());
    let (hits, _) = cmd::grep_hits(&root, "LINE1", None, true, false, None, None).unwrap();
    assert_eq!(hits.len(), 1);

    // --glob 限定：*.md 全部命中，而伪装的过滤条件 *.txt 应无命中
    let (hits, _) = cmd::grep_hits(&root, "张伟", Some("*.txt"), false, false, None, None).unwrap();
    assert!(hits.is_empty());
    let (hits, _) = cmd::grep_hits(&root, "张伟", Some("*.md"), false, false, None, None).unwrap();
    assert_eq!(hits.len(), 2);
}

#[test]
fn grep_limit_and_binary_skip() {
    let (root, alpha, beta) = fixture("greplimit");
    let (hits, stopped) = cmd::grep_hits(&root, "张伟", None, false, false, None, Some(1)).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(stopped);

    // 二进制文件（含 NUL）跳过
    let bin = root.join(&alpha).join(&beta).join("blob.bin");
    std::fs::write(&bin, [0x00u8, 0x01, 0x02]).unwrap();
    cmd::entry_add(&root, Some(beta.clone()), "blob.bin", &EntryNew::default()).unwrap();
    let (hits, _) = cmd::grep_hits(&root, "张伟", None, false, false, None, None).unwrap();
    assert_eq!(hits.len(), 2, "二进制文件不参与检索");
}

// ───────────────────────── tags ─────────────────────────

#[test]
fn tags_counts_branch_level_tags() {
    let (root, _alpha, _beta) = fixture("tags");
    let rows = cmd::tag_counts(&root).unwrap();
    assert_eq!(
        rows,
        vec![
            ("crm".into(), 1),
            ("demo".into(), 1),
            ("followup".into(), 1),
            ("vip".into(), 1),
        ]
    );
}

// ───────────────────────── where ─────────────────────────

#[test]
fn where_builds_breadcrumb_chain() {
    let (root, alpha, beta) = fixture("where");
    let j = cmd::where_json(&root, Some(beta.clone())).unwrap();
    assert!(j["bundle"]
        .as_str()
        .unwrap()
        .starts_with("strquery-where"));
    let chain = j["chain"].as_array().unwrap();
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0]["path"].as_str(), Some("."));
    assert_eq!(chain[1]["path"].as_str(), Some(alpha.as_str()));
    assert_eq!(
        chain[2]["path"].as_str(),
        Some(format!("{alpha}/{beta}").as_str())
    );
    assert_eq!(j["target"]["id"].as_str(), Some(beta.as_str()));
}

// ───────────────────────── get ─────────────────────────

#[test]
fn get_returns_registered_entry_body_and_info() {
    let (root, _alpha, _beta) = fixture("get");
    let bytes = cmd::get_bytes(&root, None, "root.md").unwrap();
    assert_eq!(bytes, "ROOT 提到张伟\n".as_bytes());

    let info = cmd::get_info(&root, None, "root.md").unwrap();
    assert_eq!(info["entry"]["role"].as_str(), Some("payload"));
    assert!(info["entry"]["sha256"].as_str().is_some());
    assert_eq!(info["branch"]["path"].as_str(), Some("."));
}

#[test]
fn get_rejects_unregistered_and_structural_entries() {
    let (root, _alpha, _beta) = fixture("getreject");
    // 未登记
    assert!(cmd::get_bytes(&root, None, "nope.md").is_err());
    // 路径不合法（含分隔符）
    assert!(cmd::get_bytes(&root, None, "a/b.md").is_err());
    // 子分支条目（role = node）拒绝
    let bundle = Bundle::new(root.clone()).unwrap();
    let scan = bundle.scan().unwrap();
    let alpha = scan
        .visits
        .iter()
        .find(|v| v.depth == 1)
        .map(|v| v.dir.file_name().unwrap().to_string_lossy().to_string())
        .unwrap();
    assert!(cmd::get_bytes(&root, None, &alpha).is_err(), "role = node 拒绝");
}

// ───────────────── 未登记子项与实际路径 ─────────────────

/// 在 Beta 分支内建内容文件夹 `reports/`（登记为 role=dir），
/// 其中的 `r.md` / `deep/note.txt` 均为**未登记**子项。
fn with_content_folder(name: &str) -> (PathBuf, String, String) {
    let (root, alpha, beta) = fixture(name);
    let beta_dir = root.join(&alpha).join(&beta);
    std::fs::create_dir_all(beta_dir.join("reports/deep")).unwrap();
    std::fs::write(beta_dir.join("reports/r.md"), "报告正文：季度目标\n").unwrap();
    std::fs::write(beta_dir.join("reports/deep/note.txt"), "深层笔记提到张伟\n").unwrap();
    cmd::entry_add(&root, Some(beta.clone()), "reports", &EntryNew::default()).unwrap();
    (root, alpha, beta)
}

#[test]
fn grep_covers_unregistered_files_under_content_folders() {
    let (root, _alpha, _beta) = with_content_folder("grepdir");
    // 缺省口径：未登记子项参与检索
    let (hits, _) = cmd::grep_hits(&root, "季度目标", None, false, false, None, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["entry"].as_str(), Some("reports/r.md"));
    assert_eq!(hits[0]["registered"].as_bool(), Some(false));
    // file 字段是绝对路径且真实存在
    let file = hits[0]["file"].as_str().unwrap();
    assert!(std::path::Path::new(file).is_file());

    // 多层未登记子目录同样覆盖
    let (hits, _) = cmd::grep_hits(&root, "张伟", None, false, false, None, None).unwrap();
    assert!(hits
        .iter()
        .any(|h| h["entry"].as_str() == Some("reports/deep/note.txt")));

    // --manifest-only 回到旧口径：未登记子项不再命中
    let (hits, _) = cmd::grep_hits(&root, "季度目标", None, false, true, None, None).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn grep_respects_ignore_and_branch_boundaries() {
    let (root, alpha, beta) = with_content_folder("grepignore");
    // .gitignore 排除 reports/
    std::fs::write(root.join(".gitignore"), "reports/\n").unwrap();
    let (hits, _) = cmd::grep_hits(&root, "季度目标", None, false, false, None, None).unwrap();
    assert!(hits.is_empty(), "被 .gitignore 剪中的路径不参与检索");
    std::fs::remove_file(root.join(".gitignore")).unwrap();

    // 不进入其它分支目录：在 Alpha 的另一个未登记文件放关键词，
    // 该文件只由 Alpha 自己的 visit 命中一次（Beta 的遍历不进入 Alpha 目录）
    std::fs::write(root.join(&alpha).join("stray.md"), "散落文件：季度目标\n").unwrap();
    let (hits, _) = cmd::grep_hits(&root, "季度目标", None, false, false, None, None).unwrap();
    // 两处命中：Alpha 的 stray.md（未登记）+ Beta 的 reports/r.md（.gitignore 已移除）
    assert_eq!(hits.len(), 2);
    let stray = hits
        .iter()
        .find(|h| h["entry"].as_str() == Some("stray.md"))
        .unwrap();
    assert_eq!(stray["branch"]["path"].as_str(), Some(alpha.as_str()));
    assert_eq!(stray["registered"].as_bool(), Some(false));
}

#[test]
fn get_reads_unregistered_children_of_registered_dir() {
    let (root, _alpha, beta) = with_content_folder("getdir");
    // 多段路径直读未登记子项（目标 = Beta 分支）
    let bytes = cmd::get_bytes(&root, Some(beta.clone()), "reports/r.md").unwrap();
    assert_eq!(bytes, "报告正文：季度目标\n".as_bytes());
    // --info 标注 registered = false，指纹按磁盘实算
    let info = cmd::get_info(&root, Some(beta.clone()), "reports/r.md").unwrap();
    assert_eq!(info["registered"].as_bool(), Some(false));
    assert_eq!(info["entry"]["role"].as_str(), Some("payload"));
    assert!(info["entry"]["sha256"].as_str().is_some());
    // 首段不是已登记 dir → 拒绝；目录本身 → 拒绝并指路
    assert!(cmd::get_bytes(&root, Some(beta.clone()), "nope/r.md").is_err());
    assert!(
        cmd::get_bytes(&root, Some(beta.clone()), "reports").is_err(),
        "dir 条目本身拒绝"
    );
    assert!(
        cmd::get_bytes(&root, Some(beta.clone()), "reports/deep").is_err(),
        "子目录拒绝"
    );
}

#[test]
fn find_and_grep_emit_absolute_paths() {
    let (root, alpha, _beta) = fixture("realpath");
    let hits = cmd::find_hits(&root, Some("Alpha".into()), None, &[], None, None, None, None).unwrap();
    let abs = hits[0]["path_abs"].as_str().unwrap();
    assert!(std::path::Path::new(abs).is_dir(), "path_abs 是真实存在的目录");
    assert!(abs.ends_with(alpha.as_str()));

    let (ghits, _) = cmd::grep_hits(&root, "张伟", None, false, false, None, None).unwrap();
    let file = ghits[0]["file"].as_str().unwrap();
    assert!(std::path::Path::new(file).is_file(), "file 是真实存在的文件");
}

// ───────────────────────── 检索范围（scope） ─────────────────────────

#[test]
fn scope_limits_find_and_grep_to_subtree() {
    let (root, alpha, beta) = fixture("scope");
    // 缺省 = 全 bundle（[dir] 为 bundle 根）
    let all = cmd::find_hits(&root, Some("跟进".into()), None, &[], None, None, None, None).unwrap();
    assert!(all.iter().any(|h| h["title"].as_str() == Some("Beta")));

    // --scope 按 uuid：限定 Alpha 子树
    let hits =
        cmd::find_hits(&root, Some("跟进".into()), None, &[], None, Some(&alpha), None, None)
            .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["title"].as_str(), Some("Beta"));

    // --scope 按 bundle 相对路径（alpha/beta）
    let rel = format!("{alpha}/{beta}");
    let hits =
        cmd::find_hits(&root, None, None, &[], None, Some(rel.as_str()), None, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["id"].as_str(), Some(beta.as_str()));

    // scope = "." 即 ROOT 子树 = 整棵 bundle
    let hits = cmd::find_hits(&root, None, None, &[], None, Some("."), None, None).unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0]["path"].as_str(), Some("."));

    // [dir] 指向分支目录 → 缺省范围为该分支子树（当前节点规则）
    let alpha_dir = root.join(&alpha);
    let hits =
        cmd::find_hits(&alpha_dir, Some("跟进".into()), None, &[], None, None, None, None)
            .unwrap();
    assert_eq!(hits.len(), 1, "只搜 Alpha 子树，不含 ROOT");
    assert_eq!(hits[0]["title"].as_str(), Some("Beta"));

    // grep 同样受 scope 限定：Alpha 子树内无 root.md
    let (hits, _) = cmd::grep_hits(&root, "张伟", None, false, false, Some(&alpha), None).unwrap();
    assert_eq!(hits.len(), 1, "只命中 notes.md，不含 root.md");
    assert_eq!(hits[0]["entry"].as_str(), Some("notes.md"));

    // 无法解析的 scope → BadArg
    assert!(cmd::find_hits(&root, None, None, &[], None, Some("nope"), None, None).is_err());
}

