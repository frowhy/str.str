//! CLI 行为测试：规范 §4.9 确定性序列化、§9 命令面、§6.1 `E_REVISION_STALE` 历史判定。
//!
//! 与 `validate_codes.rs`（错误码矩阵）互补：这里断言的是**命令的实际效果**。

use std::path::{Path, PathBuf};

use str_format::bundle::Bundle;
use str_format::cmd;
use str_format::meta::MetaLoad;
use str_format::validate::validate;

/// 当前规范版本（`str init` 写出的 `spec` 值）。
const SPEC: &str = str_format::SPEC_VERSION;

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("strcmd-{name}-{}.str", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

/// 建一个含两个独立节点的 bundle，返回 `(bundle 路径, AAA 节点 id, BBB 节点 id)`。
fn two_nodes(name: &str) -> (PathBuf, String, String) {
    let raw = tmp(name);
    let parent = raw.parent().unwrap().to_path_buf();
    let stem = raw.file_stem().unwrap().to_string_lossy().to_string();
    cmd::init(&parent.join(&stem), None, None, None, 7).unwrap();
    cmd::node_add(&raw, Some("a.one".into()), Some("AAA".into()), Some("s1".into())).unwrap();
    cmd::node_add(&raw, Some("b.two".into()), Some("BBB".into()), Some("s2".into())).unwrap();

    let bundle = Bundle::new(raw.clone()).unwrap();
    let scan = bundle.scan().unwrap();
    let mut pairs: Vec<(String, String)> = scan
        .visits
        .iter()
        .filter(|v| v.depth == 1)
        .map(|v| {
            let id = v.dir.file_name().unwrap().to_string_lossy().to_string();
            let title = v.meta.as_ref().and_then(|m| m.title.clone()).unwrap_or_default();
            (title, id)
        })
        .collect();
    pairs.sort();
    (raw, pairs[0].1.clone(), pairs[1].1.clone())
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

/// `(错误数, 告警数, 错误码集合)`。
fn report(path: &Path) -> (usize, usize, Vec<String>) {
    let bundle = Bundle::new(path.to_path_buf()).unwrap();
    let r = validate(&bundle).unwrap();
    (
        r.error_count(),
        r.warning_count(),
        r.issues.iter().map(|i| i.code.clone()).collect(),
    )
}

/// `._meta` 中 `[[entries]]` 的 `path` 序列（即落盘条目顺序）。
fn entry_paths(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("path = \""))
        .filter_map(|l| l.strip_suffix('"'))
        .map(str::to_string)
        .collect()
}

/// 把裸键 `key = ...` 整行替换为 `key = value`。
fn replace_line(text: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key} = ");
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if line.starts_with(&prefix) {
            out.push_str(&prefix);
            out.push_str(value);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

// ───────────────────────── §4.9 确定性序列化 ─────────────────────────

#[test]
fn entries_are_sorted_by_order_then_path() {
    let (root, aaa, bbb) = two_nodes("order");
    let meta = root.join("._meta");

    cmd::fmt(&root, false, false).unwrap();
    let expected = vec![aaa.clone(), bbb.clone(), "._schema".to_string()];
    assert_eq!(entry_paths(&read(&meta)), expected);

    // 交换 `order` → 顺序随之下沉/上浮（`._schema` 没有 `order`，始终在最后）
    cmd::entry_set(
        &root,
        None,
        &aaa,
        &cmd::EntryPatch {
            order: Some(5),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        entry_paths(&read(&meta)),
        vec![bbb.clone(), aaa.clone(), "._schema".to_string()]
    );

    // 幂等：再规范化一次字节不变
    let once = read(&meta);
    cmd::fmt(&root, false, false).unwrap();
    assert_eq!(read(&meta), once);
}

#[test]
fn fmt_detects_and_fixes_out_of_order_entries() {
    let (root, aaa, bbb) = two_nodes("fixorder");
    let meta = root.join("._meta");

    // 手工把两条 node 的 order 互换（`order = 99` 只是中转，避免就地覆盖冲突）
    let swapped = read(&meta)
        .replace("order = 1", "order = 99")
        .replace("order = 2", "order = 1")
        .replace("order = 99", "order = 2");
    write(&meta, &swapped);

    assert_eq!(cmd::fmt(&root, true, false).unwrap(), 1, "乱序必须被检测出来");
    cmd::fmt(&root, false, false).unwrap();
    assert_eq!(
        entry_paths(&read(&meta)),
        vec![bbb.clone(), aaa.clone(), "._schema".to_string()]
    );
    assert_eq!(cmd::fmt(&root, true, false).unwrap(), 0);
}

#[test]
fn fmt_keeps_key_order_comments_and_can_strip_them() {
    let (root, _, _) = two_nodes("comments");
    let meta = root.join("._meta");

    // 顶层裸键乱序 + 一条注释 → 规范化后键序归位、注释保留
    let scrambled = read(&meta)
        .replace("# 保留我", "")
        .replace(
            &format!("str = 1\nspec = \"{SPEC}\""),
            &format!("# 保留我\nspec = \"{SPEC}\"\nstr = 1"),
        );
    write(&meta, &scrambled);
    cmd::fmt(&root, false, false).unwrap();

    let fixed = read(&meta);
    assert!(fixed.contains("# 保留我"), "{fixed}");
    assert!(
        fixed.find("str = 1").unwrap() < fixed.find("spec = ").unwrap(),
        "`str` 必须排在 `spec` 之前：\n{fixed}"
    );

    cmd::fmt(&root, false, true).unwrap();
    assert!(!read(&meta).contains("# 保留我"));
}

// ───────────────────────── §9 命令面 ─────────────────────────

#[test]
fn init_id_version_4_generates_v4_uuids() {
    let raw = tmp("v4");
    let parent = raw.parent().unwrap().to_path_buf();
    let stem = raw.file_stem().unwrap().to_string_lossy().to_string();
    cmd::init(&parent.join(&stem), None, None, None, 4).unwrap();
    cmd::node_add(&raw, Some("x.y".into()), None, Some("s".into())).unwrap();

    let bundle = Bundle::new(raw.clone()).unwrap();
    let scan = bundle.scan().unwrap();
    let id = scan
        .visits
        .iter()
        .find(|v| v.depth == 1)
        .map(|v| v.dir.file_name().unwrap().to_string_lossy().to_string())
        .unwrap();
    assert_eq!(str_format::util::uuid_version(&id), Some(4), "id = {id}");
    assert_eq!(report(&raw).0, 0, "生成的 UUID 版本必须满足 `E_ID_VERSION`");
}

#[test]
fn init_rejects_unsupported_id_version() {
    let raw = tmp("v9");
    let parent = raw.parent().unwrap().to_path_buf();
    let stem = raw.file_stem().unwrap().to_string_lossy().to_string();
    assert!(cmd::init(&parent.join(&stem), None, None, None, 9).is_err());
}

#[test]
fn validate_fix_manifest_actually_writes() {
    let (root, aaa, _) = two_nodes("fixmanifest");
    write(&root.join(&aaa).join("extra.json"), "{}\n");
    assert_eq!(report(&root).0, 1, "未登记的文件应报 E_MANIFEST_MISSING");

    let code = cmd::validate(&root, true, false, true).unwrap();
    assert_eq!(code, 0, "`--fix-manifest` 必须真正写盘并让校验通过");
    assert!(read(&root.join(&aaa).join("._meta")).contains("extra.json"));
}

#[test]
fn ref_rm_locates_source_from_positional_id() {
    let (root, aaa, bbb) = two_nodes("refrm");
    cmd::ref_add(&root, &aaa, &bbb, "related".into(), None, None).unwrap();
    let ref_id = {
        let bundle = Bundle::new(root.clone()).unwrap();
        let scan = bundle.scan().unwrap();
        let idx = scan.resolve(&aaa).unwrap();
        scan.visits[idx].meta.as_ref().unwrap().refs[0].id.clone()
    };

    // 规范 §9 形式：只给关联线 id，源分支由 CLI 定位
    cmd::ref_rm(&root, None, &ref_id).unwrap();
    let bundle = Bundle::new(root.clone()).unwrap();
    let scan = bundle.scan().unwrap();
    let idx = scan.resolve(&aaa).unwrap();
    assert!(scan.visits[idx].meta.as_ref().unwrap().refs.is_empty());
    assert!(cmd::ref_rm(&root, None, "no-such-ref").is_err());
}

#[test]
fn meta_set_entry_set_and_author_round_trip() {
    let (root, aaa, _) = two_nodes("metaset");

    // 空串 = 移除字段 → 应报 W_NO_TYPE / W_NO_SUMMARY
    cmd::meta_set(
        &root,
        Some(aaa.clone()),
        Some(String::new()),
        Some(String::new()),
        Some(String::new()),
        None,
        None,
    )
    .unwrap();
    let codes = report(&root).2;
    assert!(codes.contains(&"W_NO_TYPE".to_string()));
    assert!(codes.contains(&"W_NO_SUMMARY".to_string()));

    // 用 CLI 补齐（不再需要「手改 `._meta` 的唯一例外」）
    cmd::meta_set(
        &root,
        Some(aaa.clone()),
        Some("crm.customer".into()),
        Some("客户A".into()),
        Some("示例客户".into()),
        None,
        Some(vec!["crm".into(), "demo".into()]),
    )
    .unwrap();
    cmd::entry_set(
        &root,
        None,
        &aaa,
        &cmd::EntryPatch {
            type_: Some("crm.customer".into()),
            title: Some("客户A".into()),
            summary: Some("示例客户".into()),
            note: Some("别名".into()),
            order: None,
        },
    )
    .unwrap();
    cmd::author_add(
        &root,
        None,
        "u:frowhy".into(),
        Some("Frowhy".into()),
        "owner".into(),
        Some("2026-09-14T10:03:11+08:00".into()),
    )
    .unwrap();

    let node_meta = read(&root.join(&aaa).join("._meta"));
    assert!(node_meta.contains("tags = [\"crm\", \"demo\"]"), "{node_meta}");
    let root_meta = read(&root.join("._meta"));
    assert!(root_meta.contains("note = \"别名\""), "{root_meta}");
    assert!(root_meta.contains("[[authors]]"), "{root_meta}");
    let (errors, warnings, _) = report(&root);
    assert_eq!((errors, warnings), (0, 0), "{root_meta}");

    // 非法 role / 非法时间 / 空参数 / 不存在的 path 都必须被拒
    assert!(cmd::author_add(&root, None, "x".into(), None, "bogus".into(), None).is_err());
    assert!(
        cmd::author_add(
            &root,
            None,
            "x".into(),
            None,
            "owner".into(),
            Some("2026-09-14".into())
        )
        .is_err()
    );
    assert!(cmd::meta_set(&root, None, None, None, None, None, None).is_err());
    assert!(
        cmd::entry_set(
            &root,
            None,
            "nope",
            &cmd::EntryPatch {
                title: Some("t".into()),
                ..Default::default()
            }
        )
        .is_err()
    );

    cmd::author_rm(&root, None, "u:frowhy").unwrap();
    assert!(!read(&root.join("._meta")).contains("[[authors]]"));
}

#[test]
fn norm_and_export_out_targets() {
    let (root, _, _) = two_nodes("out");
    let dir = root.parent().unwrap().to_path_buf();

    let f = dir.join("norm.json");
    cmd::norm(&root, None, Some(f.display().to_string())).unwrap();
    assert!(read(&f).contains("\"kind\": \"root\""));

    let f = dir.join("exp.json");
    cmd::export(&root, "json".into(), None, Some(f.display().to_string())).unwrap();
    assert!(read(&f).contains("\"path\""));

    // 派生数据不得写回 bundle 内部（含符号链接别名）
    let inside = root.join("x.json");
    assert!(cmd::export(&root, "json".into(), None, Some(inside.display().to_string())).is_err());
    assert!(
        cmd::export(
            &root,
            "json".into(),
            None,
            Some(root.join("sub").join("x.json").display().to_string())
        )
        .is_err()
    );
    // 未知格式直接拒绝
    assert!(cmd::export(&root, "yaml".into(), None, None).is_err());
}

#[test]
fn branch_rm_accepts_recursive_and_cleans_baseline() {
    let (root, aaa, _) = two_nodes("rmrecursive");
    cmd::sync(&root, false).unwrap();
    let bundle = Bundle::new(root.clone()).unwrap();
    assert!(str_format::baseline::load(&bundle).branches.contains_key(&aaa));

    cmd::branch_rm(&root, &aaa, true).unwrap();
    assert!(!root.join(&aaa).exists());
    let bundle = Bundle::new(root.clone()).unwrap();
    assert!(
        !str_format::baseline::load(&bundle).branches.contains_key(&aaa),
        "被删分支的基线应被清理"
    );
    assert_eq!(report(&root).0, 0);
}

#[test]
fn metadata_stays_parsable_after_writes() {
    let (root, aaa, _) = two_nodes("parseafter");
    cmd::meta_set(&root, Some(aaa.clone()), Some("t".into()), None, None, None, None).unwrap();

    let bundle = Bundle::new(root.clone()).unwrap();
    match bundle.read_meta(&root).unwrap() {
        MetaLoad::Ok(_, issues) => assert!(issues.is_empty(), "{issues:?}"),
        MetaLoad::Failed(i) => panic!("root `._meta` 解析失败：{i:?}"),
    }
    // 键序规范 + 可再次规范化（幂等）
    assert_eq!(cmd::fmt(&root, true, false).unwrap(), 0);
}

// ───────────────────────── §6.1 `E_REVISION_STALE` ─────────────────────────

#[test]
fn revision_stale_uses_written_baseline() {
    let (root, aaa, _) = two_nodes("revhistory");
    cmd::sync(&root, false).unwrap();
    assert_eq!(report(&root).0, 0);

    // 绕过 CLI 改 `updated_at` 而不推进 `revision`
    let node_meta = root.join(&aaa).join("._meta");
    write(
        &node_meta,
        &replace_line(&read(&node_meta), "updated_at", "2030-01-01T00:00:00+00:00"),
    );
    let codes = report(&root).2;
    assert!(
        codes.contains(&"E_REVISION_STALE".to_string()),
        "应报 E_REVISION_STALE，实际 {codes:?}"
    );

    // `str sync` 不得把刚犯下的违规洗白
    cmd::sync(&root, false).unwrap();
    let codes = report(&root).2;
    assert!(
        codes.contains(&"E_REVISION_STALE".to_string()),
        "违规应跨 sync 持续可见，实际 {codes:?}"
    );

    // 真正修正：`revision` 前进
    let text = read(&node_meta);
    let bumped = replace_line(
        &text,
        "revision",
        &(text
            .lines()
            .find_map(|l| l.strip_prefix("revision = "))
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap()
            + 1)
            .to_string(),
    );
    write(&node_meta, &bumped);
    assert_eq!(report(&root).0, 0, "revision 前进后应恢复干净");

    // 基线随之推进
    cmd::sync(&root, false).unwrap();
    let bundle = Bundle::new(root.clone()).unwrap();
    let snap = str_format::baseline::load(&bundle);
    assert_eq!(snap.branches.get(&aaa).map(|s| s.revision), Some(2));
}

#[test]
fn revision_stale_is_skipped_without_baseline() {
    // 从未被 CLI 写过的 bundle 没有基线：单份 `._meta` 不含历史，不得凭空报错
    let (root, aaa, _) = two_nodes("revnobase");
    let _ = std::fs::remove_dir_all(root.join("._cache"));

    let node_meta = root.join(&aaa).join("._meta");
    write(
        &node_meta,
        &replace_line(&read(&node_meta), "updated_at", "2030-01-01T00:00:00+00:00"),
    );
    let codes = report(&root).2;
    assert!(!codes.contains(&"E_REVISION_STALE".to_string()), "{codes:?}");
}
