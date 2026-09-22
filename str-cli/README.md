# str-format

[![crates.io](https://img.shields.io/crates/v/str-format.svg)](https://crates.io/crates/str-format)
[![docs.rs](https://img.shields.io/docsrs/str-format.svg)](https://docs.rs/str-format)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/frowhy/str.str/blob/main/LICENSE-MIT)

**STR（Structured Tree Resource，结构化树资源）** 格式的参考实现：读写库 + 校验器 + `str` 命令行工具。
用一个**目录树**存储结构化资源，让人类与 AI Agent 在同一份资产上安全地读写、协作与版本控制。

- 格式：`.str` 目录 bundle（形态对标 macOS `.app`）—— 纯目录 + 纯文本，零平台依赖
- 本 crate：`str` 二进制（20+ 子命令、35 个校验错误码）+ 可复用的纯 Rust 库
- 规范正文（唯一真源）、JSON Schema、示例 bundle、Agent 技能包：见
  [frowhy/str.str](https://github.com/frowhy/str.str)

## 安装

```sh
cargo install str-format
```

装出的可执行文件名为 `str`（无运行时依赖）。也可不安装，直接从仓库源码构建：

```sh
git clone https://github.com/frowhy/str.str
cd str.str/str-cli && cargo build --release   # 产物：target/release/str
```

## 快速上手

```sh
str init 我的项目.str --name 我的项目
str node add   我的项目.str --type code.project --title 核心引擎 --summary "…"
str branch add 我的项目.str [uuid] --type code.docs --title 设计文档
str tree       我的项目.str --show-refs
str sync       我的项目.str          # 磁盘实际状态 → 修正 entries（幂等）
str validate   我的项目.str --strict # 0 errors / 0 warnings 才算交付
```

作为库使用：

```rust
use str_format::{bundle::Bundle, validate};

let bundle = Bundle::new("项目.str")?;      // 路径不存在 → Err(Error::NotFound)
let report = validate::validate(&bundle)?;  // 遍历整棵树并按规范逐项校验
assert_eq!(report.error_count(), 0);
```

## 三份内嵌 Schema 与仓库根真源的关系

`src/lib.rs` 通过 `include_str!("../schema/…")` 内嵌三份档位 JSON Schema
（`root` / `node` / `branch`），供 bundle 未自带 `._schema/` 时兜底。

- **唯一真源**在仓库根 `._schema/`（属格式规范资产，与 `examples/` 同级）；
- crate 内 `schema/` 是它的**派生副本** —— 因为 crates.io 只打包 crate 目录内的文件，
  且 `include_str!` 无法引用包外路径（否则 `cargo package` 的验证构建必然失败）；
- 副本由 `bash scripts/sync-schema.sh` 生成，并由 `tests/schema_sync.rs` 守卫二者
  逐字节一致：**一旦漂移，`cargo test` 直接失败**，不会静默分叉。

## 版本

本 crate 的版本（当前 **0.7.2**）是**独立于格式规范版本**的一条轴：规范定义磁盘上的数据契约，
crate 定义代码 / 命令契约，两者可各自演进。当前实现对应规范 **v1.13.0**（`str` 主版本 = `1`）；
各条轴与发行 tag 的对应关系登记在仓库根 `VERSIONS.toml`。逐版变更见 [`CHANGELOG.md`](CHANGELOG.md)。

```sh
str --version     # str 0.7.2
```

## 贡献

仓库级贡献流程（版本轴纪律、提交信息规范、提交前自检）见根
[README 的「贡献指南」](https://github.com/frowhy/str.str#贡献指南)；
本 crate 相关变更请同步 `CHANGELOG.md` 并确保 `cargo test` 通过。

## 许可

双许可 `MIT OR Apache-2.0`，见 [LICENSE-MIT](https://github.com/frowhy/str.str/blob/main/LICENSE-MIT) / [LICENSE-APACHE](https://github.com/frowhy/str.str/blob/main/LICENSE-APACHE)。
