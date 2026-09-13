//! 错误码测试矩阵：规范 6.1 中每个 `E_*` / `W_*` 至少一个故意破坏用例。
//!
//! 同时覆盖 DoD 中可自动化的条目（幂等、深度规则、清单双向一致、OS 噪声豁免等）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use str_format::bundle::Bundle;
use str_format::util;
use str_format::validate::validate;

const ROOT_ID: &str = "01928f3a-7c4b-7000-8000-000000000000";
const A: &str = "01928f3a-7c4b-7001-8a01-000000000001";
const B: &str = "01928f3a-7c4b-7002-8a02-000000000002";
/// 合法 UUID 但版本为 4（用于 `E_ID_VERSION`）。
const V4: &str = "01928f3a-7c4b-4001-8a01-000000000004";

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

fn tmp_plain(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("strtest-{name}-{}-plain", std::process::id()));
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

fn branch_entry(path: &str, extra: &str) -> String {
    format!("[[entries]]\npath = \"{path}\"\nrole = \"branch\"\nid = \"{path}\"\n{extra}\n")
}

fn payload_entry(path: &str, body: &str, media: &str) -> String {
    format!(
        "[[entries]]\npath = \"{path}\"\nrole = \"payload\"\nmedia_type = \"{media}\"\nsize = {}\nsha256 = \"{}\"\n",
        body.len(),
        util::sha256_bytes(body.as_bytes())
    )
}

/// 一个干净的基线 bundle：ROOT + 一个独立节点 A（含一个 payload）。
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

/// 取报告中的全部错误码 + 文本。
fn codes(root: &Path) -> (HashSet<String>, String) {
    let bundle = Bundle::new(root.to_path_buf()).unwrap();
    let report = validate(&bundle).unwrap();
    (
        report.issues.iter().map(|i| i.code.clone()).collect(),
        report.to_text(),
    )
}

#[track_caller]
fn assert_code(root: &Path, code: &str) {
    let (set, text) = codes(root);
    assert!(set.contains(code), "期望出现 {code}，实际报告：\n{text}");
}

#[track_caller]
fn assert_clean(root: &Path) {
    let bundle = Bundle::new(root.to_path_buf()).unwrap();
    let report = validate(&bundle).unwrap();
    assert_eq!(
        report.error_count(),
        0,
        "期望零错误，实际：\n{}",
        report.to_text()
    );
    assert_eq!(
        report.warning_count(),
        0,
        "期望零告警，实际：\n{}",
        report.to_text()
    );
}

// ─────────────────────────── 基线 ───────────────────────────

#[test]
fn baseline_is_clean() {
    assert_clean(&baseline("baseline"));
}

// ─────────────────────────── 结构 / 身份 ───────────────────────────

#[test]
fn e_parse_invalid_toml() {
    let root = tmp_bundle("parse");
    w(&root, "._meta", "str = = 1\n");
    assert_code(&root, "E_PARSE");
}

#[test]
fn e_parse_bom_and_string_datetime() {
    let root = baseline("bom");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    std::fs::write(root.join("._meta"), format!("\u{feff}{text}")).unwrap();
    assert_code(&root, "E_PARSE");

    let root = baseline("dtstr");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    let bad = text.replace(
        "created_at = 2026-09-01T09:00:00+08:00",
        "created_at = \"2026-09-01T09:00:00+08:00\"",
    );
    w(&root, "._meta", &bad);
    assert_code(&root, "E_PARSE");
}

#[test]
fn e_meta_missing_root_and_declared_branch() {
    let root = tmp_bundle("nometa");
    assert_code(&root, "E_META_MISSING");

    let root = baseline("nometa2");
    std::fs::remove_file(root.join(A).join("._meta")).unwrap();
    assert_code(&root, "E_META_MISSING");
}

#[test]
fn e_meta_missing_when_meta_is_inside_non_branch_dir() {
    let root = baseline("nestedmeta");
    std::fs::create_dir_all(root.join(A).join("attachments").join("deep")).unwrap();
    w(
        &root,
        &format!("{A}/attachments/deep/._meta"),
        &meta_text("branch", B, "", ""),
    );
    assert_code(&root, "E_META_MISSING");
}

#[test]
fn e_spec_unsupported() {
    let root = baseline("spec");
    let text = std::fs::read_to_string(root.join("._meta"))
        .unwrap()
        .replace("str = 1", "str = 2");
    w(&root, "._meta", &text);
    assert_code(&root, "E_SPEC_UNSUPPORTED");
}

#[test]
fn e_kind_invalid() {
    let root = baseline("kindinv");
    let text = std::fs::read_to_string(root.join(A).join("._meta"))
        .unwrap()
        .replace("kind = \"node\"", "kind = \"foo\"");
    w(&root, &format!("{A}/._meta"), &text);
    assert_code(&root, "E_KIND_INVALID");
}

#[test]
fn e_kind_depth() {
    let root = baseline("kinddepth");
    let text = std::fs::read_to_string(root.join(A).join("._meta"))
        .unwrap()
        .replace("kind = \"node\"", "kind = \"branch\"");
    w(&root, &format!("{A}/._meta"), &text);
    assert_code(&root, "E_KIND_DEPTH");

    // ROOT 写成 node
    let root = baseline("kinddepth2");
    let text = std::fs::read_to_string(root.join("._meta"))
        .unwrap()
        .replace("kind = \"root\"", "kind = \"node\"");
    w(&root, "._meta", &text);
    assert_code(&root, "E_KIND_DEPTH");
}

#[test]
fn e_schema_field_unknown_and_wrong_type() {
    let root = baseline("fieldunknown");
    let text = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &text.replace("kind = \"node\"", "kind = \"node\"\nvendor_extra = 1"),
    );
    assert_code(&root, "E_SCHEMA_FIELD");

    let root = baseline("fieldtype");
    let text = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &text.replace("revision = 1", "revision = \"one\""),
    );
    assert_code(&root, "E_SCHEMA_FIELD");
}

#[test]
fn e_id_mismatch() {
    let root = baseline("idmismatch");
    let text = std::fs::read_to_string(root.join(A).join("._meta"))
        .unwrap()
        .replace(&format!("id = \"{A}\""), &format!("id = \"{B}\""));
    w(&root, &format!("{A}/._meta"), &text);
    assert_code(&root, "E_ID_MISMATCH");
}

#[test]
fn e_id_not_uuid() {
    let root = tmp_bundle("idnotuuid");
    w(
        &root,
        "._meta",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &node_entry("users", "type = \"crm.customer\"\nsummary = \"s\""),
        ),
    );
    w(
        &root,
        "users/._meta",
        &meta_text("node", "users", "type = \"crm.customer\"\nsummary = \"s\"", ""),
    );
    assert_code(&root, "E_ID_NOT_UUID");
}

#[test]
fn e_id_version() {
    let root = tmp_bundle("idversion");
    w(
        &root,
        "._meta",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &format!("[policies]\n\n{}", node_entry(V4, "type = \"x\"\nsummary = \"s\"")),
        ),
    );
    w(
        &root,
        &format!("{V4}/._meta"),
        &meta_text("node", V4, "type = \"x\"\nsummary = \"s\"", ""),
    );
    assert_code(&root, "E_ID_VERSION");
}

#[test]
fn e_id_dup() {
    let root = baseline("iddup");
    let text = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    // B 与 A 同级；A 下再嵌一个同名目录 B，两处 id 均为 B
    w(
        &root,
        "._meta",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &format!(
                "[policies]\n\n{}{}",
                node_entry(A, "type = \"x\"\nsummary = \"s\""),
                node_entry(B, "type = \"x\"\nsummary = \"s\"")
            ),
        ),
    );
    w(&root, &format!("{B}/._meta"), &meta_text("node", B, "type = \"x\"\nsummary = \"s\"", ""));
    let a_text = text.replace(
        "[ext]",
        "",
    );
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{}\n{}",
            a_text,
            branch_entry(B, "type = \"x\"\nsummary = \"s\"")
        ),
    );
    w(&root, &format!("{A}/{B}/._meta"), &meta_text("branch", B, "type = \"x\"\nsummary = \"s\"", ""));
    assert_code(&root, "E_ID_DUP");
}

#[test]
fn e_entry_role_depth_dir_holding_meta() {
    let root = baseline("roledepth");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("role = \"node\"", "role = \"dir\""),
    );
    assert_code(&root, "E_ENTRY_ROLE_DEPTH");
}

#[test]
fn e_entry_id_mismatch() {
    let root = baseline("entryid");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    let bad = text.replace(
        &format!("id = \"{A}\"\ntype = \"crm.customer\""),
        &format!("id = \"{B}\"\ntype = \"crm.customer\""),
    );
    w(&root, "._meta", &bad);
    assert_code(&root, "E_ENTRY_ID_MISMATCH");
}

#[test]
fn e_depth_exceeded() {
    let root = baseline("depthex");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]", "[policies]\nmax_depth = 1"),
    );
    w(
        &root,
        &format!("{A}/x/._meta"),
        &meta_text("branch", "01928f3a-7c4b-7101-8b01-000000000101", "type = \"x\"\nsummary = \"s\"", ""),
    );
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{}\n{}",
            a.replace("[ext]", ""),
            branch_entry("x", "type = \"x\"\nsummary = \"s\"")
        ),
    );
    assert_code(&root, "E_DEPTH_EXCEEDED");
}

// ─────────────────────────── 引用 ───────────────────────────

#[test]
fn e_ref_no_target_and_self() {
    let root = baseline("refno");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{}\n[[refs]]\nid = \"01928f3a-7c4b-7201-8d01-000000000201\"\ntarget = \"01928f3a-7c4b-7909-8909-000000000909\"\nrel = \"related\"\n",
            a
        ),
    );
    assert_code(&root, "E_REF_NO_TARGET");

    let root = baseline("refself");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{}\n[[refs]]\nid = \"01928f3a-7c4b-7201-8d01-000000000201\"\ntarget = \"{A}\"\nrel = \"related\"\n",
            a
        ),
    );
    assert_code(&root, "E_REF_SELF");
}

#[test]
fn e_ref_cycle() {
    let root = baseline("refcycle");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &format!(
                "[policies]\n\n{}{}",
                node_entry(A, "type = \"x\"\nsummary = \"s\""),
                node_entry(B, "type = \"x\"\nsummary = \"s\"")
            ),
        ),
    );
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{}\n[[refs]]\nid = \"01928f3a-7c4b-7201-8d01-000000000201\"\ntarget = \"{B}\"\nrel = \"related\"\n",
            a
        ),
    );
    w(
        &root,
        &format!("{B}/._meta"),
        &format!(
            "{}\n[[refs]]\nid = \"01928f3a-7c4b-7202-8d02-000000000202\"\ntarget = \"{A}\"\nrel = \"related\"\n",
            meta_text("node", B, "type = \"x\"\nsummary = \"s\"", "")
        ),
    );
    assert_code(&root, "E_REF_CYCLE");
}

// ─────────────────────────── 清单 ───────────────────────────

#[test]
fn e_manifest_missing_and_ghost_and_hash_and_dup() {
    let root = baseline("m1");
    w(&root, &format!("{A}/extra.txt"), "x");
    assert_code(&root, "E_MANIFEST_MISSING");

    let root = baseline("m2");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!("{a}\n{}", payload_entry("ghost.json", "{}\n", "application/json")),
    );
    assert_code(&root, "E_MANIFEST_GHOST");

    let root = baseline("m3");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    let real = util::sha256_bytes(b"{\"a\":1}\n");
    w(
        &root,
        &format!("{A}/._meta"),
        &a.replace(&real, &"0".repeat(64)),
    );
    assert_code(&root, "E_MANIFEST_HASH");

    let root = baseline("m4");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!("{a}\n{}", payload_entry("data.json", "x", "text/plain")),
    );
    assert_code(&root, "E_MANIFEST_DUP");
}

#[test]
fn e_manifest_digest_missing() {
    let root = baseline("digest");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &a.replace("sha256 = \"", "sha256x = \""),
    );
    assert_code(&root, "E_MANIFEST_DIGEST_MISSING");
}

#[test]
fn e_reserved_name_dir_and_declared_file() {
    let root = baseline("reserved");
    std::fs::create_dir_all(root.join(A).join("._mine")).unwrap();
    assert_code(&root, "E_RESERVED_NAME");

    let root = baseline("reserved2");
    w(&root, &format!("{A}/._orders.csv"), "a\n");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!("{a}\n{}", payload_entry("._orders.csv", "a\n", "text/csv")),
    );
    assert_code(&root, "E_RESERVED_NAME");
}

#[test]
fn e_revision_stale() {
    let root = baseline("rev1");
    let text = std::fs::read_to_string(root.join("._meta"))
        .unwrap()
        .replace("revision = 1", "revision = 0");
    w(&root, "._meta", &text);
    assert_code(&root, "E_REVISION_STALE");

    let root = baseline("rev2");
    let text = std::fs::read_to_string(root.join("._meta"))
        .unwrap()
        .replace("updated_at = 2026-09-01T09:00:00+08:00", "updated_at = 2026-01-01T09:00:00+08:00");
    w(&root, "._meta", &text);
    assert_code(&root, "E_REVISION_STALE");
}

#[test]
fn e_schema_fail_payload() {
    let root = baseline("schemafail");
    w(
        &root,
        "._schema/x.schema.json",
        "{\"$schema\":\"https://json-schema.org/draft/2020-12/schema\",\"type\":\"object\",\"required\":[\"missing\"]}",
    );
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &a.replace("[ext]", "").to_string(),
    );
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &a.replace(
            &payload_entry("data.json", "{\"a\":1}\n", "application/json"),
            &payload_entry("data.json", "{\"a\":1}\n", "application/json")
                .replace("size =", "schema = \"._schema/x.schema.json\"\nsize ="),
        ),
    );
    assert_code(&root, "E_SCHEMA_FAIL");
}

// ─────────────────────────── 告警 ───────────────────────────

#[test]
fn w_bundle_suffix() {
    let root = tmp_plain("suffix");
    w(
        &root,
        "._meta",
        &meta_text("root", ROOT_ID, "title = \"T\"\nsummary = \"s\"", ""),
    );
    assert_code(&root, "W_BUNDLE_SUFFIX");
}

#[test]
fn w_dotfile_and_root_stray() {
    let root = baseline("dotfile");
    w(&root, ".gitignore", "._cache/\n");
    assert_code(&root, "W_DOTFILE");

    let root = baseline("rootstray");
    w(&root, "notes.md", "x");
    assert_code(&root, "W_ROOT_STRAY");
}

#[test]
fn w_no_summary_and_no_type() {
    let root = baseline("nosummary");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &a.replace("type = \"crm.customer\"\nsummary = \"s\"\n", ""),
    );
    assert_code(&root, "W_NO_SUMMARY");
    assert_code(&root, "W_NO_TYPE");
}

#[test]
fn w_deep_tree_and_large_asset() {
    let root = baseline("deeptree");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]", "[policies]\ndeep_tree_warn = 1"),
    );
    w(
        &root,
        &format!("{A}/x/._meta"),
        &meta_text("branch", "01928f3a-7c4b-7101-8b01-000000000101", "type = \"x\"\nsummary = \"s\"", ""),
    );
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{}\n{}",
            a.replace("[ext]", ""),
            branch_entry("x", "type = \"x\"\nsummary = \"s\"")
        ),
    );
    assert_code(&root, "W_DEEP_TREE");

    let root = baseline("large");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]", "[policies]\nlarge_asset_bytes = 1"),
    );
    assert_code(&root, "W_LARGE_ASSET");
}

#[test]
fn w_optional_missing() {
    let root = baseline("optional");
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{a}\n{}",
            payload_entry("maybe.json", "{}\n", "application/json").replace(
                "sha256 = ",
                "optional = true\nsha256 = "
            )
        ),
    );
    assert_code(&root, "W_OPTIONAL_MISSING");
}

#[test]
fn w_manifest_advisory_downgrades() {
    let root = baseline("advisory");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]", "[policies]\nmanifest = \"advisory\""),
    );
    w(&root, &format!("{A}/extra.txt"), "x");
    let (set, report) = codes(&root);
    assert!(set.contains("W_MANIFEST_MISSING"), "报告：\n{report}");
    assert!(!set.contains("E_MANIFEST_MISSING"), "报告：\n{report}");
    let bundle = Bundle::new(root.clone()).unwrap();
    let report = validate(&bundle).unwrap();
    assert_eq!(
        report.error_count(),
        0,
        "advisory 模式下不应有 error：\n{}",
        report.to_text()
    );

    // advisory 下的幽灵条目
    let root = baseline("advisory2");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]", "[policies]\nmanifest = \"advisory\""),
    );
    std::fs::remove_file(root.join(A).join("data.json")).unwrap();
    let (set, report) = codes(&root);
    assert!(set.contains("W_MANIFEST_GHOST"), "报告：\n{report}");

    // advisory 下的指纹不符
    let root = baseline("advisory3");
    let text = std::fs::read_to_string(root.join("._meta")).unwrap();
    w(
        &root,
        "._meta",
        &text.replace("[policies]", "[policies]\nmanifest = \"advisory\""),
    );
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    let real = util::sha256_bytes(b"{\"a\":1}\n");
    w(
        &root,
        &format!("{A}/._meta"),
        &a.replace(&real, &"0".repeat(64)),
    );
    let (set, report) = codes(&root);
    assert!(set.contains("W_MANIFEST_HASH"), "报告：\n{report}");
}

// ─────────────────────────── DoD 自动化条目 ───────────────────────────

#[test]
fn dod_os_noise_is_exempt() {
    let root = baseline("osnoise");
    w(&root, &format!("{A}/._data.json"), "junk");
    w(&root, &format!("{A}/._._meta"), "junk");
    w(&root, &format!("{A}/.DS_Store"), "junk");
    let (set, report) = codes(&root);
    assert!(
        !set.contains("E_RESERVED_NAME") && !set.contains("E_MANIFEST_MISSING"),
        "OS 噪声必须被豁免，实际报告：\n{report}"
    );
    assert_clean(&root);
}

#[test]
fn dod_any_depth_can_hold_data() {
    let root = baseline("deepdata");
    let l1 = "01928f3a-7c4b-7101-8b01-000000000101";
    let l2 = "01928f3a-7c4b-7102-8b02-000000000102";
    let body = "{\"deep\":true}\n";
    let a = std::fs::read_to_string(root.join(A).join("._meta")).unwrap();
    w(
        &root,
        &format!("{A}/._meta"),
        &format!(
            "{}\n{}",
            a.replace("[ext]", ""),
            branch_entry(l1, "type = \"x\"\nsummary = \"s\"")
        ),
    );
    w(
        &root,
        &format!("{A}/{l1}/._meta"),
        &meta_text(
            "branch",
            l1,
            "type = \"x\"\nsummary = \"s\"",
            &format!(
                "{}{}",
                payload_entry("f1.json", body, "application/json"),
                branch_entry(l2, "type = \"x\"\nsummary = \"s\"")
            ),
        ),
    );
    w(&root, &format!("{A}/{l1}/f1.json"), body);
    w(
        &root,
        &format!("{A}/{l1}/{l2}/._meta"),
        &meta_text(
            "branch",
            l2,
            "type = \"x\"\nsummary = \"s\"",
            &payload_entry("f2.json", body, "application/json"),
        ),
    );
    w(&root, &format!("{A}/{l1}/{l2}/f2.json"), body);
    assert_clean(&root);
}

#[test]
fn dod_manifest_both_directions() {
    let root = baseline("bothdir");
    w(&root, &format!("{A}/added.txt"), "x");
    assert_code(&root, "E_MANIFEST_MISSING");

    let root = baseline("bothdir2");
    std::fs::remove_file(root.join(A).join("data.json")).unwrap();
    assert_code(&root, "E_MANIFEST_GHOST");
}
