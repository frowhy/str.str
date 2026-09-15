# 变更日志 · `str-skill`（Agent 技能包）

本文件记录技能包**自身**的版本变更（`SKILL.md` frontmatter 的 `version`）。

技能包版本是一条**独立于 CLI 版本与发行 tag** 的轴：技能包只在自己内容变化时升版，因此
`0.2.1` 会随发行 tag `v0.3.0` 一起打包发布 —— 这是设计，不是笔误。三条轴的对应关系登记在
[`../VERSIONS.toml`](../VERSIONS.toml)，并由 `scripts/check-versions.sh` 在 CI 中守卫。

> 为什么以前会乱：技能包此前**没有版本号**，于是 `README.md` 用「技能包与规范同步到 v1.9.0」
> 来代替版本 —— 而同一份制品打出来的包叫 `str-skill-v0.2.0.zip`。两个互相矛盾的版本标识，
> 用户无法判断拿到的是哪一版。0.2.1 起技能包有自己的版本，并在文档里明确三条轴的关系。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，日期为 Asia/Shanghai。

---

## [0.3.3] — 2026-09-16

### 变更

- `spec-digest.md` / `cli-reference.md` 头部规范版本对齐 **v1.12.0**；`str --version` 示例与
  `ensure-str.sh` 的 `DEFAULT_VERSION` 同步为 `str 0.5.1` / `v0.5.1`。
- 保留目录免登记的说明随规范同步（`._schema/` / `._cache/` 不参与 `entries[]` 清单比对）。

---

## [0.3.2] — 2026-09-14

### 变更

- **`[uuid]` 缺省说明更新为「当前节点」语义**（规范 v1.11.0 / CLI 0.5.0）：`[dir]` 为 bundle 根
  时省略 `<uuid>` 即 ROOT；指向分支目录时即该分支（`branch add` 挂靠、`branch rm` 自删、
  `show` 打印当前分支），并补充 `node add` 在分支目录下的带原因拒绝。
- `--version` 示例输出同步为 `str 0.5.0`；`ensure-str.sh` 的 `DEFAULT_VERSION` 同步为 `v0.5.0`。

---

## [0.3.1] — 2026-09-14

### 变更

- **命令索引补 `[dir]` 缺省说明**：全部 bundle 命令的目录位置参数可省略（缺省当前工作目录；
  `init` 缺省以当前路径为基准目标，追加 `.str` 定名），对应规范 v1.10.0 的 §9 条文与 CLI 0.4.0。
- `--version` 示例输出同步为 `str 0.4.0`；`ensure-str.sh` 的 `DEFAULT_VERSION` 同步为 `v0.4.0`。

---

## [0.3.0] — 2026-09-14

### 变更

- **默认开启（always-on）**：`SKILL.md` frontmatter `description` 从被动触发措辞
  （"whenever a STR bundle is involved / user mentions STR"）改写为声明式默认加载措辞
  （"ALWAYS-ON skill - load it automatically at the start of every session by default,
  do NOT wait for the user to mention STR"）。skill 的自动触发由 `description` 驱动，
  被动措辞导致 Agent 只在用户显式提到 STR 时才加载本技能 —— 现在安装即默认生效。
- **触发面扩大到一切文件写入**：description 与 Default-on activation 明确
  "Whenever the agent creates or modifies ANY file, STR is the DEFAULT"——
  只要 Agent 创建或修改任何文件（笔记/记录/文档/数据集/资产/导出报告），
  一律存入 STR bundle（无合适 bundle 时用 `str init` / `str node add` / `str branch add`
  新建，而不是散落工作目录）；bundle 内 payload 仍用常规文件工具编辑，
  收尾 `str sync` + `str validate --strict`。
- **README 新增「保证自动触发：Rules 兜底」章节**：实测 `description` 声明式措辞仍不能在
  所有客户端保证 always-on（按语义检索、加载预算、无自动加载三类宿主都会漏触发），
  给出唯一可靠兜底 —— 宿主 always-apply 规则：CodeBuddy 项目级
  `.codebuddy/rules/str-skill/RULE.mdc`（`alwaysApply: true` + 会话开始先读 SKILL.md
  并遵循硬规则，规则正文可直接复制）与用户级 `~/.codebuddy/rules/` 跨项目写法，
  以及 Claude Code `CLAUDE.md` / 根 `CODEBUDDY.md` 等同构载体；并附加载生效的验证方法。
  强调规则的 frontmatter 必须带 `alwaysApply: true`，否则与 skill 同样退化为检索触发。

---

## [0.2.1] — 2026-09-14

### 新增

- `SKILL.md` frontmatter 增加 `version` 字段（此前技能包**没有任何版本号**）。
- **`SKILL.md` 补齐 SkillHub 发布 frontmatter**：`slug` / `displayName` / `summary` / `license`。
  此前技能包没有任何发布身份声明，`slug` 只能靠平台按目录名兜底推断 —— 不可追溯、也无法在提交前校验；
  现在三个必填项（`slug` / `version` / `displayName`）显式声明，并由门禁逐字守卫。
  键名以 **CLI 源码**为准：`_validate_metadata()` 只读 camelCase `displayName` 且缺失即报错，
  故不使用市场里常见的 snake_case `display_name`（曾一度并存，属实测误判，已移除）。
- 仓库新增 [`.github/workflows/publish-skillhub.yml`](../.github/workflows/publish-skillhub.yml)：
  推 `v*` tag 自动发布到 SkillHub（版本门禁 → 本地预检 → 正式发布），且当
  `str-skill/` 自上一条 tag 无变化时**自动跳过**（技能包是独立版本轴）。
- 本 `CHANGELOG.md`。

### 变更

- 同步规范 **v1.9.0**：`[uuid]` 位置参数**省略即 ROOT**（`branch add` / `branch rm` 在 ROOT 上
  带原因拒绝）；`._meta` 的 `spec` 字段改由 `str spec set` 写入 —— 因此「不得手改 `._meta`」
  在技能包的硬规则里不再有例外。
- 声明**依赖 CLI >= 0.3.0**（上述命令自该版本起提供）。
- **发布身份定案**：slug = `str-skill`（2026-09-14，格式所有者拍板；与本地目录名一致）。
  曾评估备选 `str-tree`（在技能市场里可搜索性更好），已否决 —— `slug` 是单行道，
  首次发布后更改等于另立一个 skill。
- 发布身份纳入版本真源：`VERSIONS.toml` 新增 `[skill].slug` / `displayName`（键名与 SKILL.md 一致），
  `scripts/check-versions.sh` 相应增加 3 项检查（`slug` 逐字一致、
  `displayName` 逐字一致、`slug` 满足 kebab-case 且长度 2~128 —— 长度依据 CLI 源码
  `_SLUG_PATTERN` + `len < 2` 判定，而非其报错文案里不一致的「3-128」）—— `slug` 全网唯一且发布后
  不宜更改，必须在发布**前**拦住错误。
- `README.md` 版本节改写为「技能包版本 / 适配规范 / 依赖 CLI / 发布身份」四项，不再借用规范版本号；
  新增「发布到 SkillHub」章节（frontmatter 字段表、CI 鉴权与幂等、手动命令、常见错误码）；
  `SKILL.md` 与 `references/cli-reference.md` 中的 `str --version` 示例同步到 `0.3.0`。

---

## [0.2.0] — 2026-09-14

### 新增

- `ensure-str.sh` 第 6 步：`cargo install str-format --version <tag> --locked --root <缓存>`
  兜底安装。安装根隔离在缓存目录，**不写 `~/.cargo/bin`**（不在用户不知情时全局装东西）；
  排在下载之后（源码编译首次数分钟，而下载是带 SHA-256 校验的预编译产物）；
  新增开关 `STR_NO_CARGO_INSTALL=1`；失败一律落到最后的失败分支，**绝不静默降级为手改 `._meta`**。

---

## [0.1.1] — 2026-09-14

### 新增

- `ensure-str.sh` 第 5 步：从 GitHub Releases 下载当前平台（5 种映射）的预编译二进制；
  **强制**用同一 release 的 `SHA256SUMS.txt` 逐字节校验，取不到校验和 / 缺条目 / 不匹配一律
  **中止且不写入缓存** —— 不存在「未校验就用」的降级分支；结果按 `<tag>/<target>` 缓存复用。

---

## [0.1.0] — 2026-09-14

### 新增

- 初版：`SKILL.md`（MUST / NEVER 硬规则 + 读写路径 + 命令索引，英文书写以利分发）、
  `references/`（`cli-reference.md` / `spec-digest.md` / `workflows.md`，中文详注）、
  `scripts/ensure-str.sh`（解析 / 构建 CLI 的 6 步链，命中即返回）。
