//! Schema 副本守卫（drift gate）。
//!
//! `str-cli/schema/` 是仓库根 `._schema/`（唯一真源）的**派生副本** —— 存在的唯一
//! 原因是 crates.io 只打包 crate 目录内的文件，而 `include_str!` 无法引用包外路径。
//!
//! 本测试在**仓库内**构建时校验二者逐字节一致；在**已发布的 crate** 中（无
//! `._schema/` 可比对）自动跳过，因此不会误报。

use std::path::Path;

/// 真源目录相对 crate 根的位置（`<repo>/._schema`）。
const SOURCE_OF_TRUTH: &str = "._schema";
/// crate 内派生副本目录。
const MIRROR: &str = "schema";

/// 读文件为字符串；失败即测试失败（含路径，便于定位）。
fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("读取失败：{}（{e}）", path.display()))
}

/// ① 直接比对两处**磁盘文件**：这是真正的漂移门禁，不依赖编译期快照。
#[test]
fn mirror_dir_matches_repo_source_of_truth() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(repo_root) = manifest_dir.parent() else {
        eprintln!("跳过：找不到 crate 的上级目录");
        return;
    };
    let canonical = repo_root.join(SOURCE_OF_TRUTH);
    if !canonical.is_dir() {
        eprintln!(
            "跳过：{} 不存在（独立构建 / 已发布的 crate，无真源可比对）",
            canonical.display()
        );
        return;
    }

    let mut checked = 0;
    for entry in std::fs::read_dir(manifest_dir.join(MIRROR)).expect("crate 内 schema/ 目录缺失") {
        let path = entry.expect("读取 schema/ 目录项失败").path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // 只比对真源里也存在的同名文件；多余文件另行报错
        let truth = canonical.join(name);
        assert!(
            truth.is_file(),
            "{MIRROR}/{name} 在真源 {SOURCE_OF_TRUTH}/ 中不存在；\
             请勿手工向 crate 内 schema/ 添加文件（它由 scripts/sync-schema.sh 生成）"
        );
        assert_eq!(
            read(&path),
            read(&truth),
            "{MIRROR}/{name} 与仓库根 {SOURCE_OF_TRUTH}/{name} 不一致；\
             请运行 `bash scripts/sync-schema.sh` 重新生成派生副本"
        );
        checked += 1;
    }
    assert_eq!(checked, 3, "crate 内 schema/ 应恰为 3 份（root / node / branch）");
}

/// ② 内嵌常量必须等于真源：确保 `include_str!` 指向的确实是这份副本。
#[test]
fn embedded_schemas_match_repo_source_of_truth() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(repo_root) = manifest_dir.parent() else {
        eprintln!("跳过：找不到 crate 的上级目录");
        return;
    };
    let canonical = repo_root.join(SOURCE_OF_TRUTH);
    if !canonical.is_dir() {
        eprintln!(
            "跳过：{} 不存在（独立构建 / 已发布的 crate，无真源可比对）",
            canonical.display()
        );
        return;
    }

    assert_eq!(str_format::EMBEDDED_SCHEMAS.len(), 3, "内嵌 Schema 应恰为 3 份");
    for (name, embedded) in str_format::EMBEDDED_SCHEMAS {
        let truth = read(&canonical.join(name));
        assert_eq!(
            *embedded,
            truth.as_str(),
            "内嵌 Schema {name} 与仓库根 {SOURCE_OF_TRUTH}/{name} 不一致；\
             请运行 `bash scripts/sync-schema.sh` 后重新构建"
        );
    }
}
