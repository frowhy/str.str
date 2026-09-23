# STR — AI 时代的结构化资源标准

[![crates.io](https://img.shields.io/crates/v/str-format.svg)](https://crates.io/crates/str-format)
[![docs.rs](https://img.shields.io/docsrs/str-format.svg)](https://docs.rs/str-format)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

**STR（Structured Tree Resource，结构化树资源）** 是一种开放的文件格式与工具链：
用**目录树**存储一切结构化资源，用**思维导图语义**组织它们，让**人类与 AI 代理**
在同一份资产上安全地读写、协作与版本控制。

- 格式：`.str` 目录 bundle（形态对标 macOS `.app`）—— 纯目录 + 纯文本，零平台依赖
- 规范版本：v1.13.0（`str` 主版本号 = `1`）
- 参考实现：Rust CLI（`str-cli/`），20+ 子命令，35 个校验错误码
- 状态：`DRAFT → 待评审`

```sh
$ str tree examples/客户运营.str --show-refs
客户运营.str
├─ [1] 客户档案 · 张伟  (crm.customer)
│  ⇢ 关联: 01928f3a-…-0002  --related--
│  └─ [1] 跟进记录  (crm.followup_log)
│     └─ [1] 2026-09 会议纪要  (doc.meeting_note)
├─ [2] 订单数据集  (crm.order_dataset)
└─ [3] 标签体系  (crm.tag_system)
```

---

## 愿景：从一种格式，到 AI 时代的资源标准

AI 正在从「生成一段内容」走向「**持续运营一整套资产**」：

- **vibe coding**：AI 代理在仓库里写代码、维护文档、管理任务；
- **工作台制作**：AI 编排项目看板、知识库、协作空间；
- **图形创作**：AI 管理设计稿、提示词、配色、素材版本；
- **视频创作**：AI 维护分镜、脚本、素材库、成片工程；
- **文学创作**：AI 与作者共建世界观、角色、章节、修订史。

每一个领域今天都在用**不同的、互不相通的容器**：数据库、专有文档格式、
云笔记、工程文件……共同的问题是——

> **数据不开放、AI 读不全、人机改会撞、离开平台即失效。**

STR 的野心是做**所有这些领域的同一层底座**：
一种 AI 与人类共用的、可校验、可 diff、可无限生长的**通用资源容器**。

**一个格式，一套工具，一种协议，适配全部 AI 创作/生产场景。**

---

## 为什么需要 .str

| 痛点 | 数据库 | 专有格式（Notion 等） | 裸文件夹 | **`.str`** |
| --- | --- | --- | --- | --- |
| AI 按需读取、上下文可控 | ❌ | ❌ | ⚠️ 无语义 | ✅ 渐进式披露 |
| Git diff / merge 友好 | ❌ | ❌ | ✅ | ✅ 确定性序列化 |
| 数据主权（`cp -r` 即备份） | ❌ | ❌ | ✅ | ✅ |
| 结构语义（导图/实体/关联） | ✅ | ⚠️ 导出即降级 | ❌ | ✅ |
| 强校验（改错立刻报错定位） | ✅ | ❌ | ❌ | ✅ 35 个错误码 |
| 跨平台零依赖 | ⚠️ | ❌ | ✅ | ✅ |

---

## 30 秒了解格式

```
项目.str/
├── ._meta                        # ROOT：bundle 元信息 + 一级分支结构
├── ._schema/                     # 保留目录：JSON Schema
├── <UUID-v7>/                    # 独立节点（深度 1）—— 一等实体
│   ├── ._meta                    # 元信息 + 内容清单 + 跨枝关联
│   ├── <任意文件>                 # payload / asset，登记进 entries[]
│   └── <UUID-v7>/                # 关联分支（深度 ≥2）—— 思维导图生长
│       ├── ._meta
│       └── <任意深度继续嵌套>
└── <UUID-v7>/                    # 更多独立节点
```

三条核心规则：

1. **目录树即思维导图**：深度 1 = 独立节点（`node`），深度 ≥2 = 关联分支（`branch`），任意层级都能存任意文件；
2. **`._meta` 是每层的大脑**：`type / title / summary / tags` 供 AI 路由检索，`entries[]` 是内容清单唯一真源，`refs[]` 声明跨枝关联线（不复制数据）；
3. **文件系统 = 存在性权威，`._meta` = 语义权威**：校验器双向比对，改错立刻报错并给出可定位路径。

## 核心功能一览

- **结构即导图**：`str tree` 文本树与 GUI 思维导图同源呈现，深度 1 独立节点 + 任意深度关联分支；
- **渐进式披露**：`str context --budget` 裁剪出可直接拼接进模型上下文的片段，token 成本可控；
- **强校验**：35 个错误码双向比对文件系统与 `._meta`，改错立刻报错并给出可定位路径；
- **Git 友好**：确定性序列化（固定键序）+ 幂等 `sync`，diff 永远只反映真实变更；
- **人机双入口**：Agent 走 `str` CLI，人类用 str-gui 桌面编辑器，同一份磁盘真相；
- **跨 Agent 技能包**：str-skill 把用法固化成 MUST / NEVER 行为契约，一次安装处处生效。

---

## 为 AI 而生：渐进式披露协议

AI 读取 bundle 不需要全量加载——每一层 `._meta` 自带摘要，按需下钻：

| 步骤 | 动作 | 上下文成本 |
| --- | --- | --- |
| 1 | 读 ROOT `._meta` 的一级分支摘要 | 极小 |
| 2 | 按 `type/tags/title/summary` 锁定目标分支 | — |
| 3 | 读目标分支 `._meta`（元信息 + 下级摘要） | 小 |
| 4 | 需要时才读具体 payload | 按需 |

配套命令与技能：

```sh
str context 项目.str [uuid] --depth 2 --budget 8k   # 裁剪出可直接拼接进模型上下文的片段（省略 uuid 即从 ROOT 起）
```

- [`str-skill/`](str-skill/) — 通用 Agent 技能包（CodeBuddy / Claude Code / Cursor 等），
  强制所有 `._meta` 写操作经 `str` CLI 完成，写后必须 `sync` + `validate --strict` 零错误才算交付；
  并内置 str-gui 协作引导：Agent 侧永远走 CLI，何时建议人类转向桌面编辑器有明确判据；
- `str norm` / `str export` — 输出归一化 JSON，供任何语言的外部工具消费。

---

## 全场景适配：`type` 点分命名空间

`._meta.type` 采用点分命名空间（业务类型零注册成本，`x-` 前缀支持厂商扩展），
同一个格式在不同 AI 领域只需约定不同的 type 词汇表：

| 领域 | 典型 type | 分支树长什么样 |
| --- | --- | --- |
| **vibe coding** | `code.project` `code.module` `code.docs` | 仓库结构 + 模块说明 + ADR，AI 按摘要路由 |
| **工作台** | `work.board` `work.task` `work.workflow` | 项目 → 任务 → 子任务，`refs` 表达依赖（`depends_on`） |
| **图形创作** | `art.illustration` `art.design` `art.palette` | 作品 → 迭代版本 → 提示词/参考图/导出件 |
| **视频创作** | `video.project` `video.scene` `video.shot` | 项目 → 分集 → 分镜，asset 挂素材/配音/字幕 |
| **文学创作** | `lit.novel` `lit.chapter` `lit.lore` | 书 → 章节，世界观/角色库经 `refs` 跨章引用（`instance_of`） |
| **业务数据** | `crm.customer` `crm.followup_log` | 客户 → 跟进记录 → 会议纪要（见 `examples/客户运营.str`） |

核心收益：**跨领域的资产可以互相引用**——一个视频项目 `refs` 到图形创作库里的
角色设定，一个文学项目 `refs` 到工作台里的选题任务，全部在同一格式、同一工具链内闭环。

---

## 快速开始

```sh
# 安装 CLI：crates.io 发行版（包名 str-format，可执行文件名 str）
cargo install str-format

# 或从本仓库源码安装
cargo install --path str-cli

# 创建 bundle
str init 我的项目.str --name 我的项目

# 日常操作
str node add 我的项目.str --type code.project --title 核心引擎 --summary "…"
str branch add 我的项目.str [uuid] --type code.docs --title 设计文档
str ref add  我的项目.str [uuid] --target <uuid> --rel depends_on
str sync 我的项目.str          # 磁盘实际状态 → 修正 entries（幂等）
str validate 我的项目.str --strict

# 提交前自检（本仓库自身即一个 bundle）
str validate .
```

> **`[uuid]` 缺省 ROOT**：凡以单个分支为目标的命令，其 `<UUID>` 位置参数都可省略，缺省目标为 **ROOT** ——
> 例如 `str show 我的项目.str` 打印 ROOT 的 `._meta`、`str context 我的项目.str` 从 ROOT 起裁剪上下文。
> 只有 `str ref add --target` 必填；`branch add` / `branch rm` 在 ROOT 上无意义，会以带原因的 `BadArg`（exit 2）拒绝。

关键设计：`str sync` 幂等（第二次零 diff）、`._meta` 写回保注释、
序列化确定性（固定键序/表序/排序）—— Git diff 永远只反映真实变更。

---

## Agent 接入指南（str-skill）

新用户三步：**装 CLI → 装 Skill → 首次激活自举**。深度细节见
[`str-skill/README.md`](str-skill/README.md)。

### 1. 安装

先装 CLI（三选一，也可以什么都不做——技能的解析脚本会自动搞定）：

```sh
cargo install str-format     # crates.io（包名 str-format，可执行文件 str）
# 或：从 GitHub Releases 下载当前平台预编译二进制（随附 SHA256SUMS 校验和）
# 或：本仓库源码构建 cargo build --release（见「依赖项」）
```

再把技能装进你的 Agent：

- **CodeBuddy**：`mkdir -p .codebuddy/skills && cp -R str-skill .codebuddy/skills/str-skill`
  （用户级装 `~/.codebuddy/skills/`，跨项目生效）；
- **SkillHub（推荐）**：技能页 <https://skillhub.cn/skills/indiv-frowhy/str-skill>。
  最省事的方式是把下面这句提示词发给你的 Agent，由它按 SkillHub 官方引导完成安装：

  > 请根据 <https://skillhub.cn/install/skillhub.md>，安装 `@indiv-frowhy/str-skill`。

  也可手动执行：`skillhub install str-skill --namespace indiv-frowhy --dir ~/.codebuddy/skills --force`
  （装完在 `@indiv-frowhy/` 命名空间目录下，需把内容镜像回 `str-skill/` 并删除命名空间目录）；
- **其它 Agent（Claude Code / Cursor 等）**：按各自的 skill / rules 目录约定放置即可。

### 2. 配置：让「自动触发」万无一失

技能本身是 **always-on**（安装即默认加载，不等你点名 STR）。不同宿主对 skill 的加载
策略不同，两级兜底保证触发：

- **宿主 always-apply 规则**（最可靠）：在 `.codebuddy/rules/str-skill/RULE.mdc` 写一条
  `alwaysApply: true` 的「会话开始先读 SKILL.md」规则——模板见
  [`str-skill/README.md`](str-skill/README.md)「Rules 兜底」一节，原样复制即可；
- **首次激活自举**（自动化）：技能的 Step 0.5 在项目记忆文件缺失规则块时运行
  `scripts/bootstrap-rule.sh`，把「调用时机 / 调用方式 / 参数格式」规则块幂等写入
  项目根 `AGENTS.md`（自动探测 `AGENT.md` / `CLAUDE.md` / `CODEBUDDY.md` / `GEMINI.md`
  等等效文件），此后每次会话都能据此正确使用 str。

### 3. 调用方式

Agent 无需自己判断安装途径——解析脚本按序尝试：`$STR_BIN` → PATH → 源码构建 →
GitHub Releases 下载（SHA-256 强制校验）→ crates.io 编译：

```sh
STR="$(sh <skill-dir>/scripts/ensure-str.sh)" || exit 1
"$STR" --version                    # 期望：str 0.7.2
```

高频命令分三组：

| 阶段 | 命令 |
| --- | --- |
| 读 | `str tree <dir> --show-refs` · `str context <dir> [uuid] --depth 2 --budget 8000` · `str show <dir> [uuid]` |
| 写 | `str init <dir>` · `str node add` · `str branch add` · `str entry add|set` · `str meta set` · `str ref add --target <uuid>` |
| 收尾（交付门禁） | `str sync <dir>` → `str validate <dir> --strict`（0 errors / 0 warnings）→ `str fmt <dir> --check`（0） |

### 4. 常见使用场景

| 场景 | 起手动作 |
| --- | --- |
| vibe coding：仓库说明 / ADR / 任务树 | `str init` → `node add --type code.*`，AI 按摘要路由读取 |
| 工作台：项目 → 任务 → 子任务 | `branch add` 层级 + `ref add --rel depends_on` 表达依赖 |
| 会议纪要 / 客户跟进 / 业务数据 | 参照 [`examples/客户运营.str`](examples/客户运营.str)（`crm.*` 词汇表） |
| 创作：设计稿 / 分镜 / 章节 / 世界观 | `art.*` / `video.*` / `lit.*`，素材作为 payload 挂分支 |
| 想可视化浏览 / 手动整理 | 建议人类使用 str-gui 桌面编辑器；Agent 侧永远走 CLI |

## 依赖项

| 组件 | 依赖 | 说明 |
| --- | --- | --- |
| `str` CLI | Rust ≥1.88 工具链（源码安装时）；预编译二进制无运行时依赖 | `cargo install str-format`，或直接用 GitHub Releases 产物（五平台） |
| str-skill | 任意支持 Skill / Rules 的 Agent | `ensure-str.sh` 需 bash、curl 或 wget、sha256sum 或 shasum、tar；`bootstrap-rule.sh` 需 python3 |
| str-gui | Rust + Slint（源码构建） | 首次构建前先跑 `str-gui/scripts/vendor-winit.sh` 与 `vendor-slint.sh`；或直接用 Release 附带的 Windows / Linux / macOS 产物 |

## 常见问题（FAQ）

**`str validate` 报 `E_MANIFEST_MISSING`？**
新文件没有登记进 `entries[]`。跑 `str sync <dir>` 把磁盘状态登记后再校验。

**报 `E_REVISION_STALE`？**
有人绕过 CLI 手改了 `._meta`（动了 `updated_at` 但没推进 `revision`）。用 CLI 重写相关
字段即可，`._cache` 基线不会被骗过。

**报 `W_BUNDLE_SUFFIX`？**
目录是合法 bundle 但名字不以 `.str` 结尾，仅命名约定告警，说明后可保留。

**装了 skill 却不自动触发？**
宿主可能按语义检索加载。两级兜底：配置 always-apply 规则（见「配置」一节），
或等技能首次激活时自举 `AGENTS.md` 规则块。

**为什么禁止手改 `._meta`？**
键序、`revision`、`updated_at`、摘要与指纹都是格式契约的一部分；所有字段都有 CLI
写入路径（`str meta set` / `str entry set` / `str author add`…），手改必然产生漂移。

**GUI 编辑器从哪里获取？**
每个 GitHub Release 附带 Windows / Linux / macOS 产物；源码构建见「依赖项」。

**和数据库 / Notion 有什么区别？**
见上文对比表：纯目录 + 纯文本，零平台依赖，Git 可 diff，AI 渐进式读取；
`str norm` / `str export` 随时导出 JSON，数据主权在自己手里。

---

## 设计原则

1. **SSOT**：`entries[]` 是内容清单唯一真源；本仓库规范文档是格式的唯一真源；
2. **禁止持久化索引**：可重建缓存只进 `._cache/`（可删、gitignore）；
3. **不得静默容错**：结构损坏必须报错码 + 可定位路径；
4. **跨平台**：不依赖符号链接 / 扩展属性 / 文件系统大小写；
5. **OS 噪声豁免**：`._*` AppleDouble、`.DS_Store`、`.git/` 等一律忽略不误报；
6. **bundle 硬边界**：`.str` 目录即独立 bundle，可安全嵌套；
7. **前向兼容**：未知字段（`[ext]` 命名空间）必须原样保留。

完整规则（目录结构、字段表、35 个错误码、协作协议、AI 读写协议、ADR）见
**[`SPEC.md`](SPEC.md) —— 格式规范的唯一真源**。

---

## 仓库结构

| 位置 | 说明 |
| --- | --- |
| `SPEC.md` | 格式规范（唯一真源，v1.13.0） |
| `VERSIONS.toml` | **版本唯一真源**：规范 / CLI / 技能包 / 界面四条轴 + 发行 tag 锚 |
| `CHANGELOG.md` | 仓库与发行的变更日志（每个 tag 冻结的「规范 · CLI · skill · gui」四元组） |
| `SPEC-CHANGELOG.md` | 格式规范的变更日志（发行 / 兼容视角；细则真源仍是正文《修订记录》） |
| `._schema/*.json` | 三档 JSON Schema（2020-12），格式规范性产物（Schema 的唯一真源） |
| `examples/客户运营.str/` | 官方示例 bundle（`str validate --strict` 零错误） |
| `scripts/*.sh` | `build-example.sh` 幂等重建示例 bundle；`sync-schema.sh` 同步 Schema 派生副本；`check-versions.sh` 版本一致性门禁；（另有 `str-gui/scripts/` 下的 vendor 脚本，见 `str-gui/` 一行） |
| `str-cli/` | Rust 参考实现（`str` 二进制；crates.io 包名 `str-format`） |
| `str-cli/schema/*.json` | 三份 Schema 的 **crate 内派生副本**（crates.io 只打包 crate 目录内的文件，故为发版必需）；由 `sync-schema.sh` 生成、`tests/schema_sync.rs` 守卫与真源逐字节一致 |
| `str-gui/` | bundle 的桌面编辑器（Rust + Slint）：列表 / 思维导图双视图、拖拽重排与外部文件拖入、内置校验——功能明细、安装与使用见 [`str-gui/README.md`](str-gui/README.md) |
| `str-gui/patches/*.patch` | 上游缺失能力的四份补丁（winit 拖拽事件 / Slint 拖入链路 / AppKit 拖放修正），由 vendor 脚本施加到 sha256 钉死的官方源码——详见 [`str-gui/README.md`](str-gui/README.md) |
| `str-skill/` | Agent 技能包（含 CLI 安装器与下载校验） |

> 自举（dogfooding）：本仓库根目录自身就是一个 `.str` bundle，
> `str validate .` 恒为 0 errors / 0 warnings——格式在自己的仓库上先行验证。

---

## 版本控制：四条轴 + 一个发行锚

仓库里的版本号不是一个号，而是**四条独立演进的轴** —— 它们的变更频率与破坏面都不同，
因此**不要求相等**；对应关系登记在 [`VERSIONS.toml`](VERSIONS.toml)，不靠「约定相等」：

| 轴 | 当前 | 定义什么契约 | 怎么升 |
| --- | --- | --- | --- |
| 规范 `spec` | `1.13.0` | 磁盘上的数据契约 | 措辞 / 示例 → patch；新增可选字段或枚举值 → minor；收紧校验或语义变更 → major（须配套 `str migrate`，且 `str` 主版本 +1） |
| 实现 `cli` | `0.7.2` | 代码 / 命令契约（crate `str-format`） | 修复 → patch；新命令 / 新 flag → minor；命令面不兼容 → major（含「支持新的 spec major」） |
| 技能包 `skill` | `0.4.2` | Agent 行为契约（MUST / NEVER） | 文案 / 示例 → patch；新增 references 或流程 → minor；Hard rules 变更 → major |
| 界面 `gui` | `0.4.0` | 图形界面契约（crate `str-gui`） | 修复 / 文案 → patch；新功能 / 新视图 → minor；交互或写入行为不兼容 → major |
| **发行 tag** | `v0.7.2` | 把上面四者的某个组合**冻结命名** | **= `v` + cli 版本**（锚定规则） |

- **唯一需要工具显式支持的只有 `str` 主版本号**（当前 `1`）；`spec` 供人类追溯 —— 见规范 §13。
- **门禁**：`bash scripts/check-versions.sh` 逐点比对「真源 ↔ 各声明点」（正文头部、`._meta`、
  Rust 常量、Schema description、示例生成脚本、README、技能包 frontmatter 与发布身份、`ensure-str.sh`、
  workflow 默认 tag），CI 在三个发布工作流中强制执行；tag 推送时还会校验
  「推送的 tag = 真源声明的 tag」，杜绝「tag 打了、真源没改」。
- **发布载体**：推 `v*` tag 同时触发三个互相独立的工作流 —— `release.yml`（GitHub Release：五平台
  CLI 二进制 + Windows/Linux GUI 编辑器 + 技能包 zip + SHA256SUMS）、`publish.yml`（crates.io）、
  `publish-skillhub.yml`（SkillHub 技能市场，`str-skill/` 无变化时自动跳过）；任一条失败都不阻塞另外两条。
  SkillHub 侧需先在 Actions 里配置仓库级 Secret `SKILLHUB_KEY`（个人 API Token，见
  [`str-skill/README.md`](str-skill/README.md) 的「发布到 SkillHub」）。
- **逐版变更**：[`CHANGELOG.md`](CHANGELOG.md)（仓库 / 发行）、[`SPEC-CHANGELOG.md`](SPEC-CHANGELOG.md)
  （规范）、[`str-cli/CHANGELOG.md`](str-cli/CHANGELOG.md)、[`str-skill/CHANGELOG.md`](str-skill/CHANGELOG.md)。
- 别把 `._meta` 的 `revision`（bundle 内的内容修订计数，与 git 无关）当成发布版本。

---

## 路线图

- [x] v1 格式规范 + Rust 参考实现 + 错误码全覆盖测试矩阵 + 自举
- [x] Agent 技能包（`str-skill`）
- [ ] 规范评审 → `APPROVED` / `IMPLEMENTED`
- [ ] 领域词汇表：`work.*` / `art.*` / `video.*` / `lit.*` / `code.*` 逐领域定稿
- [x] GUI 编辑器（列表 + 思维导图双视图，读 ROOT 一层即可渲染）
- [ ] 公开生态：bundle 模板市场、跨 bundle 引用、发布与校验流水线

---

## 贡献指南

欢迎 issue 与 PR。提交前请了解仓库的几条硬约束：

- **版本轴纪律**：改规范升 `spec`、改实现升 `cli`、改技能升 `skill`、改界面升 `gui`——
  各声明点由 `bash scripts/check-versions.sh` 逐点校验，CI 强制执行；规范变更还需
  `bash scripts/sync-schema.sh` 同步 Schema 派生副本；
- **提交信息**遵循 Conventional Commits（`type(scope): 描述`，scope 用组件名：
  `spec` / `cli` / `skill` / `gui` / `release`）；
- **提交前自检**：`str validate . --strict` 与 `str fmt . --check` 必须双零
  （本仓库自身即一个 bundle，格式在自己的仓库上先行验证）；
- **不发明字段**：`._meta` 键集封闭，语义不明先开 issue 讨论，扩展只走 `[ext]` 且
  须 `vendor.*` 命名空间。

---

## 许可

仓库默认双许可：**`MIT OR Apache-2.0`**（[SPDX 表达式](https://spdx.dev/licenses/)），
**唯一例外是 `str-gui/`**，采用 **`AGPL-3.0-only`**（见 [`str-gui/LICENSE`](str-gui/LICENSE)）。

| 范围 | 许可证 | 说明 |
| --- | --- | --- |
| 格式规范 / Schema / 示例 bundle | `MIT OR Apache-2.0` | 任何实现均可无障碍采用 STR 格式 |
| `str-cli/`（crate `str-format`） | `MIT OR Apache-2.0` | [`LICENSE-MIT`](LICENSE-MIT) / [`LICENSE-APACHE`](LICENSE-APACHE) 取其一即满足全部义务；crates.io 元数据同步 |
| `str-skill/` | `MIT OR Apache-2.0` | Agent 技能包 |
| `str-gui/` | `AGPL-3.0-only` | 桌面编辑器（参考实现）；闭源分叉 / 改皮分发不被允许；其内 vendored 的第三方依赖源码保留各自上游许可证 |
