# 变更日志 · `str-format`（CLI / 库）

crate `str-format` 的版本变更 —— 可执行文件名 `str`，另含可复用的纯 Rust 库。
版本号真源见 [`../VERSIONS.toml`](../VERSIONS.toml)；本 crate 的版本是**独立于规范版本**的一条轴
（`0.x` 阶段允许非兼容改动，1.0.0 之后按常规 semver）。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，日期为 Asia/Shanghai。
发行动作由 `.github/workflows/publish.yml`（crates.io）与 `release.yml`（五平台二进制）承担，
两者都由 `v*` tag 触发，且强制 `tag == Cargo.toml version`。

---

## [Unreleased] — 0.3.0

对应规范 **v1.9.0**。

### 新增

- **`str spec set <dir> <VERSION>`**：把整份 bundle 的 `spec`（规范版本声明）递归改写为目标版本
  —— 子 bundle 除外（§3.5），只改有差异的分支（**幂等**，第二次输出「已是 …（N 份）」），
  任一份 `._meta` 解析失败即**整体拒绝**、不写出部分结果；目标版本须形如 `1.<minor>.<patch>`。
  这是此前**唯一只能手改**的字段，至此「不得手改 `._meta`」不再有例外（规范 DoD 第 25 项）。

### 变更

- **`[uuid]` 位置参数统一「省略即 ROOT」**：`show` / `context` / `norm` / `ref add` / `branch add` /
  `branch rm` 的 UUID 位置参数改为可省略（此前前五个必填）。省略时目标为 ROOT；ROOT 上非法的两个
  操作不再靠「参数缺失」挡住，而是解析出 ROOT 后给出**带原因**的 `BadArg`（exit 2）——
  `branch add` 指引改用 `node add`，`branch rm` 明确「不能删除 ROOT」。
  规则是**放宽**，旧调用全部仍合法（规范 §9 条文，DoD 第 24 项）。
- `SPEC_VERSION` 1.8.0 → **1.9.0**；`str --version` 现在输出 `str 0.3.0`。

### 说明

- **为什么是 0.3.0**：`0.2.0` 已发布在 crates.io 且版本号不可复用（只能 yank），工作区内容必须
  换号发布；本次含功能新增与行为放宽，按 semver 为 minor。
- 改 `Cargo.toml` 版本后必须同步 `Cargo.lock`（`cargo build` 会自动改，`--locked` 不会），
  否则 CI 的 `cargo test --locked` 直接失败。

---

## [0.2.0] — 2026-09-14

对应规范 **v1.8.0**。

### 新增

- 字段写入命令：`str meta set` / `str entry set` / `str author add|rm` —— `._meta` 的描述性字段
  （`type` / `title` / `summary` / `note` / `order` / `tags` / `authors[]`）全部可经 CLI 写入。
- `str ls <dir> [uuid]`、`str ref rm <dir> <ref-id>`（位置参数形式）。
- `._cache/revisions.json` 基线：让 `E_REVISION_STALE` **可判定**（`sync` 只在 `revision` 前进时推进基线）。
- crate 内 `schema/` 派生副本 + `tests/schema_sync.rs`：与仓库根 `._schema/` 真源漂移即测试失败。
- `publish.yml`：发布到 crates.io（可信发布 OIDC / Secret 二选一；版本已存在时跳过）。

### 变更

- 强制 §4.9 确定性序列化：数组表按 `(order, path|id)` 排序、`order` 缺省视为最大；写命令落盘的
  `._meta` 一律已是规范形式，故「改完再 `fmt`」无事可做。

### 移除

- `policies.unknown_entry`（与 §4.8 `manifest` 语义重叠，从未被 Schema 与实现采纳）。

---

## [0.1.1] — 2026-09-14

**本 crate 未变更**（版本仍为 `0.1.0`）：该 tag 的改动只在技能包侧（`ensure-str.sh` 增加
GitHub Releases 下载 + SHA-256 强制校验）。当时尚无 `publish.yml` 的「tag == Cargo 版本」门禁，
因此 tag 与 crate 版本尚未绑定 —— 该绑定自 `v0.2.0` 起强制，现由 `scripts/check-versions.sh` 一并守卫。

---

## [0.1.0] — 2026-09-14

对应规范 **v1.7.0**。

### 新增

- 首个发行：`str` 单二进制（五平台预编译，无运行时依赖）+ 可复用库。
- M1–M10 全命令面（20+ 子命令，含 `init` / `node add` / `branch add` / `ref add` / `tree` / `ls` /
  `show` / `sync` / `validate` / `fmt` / `norm` / `context` / `export` / `codes` 等）。
- 规范 §6 的全部 **35 个错误码**各有 ≥1 个故意破坏用例，且断言精确到码
  （`tests/validate_codes.rs`）。
- 自举：`str validate . --strict` 在仓库自身恒为 0 errors / 0 warnings；`str fmt --check` 恒返回 0；
  在 bundle 未自带 `._schema/` 时使用内嵌 Schema 兜底。
