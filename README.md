# STR — AI 时代的结构化资源标准

[![crates.io](https://img.shields.io/crates/v/str-format.svg)](https://crates.io/crates/str-format)
[![docs.rs](https://img.shields.io/docsrs/str-format.svg)](https://docs.rs/str-format)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**STR（Structured Tree Resource，结构化树资源）** 是一种开放的文件格式与工具链：
用**目录树**存储一切结构化资源，用**思维导图语义**组织它们，让**人类与 AI 代理**
在同一份资产上安全地读写、协作与版本控制。

- 格式：`.str` 目录 bundle（形态对标 macOS `.app`）—— 纯目录 + 纯文本，零平台依赖
- 规范版本：v1.10.0（`str` 主版本号 = `1`）
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

每一个领域 today 都在用**不同的、互不相通的容器**：数据库、专有文档格式、
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

## 设计原则

1. **SSOT**：`entries[]` 是内容清单唯一真源；本仓库规范文档是格式的唯一真源；
2. **禁止持久化索引**：可重建缓存只进 `._cache/`（可删、gitignore）；
3. **不得静默容错**：结构损坏必须报错码 + 可定位路径；
4. **跨平台**：不依赖符号链接 / 扩展属性 / 文件系统大小写；
5. **OS 噪声豁免**：`._*` AppleDouble、`.DS_Store`、`.git/` 等一律忽略不误报；
6. **bundle 硬边界**：`.str` 目录即独立 bundle，可安全嵌套；
7. **前向兼容**：未知字段（`[ext]` 命名空间）必须原样保留。

完整规则（目录结构、字段表、35 个错误码、协作协议、AI 读写协议、ADR）见
**[`STR-FORMAT-PROMPT.md`](STR-FORMAT-PROMPT.md) —— 格式规范的唯一真源**。

---

## 仓库结构

| 位置 | 说明 |
| --- | --- |
| `STR-FORMAT-PROMPT.md` | 格式规范（唯一真源，v1.10.0） |
| `VERSIONS.toml` | **版本唯一真源**：规范 / CLI / 技能包三条轴 + 发行 tag 锚 |
| `CHANGELOG.md` | 仓库与发行的变更日志（每个 tag 冻结的「规范 · CLI · skill」三元组） |
| `SPEC-CHANGELOG.md` | 格式规范的变更日志（发行 / 兼容视角；细则真源仍是正文《修订记录》） |
| `._schema/*.json` | 三档 JSON Schema（2020-12），格式规范性产物（Schema 的唯一真源） |
| `examples/客户运营.str/` | 官方示例 bundle（`str validate --strict` 零错误） |
| `scripts/*.sh` | `build-example.sh` 幂等重建示例 bundle；`sync-schema.sh` 同步 Schema 派生副本；`check-versions.sh` 版本一致性门禁 |
| `str-cli/` | Rust 参考实现（`str` 二进制；crates.io 包名 `str-format`） |
| `str-cli/schema/*.json` | 三份 Schema 的 **crate 内派生副本**（crates.io 只打包 crate 目录内的文件，故为发版必需）；由 `sync-schema.sh` 生成、`tests/schema_sync.rs` 守卫与真源逐字节一致 |
| `str-skill/` | Agent 技能包（含 CLI 安装器与下载校验） |

> 自举（dogfooding）：本仓库根目录自身就是一个 `.str` bundle，
> `str validate .` 恒为 0 errors / 0 warnings——格式在自己的仓库上先行验证。

---

## 版本控制：三条轴 + 一个发行锚

仓库里的版本号不是一个号，而是**三条独立演进的轴** —— 它们的变更频率与破坏面都不同，
因此**不要求相等**；对应关系登记在 [`VERSIONS.toml`](VERSIONS.toml)，不靠「约定相等」：

| 轴 | 当前 | 定义什么契约 | 怎么升 |
| --- | --- | --- | --- |
| 规范 `spec` | `1.10.0` | 磁盘上的数据契约 | 措辞 / 示例 → patch；新增可选字段或枚举值 → minor；收紧校验或语义变更 → major（须配套 `str migrate`，且 `str` 主版本 +1） |
| 实现 `cli` | `0.4.0` | 代码 / 命令契约（crate `str-format`） | 修复 → patch；新命令 / 新 flag → minor；命令面不兼容 → major（含「支持新的 spec major」） |
| 技能包 `skill` | `0.3.1` | Agent 行为契约（MUST / NEVER） | 文案 / 示例 → patch；新增 references 或流程 → minor；Hard rules 变更 → major |
| **发行 tag** | `v0.4.0` | 把上面三者的某个组合**冻结命名** | **= `v` + cli 版本**（锚定规则） |

- **唯一需要工具显式支持的只有 `str` 主版本号**（当前 `1`）；`spec` 供人类追溯 —— 见规范 §13。
- **门禁**：`bash scripts/check-versions.sh` 逐点比对「真源 ↔ 各声明点」（正文头部、`._meta`、
  Rust 常量、Schema description、示例生成脚本、README、技能包 frontmatter 与发布身份、`ensure-str.sh`、
  workflow 默认 tag），CI 在三个发布工作流中强制执行；tag 推送时还会校验
  「推送的 tag = 真源声明的 tag」，杜绝「tag 打了、真源没改」。
- **发布载体**：推 `v*` tag 同时触发三个互相独立的工作流 —— `release.yml`（GitHub Release：五平台
  二进制 + 技能包 zip + SHA256SUMS）、`publish.yml`（crates.io）、`publish-skillhub.yml`
  （SkillHub 技能市场，`str-skill/` 无变化时自动跳过）；任一条失败都不阻塞另外两条。
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
- [ ] GUI 编辑器（思维导图视图，读 ROOT 一层即可渲染）
- [ ] 公开生态：bundle 模板市场、跨 bundle 引用、发布与校验流水线

---

## 许可

见 [LICENSE](LICENSE)。
