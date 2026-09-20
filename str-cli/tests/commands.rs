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
    // v1.12.0 起保留目录免登记：ROOT 的 entries 只有两条 node。
    let expected = vec![aaa.clone(), bbb.clone()];
    assert_eq!(entry_paths(&read(&meta)), expected);

    // 交换 `order` → 顺序随之下沉/上浮
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
    assert_eq!(entry_paths(&read(&meta)), vec![bbb.clone(), aaa.clone()]);

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
    assert_eq!(entry_paths(&read(&meta)), vec![bbb.clone(), aaa.clone()]);
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
    cmd::ref_add(&root, Some(aaa.clone()), &bbb, "related".into(), None, None).unwrap();
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

/// 规范 §9：`[uuid]` 位置参数缺省一律为 ROOT；对 ROOT 非法的操作必须给出**带原因**的拒绝。
#[test]
fn uuid_argument_defaults_to_root() {
    let (root, aaa, _) = two_nodes("rootdefault");

    // 读类命令：省略 `<UUID>` 应落到 ROOT（而非报「参数缺失」）
    cmd::ls(&root, None, false).unwrap();
    cmd::show(&root, None, false).unwrap();
    cmd::context(&root, None, 2, 8000).unwrap();

    // 缺省不等于忽略参数：显式给出不存在的 id 仍必须失败
    assert!(cmd::show(&root, Some("no-such-uuid".into()), false).is_err());
    assert!(cmd::context(&root, Some("no-such-uuid".into()), 2, 8000).is_err());

    // `ref add` 缺省源分支 = ROOT → 关联线落在 ROOT 的 `refs[]`
    cmd::ref_add(&root, None, &aaa, "related".into(), None, None).unwrap();
    let root_meta = read(&root.join("._meta"));
    assert!(root_meta.contains("[[refs]]"), "{root_meta}");
    assert_eq!(report(&root).0, 0, "ROOT 持有 `refs` 必须合法：{root_meta}");

    // `branch add` / `branch rm` 缺省同样是 ROOT，但 ROOT 上这两个操作非法 → 拒绝且说明原因
    let err = cmd::branch_add(&root, None, Some("x.y".into()), None, None, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("node add"), "应指引到 `node add`，实际：{err}");
    let err = cmd::branch_rm(&root, None, true).unwrap_err().to_string();
    assert!(err.contains("不能删除 ROOT"), "实际：{err}");

    // 给出合法 uuid 时两者照常工作（缺省值不改变原有语义）
    cmd::branch_add(
        &root,
        Some(aaa.clone()),
        Some("a.sub".into()),
        Some("SUB".into()),
        None,
        None,
    )
    .unwrap();
    let sub = {
        let bundle = Bundle::new(root.clone()).unwrap();
        let scan = bundle.scan().unwrap();
        let parent = scan.resolve(&aaa).unwrap();
        scan.visits
            .iter()
            .filter(|v| v.parent == Some(parent))
            .map(|v| v.dir.file_name().unwrap().to_string_lossy().to_string())
            .next()
            .expect("应新增一个下级分支")
    };
    cmd::branch_rm(&root, Some(sub), true).unwrap();
}

/// 规范 §9（v1.11.0）：`[uuid]` 缺省目标 = **当前节点** —— `[dir]` 指向分支目录时，
/// `branch add` / `branch rm` / `show` 等命令的缺省目标即该分支，而非整份 bundle 的 ROOT。
#[test]
fn uuid_argument_defaults_to_current_branch() {
    let (root, aaa, _) = two_nodes("curbranch");
    let node_dir = root.join(&aaa);

    // `branch add`：`[dir]` 指向分支目录 + 省略锚点 → 新分支挂在该分支下
    cmd::branch_add(&node_dir, None, Some("a.sub".into()), Some("SUB".into()), None, None).unwrap();
    let sub = {
        let bundle = Bundle::new(root.clone()).unwrap();
        let scan = bundle.scan().unwrap();
        let p = scan.resolve(&aaa).unwrap();
        scan.visits
            .iter()
            .find(|v| v.parent == Some(p))
            .map(|v| v.dir.file_name().unwrap().to_string_lossy().to_string())
            .expect("子分支应登记在 AAA 下")
    };
    assert_eq!(report(&root).0, 0);

    // `branch rm`：`[dir]` 指向子分支目录 + 省略 uuid → 删除**当前分支本身**（父级 entries 同步修复）
    cmd::branch_rm(&node_dir.join(&sub), None, true).unwrap();
    assert!(!node_dir.join(&sub).exists());
    assert_eq!(report(&root).0, 0, "父级 entries 必须被同步修复");

    // 读类命令：`[dir]` 指向分支目录 → 目标为该分支（而非 ROOT）
    cmd::show(&node_dir, None, false).unwrap();
    cmd::ls(&node_dir, None, false).unwrap();
    cmd::context(&node_dir, None, 1, 8000).unwrap();

    // `node add` 在分支目录下 → 带原因拒绝（独立节点只能挂 ROOT）
    let err = cmd::node_add(&node_dir, None, None, None).unwrap_err().to_string();
    assert!(err.contains("branch add"), "应指引到 `branch add`，实际：{err}");
}

/// 规范 §9：`str spec set` 覆盖整份 bundle 的 `spec`，**幂等**，且只接受 `1.<minor>.<patch>`。
#[test]
fn spec_set_rewrites_whole_bundle_and_is_idempotent() {
    let (root, aaa, bbb) = two_nodes("specset");
    let root_meta = root.join("._meta");
    let a_meta = root.join(&aaa).join("._meta");
    let b_meta = root.join(&bbb).join("._meta");

    // 初始由 `str init` / `node add` 写出本实现的规范版本
    assert!(read(&root_meta).contains(&format!("spec = \"{SPEC}\"")));

    // dry-run 不动盘
    cmd::spec_set(&root, "1.7.0", true).unwrap();
    assert!(read(&root_meta).contains(&format!("spec = \"{SPEC}\"")));

    // 真写：ROOT + 两个节点共 3 份全部落到目标版本（`v` 前缀应被规整掉）
    cmd::spec_set(&root, "v1.7.0", false).unwrap();
    for f in [&root_meta, &a_meta, &b_meta] {
        assert!(read(f).contains("spec = \"1.7.0\""), "{f:?}");
    }
    assert_eq!(report(&root).0, 0, "`spec` 只供人类追溯，改它不得让校验失败");

    // 幂等：值相同则一个字节都不写（含 `revision` / `updated_at`）
    let frozen = read(&root_meta);
    cmd::spec_set(&root, "1.7.0", false).unwrap();
    assert_eq!(read(&root_meta), frozen, "值未变则不得改写 `._meta`");

    // 非法版本串：major 必须为 1 且必须是三段（与 `._schema` 的正则同源）
    for bad in ["2.0.0", "1.9", "1.9.0.1", "", "abc", "1.x.0"] {
        assert!(
            cmd::spec_set(&root, bad, false).is_err(),
            "{bad:?} 应被拒绝"
        );
    }
}

#[test]
fn branch_rm_accepts_recursive_and_cleans_baseline() {
    let (root, aaa, _) = two_nodes("rmrecursive");
    cmd::sync(&root, false).unwrap();
    let bundle = Bundle::new(root.clone()).unwrap();
    assert!(str_format::baseline::load(&bundle).branches.contains_key(&aaa));

    cmd::branch_rm(&root, Some(aaa.clone()), true).unwrap();
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

// ─────────────────────────── entry add / entry rm ───────────────────────────

#[test]
fn entry_add_registers_with_inferred_metadata() {
    let (root, aaa, _) = two_nodes("entry-add");
    let node_dir = root.join(&aaa);

    // 文件：登记即自动补 size / sha256 / media_type
    write(&node_dir.join("notes.md"), "# hi\n");
    cmd::entry_add(
        &root,
        Some(aaa.clone()),
        "notes.md",
        &cmd::EntryNew {
            title: Some("说明".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let text = read(&node_dir.join("._meta"));
    assert!(text.contains("sha256 = \""));
    assert!(text.contains("media_type = \"text/markdown\""));
    assert!(text.contains("title = \"说明\""));

    // 重复登记拒绝
    assert!(
        cmd::entry_add(&root, Some(aaa.clone()), "notes.md", &cmd::EntryNew::default()).is_err()
    );
    // 磁盘不存在且非 optional 拒绝
    assert!(
        cmd::entry_add(&root, Some(aaa.clone()), "ghost.bin", &cmd::EntryNew::default()).is_err()
    );
    // optional 占位允许
    cmd::entry_add(
        &root,
        Some(aaa.clone()),
        "ghost.bin",
        &cmd::EntryNew {
            optional: true,
            ..Default::default()
        },
    )
    .unwrap();

    let (e, w, codes) = report(&root);
    assert!(
        e == 0 && w == 1 && codes.contains(&"W_OPTIONAL_MISSING".to_string()),
        "optional 占位应报 W_OPTIONAL_MISSING 告警（允许缺失）：{e} errors / {w} warnings {codes:?}"
    );

    // rm：只移除登记，不动磁盘
    cmd::entry_rm(&root, Some(aaa.clone()), "notes.md").unwrap();
    assert!(node_dir.join("notes.md").exists());
    let codes = report(&root).2;
    assert!(
        codes.contains(&"E_MANIFEST_MISSING".to_string()),
        "移除登记后磁盘文件应报缺失：{codes:?}"
    );

    // 写入的 `._meta` 仍是规范形式（canonicalize 生效）
    assert_eq!(cmd::fmt(&root, true, false).unwrap(), 0);
}

#[test]
fn entry_add_rejects_reserved_and_bad_roles() {
    let (root, aaa, _) = two_nodes("entry-bad");
    assert!(cmd::entry_add(&root, Some(aaa.clone()), "._x.bin", &cmd::EntryNew::default()).is_err());
    assert!(cmd::entry_add(&root, Some(aaa.clone()), "a/b", &cmd::EntryNew::default()).is_err());
    assert!(
        cmd::entry_add(
            &root,
            Some(aaa.clone()),
            "x.bin",
            &cmd::EntryNew {
                role: Some("node".into()),
                ..Default::default()
            }
        )
        .is_err()
    );
}

// ─────────────────────────── ignore / policies ───────────────────────────

#[test]
fn ignore_commands_manage_root_policies() {
    let (root, _aaa, _bbb) = two_nodes("ignore-cmd");

    // 幂等追加
    cmd::ignore_add(&root, "build/").unwrap();
    cmd::ignore_add(&root, "build/").unwrap();
    let text = read(&root.join("._meta"));
    assert!(text.contains("ignore = [\"build/\"]"), "{text}");

    // 生效：未登记的 build/ 不再报缺失
    std::fs::create_dir_all(root.join("build")).unwrap();
    write(&root.join("build/out.o"), "x");
    let (e, w, _) = report(&root);
    assert_eq!((e, w), (0, 0));

    // fmt --check：写入即规范（canonicalize 收口键序）
    assert_eq!(cmd::fmt(&root, true, false).unwrap(), 0);

    // rm 后恢复报缺失；重复 rm 拒绝
    cmd::ignore_rm(&root, "build/").unwrap();
    assert!(cmd::ignore_rm(&root, "build/").is_err());
    let codes = report(&root).2;
    assert!(codes.contains(&"E_MANIFEST_MISSING".to_string()), "{codes:?}");
}

#[test]
fn policies_set_validates_values_and_persists() {
    let (root, _aaa, _bbb) = two_nodes("policies-set");

    cmd::policies_set(&root, "gitignore", "false").unwrap();
    assert!(read(&root.join("._meta")).contains("gitignore = false"));

    // 非法值拒绝
    assert!(cmd::policies_set(&root, "gitignore", "yes").is_err());
    assert!(cmd::policies_set(&root, "manifest", "bogus").is_err());
    assert!(cmd::policies_set(&root, "id_version", "5").is_err());
    // ignore 是数组键：指引改用 ignore add/rm
    assert!(cmd::policies_set(&root, "ignore", "[]").is_err());
    // 未知键拒绝
    assert!(cmd::policies_set(&root, "nope", "1").is_err());

    cmd::policies_set(&root, "max_depth", "8").unwrap();
    let (e, w, codes) = report(&root);
    assert!(e == 0 && w == 0, "{e} errors / {w} warnings {codes:?}");
    assert_eq!(cmd::fmt(&root, true, false).unwrap(), 0);
}
