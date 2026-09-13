# 变更日志 · 仓库 / 发行

本文件记录**发行视角**的变更：每个 tag 冻结的「规范 · CLI · 技能包」三元组，以及仓库级
（工程、CI、文档）的改动。三个制品各自的详细变更见：

| 制品 | 变更日志 |
| --- | --- |
| 仓库 / 发行 | 本文件 |
| 格式规范（`spec` 轴） | [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md) |
| 参考实现 `str-format`（`cli` 轴） | [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md) |
| Agent 技能包 `str-skill`（`skill` 轴） | [`str-skill/CHANGELOG.md`](str-skill/CHANGELOG.md) |

版本号的真源是 [`VERSIONS.toml`](VERSIONS.toml)；本文件的版本矩阵必须与它一致。
格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，日期为 Asia/Shanghai。

## 发行版本矩阵

| tag | 日期 | 规范 | CLI | 技能包 |
| --- | --- | --- | --- | --- |
| 工作区（未发布） | — | 1.11.0 | 0.5.0 | 0.3.2 |
| [`v0.3.0`](https://github.com/frowhy/str.str/releases/tag/v0.3.0) | 2026-09-14 | 1.9.0 | 0.3.0 | 0.3.0 |
| [`v0.2.0`](https://github.com/frowhy/str.str/releases/tag/v0.2.0) | 2026-09-14 | 1.8.0 | 0.2.0 | 0.2.0 |
| [`v0.1.1`](https://github.com/frowhy/str.str/releases/tag/v0.1.1) | 2026-09-14 | 1.7.0 | 0.1.0（未变更） | 0.1.1 |
| [`v0.1.0`](https://github.com/frowhy/str.str/releases/tag/v0.1.0) | 2026-09-14 | 1.7.0 | 0.1.0 | 0.1.0 |

> 发行 tag = `v` + CLI 版本（锚定规则，见 `VERSIONS.toml`）；规范与技能包**不各自打 tag**，
> 它们的版本随发行一起冻结在该矩阵里。

---

## [`v0.5.0`](https://github.com/frowhy/str.str/releases/tag/v0.5.0) — 2026-09-14

规范 1.11.0 · CLI 0.5.0 · 技能包 0.3.2

### 变更

- **规范 1.10.0 → 1.11.0**：§9 的 `[uuid]` 缺省目标从 ROOT 细化为「当前节点」—— `[dir]` 指向
  分支目录时，省略 `<uuid>` 的命令以该分支为目标（`branch add` 挂靠、`branch rm` 自删并修复
  父级清单）；工具须以整份 bundle 为扫描视角，`.str` 硬边界不被向上穿越。详见
  [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **CLI 0.4.0 → 0.5.0**：`[uuid]` 缺省当前节点 + `node add` 在分支目录下的带原因拒绝。详见
  [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.3.1 → 0.3.2**：`[uuid]` 缺省说明更新为「当前节点」语义。
- `release.yml` 的 `workflow_dispatch` 默认 tag 更新为 `v0.5.0`。

---

## [Unreleased] — 目标 tag `v0.4.0`

### 变更

- **规范 1.9.0 → 1.10.0**：§9 全部命令的 `<dir>` 统一放宽为 `[dir]`（省略即当前工作目录；
  `init` 缺省以当前路径为基准目标）—— 与 v1.9.0 的 `[uuid]` 缺省 ROOT 同构，属纯放宽。
  连带 `spec set` 签名调整为 `<VERSION> [dir]`。详见 [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **CLI 0.3.0 → 0.4.0**：`[dir]` 缺省化 + `spec set` 参数顺序调整。详见
  [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 0.3.0 → 0.3.1**：命令索引补 `[dir]` 缺省说明，`--version` 示例同步到 0.4.0。
- `release.yml` 的 `workflow_dispatch` 默认 tag 更新为 `v0.4.0`。

---

## [`v0.3.0`](https://github.com/frowhy/str.str/releases/tag/v0.3.0) — 2026-09-14

规范 1.9.0 · CLI 0.3.0 · 技能包 0.3.0

### 新增

- **`VERSIONS.toml`** —— 版本唯一真源：规范 / CLI / 技能包三条轴 + 发行 tag 锚。
- **`scripts/check-versions.sh`** —— 版本一致性门禁：逐点比对「真源 ↔ 各声明点」，CI 在
  release / publish 的 test 阶段强制执行；tag 推送时还会校验「推送的 tag = 真源声明的 tag」。
- **四份 CHANGELOG.md**（本文件 + 规范 / CLI / 技能包各一份）。
- **SkillHub 自动发布**：`.github/workflows/publish-skillhub.yml` —— 推 `v*` tag 即发布技能包到
  SkillHub（版本门禁 → 本地预检 `--dry-run` → 正式发布）；`str-skill/` 自上一条 tag 无变化时自动跳过。
- 技能包 `SKILL.md` 增加 `version` 字段（此前技能包**没有版本号**），并补齐平台要求的发布
  frontmatter（`slug` / `displayName` / `summary` / `license`）。
- CLI 新增 `str spec set <dir> <VERSION>` 与规范 §9 对应条文、DoD 第 25 项。

### 变更

- **规范 1.8.0 → 1.9.0**：§9 的 `<uuid>` / `<anchor-uuid>` 统一放宽为 `[uuid]`（省略即 ROOT），
  并新增 `spec` 的 CLI 写入路径 —— 旧调用全部仍合法，属纯放宽。详见 [`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)。
- **CLI 0.2.0 → 0.3.0**：0.2.0 已被 crates.io 占用且版本号不可复用，工作区内容必须换号发布
  （否则 `publish.yml` 会因「版本已存在」而静默跳过）。详见 [`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)。
- **技能包 → 0.3.0**：always-on 触发 + 触发面扩大到一切文件写入，并声明依赖 CLI >= 0.3.0。
- `release.yml` 的 `workflow_dispatch` 默认 tag 由 `v0.1.0` 更正为 `v0.3.0`（此前长期滞后）。

### 修复

- 消除「同一份制品两个版本号」：`str-skill-<tag>.zip` 此前打包为 `v0.2.0`，而技能包文档自称
  「与规范同步到 v1.9.0」——现在技能包有自己的 `version`，并在文档里明确三条轴的对应关系。

---

## [`v0.2.0`](https://github.com/frowhy/str.str/releases/tag/v0.2.0) — 2026-09-14

规范 1.8.0 · CLI 0.2.0 · 技能包 0.2.0

### 新增

- 发布到 **crates.io**：`publish.yml`（可信发布 OIDC 与 Secret 二选一；目标版本已存在时跳过上传）；
  crate 内 `schema/` 派生副本 + `tests/schema_sync.rs` 守卫与仓库根真源逐字节一致。
- CLI 字段写入命令补齐：`str meta set` / `str entry set` / `str author add|rm`。
- `._cache/revisions.json` 基线，使 `E_REVISION_STALE` 可判定。
- 技能包 `ensure-str.sh` 第 6 步：`cargo install` 兜底安装（隔离到缓存目录）。

### 变更

- 规范 1.7.0 → 1.8.0：§4.9 排序细则明确化（`order` 缺省视为最大）、§9 命令面补齐、§9「写前校验」
  改述为「产出即合法且规范」。
- CLI 0.1.0 → 0.2.0；`ensure-str.sh` 内置默认版本同步。

### 移除

- `policies.unknown_entry`（与 `manifest` 语义重叠，从未被采纳）。

---

## [`v0.1.1`](https://github.com/frowhy/str.str/releases/tag/v0.1.1) — 2026-09-14

规范 1.7.0 · CLI 0.1.0（未变更）· 技能包 0.1.1

### 新增

- 技能包 `ensure-str.sh` 第 5 步：从 GitHub Releases 自动下载预编译二进制，并用 `SHA256SUMS.txt`
  **强制校验**（取不到校验和一律中止，不做降级）。

### 说明

- 本次仅技能包变更，crate 版本未 bump —— 当时 `publish.yml` 尚未建立，tag 与 Cargo 版本尚未绑定；
  该绑定自 v0.2.0 起由 `publish.yml` 强制（现在由 `scripts/check-versions.sh` 一并守卫）。

---

## [`v0.1.0`](https://github.com/frowhy/str.str/releases/tag/v0.1.0) — 2026-09-14

规范 1.7.0 · CLI 0.1.0 · 技能包 0.1.0

### 新增

- 首个发行：五平台预编译二进制 + `str-skill-<tag>.zip` + `SHA256SUMS.txt`。
- Rust 参考实现全命令面（20+ 子命令），§6 全部 **35 个错误码**各有破坏用例且断言精确到码。
- 自举：本仓库自身即 `.str` bundle，`str validate . --strict` 恒为 0 errors / 0 warnings。
- 规范 v1.7.0：`.str` 子 bundle 硬边界与 `role = "bundle"`、元数据豁免扩至 VCS、`W_ROOT_STRAY`
  语义修正、Schema 与 policy 冲突修正。
- Agent 技能包 `str-skill` 初版（MUST / NEVER 硬规则 + references + `ensure-str.sh`）。
