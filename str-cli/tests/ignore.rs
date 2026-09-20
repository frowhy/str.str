//! 忽略名单测试（规范 v1.13.0 §4.7 / §4.8 / DoD 第 29 项）。
//!
//! 覆盖：`.gitignore` 自动检测（bundle 根 / 外层仓库 / 分支目录内）、
//! `policies.ignore`、`gitignore = false` 关闭检测、`!` 取反、
//! 「已登记条目不受忽略名单影响」。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use str_format::bundle::Bundle;
use str_format::util;
use str_format::validate::validate;

const ROOT_ID: &str = "01928f3a-7c4b-7000-8000-000000000000";
const A: &str = "01928f3a-7c4b-7001-8a01-000000000001";

fn tmp_bundle(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("strtest-ig-{name}-{}.str", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn tmp_dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("strtest-ig-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn w(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

/// `._meta` 骨架。
fn meta_text(kind: &str, id: &str, extra_bare: &str, tables: &str) -> String {
    format!(
        "str = 1\nspec = \"1.5.0\"\nkind = \"{kind}\"\nid = \"{id}\"\nname = \"T\"\nrevision = 1\ncreated_at = 2026-09-01T09:00:00+08:00\nupdated_at = 2026-09-01T09:00:00+08:00\n{extra_bare}\n{tables}\n"
    )
}

fn node_entry(path: &str, extra: &str) -> String {
    format!("[[entries]]\npath = \"{path}\"\nrole = \"node\"\nid = \"{path}\"\n{extra}\n")
}

fn payload_entry(path: &str, body: &str, media: &str) -> String {
    format!(
        "[[entries]]\npath = \"{path}\"\nrole = \"payload\"\nmedia_type = \"{media}\"\nsize = {}\nsha256 = \"{}\"\n",
        body.len(),
        util::sha256_bytes(body.as_bytes())
    )
}

/// 干净基线：ROOT + 节点 A（含 data.json）。
fn baseline(name: &str) -> PathBuf {
    let root = tmp_bundle(name);
    let body = "{\"a\":1}\n";
    w(
        &root,
        "._meta",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &format!("[policies]\n\n{}", node_entry(A, "type = \"crm.customer\"\nsummary = \"s\"")),
        ),
    );
    w(
        &root,
        &format!("{A}/._meta"),
        &meta_text(
            "node",
            A,
            "type = \"crm.customer\"\nsummary = \"s\"",
            &payload_entry("data.json", body, "application/json"),
        ),
    );
    w(&root, &format!("{A}/data.json"), body);
    root
}

fn codes(root: &Path) -> (HashSet<String>, String) {
    let bundle = Bundle::new(root.to_path_buf()).unwrap();
    let report = validate(&bundle).unwrap();
    (
        report.issues.iter().map(|i| i.code.clone()).collect(),
        report.to_text(),
    )
}

#[track_caller]
fn assert_clean(root: &Path) {
    let (set, text) = codes(root);
    assert!(
        !set.iter().any(|c| c.starts_with("E_") || c.starts_with("W_")),
        "期望零错误零告警，实际：\n{text}"
    );
}

#[track_caller]
fn assert_contains(root: &Path, code: &str) {
    let (set, text) = codes(root);
    assert!(set.contains(code), "期望出现 {code}，实际报告：\n{text}");
}

/// bundle 根 `.gitignore` 豁免未登记条目（validate + sync 双向）。
#[test]
fn gitignore_exempts_unregistered_entries() {
    let root = baseline("gi-exempt");
    w(&root, &format!("{A}/build/out.o"), "binary");
    assert_contains(&root, "E_MANIFEST_MISSING");

    w(&root, ".gitignore", "build/\n");
    assert_clean(&root);

    // sync 不补登被忽略条目
    let bundle = Bundle::new(root.to_path_buf()).unwrap();
    str_format::cmd::sync(&root, false).unwrap();
    let scan = bundle.scan().unwrap();
    let a = scan.resolve(A).unwrap();
    let meta = scan.visits[a].meta.as_ref().unwrap();
    assert!(
        !meta.entries.iter().any(|e| e.path == "build"),
        "被忽略目录不应被 sync 补登"
    );
}

/// `policies.ignore` 同样豁免，且优先级高于 `.gitignore` 的取反。
#[test]
fn policies_ignore_exempts_and_overrides() {
    let root = baseline("pol-ignore");
    w(&root, &format!("{A}/dist/x.bin"), "x");
    w(&root, ".gitignore", "!dist\n");
    assert_contains(&root, "E_MANIFEST_MISSING");

    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]\n", "[policies]\nignore = [\"dist\"]\n"),
    );
    assert_clean(&root);
}

/// `policies.gitignore = false` 关闭自动检测。
#[test]
fn gitignore_can_be_disabled() {
    let root = baseline("gi-off");
    w(&root, &format!("{A}/build/out.o"), "binary");
    w(&root, ".gitignore", "build/\n");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]\n", "[policies]\ngitignore = false\n"),
    );
    assert_contains(&root, "E_MANIFEST_MISSING");
}

/// 外层 git 仓库的 `.gitignore` 自动生效（basename 模式任意深度；锚定模式不越界）。
#[test]
fn outer_repo_gitignore_applies() {
    let repo = tmp_dir("outer-repo");
    w(&repo, ".git", ""); // .git 存在 → 向上收集到此为止
    w(&repo, ".gitignore", "*.log\n/outsideroot\n");
    // baseline 建在临时目录，再整体搬进 repo 作为 bundle.str
    let staged = baseline("outer");
    let moved = repo.join("bundle.str");
    std::fs::rename(&staged, &moved).unwrap();

    w(&moved, &format!("{A}/a.log"), "log");
    assert_clean(&moved);

    // 锚定模式 /outsideroot 只作用于 repo 根，不影响 bundle 内同名目录
    w(&moved, "outsideroot/x.txt", "x");
    let (set, _) = codes(&moved);
    assert!(
        set.contains("E_MANIFEST_MISSING"),
        "外层锚定模式不应命中 bundle 内同名条目"
    );
}

/// 系统级忽略（规范 4.7）：`._meta` / `._schema/` / `._cache/` 等系统条目恒被忽略，
/// 用户 `.gitignore` / `policies.ignore` 的 `!` 取反不能恢复它们。
#[test]
fn system_entries_cannot_be_resurrected() {
    let root = baseline("sys-ignore");
    w(&root, "._cache/rev.json", "{}");
    w(&root, "._schema/extra.json", "{}");
    w(&root, ".gitignore", "!._*\n");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace(
            "[policies]\n",
            "[policies]\nignore = [\"!._cache\", \"!._schema\"]\n",
        ),
    );
    assert_clean(&root);
}

/// 忽略名单不是删除开关：已登记条目被删除仍报 `E_MANIFEST_GHOST`。
#[test]
fn ignore_does_not_silence_ghosts() {
    let root = baseline("ghost");
    w(&root, ".gitignore", "data.json\n"); // 连 data.json 一起忽略也无妨
    std::fs::remove_file(root.join(A).join("data.json")).unwrap();
    assert_contains(&root, "E_MANIFEST_GHOST");
}
