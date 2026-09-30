//! `str sync` × 链接条目（规范 §4.6.1）与删除门禁（规范 §9）测试矩阵。
//!
//! 覆盖三类用例：
//! 1. 软连接（`role = "link"`）条目**不得**被 `sync` 当作「已消失」移除
//!    （本文件对应现场缺陷：GUI 建立的跨分支软链接被一次 `sync` 清除 86 条）；
//! 2. 普通 path 条目的对账行为不变（补登 + 移除）；
//! 3. 批量删除门禁：移除条目数 ≥ [`str_format::cmd::SYNC_REMOVE_GATE`] 且未带
//!    `--yes` 时拒绝执行、一字不写。

use std::path::{Path, PathBuf};

use str_format::bundle::Bundle;
use str_format::util;
use str_format::validate::validate;

const ROOT_ID: &str = "01928f3a-7c4b-7000-8000-000000000000";
const A: &str = "01928f3a-7c4b-7001-8a01-000000000001";
const B: &str = "01928f3a-7c4b-7002-8a02-000000000002";

// ─────────────────────────── 构造辅助 ───────────────────────────

fn tmp_bundle(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "strtest-{name}-{}.str",
        std::process::id()
    ));
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

/// `._meta` 骨架（裸键在前，表在后 —— TOML 要求）。
fn meta_text(kind: &str, id: &str, extra_bare: &str, tables: &str) -> String {
    format!(
        "str = 1\nspec = \"1.5.0\"\nkind = \"{kind}\"\nid = \"{id}\"\nname = \"T\"\nrevision = 1\ncreated_at = 2026-09-01T09:00:00+08:00\nupdated_at = 2026-09-01T09:00:00+08:00\n{extra_bare}\n{tables}\n"
    )
}

fn node_entry(path: &str, extra: &str) -> String {
    format!("[[entries]]\npath = \"{path}\"\nrole = \"node\"\nid = \"{path}\"\n{extra}\n")
}

/// 链接条目（`role = "link"`）：`path` = 目标 id，**不带 id**（规范 §4.6.1）。
fn link_entry(path: &str) -> String {
    format!("[[entries]]\npath = \"{path}\"\nrole = \"link\"\ntarget = \"{path}\"\nmode = \"soft\"\n")
}

fn payload_entry(path: &str, body: &str) -> String {
    format!(
        "[[entries]]\npath = \"{path}\"\nrole = \"payload\"\nmedia_type = \"application/json\"\nsize = {}\nsha256 = \"{}\"\n",
        body.len(),
        util::sha256_bytes(body.as_bytes())
    )
}

/// 干净基线：ROOT + 独立节点 A（含一个 payload）。
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
            &format!(
                "[policies]\n\n{}",
                node_entry(A, "type = \"crm.customer\"\nsummary = \"s\"")
            ),
        ),
    );
    w(
        &root,
        &format!("{A}/._meta"),
        &meta_text(
            "node",
            A,
            "type = \"crm.customer\"\nsummary = \"s\"",
            &payload_entry("data.json", body),
        ),
    );
    w(&root, &format!("{A}/data.json"), body);
    root
}

/// ROOT + 两个独立节点 A / B（挂载用例需要第二个分支当目标）。
fn two_nodes(name: &str) -> PathBuf {
    let root = baseline(name);
    let body = "{\"b\":2}\n";
    let root_meta = std::fs::read_to_string(root.join("._meta")).unwrap();
    let node_b = node_entry(B, "type = \"crm.tag\"\nsummary = \"s\"");
    w(
        &root,
        "._meta",
        &root_meta.replace(
            &node_entry(A, "type = \"crm.customer\"\nsummary = \"s\""),
            &format!(
                "{node_b}{}",
                node_entry(A, "type = \"crm.customer\"\nsummary = \"s\"")
            ),
        ),
    );
    w(
        &root,
        &format!("{B}/._meta"),
        &meta_text(
            "node",
            B,
            "type = \"crm.tag\"\nsummary = \"s\"",
            &payload_entry("b.json", body),
        ),
    );
    w(&root, &format!("{B}/b.json"), body);
    root
}

fn append_entries(root: &Path, branch: &str, extra: &str) {
    let p = root.join(branch).join("._meta");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("{text}{extra}\n")).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn error_count(root: &Path) -> usize {
    let bundle = Bundle::new(root.to_path_buf()).unwrap();
    validate(&bundle).unwrap().error_count()
}

// ─────────────────────────── 用例 ───────────────────────────

/// 现场缺陷回归：跨分支软链接条目必须原样存活于 `sync`（含幂等），校验保持干净。
#[test]
fn sync_keeps_soft_link_entries() {
    let root = two_nodes("synclink");
    append_entries(&root, A, &link_entry(B));
    assert_eq!(error_count(&root), 0);

    let meta = root.join(A).join("._meta");
    let before = read(&meta);

    str_format::cmd::sync(&root, false, true).unwrap();

    let after = read(&meta);
    assert!(after.contains(&format!("role = \"link\"")), "{after}");
    assert!(after.contains(&format!("path = \"{B}\"")), "{after}");
    // A 分支无需任何变更 → 不得写盘（`revision` / 字节均不变）
    assert_eq!(before, after, "软连接豁免后 A 的 `._meta` 不应有任何变更");

    // 幂等：再次 sync 依旧零变更
    str_format::cmd::sync(&root, false, true).unwrap();
    assert_eq!(read(&meta), after);
    assert_eq!(error_count(&root), 0);

    // 目标分支数据原封不动（挂载是视图，不是搬家）
    assert!(root.join(B).join("b.json").exists());
}

/// 普通 path 条目的对账行为不变：未登记文件补登、已消失文件移除。
#[test]
fn sync_still_reconciles_regular_paths() {
    let root = two_nodes("syncnorm");
    let body = "{\"new\":1}\n";
    w(&root, &format!("{A}/extra.json"), body);
    // 登记过的 data.json 从磁盘消失 → 应被移除（`-` 行）
    std::fs::remove_file(root.join(A).join("data.json")).unwrap();

    str_format::cmd::sync(&root, false, true).unwrap();

    let meta = read(&root.join(A).join("._meta"));
    assert!(meta.contains("extra.json"), "未登记文件必须被补登：{meta}");
    assert!(
        !meta.contains("path = \"data.json\""),
        "已消失条目必须被移除：{meta}"
    );
    assert_eq!(error_count(&root), 0);
}

/// 批量删除门禁：≥ 10 条移除且未 `--yes` → 拒绝执行、一字不写；`--yes` 放行。
#[test]
fn sync_gate_refuses_mass_removal_without_yes() {
    let root = tmp_bundle("syncgate");
    let bodies: Vec<String> = (0..12).map(|i| format!("{{\"i\":{i}}}\n")).collect();
    let mut a_tables = String::new();
    for (i, b) in bodies.iter().enumerate() {
        a_tables.push_str(&payload_entry(&format!("f{i}.json"), b));
    }
    w(
        &root,
        "._meta",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &format!(
                "[policies]\n\n{}",
                node_entry(A, "type = \"crm.customer\"\nsummary = \"s\"")
            ),
        ),
    );
    w(&root, &format!("{A}/._meta"), &meta_text("node", A, "type = \"x\"\nsummary = \"s\"", &a_tables));
    for (i, b) in bodies.iter().enumerate() {
        w(&root, &format!("{A}/f{i}.json"), b);
    }
    assert_eq!(error_count(&root), 0);

    // 从磁盘删除 10 个（= 门禁 10）→ 未确认必须拒绝
    for i in 0..10 {
        std::fs::remove_file(root.join(A).join(format!("f{i}.json"))).unwrap();
    }
    let meta_path = root.join(A).join("._meta");
    let before = read(&meta_path);
    let err = str_format::cmd::sync(&root, false, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("拒绝执行"), "实际：{err}");
    assert!(err.contains("--dry-run"), "应指引 `--dry-run`：{err}");
    assert!(err.contains("--yes"), "应指引 `--yes`：{err}");
    assert_eq!(read(&meta_path), before, "拒绝执行时不得写盘");

    // 显式确认后放行，条目被移除
    str_format::cmd::sync(&root, false, true).unwrap();
    let after = read(&meta_path);
    for i in 0..10 {
        assert!(!after.contains(&format!("path = \"f{i}.json\"")), "{after}");
    }
    assert!(after.contains("f10.json") && after.contains("f11.json"), "{after}");
    assert_eq!(error_count(&root), 0);
}
