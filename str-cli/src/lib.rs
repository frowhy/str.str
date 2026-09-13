//! # STR 结构化树资源格式（`.str`）
//!
//! 规范见仓库根目录 `STR-FORMAT-PROMPT.md`（唯一真源）；
//! 本实现对应的规范版本见常量 [`SPEC_VERSION`]。
//!
//! 本 crate 提供：
//! - `meta`：`._meta`（TOML）的解析、**归一化**（TOML → 规范 JSON，键序/表序固定）与**保注释写回**；
//! - `bundle`：分支树遍历、懒加载、`id` 索引；
//! - `validate`：规范第 6 章全部错误码的实现；
//! - `cmd`：CLI 各子命令。
//!
//! ## 校验链路
//!
//! `._meta`（TOML）→ 解析 → 归一化为规范 JSON（`meta::Meta::to_json`）→ JSON Schema 2020-12 校验。

pub mod baseline;
pub mod bundle;
pub mod cmd;
pub mod error;
pub mod meta;
pub mod meta_edit;
pub mod util;
pub mod validate;

/// 本实现对应的 `str` 格式主版本。
pub const STR_MAJOR: i64 = 1;
/// 本实现对应的规范版本。
pub const SPEC_VERSION: &str = "1.9.0";
/// 内嵌的三种档位 JSON Schema（bundle 未自带 `._schema/` 时的兜底）。
///
/// Schema 属**格式规范资产**，唯一真源位于**仓库根** `._schema/`（与 `examples/` 同级）。
/// 而 crates.io 只打包 crate 目录内的文件、`include_str!` 也无法引用包外路径，
/// 因此 crate 内 `schema/` 保留一份**派生副本**：由 `bash scripts/sync-schema.sh`
/// 生成，并由 `tests/schema_sync.rs` 守卫其与真源逐字节一致（漂移即测试失败）。
pub const EMBEDDED_SCHEMAS: &[(&str, &str)] = &[
    (
        "root-meta.schema.json",
        include_str!("../schema/root-meta.schema.json"),
    ),
    (
        "node-meta.schema.json",
        include_str!("../schema/node-meta.schema.json"),
    ),
    (
        "branch-meta.schema.json",
        include_str!("../schema/branch-meta.schema.json"),
    ),
];
