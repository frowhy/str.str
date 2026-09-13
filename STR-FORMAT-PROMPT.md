# STR 结构化树资源格式 · 规范与开发提示词

| 项目 | 内容 |
| --- | --- |
| 格式名称 | STR（Structured Tree Resource，结构化树资源） |
| 扩展名 | `.str`（目录 bundle，形态对标 macOS `.app`） |
| 规范版本 | **v1.7.0**（`str` 主版本号 = `1`） |
| 文档状态 | `DRAFT → 待评审`（评审通过后转 `APPROVED`，实现完成转 `IMPLEMENTED`） |
| 文档日期 | 2026-09-14 |
| 文档定位 | **本文件即提示词（Prompt）**，整份可直接投喂给 AI 开发代理；第 1 章为指令主体，第 2~13 章为规范性附录（即指令的「事实来源」） |
| 目标读者 | AI 开发代理（主）、格式实现者、编辑软件开发者 |
| 事实来源（SSOT） | 本文件第 3~9 章为格式的唯一真源；实现代码不得偏离，如需偏离必须先修订本文件 |

### 修订记录

| 版本 | 日期 | 变更摘要 |
| --- | --- | --- |
| v1.0.0 | 2026-09-14 | 初版 |
| **v1.1.0** | 2026-09-14 | **撤回「深度 ≥2 禁止承载 payload」**：任意分支（任意层级）均可存放任意文件与文件夹，只要在 `._meta.entries[]` 中登记。层级差异仅体现在**分支身份**上（`node` 独立节点 / `branch` 关联分支），不再限制数据存放位置。相应地：`kind` 枚举改为 `root｜node｜branch`；原「link 目录 = 纯引用」机制改为可选的 **`refs[]` 跨枝关联声明**；删除 `E_DEPTH_OWNED`、`E_LINK_*` 等 5 个错误码，新增 `E_KIND_DEPTH`、`E_REF_*` 等。 |
| **v1.2.0** | 2026-09-14 | **取消「素材目录」概念**：**每个节点/分支目录本身就是素材目录**，图片、PDF、附件等直接放在分支目录内即可。子目录一律降级为**纯内容容器**（`role: dir`），不再有任何特殊语义；删除 `E_STRAY_META`，「子目录内含 `._meta`」一律按「该目录是关联分支」处理（role 未同步则报 `E_ENTRY_ROLE_DEPTH`）。 |
| **v1.3.0** | 2026-09-14 | **统一 `._` 保留前缀**：`.meta` → **`._meta`**；`_schema` / `_audit` / `_cache` → **`._schema` / `._audit` / `._cache`**。格式保留名一律以 `._` 开头，业务条目不得以 `._` 开头。连带：新增**操作系统噪声豁免**（`._*` 形式的 AppleDouble 伴生文件与 `.DS_Store` 一律忽略，不报 `E_RESERVED_NAME`、不参与清单比对）；`.gitignore` 模板补 `**/._*`。 |
| **v1.4.0** | 2026-09-14 | **载体由 JSON 改为 TOML v1.0.0**（`._meta` 现为 TOML；允许 `#` 注释且工具必须保注释；时间改用 TOML 原生 offset date-time；清单改为数组表 `[[entries]]` / `[[refs]]` / `[[authors]]`，`[policies]` / `[ext]` 为表；新增「TOML → 规范 JSON 归一化 → JSON Schema 校验」链路，ADR-1 重写）。**决策收敛**：UUID v7 固定（`id_version = 7`）、保留 `refs` 跨枝关联、`sha256` 改为**强制**（`policies.sha256 = "required"`，新增 `E_MANIFEST_DIGEST_MISSING`）。顺带清理：`[[entries]]` 不再重复声明 `kind`（与 `role` 冗余），由 `role` 唯一表达；CLI 新增 `str fmt` / `str norm`，`str export` 支持 `--format json\|toml`。 |
| **v1.5.0** | 2026-09-14 | **移除 `._audit/` 与 `journal` role**：审计能力交由 Git / 协作平台提供，格式内不设审计目录（删除 7.3 节、7.1 表相关行、`role: journal`、示例与 .gitignore 相关项）。**固定深度分界**：深度 1 = `node`、深度 ≥2 = `branch` 为硬规则，删除 `policies.branch_min_depth` 字段（2 是唯一自洽值，暴露可配置开关只会误导）。附录 A 全部决策关闭，转为决策索引。 |
| **v1.6.0** | 2026-09-14 | **补齐实现期发现的三处澄清**（格式语义无变化）：① **`entries` 与 `refs` 同因 TOML 无法表达空数组表而允许整表省略**（归一化补 `[]`），schema 中不再必填；② `E_RESERVED_NAME` 判定精确化 —— 业务**目录**以 `._` 开头必报、**已登记**的 `._*` 条目必报、未登记的 `._*` **普通文件**豁免；③ `str sync` 去掉 `--recursive`（始终递归）。另补充**参考实现（Rust）与仓库结构**、错误码覆盖率验收项，示例 §10 全部替换为**真实 `size`/`sha256`**（由 `scripts/build-example.sh` 生成并逐字对齐）。 |
| **v1.7.0** | 2026-09-14 | **自举驱动修订**：以本仓库自身作为 `.str` bundle 跑 `str validate .`，暴露并修正 4 处规范缺陷。① 新增 **3.5「`.str` 目录是 bundle 硬边界」** + `role = "bundle"`（父 bundle 不进入子 bundle，示例 / 测试夹具 bundle 可安全内嵌）；② **操作系统 / 工具元数据豁免**扩展至版本控制元数据（`.git/`、`.gitignore`、`.gitattributes`、`.gitmodules`、`.hg/`、`.svn/`）；③ `W_ROOT_STRAY` 语义修正为「既非 UUID 命名的分支目录、**又未登记进 `entries`** 的散落条目」（消除与 3.3「任意目录可放任意文件」的矛盾）；④ **schema / policy 冲突修正**：JSON Schema 不再无条件要求 `size` / `sha256`，该强制改由校验器按 `policies.sha256` 判定（Schema 读不到 `[policies]`）。`role: cache` 措辞放宽为「派生缓存 / 构建产物」。 |

---

## 目录

- [0. 使用说明](#0-使用说明)
- [1. 主提示词（可直接投喂 AI）](#1-主提示词可直接投喂-ai)
- [2. 设计目标与非目标](#2-设计目标与非目标)
- [3. 目录结构规范](#3-目录结构规范)
- [4. `._meta` 数据规范](#4-meta-数据规范)
- [5. 分支语义：思维导图模型](#5-分支语义思维导图模型)
- [6. 校验规则与错误码](#6-校验规则与错误码)
- [7. 多人协作协议](#7-多人协作协议)
- [8. AI 读取与写入协议](#8-ai-读取与写入协议)
- [9. 工具链 CLI 规格](#9-工具链-cli-规格)
- [10. 完整示例 bundle](#10-完整示例-bundle)
- [11. 设计决策记录（ADR）](#11-设计决策记录adr)
- [12. 验收标准（DoD）](#12-验收标准dod)
- [13. 版本演进策略](#13-版本演进策略)

---

## 0. 使用说明

- **投喂方式**：将本文件整体作为上下文提供给 AI 开发代理；若上下文受限，至少投喂第 1、3、4、5、6、10 章。
- **指令优先级**：第 1 章「主提示词」为执行指令；第 3~9 章为规范约束。**指令与规范冲突时以规范为准**。
- **禁止臆造**：规范未定义的行为，实现者必须提问，不得自行发明字段名或语义（新增字段一律走 `ext` 命名空间）。

---

## 1. 主提示词（可直接投喂 AI）

> 以下为指令正文。

### 1.1 角色

你是 **STR 格式的首席实现者**。你精通文件格式设计、结构化数据处理、跨平台目录语义、JSON Schema、以及 AI 上下文工程。你的产出必须可直接运行、可校验、可被第三方工具解析。

### 1.2 任务

依据本文件第 3~9 章的规范，实现 `.str` 格式的完整工具链与校验器，并交付：

| 编号 | 交付物 | 说明 |
| --- | --- | --- |
| D1 | 规范文档 | 本文件即规范，实现过程中若发现规范缺陷，**先改文档再改代码** |
| D2 | 校验 Schema ×3 | `root-meta.schema.json` / `node-meta.schema.json` / `branch-meta.schema.json`（**JSON Schema 2020-12**，置于 `._schema/` 下）；用于校验**归一化后的 JSON**（校验链路见 4.1），**不**直接校验 TOML 原文 |
| D3 | 示例 bundle | 至少 1 个可直接用校验器跑通的 `.str` 目录（含独立节点、多层关联分支、跨枝关联声明、强制 `sha256`） |
| D4 | 校验器 `str validate` | 覆盖第 6 章全部错误码，输出人类可读 + `--json` 两种格式 |
| D5 | 读写库 | TOML 解析与**保注释写回**、**TOML → 规范 JSON 归一化**、遍历、增删任意层级分支、内容清单同步（`sync`） |
| D6 | CLI | 实现第 9 章全部命令 |
| D7 | 测试 | 单测 + 对 D3 的端到端校验 + **故意破坏用例**（每个 error 码至少 1 例） |
| D8 | 平台适配 | macOS 可选 Bundle 位；Windows/Linux 保持普通目录（不得依赖平台特性才能工作） |

**参考实现（本仓库，已完成）**：Rust 2024；依赖 `toml_edit`（保注释写回）、`jsonschema`（2020-12）、`clap`、`sha2`、`uuid`（v7）、`time`、`walkdir`。

**仓库分层原则**：**格式规范资产放仓库根**（Schema / 示例 / 生成脚本，可被任何实现复用）；**`str-cli/` 只放 CLI 实现**（一个可 `cargo` 构建的最小 crate）。此外，**仓库自身即一个 `.str` bundle（自举 / dogfooding）**：根目录名为 `str.str`，其 `._meta` 记录本仓库的分支与内容，可直接用 `str validate .` 自检 —— 这也是格式在**真实项目**上的第一个用例。

| 位置 | 说明 |
| --- | --- |
| `STR-FORMAT-PROMPT.md` | **本规范（唯一真源）** |
| `schema/*.json` | 三份档位 JSON Schema（2020-12）—— 格式的规范性产物 |
| `examples/客户运营.str/` | 与 §10 逐字一致的示例 bundle（真实 `sha256`） |
| `scripts/build-example.sh` | 幂等重建示例 bundle |
| `str-cli/Cargo.toml` / `str-cli/Cargo.lock` | Rust 工程（`cargo` 在 `str-cli/` 下执行） |
| `str-cli/src/{meta,meta_edit}.rs` | `._meta` 模型、提取、归一化、保注释写回、模板渲染 |
| `str-cli/src/bundle.rs` | 分支树遍历、`id` 索引、懒加载 |
| `str-cli/src/validate.rs` | 规范第 6 章全部错误码 |
| `str-cli/src/cmd.rs` / `str-cli/src/main.rs` | CLI 子命令 / clap 定义与退出码 |
| `str-cli/tests/validate_codes.rs` | 错误码测试矩阵（每个 `E_*` / `W_*` ≥1 例） |
| `str-cli/target/` | 构建产物（不入库） |

> ⚠ **分层代价**：Schema 属规范资产（只有一份，不复制进实现），因此 `str-cli/src/lib.rs` 以 `include_str!("../../._schema/…")` 引用它 —— **`str-cli/` 不能脱离仓库根单独构建**。`str init` 会把这三份 Schema 复制进新建 bundle 的 `._schema/`。

### 1.3 硬性约束（违反即失败）

1. **分支可无限嵌套，且任意层级都能存数据**：任何带 `._meta` 的分支目录（深度 1 的独立节点、深度 ≥2 的关联分支）**均可存放任意文件与文件夹**，唯一义务是把它们登记进 `._meta.entries[]`。**不得**以「层级过深」为由拒绝承载数据。
2. **层级差异只体现在身份上**：深度 1 = `node`（独立节点，登记在 ROOT 的 `entries[]`，全局唯一可寻址）；深度 ≥2 = `branch`（关联分支，只登记在其父分支的 `entries[]`）。深度 ≥2 的分支**不得**被登记为 `node`。
3. **UUID 命名不可变**：分支目录名一旦生成，任何工具都不得重命名。
4. **派生数据不落盘**：不允许生成持久化索引文件（如 `._index.toml`）。可重建的缓存只能放 `._cache/` 且必须可安全删除。
5. **元数据必须登记内容**：分支目录内除 `._meta` 外的每个条目都必须在 `._meta.entries[]` 中登记（`manifest` 策略见 4.8）。
6. **序列化确定性**：TOML v1.0.0、UTF-8 无 BOM、LF 换行、键序/表序固定（4.9）、数组表按 `order` 稳定排序、文件末尾保留单个换行、时间使用原生 offset date-time —— 目的是让 Git diff 只反映真实变更；工具写回时**必须保留注释**（comment-preserving）。
7. **不得静默容错**：结构损坏必须报错并给出可定位路径，不允许「猜测后继续」。
8. **跨平台**：不得依赖符号链接、扩展属性、文件系统大小写特性才能正确工作。
9. **`._` 前缀专属格式内部**：业务条目**不得**以 `._` 开头；同时 `._*` 形式的**普通文件**（macOS AppleDouble 伴生文件）与 `.DS_Store` 属**操作系统噪声**，必须豁免 —— 不报错、不计入清单、不参与统计。

### 1.4 实施顺序

`D1 → D2 → D3 → D4 → D5 → D6 → D7 → D8`。每个阶段完成后运行校验器自检并输出结论，再进行下一阶段。

### 1.5 交付纪律

- 每完成一个阶段：给出**变更清单表格**（文件 / 变更点 / 影响面），并执行一次提交。
- 任何结构性改动前，先说明影响范围（哪些章节、哪些命令、哪些示例受影响）。
- 任务结束时更新长期记忆并提交。

---

## 2. 设计目标与非目标

### 2.1 目标

| 目标 | 落地手段 |
| --- | --- |
| 结构化存储数据与实体 | 每个分支目录 = 一个节点，自带 `._meta`（元信息 + 内容清单），可在任意层级承载任意文件 |
| 思维导图式分支 | ROOT 记录一级「独立节点」；独立节点下可无限嵌套「关联分支」，形成任意深度的分支树 |
| 天然适配 AI 读取 | 每层 `._meta` 自带 `type/title/summary/tags`；分层可下钻，无需全量加载；提供 `str context` 上下文裁剪 |
| 支持多人协作编辑 | 一分支一目录一 `._meta`（最小冲突域）；确定性序列化 + 保注释写回；`revision` 单调递增；可选 `.lock` 短锁 |
| 可 diff / 可 merge | TOML（行导向）+ 固定键序/表序 + 稳定排序 + 保留注释 |
| 跨枝关联不复制数据 | 可选的 `refs[]` 声明式关联（画导图连线），数据始终只在原分支内存在 |
| 跨平台可移植 | 纯目录 + 纯文本，零平台特性依赖 |
| 可校验 / 可演进 | TOML → 规范 JSON 归一化 + JSON Schema + 结构化错误码 + `str` 主版本 + `ext` 扩展命名空间 |

### 2.2 非目标

- ❌ 不做数据库（无事务、无锁语义保证，冲突交由 Git / 存储层）。
- ❌ 不做二进制打包容器（不使用 zip/sqlite 封装；保持目录可见）。
- ❌ 不替代通用文档格式（Markdown/Office 文件作为 payload 原样存放，不做转换）。
- ❌ 不实现权限系统（权限由宿主平台/协作平台负责）。

---

## 3. 目录结构规范

### 3.1 形态

`.str` 是**目录**（bundle），其**目录名以 `.str` 结尾**，例如 `客户运营.str/`。在 macOS 上可通过 Bundle 位让 Finder 将其视为单一文件；这仅是外观增强，**不得成为格式的必要条件**。

### 3.2 层级模型

```
<名称>.str/
├── ._meta                                  # ① ROOT 元数据：分支结构主干
├── ._schema/                               # 保留名：本 bundle 的 JSON Schema
├── ._cache/                                # 保留名：可选派生缓存（可删，建议 gitignore）
├── <UUID-v7>/                             # ② 独立节点（深度 1，kind = node）
│   ├── ._meta
│   ├── <任意 payload 文件 / 目录>          #    由 ._meta.entries[] 完整登记
│   ├── <任意子目录>/                        #    role = dir，纯内容容器；内含 ._meta 即成关联分支
│   └── <UUID-v7>/                         # ③ 关联分支（深度 2，kind = branch）
│       ├── ._meta
│       ├── <任意文件 / 目录>                #    同样完全合法，同样须登记
│       └── <UUID-v7>/                     # ④ 更深关联分支（深度 3+，仍为 branch）
│           ├── ._meta
│           └── <任意文件 / 目录>
└── <UUID-v7>/                             # 更多独立节点
```

> **核心规则**：**目录层级只决定「身份」，不决定「能否存数据」**。任何带 `._meta` 的分支目录都是完整的数据节点，可以存放任意内容并继续向下嵌套。

### 3.3 深度语义表

| 深度 | 称谓 | 必须的 `kind` | 可承载任意文件/文件夹 | 登记位置 | 身份说明 |
| --- | --- | --- | --- | --- | --- |
| 0 | ROOT | `root` | 允许（须登记） | —— | bundle 容器；`entries[role=node]` 即「子分支结构」 |
| 1 | 独立节点（子分支） | `node` | **允许** | ROOT 的 `entries[]` | 一等实体；全局唯一可寻址；可被任意分支通过 `refs[]` 关联 |
| ≥2 | 关联分支（孙分支及更深） | `branch` | **允许** | 其**父分支**的 `entries[]` | 依附父分支存在；可无限嵌套形成思维导图；不得升级为一等实体 |

> 「关联分支」的「关联」有两层含义：① **纵向**——它与其父分支构成父子关联（依附关系）；② **横向**——它可通过 `._meta.refs[]` 与其他分支建立跨枝关联线。

### 3.4 命名规则

| 对象 | 规则 | 违规码 |
| --- | --- | --- |
| bundle 根目录 | `<名称>.str`（扩展名小写） | `W_BUNDLE_SUFFIX` |
| 分支目录（`node` / `branch`） | **UUID v7** 的规范小写连字符格式：`xxxxxxxx-xxxx-7xxx-[89ab]xxx-xxxxxxxxxxxx` | `E_ID_NOT_UUID` / `E_ID_VERSION` |
| 元数据文件 | 固定名 **`._meta`**，目录内唯一，必须是普通文件 | `E_META_MISSING` |
| 保留名 | 以 **`._`** 开头（`._meta` / `._schema` / `._cache` / 未来扩展）；业务条目**不得**以 `._` 开头 | `E_RESERVED_NAME` |
| 锁文件 | `.lock`（可选，短生命周期，不得提交） | — |
| 其它点文件 | 仅 `._meta` / `.lock` 合法，其余告警 | `W_DOTFILE` |
| 操作系统 / 工具元数据 | `._*` 形式的**普通文件**（macOS AppleDouble 伴生文件）、`.DS_Store`、`Thumbs.db`、`desktop.ini`，以及**版本控制元数据**（`.git/`、`.gitignore`、`.gitattributes`、`.gitmodules`、`.hg/`、`.hgignore`、`.svn/`）**一律忽略**：不视为保留名、不参与清单比对、不报任何错 | —（豁免） |

### 3.5 `.str` 目录是 bundle 硬边界

**以 `.str` 结尾的目录一律视为独立的子 bundle**（语义同 `.app` 嵌套）：

- 父 bundle 的扫描 / 校验 **不进入** 其内部：既不把它当作分支（即使其中含 `._meta`），也不检查其内部清单；
- 它是**完整的 bundle 边界** —— 内部自成一个 `kind = root` 的 `._meta` 体系，`id` 与目录名无关；
- 父级 `entries` 中应表达为 **`role = "bundle"`**（要求目录名以 `.str` 结尾）；登记为 `role = "dir"` 会报 `E_ENTRY_ROLE_DEPTH`。

设计动机：真实项目（尤其是格式自身的仓库）必然需要在内部存放**示例 bundle / 测试夹具 bundle**；若不设边界，父 bundle 会把子 bundle 的 `._meta` 误判为「非分支目录内出现元数据」而报错。

---

## 4. `._meta` 数据规范

### 4.1 载体与编码

| 项 | 规定 |
| --- | --- |
| 格式 | **TOML v1.0.0**（严格遵循官方规范，禁用任何方言/扩展语法） |
| 编码 | UTF-8，**无 BOM** |
| 换行 | `LF`（`\n`） |
| 缩进 | 顶层裸键不缩进；表/数组表内的键值对缩进 2 空格；跨行数组每项缩进 2 空格 |
| 末尾 | 保留 1 个换行 |
| 文件名 | `._meta`（无扩展名） |
| 注释 | **允许** `#` 注释；注释**不得承载语义**（一切语义必须落在字段里）；工具写回时**必须保留注释**（comment-preserving） |
| 时间 | 一律使用 TOML **原生 offset date-time**（必须带时区偏移，如 `2026-09-14T10:03:11+08:00`），**不得**写成字符串 |
| 空数组表 | TOML 无法表达空的 `[[x]]`；`refs` / `entries` 为空时**整表省略**，归一化时补为 `[]`（故二者在 schema 中不是必填表） |
| 大小上限 | 建议 ≤ 256 KiB；超过说明该分支内容过于庞杂，宜拆分下级分支 |

> JSON 与 YAML 均已评估并否决，理由见 11 章 **ADR-1**。
>
> **校验链路**：`._meta`（TOML）→ 解析 → **归一化为规范 JSON**（键序/表序固定，见 4.9）→ JSON Schema 校验。归一化器属 D5 读写库的一部分，也是 `str export` 的基础。

### 4.2 统一信封（三种 `kind` 共用同一 Schema 家族）

`._meta` 只有一个顶层结构，通过 `kind` 区分三种**档位（profile）**：

| `kind` | 出现位置 | 作用 |
| --- | --- | --- |
| `root` | bundle 根 `._meta` | 记录一级分支结构（`entries[role=node]`）+ ROOT 级其它内容 + 策略 |
| `node` | 深度 1 目录 | 独立节点：实体元信息 + 内容清单 + 可选跨枝关联 |
| `branch` | 深度 ≥2 目录 | 关联分支：本层元信息 + 内容清单 + 可选跨枝关联 |

> `node` 与 `branch` 的**字段集合完全相同**，仅 `kind` 与登记位置不同 —— 这样工具可以用同一套读写逻辑处理任意深度，也让「某层级能否存数据」这类问题从根上消失。

### 4.3 顶层字段表

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `str` | integer | ✅ | 格式主版本，本规范固定为 `1` |
| `spec` | string | ✅ | 规范语义化版本，如 `"1.5.0"` |
| `kind` | `"root"｜"node"｜"branch"` | ✅ | 档位，必须与所在深度匹配（见 3.3） |
| `id` | uuid | ✅ | 自身标识。**`node`/`branch` 必须与所在目录名完全一致**；`root` 由工具生成，与目录名无关 |
| `name` | string | root 必填 | bundle 短名（人类可读，不含 `.str`） |
| `type` | string | 建议 | 节点类型，点分命名空间（`crm.customer`、`crm.followup_log`、`doc.spec`）；AI 依此路由 |
| `title` | string | 建议 | 单行标题 |
| `summary` | string | 强烈建议 | **一句话摘要（≤200 字）**，AI 检索的主要依据 |
| `tags` | array of strings | 否 | 扁平标签，用于筛选；建议全小写、无空格（可用 `-`/`_`） |
| `revision` | integer ≥ 1 | ✅ | 修订号，单调递增，每次写入 +1 |
| `created_at` | offset date-time | ✅ | 创建时间，必须带时区偏移（TOML 原生类型） |
| `updated_at` | offset date-time | ✅ | 最后更新时间；变化必须伴随 `revision` 前进 |
| `authors` | array of tables `[[authors]]` | 建议 | 贡献者与角色，见 4.4 |
| `schema` | string | 否 | 该分支 payload 的 JSON Schema 引用（bundle 内相对路径，如 `._schema/customer.schema.json`） |
| `policies` | table `[policies]` | 仅 root | 校验策略，见 4.7 |
| `refs` | array of tables `[[refs]]` | 否 | **跨枝关联声明**（思维导图的「关联线」），见 4.5；不复制数据、不改变目录结构 |
| `entries` | array of tables `[[entries]]` | ✅（空时可整表省略） | **本目录内容清单（唯一真源）**，见 4.6；为空时省略，归一化补 `[]` |
| `ext` | table `[ext]` | 否 | 扩展命名空间，键必须为 `vendor.xxx` 形式 |

### 4.4 `authors[]` 元素

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `id` | string | ✅ | **稳定标识符**（SSO sub / 邮箱 hash / 账号 uid），禁止用显示名当 id |
| `name` | string | 否 | 展示名 |
| `role` | `"owner"｜"editor"｜"viewer"｜"agent"` | ✅ | `agent` 表示 AI 代理 |
| `at` | offset date-time | 否 | 参与时间（TOML 原生类型，须带时区偏移） |

### 4.5 `refs[]`：跨枝关联（可选）

用于表达思维导图中「跨越分支树结构的关联线」（如 XMind 的「关联」功能）。它是**声明式**的：只描述关系，不移动、不复制任何数据。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `id` | uuid | ✅ | 关联线自身标识 |
| `target` | uuid | ✅ | 目标分支的 `id`（**任意深度均可**，含 ROOT 除外） |
| `rel` | enum | ✅ | 关联语义，见 5.3 |
| `title` | string | 否 | 关联线标签（导图上显示） |
| `order` | integer ≥ 0 | 否 | 同一源内的排序键 |
| `note` | string | 否 | 备注 |

规则：

- `target` 必须在同一 bundle 内可解析，否则 `E_REF_NO_TARGET`。
- 关联图**禁止成环**（沿 `refs` 追踪若回到起点即 `E_REF_CYCLE`）。
- 自关联（`target == id`）非法。
- `refs` 不承担父子关系统表达职责 —— 父子关系由**目录结构 + 父级 `entries[]`** 唯一决定。

### 4.6 `entries[]`：内容清单（核心）

**语义**：`entries[]` 是本目录内除 `._meta` 之外**所有条目**的完整清单，是本目录内容的**唯一权威描述**；文件系统是**存在性权威**。二者由校验器双向比对。

Entry 字段表：

| 字段 | 类型 | 必填 | 适用 `role` | 说明 |
| --- | --- | --- | --- | --- |
| `path` | string | ✅ | 全部 | 相对本目录的**单段路径**（文件名或目录名，不含 `/`）；`entries.path` 不得重复 |
| `role` | enum | ✅ | 全部 | 见下方 role 表 |
| `id` | uuid | ✅ | `node`/`branch` | 子分支自身标识，必须与 `path` 一致 |
| `type` | string | 否 | `node`/`branch` | 子分支类型（便于只读父级 `._meta` 就完成路由） |
| `title` | string | 否 | `node`/`branch` | 展示名 |
| `summary` | string | 否 | `node`/`branch` | 子分支摘要，便于免递归检索 |
| `order` | integer ≥ 0 | 建议 | `node`/`branch` | 同层排序键；缺省按 `path` 字典序 |
| `media_type` | string | 建议 | `payload`/`asset` | IANA 媒体类型 |
| `size` | integer ≥ 0 | ✅ | `payload`/`asset` | 字节数（强制，强度由 `policies.sha256` 控制） |
| `sha256` | string(64 hex) | ✅ | `payload`/`asset` | 内容指纹（强制），协作期变更检测的基础 |
| `count` | integer ≥ 0 | 建议 | `dir` | 目录内直接子项数量（非递归） |
| `schema` | string | 否 | `payload` | 该文件遵循的 Schema 相对路径 |
| `optional` | boolean | 否 | 全部 | 缺失是否允许（默认 `false`） |
| `note` | string | 否 | 全部 | 备注 |

> **强制字段规则**：`size` 与 `sha256` 对**文件类条目**（`role` ∈ `payload` / `asset`）为**强制**（`policies.sha256 = "required"`，本格式默认值）。目录类条目（`dir` / `node` / `branch`）与 `cache` **不适用** —— 目录没有内容指纹，缓存属可变/派生数据。

`role` 枚举：

| `role` | 含义 | 是否计入「分支结构」 |
| --- | --- | --- |
| `node` | 独立节点子目录（深度 1；仅出现在 ROOT 的 entries 中） | ✅ 是（即「子分支」） |
| `branch` | 关联分支子目录（深度 ≥2） | ✅ 是（即「孙分支及更深层级」） |
| `payload` | 承载结构化数据的文件（由本分支「拥有」） | ❌ |
| `asset` | 附属素材文件（图片/PDF/音视频等） | ❌ |
| `dir` | 普通子目录（纯内容容器，不含 `._meta`，因而不是分支） | ❌ |
| `schema` | `._schema/` 目录（bundle 级 Schema 存放处；内部文件不逐个登记） | ❌ |
| `cache` | 派生缓存 / 构建产物（可删；`._cache/` 内的实例因保留名免登记） | ❌ |
| `other` | 其它未分类条目（需 `note` 说明） | ❌ |

> **只有带 `._meta` 的子目录才是分支**（`role` = `node`/`branch`）；其余子目录一律是普通内容容器（`role` = `dir`）。这是「分支」与「目录」的唯一判据。
>
> **不存在「素材目录」这一独立概念**：节点/分支目录**本身就是素材目录** —— 图片、PDF、附件等直接放在分支目录内即可，**无需**为存放素材而额外建目录。子目录只是纯内容容器，不承担任何特殊语义（不因「装素材」而获得角色）。
>
> **ROOT 元数据记录分支结构** = ROOT 的 `entries[]` 中所有 `role = node` 的条目（含 `title`/`type`/`order`/`summary`），编辑软件**无需递归即可渲染第一层导图**。

### 4.7 `policies`（仅 root，可选，工具缺省值见括号）

| 字段 | 类型 | 缺省 | 说明 |
| --- | --- | --- | --- |
| `id_version` | integer | `7` | UUID 版本要求 |
| `max_depth` | integer | `32` | 分支树最大深度 |
| `manifest` | `"strict"｜"advisory"` | `strict` | `strict`：清单不一致为 error；`advisory`：仅 warning（编辑器编辑期可用） |
| `unknown_entry` | `"allow"｜"warn"｜"deny"` | `allow` | 未登记条目的处理方式 |
| `sha256` | `"required"｜"optional"｜"off"` | **`required`** | 是否强制文件类条目（`payload`/`asset`）携带 `size` + `sha256`；`optional` / `off` 仅供编辑器编辑期临时降级，**不得**出现在已提交状态 |
| `large_asset_bytes` | integer | `10485760` | 超过则告警 `W_LARGE_ASSET` |
| `deep_tree_warn` | integer | `16` | 超过则告警 `W_DEEP_TREE`（提示考虑拆分） |

### 4.8 清单一致性策略

| 场景 | `strict` | `advisory` |
| --- | --- | --- |
| 磁盘有、`entries` 无 | `E_MANIFEST_MISSING` | `W_MANIFEST_MISSING` |
| `entries` 有、磁盘无（且非 `optional`） | `E_MANIFEST_GHOST` | `W_MANIFEST_GHOST` |
| `size`/`sha256` 不符 | `E_MANIFEST_HASH` | `W_MANIFEST_HASH` |

协作建议：**CI/提交前用 `strict`，编辑器保存中用 `advisory`**；`str sync` 负责自动补登。

### 4.9 规范键序与表序（确定性序列化）

TOML 规定**裸键必须写在任何表头之前**，因此本格式的书写顺序是固定的、有语义的：

1. **顶层裸键**（按此序）：`str, spec, kind, id, name, type, title, summary, tags, revision, created_at, updated_at, schema`
2. **`[policies]`**（仅 root）
3. **`[[authors]]`** → **`[[refs]]`** → **`[[entries]]`**（数组表；各自按 `order` 稳定排序，无 `order` 者保持既有相对顺序）
4. **`[ext]`**（必须置于文件最后）

各表内的键序：

| 表 | 键序 |
| --- | --- |
| `[policies]` | `id_version, max_depth, manifest, unknown_entry, sha256, large_asset_bytes, deep_tree_warn` |
| `[[authors]]` | `id, name, role, at` |
| `[[refs]]` | `id, target, rel, title, order, note` |
| `[[entries]]` | `path, role, id, type, title, summary, order, media_type, size, sha256, count, schema, optional, note` |

> 未知键（来自 `ext` 或未来版本）**必须原样保留**，不得丢弃（前向兼容）。
>
> 归一化（TOML → 规范 JSON）时，顶层裸键与各表按上表顺序输出；`refs` 缺省时补 `[]`，使外部工具（JSON Schema 校验器、编辑器、AI）得到完全一致的视图。

---

## 5. 分支语义：思维导图模型

### 5.1 模型定义

把 bundle 视为**一棵分支树 + 若干声明式关联线**：

- **分支树**：`ROOT → 独立节点（node） → 关联分支（branch） → 关联分支（branch） → …`
  - 父子关系**唯一**由「目录结构 + 父级 `entries[]`」决定。
  - 每一层都是完整的数据节点，都可以存放任意文件与文件夹。
  - 深度 ≥2 的分支身份固定为 `branch`（关联分支），不得升级为 `node`。
- **关联线（可选）**：任意深度分支的 `._meta.refs[]` 可指向另一分支，用于表达跨枝关系。关联线**不复制数据**，因此同一份内容永远只有一处真源。

### 5.2 语义示例

```
A.str/
├── ._meta                            entries: [A1(node), A2(node), A3(node)]
├── A1/      （独立节点：客户档案）
│   ├── ._meta                        entries: [profile.json, L1(branch), refs→A2]
│   ├── profile.json
│   └── L1/  （关联分支：跟进记录）
│       ├── ._meta                    entries: [followups.json, L2(branch)]
│       ├── followups.json
│       └── L2/  （关联分支：2026-09 会议纪要）
│           ├── ._meta                entries: [2026-09-10.md]
│           └── 2026-09-10.md
├── A2/      （独立节点：订单数据集）
│   └── ._meta                        entries: [orders.csv]
└── A3/      （独立节点：标签体系）
    └── ._meta                        entries: [tags.json]
```

解读：

- `A1` 是一等实体，登记在 ROOT；`A1/L1`、`A1/L1/L2` 是它的下级关联分支，**各自都能存数据**，形成任意深度的思维导图。
- `A1` 通过 `refs[]` 关联到 `A2`（`rel: related`），导图上画出跨越分支树的关联线；`A2` 的数据仍然只存在于 `A2/` 内。

### 5.3 `rel` 枚举（用于 `refs[]`）

| `rel` | 语义 | 导图连线含义 |
| --- | --- | --- |
| `related` | 弱关联（默认） | 相关但无从属 |
| `depends_on` | 依赖 | 源分支依赖目标分支 |
| `instance_of` | 实例化 | 源分支是目标的实例（目标多为模板/类型分支） |
| `derived_from` | 派生自 | 由目标计算/转换而来 |
| `ref` | 纯引用/别名 | 仅用于在导图上「再出现一次」 |
| `x-<自定义>` | 扩展 | 厂商自定义语义，须在 `._schema/` 或本文档登记 |

### 5.4 传统「关联分支」与「跨枝关联」的区别

| 维度 | 分支树中的关联分支（`kind = branch`） | 关联线（`refs[]`） |
| --- | --- | --- |
| 载体 | **真实目录 + `._meta`** | 父分支 `._meta` 中的一个数组项 |
| 能否承载数据 | ✅ 能（且必须登记） | ❌ 不能（纯声明） |
| 参与分支结构 | ✅ 是（父级 `entries[role=branch]`） | ❌ 否 |
| 唯一性 | 目录名全局唯一 UUID | 同一目标可被多处关联 |
| 典型用途 | 拆分细粒度内容、表达从属 | 表达跨越层级的横向关系 |

### 5.5 遍历规则

1. 解析入口固定为 `ROOT/._meta`。
2. 展开一级：读取每个 `entries[role=node].path` 目录下的 `._meta`（**懒加载**，可按需）。
3. 递归展开下级：读取 `entries[role=branch].path` 的 `._meta`，直至 `max_depth`。
4. **深度限制**：分支树深度超过 `max_depth` 即 `E_DEPTH_EXCEEDED`（超过 `deep_tree_warn` 仅告警）。
5. **关联线解析**：读取 `refs[].target`，在已建立的索引中定位目标分支；未命中即 `E_REF_NO_TARGET`。
6. **关联环检测**：沿 `refs` 图做 DFS，若回到访问栈中的分支即 `E_REF_CYCLE`（分支树本身是树，天然无环）。
7. **禁止悬空**：任何分支目录必须被其父级的 `entries[]` 登记，否则 `E_MANIFEST_MISSING`。

---

## 6. 校验规则与错误码

### 6.1 错误码总表

| 码 | 级别 | 触发条件 |
| --- | --- | --- |
| `E_PARSE` | error | `._meta` 非法 TOML（重复键、裸键出现在表头之后、非法转义等）/ 编码非 UTF-8 / 含 BOM / 时间未带时区偏移 |
| `E_META_MISSING` | error | 分支目录（`node`/`branch`）缺失 `._meta` |
| `E_SPEC_UNSUPPORTED` | error | `str` 主版本不受支持 |
| `E_KIND_INVALID` | error | `kind` 非法 |
| `E_KIND_DEPTH` | error | `kind` 与所在深度不符（深度 0 必须 `root`；深度 1 必须 `node`；深度 ≥2 必须 `branch`） |
| `E_SCHEMA_FIELD` | error | 必填字段缺失 / 类型不符 / 未知字段（非 `ext` 内） |
| `E_ID_MISMATCH` | error | `id` 与所在目录名不一致 |
| `E_ID_NOT_UUID` | error | 目录名为 UUID 命名但格式非法 |
| `E_ID_VERSION` | error | UUID 版本 ≠ `policies.id_version` |
| `E_ID_DUP` | error | 同一 bundle 内出现重复分支 `id` |
| `E_ENTRY_ROLE_DEPTH` | error | 父级 `entries[].role` 与子目录实际内容或深度不符（含：子目录内含 `._meta` 已成分支，却仍被登记为 `dir`） |
| `E_ENTRY_ID_MISMATCH` | error | `entries[].id` 与子目录名不一致 |
| `E_REF_NO_TARGET` | error | `refs[].target` 无法在本 bundle 内解析 |
| `E_REF_SELF` | error | `refs[].target` 等于自身 `id` |
| `E_REF_CYCLE` | error | `refs` 关联图成环 |
| `E_DEPTH_EXCEEDED` | error | 分支树深度超过 `max_depth` |
| `E_MANIFEST_MISSING` | error | 磁盘存在但 `entries` 未登记 |
| `E_MANIFEST_GHOST` | error | `entries` 登记但磁盘不存在（且 `optional` 非真） |
| `E_MANIFEST_HASH` | error | `size` / `sha256` 与实际不符 |
| `E_MANIFEST_DIGEST_MISSING` | error | `policies.sha256 = "required"` 时，`role` ∈ `payload`/`asset` 的条目缺少 `size` 或 `sha256` |
| `E_MANIFEST_DUP` | error | `entries[].path` 重复 |
| `E_RESERVED_NAME` | error | 占用 `._` 保留命名空间：① 业务**目录**以 `._` 开头（AppleDouble 只产生文件，故 `._*` 目录必属业务命名）；② **已在 `entries` 中登记**的 `._*` 条目（作者显式声明其为业务内容）。未登记的 `._*` **普通文件**与 `.DS_Store` 属操作系统噪声，必须豁免、不得报本码 |
| `E_REVISION_STALE` | error | `updated_at` 变化但 `revision` 未前进，或 `revision` 非递增整数 |
| `E_SCHEMA_FAIL` | error | payload 不满足其声明的 JSON Schema |
| `W_BUNDLE_SUFFIX` | warn | 根目录名未以 `.str` 结尾 |
| `W_DOTFILE` | warn | 出现非 `._meta` / `.lock` 的点文件（`._*` **普通文件**、`.DS_Store` 与**版本控制元数据** `.git/` `.gitignore` 等属操作系统/工具元数据，必须豁免） |
| `W_ROOT_STRAY` | warn | ROOT 下出现**既非 UUID 命名的分支目录、又未登记进 `entries`** 的散落条目（已登记的 ROOT 内容属合法，见 3.3） |
| `W_NO_SUMMARY` | warn | `node`/`branch` 缺 `summary`（削弱 AI 检索能力） |
| `W_NO_TYPE` | warn | `node`/`branch` 缺 `type` |
| `W_DEEP_TREE` | warn | 分支树深度超过 `deep_tree_warn` |
| `W_LARGE_ASSET` | warn | 单文件超过 `large_asset_bytes` |
| `W_OPTIONAL_MISSING` | warn | `optional: true` 的条目实际缺失 |
| `W_MANIFEST_*` | warn | `manifest = advisory` 时的清单不一致 |

> **v1.1.0 已删除**：`E_DEPTH_OWNED`、`E_LINK_HAS_PAYLOAD`、`E_LINK_NO_TARGET`、`E_LINK_TARGET_NOT_NODE`、`E_LINK_CYCLE` —— 因「深度 ≥2 只能承载引用、不能承载数据」的错误假设已被撤回。
>
> **v1.2.0 已删除**：`E_STRAY_META` —— 因「素材目录」概念被取消（节点目录本身就是素材目录）。子目录内含 `._meta` 一律按「该目录是关联分支」处理，父级 `role` 未同步则报 `E_ENTRY_ROLE_DEPTH`，无需单独的「点文件位置」错误码。

### 6.2 校验输出格式

人类可读（默认）：

```
客户运营.str  3 nodes / 2 branches / 7 entries  depth=3
  ✗ E_MANIFEST_MISSING  01928f3a-…-0001/._meta  entries 未登记「notes.md」
  ✗ E_KIND_DEPTH        01928f3a-…-0001/._meta  depth=1 的 kind 必须为 node，实为 branch
  ⚠ W_NO_SUMMARY        01928f3a-…-0002  建议补充 summary
2 errors, 1 warning   exit=1
```

`--json` 时必须输出：`{ "bundle": "...", "errors": [{ code, level, path, message }], "warnings": [...], "stats": { nodes, branches, entries, depth } }`

### 6.3 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 通过（可能含 warning） |
| `1` | 存在 error |
| `2` | 用法错误 / 路径不存在 |
| `3` | I/O 或解析崩溃 |

---

## 7. 多人协作协议

### 7.1 冲突域最小化

| 设计 | 效果 |
| --- | --- |
| 一分支 = 一目录 = 一 `._meta` | 两人编辑不同分支 → 零冲突 |
| 关联线写在使用方 `._meta.refs[]` | 新增关联不影响被关联方文件 |
| 禁止持久化索引 | 索引类文件是天然的合并冲突热点 |

### 7.2 修订与合并

- 每次写入：`revision + 1`，`updated_at` 更新，`authors[]` 追加/更新当前协作者。
- Git 合并 `._meta` 冲突时：工具执行**三向合并**，字段级冲突策略为「取 `updated_at` 较新者」，但必须**同时保留双方 `authors[]` 并集**；合并后 `revision = max(revision_a, revision_b) + 1`。
- 同层 `entries[]` 冲突：按 `path` 做集合合并；同 `path` 双方都有时以 `updated_at` 较新者为准，并在 `note` 写入 `conflict: true`。
- 二进制 payload 冲突：不做自动合并，标记 `CONFLICT` 交由人工，并在 `entries[].note` 写入 `conflict: true` 与冲突来源 revision。

### 7.3 锁（可选）

- 短期写入锁文件 `.lock`（内容为 `{ actor, at, pid, ttl }`）。
- **`.lock` 不得提交**；过期通过 `ttl` 判定（建议 60s），工具应在发现过期锁时告警而非静默抢占。

### 7.4 `.gitignore` 建议

```gitignore
._cache/
**/.lock
**/.DS_Store
**/._*          # macOS AppleDouble 伴生文件（exFAT/SMB/压缩包解压产物）
```

---

## 8. AI 读取与写入协议

### 8.1 渐进式披露（读取）

| 步骤 | 动作 | 上下文成本 |
| --- | --- | --- |
| 1 | 读 `ROOT/._meta` 的 `name/title/summary` + `entries[role=node]` | 极小 |
| 2 | 按 `type/tags/title/summary` 选出目标分支 | —— |
| 3 | 读目标分支 `._meta`（元信息 + 清单 + 下级分支摘要） | 小 |
| 4 | 仅在需要时下钻 `entries[role=branch]` 或读 `entries[role=payload]` 的具体文件 | 按需 |
| 5 | 需要横向关系时读 `refs[]`，定位到目标分支后回到步骤 3 | 按需 |

**禁止**：未经筛选地递归读取整个 bundle 的所有 payload。

### 8.2 上下文裁剪命令

`str context <bundle> <uuid> --depth 2 --budget 8k` 输出「导图摘要 + 选中分支元信息 + 下级分支摘要 + 关联线」的 Markdown 片段，供直接拼接进模型上下文。

### 8.3 AI 写入约束

1. **不得**修改或新建 UUID 目录名。
2. 在已有子目录内创建 `._meta` 会使其成为**关联分支**；此时必须同步把父级 `entries[]` 中该项的 `role` 由 `dir` 改为 `branch`，并补 `id`/`kind`，否则 `E_ENTRY_ROLE_DEPTH`。
3. **可以**在任意深度的分支目录中新增文件/文件夹，但**必须**同步登记进 `entries[]`（或随后执行 `str sync`）。
4. 修改后**必须**：`revision + 1`、更新 `updated_at`、按 4.9 键序重排。
5. 不得把深度 ≥2 的分支「提升」为独立节点（需新建深度 1 节点并迁移）。
6. 删除他人 `authors[role=owner]` 的分支前必须显式确认。
7. 任何字段语义不明时**必须提问**，禁止发明新字段（除 `ext` 内）。

### 8.4 面向 AI 的最小元信息集

一个「对 AI 友好」的分支至少要有：`type`、`title`、`summary`、以及 `entries[]` 中每项的 `role`（分支项还应有 `summary` 与 `type`）。缺 `summary` 时校验器给出 `W_NO_SUMMARY`。

---

## 9. 工具链 CLI 规格

命令名统一为 `str`。

| 命令 | 作用 | 关键参数 |
| --- | --- | --- |
| `str init <dir>` | 创建 bundle（生成 ROOT `._meta` 与保留目录） | `--name` `--title` `--id-version` |
| `str validate <dir>` | 全量校验 | `--strict` `--json` `--fix-manifest` |
| `str tree <dir>` | 渲染导图（分支树 + 关联线标注） | `--depth n` `--show-refs` `--ascii` |
| `str ls <dir>` | 列出当前分支条目（读 `._meta.entries`） | `--raw`（改为直接读磁盘） |
| `str show <dir> <uuid>` | 打印某分支 `._meta` 与内容摘要 | `--full` |
| `str node add <dir>` | 新增**独立节点**（深度 1，生成目录 + `._meta` + 登记到 ROOT） | `--type` `--title` `--summary` |
| `str branch add <dir> <anchor-uuid>` | 在指定分支下新增**关联分支**（任意深度） | `--type` `--title` `--summary` `--order` |
| `str branch rm <dir> <uuid>` | 删除关联分支（含其全部下级） | `--force` `--recursive` |
| `str ref add <dir> <uuid> --target <uuid>` | 新增跨枝关联线 | `--rel` `--title` `--note` |
| `str ref rm <dir> <ref-uuid>` | 删除关联线 | — |
| `str sync <dir>` | 用磁盘实际状态修正 `entries`（补登/移除/`size`/`sha256` 更新）；**始终递归**全部分支 | `--dry-run` |
| `str fmt <dir>` | 按 4.9 键序/表序重写 `._meta`（**保注释**） | `--check` `--strip-comments` |
| `str norm <dir> <uuid>` | 输出**归一化 JSON**（供外部 Schema 工具 / AI 使用） | `--out -` |
| `str context <dir> <uuid>` | 生成 AI 上下文片段 | `--depth` `--budget` |
| `str export <dir>` | 导出为单一文件（只读、派生） | `--format json\|toml` `--out -` `--depth` |
| `str reveal <dir>` | 平台适配：macOS 设置 Bundle 位 / 取消 `._meta` 隐藏 | — |

实现要求：

- 一切写操作**必须**先通过校验（除 `sync`），校验失败则拒绝写入并打印错误码。
- `str branch add` 必须支持在任意深度操作；**不得**对深度做「只允许两层」之类的限制。
- `._meta` 的写回**必须保注释**；`str fmt --strip-comments` 是唯一允许丢弃注释的入口。
- 对内/对外一律以**归一化 JSON**（4.9）作为统一操作视图；它必须能从 TOML 无损重建，**不得**成为第二份真源。
- `str export` 的产物是派生数据，**不得**写回 bundle 内部。
- 所有命令须支持显式路径参数；`str tree` 输出必须同时表达分支树与 `refs` 关联线。

---

## 10. 完整示例 bundle

### 10.1 目录树

```
客户运营.str/
├── ._meta
├── ._schema/
│   ├── root-meta.schema.json
│   ├── node-meta.schema.json
│   ├── branch-meta.schema.json
│   └── customer.schema.json
├── 01928f3a-7c4b-7001-8a01-000000000001/          # 独立节点：客户档案
│   ├── ._meta
│   ├── profile.json
│   ├── avatar.png
│   ├── attachments/
│   │   └── 合同-2024Q1.pdf
│   └── 01928f3a-7c4b-7101-8b01-000000000101/      # 关联分支：跟进记录
│       ├── ._meta
│       ├── followups.json
│       └── 01928f3a-7c4b-7102-8b02-000000000102/  # 关联分支：2026-09 会议纪要
│           ├── ._meta
│           └── 2026-09-10.md
├── 01928f3a-7c4b-7002-8a02-000000000002/          # 独立节点：订单数据集
│   ├── ._meta
│   └── orders.csv
└── 01928f3a-7c4b-7003-8a03-000000000003/          # 独立节点：标签体系
    ├── ._meta
    └── tags.json
```

> 注意：深度 2 的「跟进记录」与深度 3 的「2026-09 会议纪要」**都在承载真实数据**，这正是 v1.1.0 明确允许的形态。

> **本节的 6 份 `._meta` 与 `str-cli/examples/客户运营.str/` 逐字一致**（`size` / `sha256` 为真实计算值，
> 非占位符），可用 `str-cli/scripts/build-example.sh` 重新生成，并用
> `str validate str-cli/examples/客户运营.str --strict` 验证。


### 10.2 `客户运营.str/._meta`

```toml
# ── STR bundle 根元数据 ──────────────────────────────────────────
# ROOT 的 [[entries]] 中 role = "node" 的条目即一级分支结构。
str = 1
spec = "1.7.0"
kind = "root"
id = "01928f3a-7c4b-7000-8000-000000000000"
name = "客户运营"
title = "客户运营结构化数据束"
summary = "以客户为主体的分支树，含跟进记录、订单与标签分支，供 CRM 与 AI 检索使用。"
tags = ["crm", "demo"]
revision = 12
created_at = 2026-09-01T09:12:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[policies]
id_version = 7
max_depth = 32
manifest = "strict"
sha256 = "required"
large_asset_bytes = 10485760
deep_tree_warn = 16

[[authors]]
id = "u:frowhy"
name = "Frowhy"
role = "owner"
at = 2026-09-01T09:12:00+08:00

[[authors]]
id = "u:agent-001"
name = "AI Agent"
role = "agent"
at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "01928f3a-7c4b-7001-8a01-000000000001"
role = "node"
id = "01928f3a-7c4b-7001-8a01-000000000001"
type = "crm.customer"
title = "客户档案 · 张伟"
summary = "企业客户主体档案，含跟进记录分支。"
order = 1

[[entries]]
path = "01928f3a-7c4b-7002-8a02-000000000002"
role = "node"
id = "01928f3a-7c4b-7002-8a02-000000000002"
type = "crm.order_dataset"
title = "订单数据集"
summary = "全部订单明细。"
order = 2

[[entries]]
path = "01928f3a-7c4b-7003-8a03-000000000003"
role = "node"
id = "01928f3a-7c4b-7003-8a03-000000000003"
type = "crm.tag_system"
title = "标签体系"
summary = "客户与订单共用的标签字典。"
order = 3

[[entries]]
path = "._schema"
role = "schema"
count = 4
note = "bundle 级校验 Schema 存放处"

[ext]
```

### 10.3 `01928f3a-7c4b-7001-8a01-000000000001/._meta`（客户档案 · 独立节点）

```toml
str = 1
spec = "1.7.0"
kind = "node"
id = "01928f3a-7c4b-7001-8a01-000000000001"
type = "crm.customer"
title = "客户档案 · 张伟"
summary = "2024 年 3 月签约的企业客户，归属华东区，当前为 VIP 等级。"
tags = ["华东区", "vip"]
revision = 8
created_at = 2026-09-01T09:20:00+08:00
updated_at = 2026-09-14T10:03:11+08:00
schema = "._schema/customer.schema.json"

[[refs]]
id = "01928f3a-7c4b-7201-8d01-000000000201"
target = "01928f3a-7c4b-7002-8a02-000000000002"
rel = "related"
title = "该客户的订单"
order = 1
note = "跨枝关联：客户档案 ⇢ 订单数据集"

[[entries]]
path = "profile.json"
role = "payload"
media_type = "application/json"
size = 87
sha256 = "852fa7846c3b3a70e053cf1b00ad8503a5f04b804cc4d1259404585260b8037f"
schema = "._schema/customer.schema.json"

[[entries]]
path = "avatar.png"
role = "asset"
media_type = "image/png"
size = 70
sha256 = "6b7fa434f92a8b80aab02d9bf1a12e49ffcae424e4013a1c4f68b67e3d2bbcd0"

[[entries]]
path = "attachments"
role = "dir"
count = 1
note = "合同扫描件；普通子目录，纯内容容器"

[[entries]]
path = "01928f3a-7c4b-7101-8b01-000000000101"
role = "branch"
id = "01928f3a-7c4b-7101-8b01-000000000101"
type = "crm.followup_log"
title = "跟进记录"
summary = "按时间的客户跟进日志，含下级会议纪要分支。"
order = 1

[ext]
```

### 10.4 `01928f3a-7c4b-7101-8b01-000000000101/._meta`（跟进记录 · 关联分支，深度 2）

```toml
# 深度 2 的关联分支同样承载真实数据（payload 直接放在本目录内）
str = 1
spec = "1.7.0"
kind = "branch"
id = "01928f3a-7c4b-7101-8b01-000000000101"
type = "crm.followup_log"
title = "跟进记录"
summary = "该客户的历次跟进摘要，含 2026-09 的会议纪要子分支。"
tags = ["followup"]
revision = 3
created_at = 2026-09-10T14:00:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "followups.json"
role = "payload"
media_type = "application/json"
size = 164
sha256 = "5ca93dc91b69a6eef2b4692cd270032f3b246ba9c8763b79afbcdfeef7bc195a"

[[entries]]
path = "01928f3a-7c4b-7102-8b02-000000000102"
role = "branch"
id = "01928f3a-7c4b-7102-8b02-000000000102"
type = "doc.meeting_note"
title = "2026-09 会议纪要"
summary = "9 月与客户的产品对齐会议纪要。"
order = 1

[ext]
```

### 10.5 `01928f3a-7c4b-7102-8b02-000000000102/._meta`（2026-09 会议纪要 · 关联分支，深度 3）

```toml
str = 1
spec = "1.7.0"
kind = "branch"
id = "01928f3a-7c4b-7102-8b02-000000000102"
type = "doc.meeting_note"
title = "2026-09 会议纪要"
summary = "深度 3 的关联分支，同样承载真实数据（Markdown 纪要）。"
tags = ["meeting"]
revision = 2
created_at = 2026-09-12T11:22:00+08:00
updated_at = 2026-09-13T09:05:00+08:00

[[entries]]
path = "2026-09-10.md"
role = "payload"
media_type = "text/markdown"
size = 156
sha256 = "162af15f76d0493282a4a5e666928f6de6a36662faa44a860288766be298e9a0"

[ext]
```

### 10.6 `01928f3a-7c4b-7002-8a02-000000000002/._meta`（订单数据集 · 独立节点）

```toml
str = 1
spec = "1.7.0"
kind = "node"
id = "01928f3a-7c4b-7002-8a02-000000000002"
type = "crm.order_dataset"
title = "订单数据集"
summary = "全部订单明细（CSV）。"
tags = ["order"]
revision = 4
created_at = 2026-09-02T10:00:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "orders.csv"
role = "payload"
media_type = "text/csv"
size = 136
sha256 = "1dd4893612cb1710550acb0624982a21e5ec5267d1dd05431e9aa20d02c16ef7"

[ext]
```

### 10.7 `01928f3a-7c4b-7003-8a03-000000000003/._meta`（标签体系 · 独立节点）

```toml
str = 1
spec = "1.7.0"
kind = "node"
id = "01928f3a-7c4b-7003-8a03-000000000003"
type = "crm.tag_system"
title = "标签体系"
summary = "客户与订单共用的标签字典。"
tags = ["taxonomy"]
revision = 2
created_at = 2026-09-03T15:30:00+08:00
updated_at = 2026-09-14T10:03:11+08:00

[[entries]]
path = "tags.json"
role = "payload"
media_type = "application/json"
size = 63
sha256 = "050b4e5bf2eaf595e0904397d45c5e6bb637d4bb4f250c047a384915a997b0fe"

[ext]
```

### 10.8 导图渲染结果

实际执行 `str tree examples/客户运营.str --show-refs` 的输出（`⇢` 即跨枝关联线）：

```
客户运营.str
├─ [1] 客户档案 · 张伟  (crm.customer)
│  ⇢ 关联: 01928f3a-7c4b-7002-8a02-000000000002  --related--
│  └─ [1] 跟进记录  (crm.followup_log)
│     └─ [1] 2026-09 会议纪要  (doc.meeting_note)
├─ [2] 订单数据集  (crm.order_dataset)
└─ [3] 标签体系  (crm.tag_system)
```

---

## 11. 设计决策记录（ADR）

### ADR-1：`._meta` 使用 TOML（否决 JSON 与 YAML）

| 维度 | **TOML（采纳）** | JSON（否决） | YAML（否决） |
| --- | --- | --- | --- |
| 人类手写 | ✅ 键值对 + 表，行导向，噪声低 | ❌ 引号/逗号/括号噪声大 | ✅ 简洁但**缩进即语义**，手改易错 |
| 注释 | ✅ 原生 `#` 注释 | ❌ 完全不支持 | ✅ 支持 |
| 隐式类型陷阱 | ✅ 低（类型显式；仅需注意整数前导零等边界） | ✅ 无 | ❌ 高（`no`→false、`1.10`→1.1、制表符） |
| Git diff / 合并 | ✅ 优（行导向，天然稳定） | ⚠️ 中（数组跨行时噪声大） | ⚠️ 中（缩进重排会产生大段 diff） |
| 原生时间类型 | ✅ offset date-time | ❌ 只能字符串 | ⚠️ 有，但格式宽松、易歧义 |
| 嵌套数组（清单） | ✅ `[[entries]]`（可读性最好） | ✅ | ✅ |
| 深层嵌套 | ⚠️ 表头模式在极深嵌套时可读性下降（本格式最多 1 层嵌套，不受影响） | ✅ | ✅ |
| Schema 生态 | ⚠️ 无官方 Schema 规范 → 需**归一化为 JSON** 后用 JSON Schema 校验 | ✅ 原生 | ❌ 需第三方方言 |

结论：**TOML 在「人写、机器读、Git 合并」三者间取得最优平衡** —— 既有 YAML 的可读性与注释，又无其隐式类型陷阱，同时天然行导向、diff 干净。

代价与对策：TOML 没有官方 Schema 规范，因此本格式采用 **`._meta`（TOML）→ 归一化 JSON → JSON Schema 2020-12 校验**的链路（Cargo 生态的通行做法）。归一化规则固定（4.9），保证外部工具与 AI 获得确定视图。

### ADR-2：分支目录命名 = 自身 UUID（v7）

| 方案 | 结论 |
| --- | --- |
| A. 目录名 = 父级/目标标识派生值 | ❌ 否决：破坏「一个分支一个稳定身份」，且父子关系变动会迫使重命名 |
| B. 目录名 = 分支自身 UUID v7（**采纳**） | ✅ 全局唯一、时间有序（利于排序与新分支定位）、与父级路径解耦（移动/重组不改变身份） |
| C. 软链接/硬链接 | ❌ 否决：跨平台与 Git 语义不一致，Windows 需特权 |

### ADR-3：**所有层级的分支都可以承载数据**（v1.0.0 的错误假设已撤回）

| 方案 | 结论 |
| --- | --- |
| A. 仅深度 1 可承载数据，深度 ≥2 只能是纯引用 | ❌ **否决（v1.0.0 错误）**：违背「每个目录下均可存放任意文件与文件夹」的原始构想，且迫使任何细粒度内容都必须提升为一级节点，思维导图无法自然生长 |
| B. **任意层级分支均可承载任意文件/文件夹（采纳）** | ✅ 层级差异**只体现在分支身份上**：深度 1 = `node`（登记在 ROOT、全局可寻址的一等实体）；深度 ≥2 = `branch`（只登记在其父分支下、依附存在）。数据存放不受层级限制 |

连带影响：原「link 目录 = 纯引用」机制被**可选的 `refs[]` 声明式关联**取代 —— 关联线不再需要真实目录，因而也不会与「分支目录可存数据」冲突。

### ADR-4：清单一律记入 `._meta.entries[]`，不另设独立结构视图

避免同一目录出现两份结构描述（如 `entries` 与 `links`/`children`）造成漂移；「下级分支视图」「关联视图」均由 `entries[role=branch]`、`refs[]` 在读取时**派生**，不得持久化。

### ADR-5：格式内部条目统一采用 `._` 保留前缀

| 维度 | 说明 |
| --- | --- |
| 规则 | 格式保留名一律以 `._` 开头：**文件** `._meta`（元数据）；**目录** `._schema` / `._cache`（未来扩展同此）。业务条目**不得**以 `._` 开头。 |
| 动机 | ① 单一、可枚举的保留命名空间，「格式内部 vs 业务内容」一眼可辨；② 与 `.lock`、业务自建点文件天然区分；③ 对标 macOS bundle 内部文件（`Contents/`）的心智模型 |
| 代价 | ① 多平台默认隐藏点文件，人工浏览需显式开启；② **`._*` 与 macOS AppleDouble 伴生文件同名模式冲突**（见下方风险） |
| 缓解 | ① `str reveal` 可将 `._meta` 等标记为可见（macOS：`chflags nohidden`）；② **操作系统噪声豁免**：校验器必须把 `._*` 形式的**普通文件**（AppleDouble 伴生文件）与 `.DS_Store` 视为噪声 —— 既不报 `E_RESERVED_NAME`，也不参与 `E_MANIFEST_*` 比对；③ `.gitignore` 必须排除 `**/._*` 与 `**/.DS_Store` |
| 备选 | 无点前缀的 `_meta` / `_schema`（对用户完全可见，但与 `._` 命名空间不一致）；若评审倾向可见性，可在 v1.0 发布前整体切换（仅需改 3.4 / 4.1 节与示例） |

> **风险与对策（`._` 前缀的特殊性）**：macOS 在 exFAT、SMB、NFS 卷及 zip/tar 归档中会为**每个普通文件**生成名为 `._<原文件名>` 的 AppleDouble 伴生文件。由于本规范采用 `._` 作为保留前缀：
>
> - **目录**不受影响 —— AppleDouble 只为文件生成，故 `._schema/` `._cache/` 这些**目录**不会与伴生文件冲突。
> - **普通文件**会受影响 —— 业务文件 `orders.csv` 在跨文件系统拷贝后会多出 `._orders.csv`。因此 **`._*` 形式的普通文件必须被豁免**（视为噪声），否则会误报 `E_RESERVED_NAME` 与 `E_MANIFEST_MISSING`。
> - `._meta` 自身若被拷贝到 exFAT，其伴生文件为 `._._meta`，同样属于噪声，按同一规则豁免。

### ADR-6：禁止持久化索引文件

索引是天然的合并冲突热点，且与文件系统存在双写漂移风险。性能需求由 `._cache/`（可删、建议 gitignore）+ 读取时惰性构建满足。

### ADR-7：跨枝关联用 `refs[]` 而非「引用目录」

| 方案 | 结论 |
| --- | --- |
| A. 建一个只含 `._meta` 的引用目录指向目标 | ❌ 否决：与「分支目录可承载数据」冲突（同一目录既像分支又像指针，语义含混）；且为纯声明关系创建真实目录会污染分支树 |
| B. 在使用方 `._meta.refs[]` 中声明（**采纳**） | ✅ 零文件系统副作用、可带 `rel`/`title`/`note`、可多处关联、易 diff |

### ADR-8：**取消「素材目录」概念**（节点目录本身就是素材目录）

| 方案 | 结论 |
| --- | --- |
| A. 为素材单独定义一种目录类型（`asset_dir`），并规定其不得含 `._meta` | ❌ **否决**：**每个节点/分支目录本身就是素材目录** —— 素材直接放在分支目录内即可，无需额外概念；且「不能含 `._meta`」的规定给普通目录强加了格式约束，一旦有人在其中放入 `._meta` 就会产生「既非分支又非素材目录」的第三态，语义反而更乱 |
| B. **不设「素材目录」概念（采纳）** | ✅ 子目录统一降级为**纯内容容器**（`role: dir`，无任何特殊语义、无任何格式约束）；「是否分支」的唯一判据仍是**该目录是否含 `._meta`**。子目录内含 `._meta` → 它就是关联分支，父级 `role` 必须同步为 `branch`（否则 `E_ENTRY_ROLE_DEPTH`） |

连带影响：删除错误码 `E_STRAY_META`（v1.2.0）；`count` 字段的适用 `role` 由 `asset_dir` 改为 `dir`。

---

## 12. 验收标准（DoD）

| # | 验收项 | 判定方式 |
| --- | --- | --- |
| 1 | 示例 bundle 零 error 通过 | `str validate 客户运营.str --strict` → exit 0 |
| 2 | 每个 error 码均有破坏用例 | 测试套件中逐码至少 1 例，断言错误码精确匹配 |
| 3 | 幂等性 | 连续两次 `str sync` 第二次不产生任何 diff |
| 4 | 确定性序列化 | 工具写入后 `git diff` 为空（对未修改内容）；注释不被丢失 |
| 5 | **任意深度可承载数据** | 在深度 3、4 分支中写入普通文件并登记，校验通过、`str context` 可定位 —— **不得**出现任何「层级过深禁止写入」的行为 |
| 6 | 身份规则强制 | 深度 1 写 `kind: branch` → `E_KIND_DEPTH`；深度 2 写 `kind: node` 且登记进 ROOT → `E_KIND_DEPTH`/`E_ENTRY_ROLE_DEPTH` |
| 7 | 关联线完整性 | 构造 `E_REF_NO_TARGET` / `E_REF_SELF` / `E_REF_CYCLE` 三例均可精确报出 |
| 8 | 深度上限 | 构造超过 `max_depth` 的分支链 → `E_DEPTH_EXCEEDED` |
| 9 | 清单双向一致 | 手动增/删文件后 `validate` 能精确定位 |
| 10 | 子目录升级为分支需同步 role | 在已登记为 `dir` 的 `attachments/` 内放 `._meta` → `E_ENTRY_ROLE_DEPTH`；补全 `role: branch` + `id`/`kind` 后通过 |
| 11 | 跨平台 | macOS / Linux / Windows 三端 `validate` 结果一致（无平台特性依赖） |
| 12 | AI 可用性 | `str context --budget 8k` 输出可直接拼入模型上下文，且不含 payload 正文 |
| 13 | 文档一致 | 本文件各章节与实现行为逐条比对无差异 |
| 14 | 操作系统噪声豁免 | 在 bundle 内放置 `._orders.csv`、`._._meta`、`.DS_Store` → 校验**通过**且不计入 `entries` 统计；业务文件以 `._` 开头 → `E_RESERVED_NAME` |
| 15 | TOML 校验链 | `._meta` → 归一化 JSON → JSON Schema 校验全链路通过；归一化后的键序/表序与 4.9 完全一致；`refs` / `entries` 缺省时补 `[]` |
| 16 | 注释保真 | 在 `._meta` 里加 `#` 注释 → 经 `str sync`/`str fmt` 写回后注释仍在；`str fmt --strip-comments` 时才可丢弃 |
| 17 | 强制指纹 | 删除任 `payload`/`asset` 条目的 `sha256` 或 `size` → `E_MANIFEST_DIGEST_MISSING` |
| 18 | 错误码覆盖率 | `str-cli/tests/validate_codes.rs` 中 6.1 的**全部 35 个错误码**各有 ≥1 个故意破坏用例，且断言精确到码 |
| 19 | 幂等 | `str sync` 连续执行两次，第二次输出「已更新 0 份」且无文件差异；`str fmt --check` 返回 0 |
| 20 | **自举** | 对仓库自身执行 `str validate .` → **0 errors / 0 warnings**（`str-cli/target/` 等构建产物因 `str-cli/` 非分支而不进入清单；`examples/客户运营.str/` 因 3.5 硬边界而被跳过） |

---

## 13. 版本演进策略

| 变更类型 | 版本位 | 兼容性 |
| --- | --- | --- |
| 新增可选字段 / 新增 `role`、`rel` 枚举值 | minor（`1.x.0`） | 向后兼容；旧工具须**保留未知字段不丢** |
| 收紧校验规则、语义变更 | major（`2.0.0`） | 需提供迁移工具 `str migrate --to 2` |
| 修正措辞、补示例 | patch（`1.0.x`） | 无影响 |

规则：

1. **只有 `str` 主版本号**需要工具显式支持；`spec` 用于人类追溯。
2. 一切厂商/实验性扩展必须放 `ext`（键名 `vendor.feature`），不得占用顶层字段。
3. 修订史：v1.1.0 / v1.2.0 属**语义放宽/收敛**（撤回深度限制、取消素材目录概念）；v1.3.0 属**命名空间变更**（保留前缀统一为 `._`）；v1.4.0 属**载体变更**（JSON → TOML）与**决策收敛**（UUID v7、保留 `refs`、强制 `sha256`）。由于 v1.0.0 从未发布，无需迁移工具；若已有基于早期草案的实现：① 按 6.1「已删除错误码」清单移除对应校验；② 保留名统一为 `._` 前缀并补操作系统噪声豁免；③ 把 `._meta` 从 JSON 改写为 TOML，并补齐「归一化链路」与「保注释写回」；④ v1.5.0 移除 `._audit/`、把深度分界固定为 2；⑤ v1.6.0 允许 `entries` / `refs` 空表省略（TOML 限制）并精确化 `E_RESERVED_NAME` 判定；⑥ v1.7.0 引入 `.str` 子 bundle 硬边界与 `role = "bundle"`、扩展元数据豁免至 VCS、修正 `W_ROOT_STRAY` 与 schema/policy 冲突。

---

## 附录 A：决策索引（全部已关闭）

> 下列决策均已由格式所有者确认并写入正文；保留本表作为**决策追溯索引**。任何改动都意味着规范版本升级（见第 13 章）。

| # | 决策点 | 结论 | 备选 |
| --- | --- | --- | --- |
| ~~A1~~ | ~~元数据文件名~~ | ✅ **已定：`._meta`；格式保留名统一为 `._` 前缀（现为 `._schema` / `._cache`）**（v1.3.0，v1.5.0 移除 `._audit`） | — |
| ~~A2~~ | ~~`._meta` 载体格式~~ | ✅ **已定：TOML v1.0.0**（JSON 与 YAML 均否决，见 ADR-1）（v1.4.0） | — |
| ~~A3~~ | ~~UUID 版本~~ | ✅ **已定：UUID v7**（时间有序，利于排序与新分支定位）（v1.4.0） | — |
| ~~A4~~ | ~~分支身份切换深度~~ | ✅ **已定：固定为 2 且不可配置**（深度 1 = 独立节点，≥2 = 关联分支；2 是唯一自洽值，理由见 3.3）。原 `policies.branch_min_depth` 字段**已删除**，避免暴露一个不允许改动的开关（v1.5.0） | — |
| ~~A5~~ | ~~深度 ≥2 是否绝对禁止 payload~~ | ✅ **已定：允许，任意层级均可承载任意文件/文件夹**（v1.1.0） | — |
| ~~A6~~ | ~~素材目录是否算分支~~ | ✅ **已定：不存在「素材目录」概念——节点/分支目录本身就是素材目录；子目录一律为纯内容容器（`role: dir`）**（v1.2.0） | — |
| ~~A7~~ | ~~`refs[]` 跨枝关联是否保留~~ | ✅ **已定：保留**（可选数组表 `[[refs]]`，声明式关联线，不复制数据）（v1.4.0） | — |
| ~~A8~~ | ~~是否需要 `._audit/` 审计日志~~ | ✅ **已定：不需要** —— 审计能力交由 Git / 协作平台提供，格式内不设 `._audit/`、不设 `journal` role（v1.5.0） | — |
| ~~A9~~ | ~~`sha256` 是否强制~~ | ✅ **已定：强制**（`policies.sha256 = "required"`，仅约束 `payload`/`asset` 文件类条目）（v1.4.0） | — |
