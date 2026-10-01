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
/// 硬链接分支自身的 id（= 目录名；C1 语义下硬链接是有身份的真实分支）。
const HARD_LID: &str = "01928f3a-7c4b-7003-8a03-000000000003";
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

/// `.str.toml` 骨架（裸键在前，表在后 —— TOML 要求）。
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

/// 链接条目（`role = "link"`）：`path` = 目标 id，**不带 id**（规范 §4.6.1）。
/// `mode_extra` 可传 `mode = "hard"` 之类。
fn link_entry(path: &str, extra: &str) -> String {
    format!(
        "[[entries]]\npath = \"{path}\"\nrole = \"link\"\ntarget = \"{path}\"\n{extra}\n"
    )
}

/// ROOT + 两个独立节点 A / B（挂载用例需要第二个分支当目标）。
fn two_nodes(name: &str) -> PathBuf {
    let root = baseline(name);
    let body = "{\"b\":2}\n";
    let root_meta = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    // B 与 A 同属 ROOT 的 entries（插入点就在 A 那条之前，避免误嵌进 `[policies]`）
    let node_b = node_entry(B, "type = \"crm.tag\"\nsummary = \"s\"");
    w(&root, ".str.toml", &root_meta.replace(&node_entry(A, "type = \"crm.customer\"\nsummary = \"s\""), &format!("{node_b}{}", node_entry(A, "type = \"crm.customer\"\nsummary = \"s\""))).as_str());
    w(
        &root,
        &format!("{B}/.str.toml"),
        &meta_text(
            "node",
            B,
            "type = \"crm.tag\"\nsummary = \"s\"",
            &payload_entry("b.json", body, "application/json"),
        ),
    );
    w(&root, &format!("{B}/b.json"), body);
    root
}

/// 往某分支的 `.str.toml` 末尾追加一段 `[[entries]]`（该分支的表区只有 entries，安全）。
fn append_entries(root: &Path, branch: &str, extra: &str) {
    let p = root.join(branch).join(".str.toml");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, format!("{text}{extra}\n")).unwrap();
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
        ".str.toml",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &format!("[policies]\n\n{}", node_entry(A, "type = \"crm.customer\"\nsummary = \"s\"")),
        ),
    );
    w(
        &root,
        &format!("{A}/.str.toml"),
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
    w(&root, ".str.toml", "str = = 1\n");
    assert_code(&root, "E_PARSE");
}

#[test]
fn e_parse_bom_and_string_datetime() {
    let root = baseline("bom");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    std::fs::write(root.join(".str.toml"), format!("\u{feff}{text}")).unwrap();
    assert_code(&root, "E_PARSE");

    let root = baseline("dtstr");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    let bad = text.replace(
        "created_at = 2026-09-01T09:00:00+08:00",
        "created_at = \"2026-09-01T09:00:00+08:00\"",
    );
    w(&root, ".str.toml", &bad);
    assert_code(&root, "E_PARSE");
}

#[test]
fn e_meta_missing_root_and_declared_branch() {
    let root = tmp_bundle("nometa");
    assert_code(&root, "E_META_MISSING");

    let root = baseline("nometa2");
    std::fs::remove_file(root.join(A).join(".str.toml")).unwrap();
    assert_code(&root, "E_META_MISSING");
}

#[test]
fn e_meta_missing_when_meta_is_inside_non_branch_dir() {
    let root = baseline("nestedmeta");
    std::fs::create_dir_all(root.join(A).join("attachments").join("deep")).unwrap();
    w(
        &root,
        &format!("{A}/attachments/deep/.str.toml"),
        &meta_text("branch", B, "", ""),
    );
    assert_code(&root, "E_META_MISSING");
}

#[test]
fn e_spec_unsupported() {
    let root = baseline("spec");
    let text = std::fs::read_to_string(root.join(".str.toml"))
        .unwrap()
        .replace("str = 1", "str = 2");
    w(&root, ".str.toml", &text);
    assert_code(&root, "E_SPEC_UNSUPPORTED");
}

#[test]
fn e_kind_invalid() {
    let root = baseline("kindinv");
    let text = std::fs::read_to_string(root.join(A).join(".str.toml"))
        .unwrap()
        .replace("kind = \"node\"", "kind = \"foo\"");
    w(&root, &format!("{A}/.str.toml"), &text);
    assert_code(&root, "E_KIND_INVALID");
}

#[test]
fn e_kind_depth() {
    let root = baseline("kinddepth");
    let text = std::fs::read_to_string(root.join(A).join(".str.toml"))
        .unwrap()
        .replace("kind = \"node\"", "kind = \"branch\"");
    w(&root, &format!("{A}/.str.toml"), &text);
    assert_code(&root, "E_KIND_DEPTH");

    // ROOT 写成 node
    let root = baseline("kinddepth2");
    let text = std::fs::read_to_string(root.join(".str.toml"))
        .unwrap()
        .replace("kind = \"root\"", "kind = \"node\"");
    w(&root, ".str.toml", &text);
    assert_code(&root, "E_KIND_DEPTH");
}

#[test]
fn e_schema_field_unknown_and_wrong_type() {
    let root = baseline("fieldunknown");
    let text = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &text.replace("kind = \"node\"", "kind = \"node\"\nvendor_extra = 1"),
    );
    assert_code(&root, "E_SCHEMA_FIELD");

    let root = baseline("fieldtype");
    let text = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &text.replace("revision = 1", "revision = \"one\""),
    );
    assert_code(&root, "E_SCHEMA_FIELD");
}

/// v1.8.0 裁决：`policies.unknown_entry` 已从规范删除（与 `manifest` 重叠、从未被
/// Schema 与实现采纳），写入它必须被拒 —— 未登记条目的处理一律由 `manifest` 表达。
#[test]
fn policies_unknown_entry_is_rejected() {
    let root = baseline("policyunknown");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &text.replace("[policies]\n", "[policies]\nunknown_entry = \"deny\"\n"),
    );
    assert_code(&root, "E_SCHEMA_FIELD");
}

#[test]
fn e_id_mismatch() {
    let root = baseline("idmismatch");
    let text = std::fs::read_to_string(root.join(A).join(".str.toml"))
        .unwrap()
        .replace(&format!("id = \"{A}\""), &format!("id = \"{B}\""));
    w(&root, &format!("{A}/.str.toml"), &text);
    assert_code(&root, "E_ID_MISMATCH");
}

#[test]
fn e_id_not_uuid() {
    let root = tmp_bundle("idnotuuid");
    w(
        &root,
        ".str.toml",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &node_entry("users", "type = \"crm.customer\"\nsummary = \"s\""),
        ),
    );
    w(
        &root,
        "users/.str.toml",
        &meta_text("node", "users", "type = \"crm.customer\"\nsummary = \"s\"", ""),
    );
    assert_code(&root, "E_ID_NOT_UUID");
}

#[test]
fn e_id_version() {
    let root = tmp_bundle("idversion");
    w(
        &root,
        ".str.toml",
        &meta_text(
            "root",
            ROOT_ID,
            "title = \"T\"\nsummary = \"s\"",
            &format!("[policies]\n\n{}", node_entry(V4, "type = \"x\"\nsummary = \"s\"")),
        ),
    );
    w(
        &root,
        &format!("{V4}/.str.toml"),
        &meta_text("node", V4, "type = \"x\"\nsummary = \"s\"", ""),
    );
    assert_code(&root, "E_ID_VERSION");
}

#[test]
fn e_id_dup() {
    let root = baseline("iddup");
    let text = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    // B 与 A 同级；A 下再嵌一个同名目录 B，两处 id 均为 B
    w(
        &root,
        ".str.toml",
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
    w(&root, &format!("{B}/.str.toml"), &meta_text("node", B, "type = \"x\"\nsummary = \"s\"", ""));
    let a_text = text.replace(
        "[ext]",
        "",
    );
    w(
        &root,
        &format!("{A}/.str.toml"),
        &format!(
            "{}\n{}",
            a_text,
            branch_entry(B, "type = \"x\"\nsummary = \"s\"")
        ),
    );
    w(&root, &format!("{A}/{B}/.str.toml"), &meta_text("branch", B, "type = \"x\"\nsummary = \"s\"", ""));
    assert_code(&root, "E_ID_DUP");
}

#[test]
fn e_entry_role_depth_dir_holding_meta() {
    let root = baseline("roledepth");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &text.replace("role = \"node\"", "role = \"dir\""),
    );
    assert_code(&root, "E_ENTRY_ROLE_DEPTH");
}

#[test]
fn e_entry_id_mismatch() {
    let root = baseline("entryid");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    let bad = text.replace(
        &format!("id = \"{A}\"\ntype = \"crm.customer\""),
        &format!("id = \"{B}\"\ntype = \"crm.customer\""),
    );
    w(&root, ".str.toml", &bad);
    assert_code(&root, "E_ENTRY_ID_MISMATCH");
}

#[test]
fn e_depth_exceeded() {
    let root = baseline("depthex");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &text.replace("[policies]", "[policies]\nmax_depth = 1"),
    );
    w(
        &root,
        &format!("{A}/x/.str.toml"),
        &meta_text("branch", "01928f3a-7c4b-7101-8b01-000000000101", "type = \"x\"\nsummary = \"s\"", ""),
    );
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
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
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &format!(
            "{}\n[[refs]]\nid = \"01928f3a-7c4b-7201-8d01-000000000201\"\ntarget = \"01928f3a-7c4b-7909-8909-000000000909\"\nrel = \"related\"\n",
            a
        ),
    );
    assert_code(&root, "E_REF_NO_TARGET");

    let root = baseline("refself");
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
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
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
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
        &format!("{A}/.str.toml"),
        &format!(
            "{}\n[[refs]]\nid = \"01928f3a-7c4b-7201-8d01-000000000201\"\ntarget = \"{B}\"\nrel = \"related\"\n",
            a
        ),
    );
    w(
        &root,
        &format!("{B}/.str.toml"),
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
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &format!("{a}\n{}", payload_entry("ghost.json", "{}\n", "application/json")),
    );
    assert_code(&root, "E_MANIFEST_GHOST");

    let root = baseline("m3");
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    let real = util::sha256_bytes(b"{\"a\":1}\n");
    w(
        &root,
        &format!("{A}/.str.toml"),
        &a.replace(&real, &"0".repeat(64)),
    );
    assert_code(&root, "E_MANIFEST_HASH");

    let root = baseline("m4");
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &format!("{a}\n{}", payload_entry("data.json", "x", "text/plain")),
    );
    assert_code(&root, "E_MANIFEST_DUP");
}

#[test]
fn e_manifest_digest_missing() {
    let root = baseline("digest");
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
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
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &format!("{a}\n{}", payload_entry("._orders.csv", "a\n", "text/csv")),
    );
    assert_code(&root, "E_RESERVED_NAME");
}

#[test]
fn e_revision_stale() {
    let root = baseline("rev1");
    let text = std::fs::read_to_string(root.join(".str.toml"))
        .unwrap()
        .replace("revision = 1", "revision = 0");
    w(&root, ".str.toml", &text);
    assert_code(&root, "E_REVISION_STALE");

    let root = baseline("rev2");
    let text = std::fs::read_to_string(root.join(".str.toml"))
        .unwrap()
        .replace("updated_at = 2026-09-01T09:00:00+08:00", "updated_at = 2026-01-01T09:00:00+08:00");
    w(&root, ".str.toml", &text);
    assert_code(&root, "E_REVISION_STALE");
}

#[test]
fn e_schema_fail_payload() {
    let root = baseline("schemafail");
    w(
        &root,
        ".str.schema/x.schema.json",
        "{\"$schema\":\"https://json-schema.org/draft/2020-12/schema\",\"type\":\"object\",\"required\":[\"missing\"]}",
    );
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &a.replace("[ext]", "").to_string(),
    );
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &a.replace(
            &payload_entry("data.json", "{\"a\":1}\n", "application/json"),
            &payload_entry("data.json", "{\"a\":1}\n", "application/json")
                .replace("size =", "schema = \".str.schema/x.schema.json\"\nsize ="),
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
        ".str.toml",
        &meta_text("root", ROOT_ID, "title = \"T\"\nsummary = \"s\"", ""),
    );
    assert_code(&root, "W_BUNDLE_SUFFIX");
}

#[test]
fn dod_relative_path_keeps_bundle_name() {
    // `str validate .` 的形式：路径以 `.` 结尾时也必须能取到目录名
    let root = baseline("relpath");
    let dotted = root.join(".");
    let bundle = Bundle::new(dotted).unwrap();
    let report = validate(&bundle).unwrap();
    assert!(
        !report
            .issues
            .iter()
            .any(|i| i.code == "W_BUNDLE_SUFFIX"),
        "以 `.` 结尾的相对路径不应误报 W_BUNDLE_SUFFIX：\n{}",
        report.to_text()
    );
}

#[test]
fn w_dotfile_and_root_stray() {
    // 真正的“其它点文件” → W_DOTFILE
    let root = baseline("dotfile");
    w(&root, ".env", "A=1\n");
    assert_code(&root, "W_DOTFILE");

    // VCS 元数据豁免：`.git/` 与 `.gitignore` 不报任何码（规范 3.4）
    let root = baseline("vcs");
    w(&root, ".gitignore", ".str.cache/\n");
    w(&root, ".git/HEAD", "ref: refs/heads/main\n");
    let (set, report) = codes(&root);
    assert!(!set.contains("W_DOTFILE"), "VCS 元数据应豁免：\n{report}");
    assert!(!set.contains("E_MANIFEST_MISSING"), "VCS 元数据应豁免：\n{report}");

    // 托管平台元数据豁免：`.github/`（Actions 工作流的强制位置）同样不报任何码
    let root = baseline("github");
    w(&root, ".github/workflows/release.yml", "name: release\n");
    let (set, report) = codes(&root);
    assert!(!set.contains("W_DOTFILE"), "`.github/` 应豁免：\n{report}");
    assert!(!set.contains("E_MANIFEST_MISSING"), "`.github/` 应豁免：\n{report}");

    // ROOT 未登记的散落内容 → W_ROOT_STRAY
    let root = baseline("rootstray");
    w(&root, "notes.md", "x");
    assert_code(&root, "W_ROOT_STRAY");

    // ROOT **已登记**的非 UUID 内容 → 合法（规范 3.3：任意目录可放任意文件）
    let root = baseline("rootstray2");
    let body = "x\n";
    w(&root, "notes.md", body);
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &format!("{text}\n{}", payload_entry("notes.md", body, "text/markdown")),
    );
    let (set, report) = codes(&root);
    assert!(
        !set.contains("W_ROOT_STRAY"),
        "已登记的 ROOT 内容不应告警：\n{report}"
    );
    assert_clean(&root);
}

#[test]
fn dod_nested_bundle_is_hard_boundary() {
    let root = baseline("nested");
    let sub = "子集.str";
    w(
        &root,
        &format!("{sub}/.str.toml"),
        &meta_text("root", "01928f3a-7c4b-7000-8000-00000000000f", "title = \"嵌套\"\nsummary = \"s\"", ""),
    );
    w(&root, &format!("{sub}/data.txt"), "inner\n");
    // 未登记 → 报「未登记」，但**不得**报「父目录不是分支」（.str 是硬边界）
    let (set, report) = codes(&root);
    assert!(
        !set.contains("E_META_MISSING"),
        "`.str` 目录必须是硬边界：\n{report}"
    );
    assert!(set.contains("E_MANIFEST_MISSING"), "未登记应报：\n{report}");

    // 登记为 role = "bundle" → 全绿
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &format!(
            "{text}\n[[entries]]\npath = \"{sub}\"\nrole = \"bundle\"\ntitle = \"嵌套子 bundle\"\n"
        ),
    );
    assert_clean(&root);

    // 登记为 role = "dir" → 提示应改用 bundle
    let root2 = baseline("nested2");
    w(
        &root2,
        &format!("{sub}/.str.toml"),
        &meta_text("root", "01928f3a-7c4b-7000-8000-00000000000f", "title = \"嵌套\"\nsummary = \"s\"", ""),
    );
    let text = std::fs::read_to_string(root2.join(".str.toml")).unwrap();
    w(
        &root2,
        ".str.toml",
        &format!("{text}\n[[entries]]\npath = \"{sub}\"\nrole = \"dir\"\n"),
    );
    assert_code(&root2, "E_ENTRY_ROLE_DEPTH");
}

#[test]
fn w_no_summary_and_no_type() {
    let root = baseline("nosummary");
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &a.replace("type = \"crm.customer\"\nsummary = \"s\"\n", ""),
    );
    assert_code(&root, "W_NO_SUMMARY");
    assert_code(&root, "W_NO_TYPE");
}

#[test]
fn w_deep_tree_and_large_asset() {
    let root = baseline("deeptree");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &text.replace("[policies]", "[policies]\ndeep_tree_warn = 1"),
    );
    w(
        &root,
        &format!("{A}/x/.str.toml"),
        &meta_text("branch", "01928f3a-7c4b-7101-8b01-000000000101", "type = \"x\"\nsummary = \"s\"", ""),
    );
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &format!(
            "{}\n{}",
            a.replace("[ext]", ""),
            branch_entry("x", "type = \"x\"\nsummary = \"s\"")
        ),
    );
    assert_code(&root, "W_DEEP_TREE");

    let root = baseline("large");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &text.replace("[policies]", "[policies]\nlarge_asset_bytes = 1"),
    );
    assert_code(&root, "W_LARGE_ASSET");
}

#[test]
fn w_optional_missing() {
    let root = baseline("optional");
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
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
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
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
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &text.replace("[policies]", "[policies]\nmanifest = \"advisory\""),
    );
    std::fs::remove_file(root.join(A).join("data.json")).unwrap();
    let (set, report) = codes(&root);
    assert!(set.contains("W_MANIFEST_GHOST"), "报告：\n{report}");

    // advisory 下的指纹不符
    let root = baseline("advisory3");
    let text = std::fs::read_to_string(root.join(".str.toml")).unwrap();
    w(
        &root,
        ".str.toml",
        &text.replace("[policies]", "[policies]\nmanifest = \"advisory\""),
    );
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    let real = util::sha256_bytes(b"{\"a\":1}\n");
    w(
        &root,
        &format!("{A}/.str.toml"),
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
    w(&root, &format!("{A}/._.str.toml"), "junk");
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
    let a = std::fs::read_to_string(root.join(A).join(".str.toml")).unwrap();
    w(
        &root,
        &format!("{A}/.str.toml"),
        &format!(
            "{}\n{}",
            a.replace("[ext]", ""),
            branch_entry(l1, "type = \"x\"\nsummary = \"s\"")
        ),
    );
    w(
        &root,
        &format!("{A}/{l1}/.str.toml"),
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
        &format!("{A}/{l1}/{l2}/.str.toml"),
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

// ─────────────────────────── 软连接（`role = "link"`，规范 §4.6.1）──────────────────────────

/// 合法挂载：B 挂到 A 下 —— 磁盘上**没有**对应目录，但不报 `E_MANIFEST_GHOST`
/// （软连接不参与清单比对），数据也不移动。
#[test]
fn link_mount_is_clean_and_keeps_data_in_place() {
    let root = two_nodes("linkok");
    append_entries(&root, A, &link_entry(B, ""));
    assert_clean(&root);
    // 目标分支仍在 ROOT 之下（挂载是视图，不是搬家）。
    assert!(root.join(B).join("b.json").exists());
}

#[test]
fn e_link_no_target() {
    let root = two_nodes("linknotarget");
    append_entries(
        &root,
        A,
        &link_entry("01928f3a-7c4b-4fff-8fff-000000000009", ""),
    );
    assert_code(&root, "E_LINK_NO_TARGET");
}

#[test]
fn e_link_target_invalid_root() {
    let root = two_nodes("linkroot");
    append_entries(&root, A, &link_entry(ROOT_ID, ""));
    assert_code(&root, "E_LINK_TARGET_INVALID");
}

/// 软连接是只读视图：不得携带 `id` / `size` / `sha256` 等身份或数据语义。
#[test]
fn e_link_has_payload() {
    let root = two_nodes("linkpayload");
    append_entries(&root, A, &link_entry(B, "size = 12\n"));
    assert_code(&root, "E_LINK_HAS_PAYLOAD");
}

/// 互相挂载成环 + 自我挂载 + 挂进自己的祖先（同一码：都会无限递归）。
#[test]
fn e_link_cycle_and_self_and_ancestor() {
    let root = two_nodes("linkcycle");
    append_entries(&root, A, &link_entry(B, ""));
    append_entries(&root, B, &link_entry(A, ""));
    assert_code(&root, "E_LINK_CYCLE");

    let root = two_nodes("linkself");
    append_entries(&root, A, &link_entry(A, ""));
    assert_code(&root, "E_LINK_CYCLE");

    // 挂进自己的祖先：A 下有子分支 C（深度 2），C 里挂载 A —— 沿真实父子边
    // A → C 与挂载边 C → A 闭合成环，渲染 C 的子树会无限递归。
    // （两层 bundle 里唯一的祖先是 ROOT，而 ROOT 不可挂，故需三层构造。）
    let root = baseline("linkancestor");
    let c = "01928f3a-7c4b-4003-8a03-000000000006";
    let body = "{\"c\":3}\n";
    append_entries(
        &root,
        A,
        &format!("[[entries]]\npath = \"{c}\"\nrole = \"branch\"\nid = \"{c}\"\n"),
    );
    w(
        &root,
        &format!("{A}/{c}/.str.toml"),
        &meta_text(
            "branch",
            c,
            "",
            &format!(
                "{}{}",
                payload_entry("c.json", body, "application/json"),
                link_entry(A, "")
            ),
        ),
    );
    w(&root, &format!("{A}/{c}/c.json"), body);
    assert_code(&root, "E_LINK_CYCLE");
}

#[test]
fn e_link_dup() {
    let root = two_nodes("linkdup");
    append_entries(&root, A, &format!("{}{}", link_entry(B, ""), link_entry(B, "")));
    assert_code(&root, "E_LINK_DUP");
}

/// 硬链接（`mode = "hard"`）：合法挂载干净；目标位于挂载点子树内（含 ROOT 下）
/// 同样合法（1.15.0 撤回「不得挂载自己的后代」：硬链接不渲染目标结构、内容只读，
/// 不存在自嵌套）；`mode` 非法值与非链接行写 `mode` 都报 `E_SCHEMA_FIELD`。
/// 在 `parent` 分支下创建一个合法的硬链接分支（C1 语义：`path` = `id` = 自身
/// 目录名，目录 + 自有 `.str.toml` 存在，自有 entries 只有子分支 / 空），并在父
/// 分支 `entries` 追加对应 `role = "link"` 行。
fn hard_link_branch(root: &Path, parent: &str, lid: &str, target: &str, extra: &str) {
    let parent_rel = if parent == "." { String::new() } else { format!("{parent}/") };
    std::fs::create_dir_all(root.join(&parent_rel).join(lid)).unwrap();
    let kind = if parent == "." { "node" } else { "branch" };
    w(
        root,
        &format!("{parent_rel}{lid}/.str.toml"),
        &meta_text(
            kind,
            lid,
            "type = \"link.view\"\nsummary = \"硬链接视图\"\ntitle = \"硬链接视图\"",
            "",
        ),
    );
    append_entries(
        root,
        parent,
        &format!(
            "[[entries]]\npath = \"{lid}\"\nrole = \"link\"\nid = \"{lid}\"\ntarget = \"{target}\"\n{extra}"
        ),
    );
}

#[test]
fn link_hard_mode_rules() {
    // 合法：硬链接 = 有身份的真实分支（目录 + 自有 meta 存在，内容来自目标）。
    let root = two_nodes("linkhardok");
    hard_link_branch(&root, A, HARD_LID, B, "mode = \"hard\"\n");
    assert_clean(&root);

    // 目标位于挂载点子树内（ROOT 硬链接 A，A 是 ROOT 的子分支）⇒ 合法
    // （1.15.0 撤回「不得挂载自己的后代」；「挂自己的祖先」仍由成环判定
    // E_LINK_CYCLE 覆盖，见 e_link_cycle_and_self_and_ancestor。）
    let root = two_nodes("linkharddesc");
    hard_link_branch(&root, ".", HARD_LID, A, "mode = \"hard\"\n");
    assert_clean(&root);

    // 硬链接缺 `id`（有身份的真实分支必须写 id）
    let root = two_nodes("linkhardnoid");
    hard_link_branch(&root, A, HARD_LID, B, "mode = \"hard\"\n");
    let p = root.join(A).join(".str.toml");
    let text = std::fs::read_to_string(&p).unwrap().replace(
        &format!("[[entries]]\npath = \"{HARD_LID}\"\nrole = \"link\"\nid = \"{HARD_LID}\""),
        &format!("[[entries]]\npath = \"{HARD_LID}\"\nrole = \"link\""),
    );
    std::fs::write(&p, text).unwrap();
    assert_code(&root, "E_LINK_TARGET_INVALID");

    // 硬链接分支登记内容条目 → E_LINK_OWN_CONTENT（内容所有权唯一在目标分支）。
    let root = two_nodes("linkhardown");
    hard_link_branch(&root, A, HARD_LID, B, "mode = \"hard\"\n");
    let body = "own\n";
    append_entries(
        &root,
        &format!("{A}/{HARD_LID}"),
        &payload_entry("own.json", body, "application/json"),
    );
    w(&root, &format!("{A}/{HARD_LID}/own.json"), body);
    assert_code(&root, "E_LINK_OWN_CONTENT");

    // mode 非法值
    let root = two_nodes("linkbadmode");
    append_entries(&root, A, &link_entry(B, "mode = \"sym\"\n"));
    assert_code(&root, "E_SCHEMA_FIELD");

    // 非 link 行写 mode
    let root = baseline("linkmodeonbranch");
    append_entries(&root, A, &format!("[[entries]]\npath = \"x.json\"\nrole = \"payload\"\nmode = \"hard\"\nsize = 2\nsha256 = \"{}\"\n", util::sha256_bytes(b"hi")));
    w(&root, &format!("{A}/x.json"), "hi");
    assert_code(&root, "E_SCHEMA_FIELD");
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
