# STR 格式规范精要

> 本文是 `STR-FORMAT-PROMPT.md`（**唯一真源**，v1.9.0）的提炼，供 Agent 离线快速查阅。两者冲突时以规范正文与 `str --help` 的实际输出为准，并请提 issue 修正本文件。
> 本文件描述**规范要求**；命令面与实现细节见 `cli-reference.md`，两者已逐步对齐（残留的规范内部不一致在该文「规范内部不一致」一节列出）。

## 1. 一句话模型

`.str` 是一个**目录 bundle**：目录树 = 分支树，每个分支目录内的 `._meta`（TOML）记录自己的元信息与内容清单，跨枝关系用 `._meta.refs[]` 声明。**目录层级只决定「身份」，不决定「能否存数据」**。

## 2. 目录形态与深度语义

```
<名称>.str/
├── ._meta                        # ROOT 元数据（kind = root）
├── ._schema/                     # 保留名：本 bundle 的 JSON Schema
├── ._cache/                      # 保留名：可选派生缓存（可删，建议 gitignore）
├── <UUID-v7>/                    # 独立节点（深度 1，kind = node）
│   ├── ._meta
│   ├── <任意 payload / asset>
│   ├── <普通子目录>/              # role = dir；内含 ._meta 即成关联分支
│   └── <UUID-v7>/                # 关联分支（深度 2，kind = branch）
│       ├── ._meta
│       └── <UUID-v7>/            # 更深关联分支（深度 3+，仍为 branch）
└── <UUID-v7>/                    # 更多独立节点
```

| 深度 | 称谓 | 必须的 `kind` | 可承载任意文件 | 登记位置 | 身份 |
| --- | --- | --- | --- | --- | --- |
| 0 | ROOT | `root` | 允许（须登记） | — | bundle 容器；`entries[role=node]` 即一级分支结构 |
| 1 | 独立节点 | `node` | **允许** | ROOT 的 `entries[]` | 一等实体，全局唯一可寻址，可被任意分支 `refs` 关联 |
| ≥2 | 关联分支 | `branch` | **允许** | **父分支**的 `entries[]` | 依附父分支；可无限嵌套；不得升级为一等实体 |

**只有带 `._meta` 的子目录才是分支**（`role` = `node`/`branch`）；其余子目录都是普通内容容器（`role` = `dir`）。不存在「素材目录」概念——节点/分支目录本身就是素材目录。普通子目录内含 `._meta` 时，父级 `entries` 的 `role` **必须**同步改为 `branch`/`node`，否则 `E_ENTRY_ROLE_DEPTH`。

## 3. 命名规则

| 对象 | 规则 | 违规码 |
| --- | --- | --- |
| bundle 根目录 | `<名称>.str`（扩展名小写） | `W_BUNDLE_SUFFIX` |
| 分支目录（node/branch） | UUID 规范小写连字符，版本须等于 `policies.id_version`（`4` 或 `7`，缺省 v7）：`xxxxxxxx-xxxx-7xxx-[89ab]xxx-xxxxxxxxxxxx` | `E_ID_NOT_UUID` / `E_ID_VERSION` |
| 元数据文件 | 固定 `._meta`，目录内唯一，普通文件 | `E_META_MISSING` |
| 保留名 | 以 `._` 开头（`._meta` / `._schema` / `._cache` / 未来扩展）；业务条目**不得**以 `._` 开头 | `E_RESERVED_NAME` |
| 锁文件 | `.lock`（短生命周期，**不得提交**） | — |
| 其它点文件 | 仅 `._meta` / `.lock` 合法 | `W_DOTFILE` |
| 豁免项 | `._*` **普通文件**（AppleDouble）、`.DS_Store`、`Thumbs.db`、`desktop.ini`、版本控制元数据（`.git/`、`.gitignore` 等） | 一律忽略 |

**UUID 必须与目录名一致**：`node`/`branch` 的 `id` 字段 = 所在目录名（`E_ID_MISMATCH`）；`root` 的 `id` 由工具生成，与目录名无关。

## 4. `.str` 目录是 bundle 硬边界

以 `.str` 结尾的目录一律视为**独立子 bundle**（同 `.app` 嵌套语义）：

- 父 bundle 的扫描 / 校验**不进入**其内部；内部自成 `kind = root` 体系；
- 父级 `entries` 中应登记为 **`role = "bundle"`**（要求目录名以 `.str` 结尾）；登记为 `dir` 会报 `E_ENTRY_ROLE_DEPTH`。

## 5. `._meta` 载体要求

| 项 | 规定 |
| --- | --- |
| 格式 | TOML v1.0.0，禁用任何方言 |
| 编码 | UTF-8 **无 BOM** |
| 换行 / 缩进 | `LF`；顶层裸键不缩进，表内键值缩进 2 空格 |
| 末尾 | 保留 1 个换行 |
| 时间 | **TOML 原生 offset date-time**，必须带时区偏移（如 `2026-09-14T10:03:11+08:00`），**不得**写成字符串 |
| 注释 | 允许 `#`；**不得承载语义**；工具写回必须保注释 |
| 空数组表 | TOML 无法表达空 `[[x]]`：`refs` / `entries` 为空时**整表省略**，归一化时补 `[]` |
| 大小 | 建议 ≤ 256 KiB，超过宜拆分下级分支 |

## 6. 顶层字段

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `str` | integer | ✅ | 格式主版本，固定 `1` |
| `spec` | string | ✅ | 语义化版本，如 `"1.9.0"`（形如 `1.<minor>.<patch>`；批量改写用 `str spec set`） |
| `kind` | `root｜node｜branch` | ✅ | 必须与所在深度匹配（`E_KIND_DEPTH`） |
| `id` | uuid | ✅ | `node`/`branch` 必须等于目录名 |
| `name` | string | root 必填 | bundle 短名（不含 `.str`） |
| `type` | string | 建议 | 点分命名空间（`crm.customer`、`doc.spec`），AI 依此路由 |
| `title` | string | 建议 | 单行标题 |
| `summary` | string | 强烈建议 | 一句话摘要（≤200 字），AI 检索主要依据；缺失报 `W_NO_SUMMARY` |
| `tags` | string[] | 否 | 扁平标签，建议全小写 |
| `revision` | integer ≥ 1 | ✅ | 单调递增，每次写入 +1 |
| `created_at` | offset date-time | ✅ | 带时区偏移 |
| `updated_at` | offset date-time | ✅ | 变化必须伴随 `revision` 前进（历史相关判定，见 §10「`E_REVISION_STALE`」） |
| `authors` | `[[authors]]` | 建议 | 见下 |
| `schema` | string | 否 | payload 的 JSON Schema 相对路径 |
| `policies` | `[policies]` | 仅 root | 见下 |
| `refs` | `[[refs]]` | 否 | 跨枝关联声明 |
| `entries` | `[[entries]]` | ✅（空可省略） | 本目录内容清单（唯一真源） |
| `ext` | `[ext]` | 否 | 扩展命名空间，键必须 `vendor.xxx` |

### `[[authors]]`

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `id` | string | ✅ | **稳定标识符**（SSO sub / 邮箱 hash / 账号 uid），**禁止**用显示名当 id |
| `name` | string | 否 | 展示名 |
| `role` | `owner｜editor｜viewer｜agent` | ✅ | `agent` 表示 AI 代理 |
| `at` | offset date-time | 否 | 参与时间 |

### `[[refs]]`（跨枝关联线）

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `id` | uuid | ✅ | 关联线自身标识 |
| `target` | uuid | ✅ | 目标分支 `id`（任意深度；**不得是 ROOT**） |
| `rel` | enum | ✅ | `related` / `depends_on` / `instance_of` / `derived_from` / `ref` / `x-<小写字母数字连字符>` |
| `title` | string | 否 | 关联线标签 |
| `order` | integer ≥ 0 | 否 | 同源内排序 |
| `note` | string | 否 | 备注 |

规则：`target` 必须可在同一 bundle 内解析（否则 `E_REF_NO_TARGET`）；**禁止成环**（`E_REF_CYCLE`）；禁止自关联（`E_REF_SELF`）；`refs` **不表达父子关系**——父子关系只由目录结构 + 父级 `entries[]` 决定。

### `[[entries]]`（内容清单，核心）

`entries[]` 是本目录内**除 `._meta` 之外全部条目**的完整清单，是本目录内容的**唯一权威描述**；文件系统是**存在性权威**。二者由校验器双向比对。

| 字段 | 类型 | 必填 | 适用 role | 说明 |
| --- | --- | --- | --- | --- |
| `path` | string | ✅ | 全部 | 相对本目录的**单段路径**（不含 `/`），不得重复 |
| `role` | enum | ✅ | 全部 | 见下表 |
| `id` | uuid | ✅ | `node`/`branch` | 必须与 `path` 一致 |
| `type` | string | 否 | `node`/`branch` | 便于只读父级 `._meta` 完成路由 |
| `title` / `summary` | string | 否 | `node`/`branch` | 展示名 / 摘要 |
| `order` | integer ≥ 0 | 建议 | `node`/`branch` | 同层排序键 |
| `media_type` | string | 建议 | `payload`/`asset` | IANA 媒体类型 |
| `size` | integer ≥ 0 | ✅ | `payload`/`asset` | 字节数 |
| `sha256` | string(64 hex) | ✅ | `payload`/`asset` | 内容指纹 |
| `count` | integer ≥ 0 | 建议 | `dir` | 直接子项数（非递归） |
| `schema` | string | 否 | `payload` | 该文件遵循的 Schema 相对路径 |
| `optional` | boolean | 否 | 全部 | 缺失是否允许（默认 `false`） |
| `note` | string | 否 | 全部 | 备注 |

`role` 枚举：`node`（深度 1 子分支）、`branch`（深度 ≥2 子分支）、`payload`（结构化数据文件）、`asset`（附属素材）、`dir`（普通内容容器）、`schema`（`._schema/` 目录）、`cache`（派生缓存）、`other`（需 `note` 说明）。
**只有 `node` / `branch` 计入分支结构。**

### `[policies]`（仅 root）

| 字段 | 缺省 | 说明 |
| --- | --- | --- |
| `id_version` | `7` | UUID 版本要求 |
| `max_depth` | `32` | 分支树最大深度（超出 `E_DEPTH_EXCEEDED`） |
| `manifest` | `strict` | `strict` = 清单不一致为 error；`advisory` = 仅 warning |
| `sha256` | **`required`** | 是否强制 `payload`/`asset` 带 `size` + `sha256`；`optional`/`off` 仅编辑期临时降级，**不得**出现在已提交状态 |
| `large_asset_bytes` | `10485760` | 超过告警 `W_LARGE_ASSET` |
| `deep_tree_warn` | `16` | 超过告警 `W_DEEP_TREE` |

清单一致性（`strict` / `advisory`）：磁盘有而 `entries` 无 → `E_/W_MANIFEST_MISSING`；`entries` 有而磁盘无（非 `optional`）→ `E_/W_MANIFEST_GHOST`；`size`/`sha256` 不符 → `E_/W_MANIFEST_HASH`。

> **v1.8.0 已删除** `policies.unknown_entry`（与 `manifest` 重叠、从未被 Schema 与实现采纳，写入即 `E_SCHEMA_FAIL`）。未登记条目的处理一律由 `manifest` 表达。

## 7. 规范键序与表序（§4.9）

TOML 要求裸键写在任何表头之前，因此书写顺序固定：

1. 顶层裸键：`str, spec, kind, id, name, type, title, summary, tags, revision, created_at, updated_at, schema`
2. `[policies]`（仅 root）
3. `[[authors]]` → `[[refs]]` → `[[entries]]`（各自按 **`(order, path|id)`** 排序：`order` 缺省视为最大，故无 `order` 者恒在末尾、彼此按 `path`/`id` 字典序）
4. `[ext]`（必须最后）

表内键序：

| 表 | 键序 |
| --- | --- |
| `[policies]` | `id_version, max_depth, manifest, sha256, large_asset_bytes, deep_tree_warn` |
| `[[authors]]` | `id, name, role, at` |
| `[[refs]]` | `id, target, rel, title, order, note` |
| `[[entries]]` | `path, role, id, type, title, summary, order, media_type, size, sha256, count, schema, optional, note` |

未知键**必须原样保留**（前向兼容）。规范化只调顺序、不动值，注释随键/表一起搬运而**不丢**。

**参考实现已强制该顺序**：每个写命令（`sync` / `node add` / `meta set` …）落盘的字节都已是规范形式，`str fmt --check` 返回 0 可直接当 CI 门禁（`str validate` 本身仍**不**检查顺序，它只报 §6 的错误码）。

## 8. 校验、输出与退出码

- 输出格式与退出码见 `cli-reference.md` §2 / §3.2。退出码：`0` 通过（可能含 warning）、`1` 有 error、`2` 用法/路径错误、`3` I/O 或解析崩溃。

### 错误码（35 个）

`E_PARSE`（非法 TOML / 非 UTF-8 / 含 BOM / 时间缺时区）、`E_META_MISSING`、`E_SPEC_UNSUPPORTED`、`E_KIND_INVALID`、`E_KIND_DEPTH`、`E_SCHEMA_FIELD`、`E_ID_MISMATCH`、`E_ID_NOT_UUID`、`E_ID_VERSION`、`E_ID_DUP`、`E_ENTRY_ROLE_DEPTH`、`E_ENTRY_ID_MISMATCH`、`E_DEPTH_EXCEEDED`、`E_REF_NO_TARGET`、`E_REF_SELF`、`E_REF_CYCLE`、`E_MANIFEST_MISSING`、`E_MANIFEST_GHOST`、`E_MANIFEST_HASH`、`E_MANIFEST_DIGEST_MISSING`、`E_MANIFEST_DUP`、`E_RESERVED_NAME`、`E_REVISION_STALE`、`E_SCHEMA_FAIL`

`W_BUNDLE_SUFFIX`、`W_DOTFILE`、`W_ROOT_STRAY`、`W_NO_SUMMARY`、`W_NO_TYPE`、`W_DEEP_TREE`、`W_LARGE_ASSET`、`W_OPTIONAL_MISSING`、`W_MANIFEST_MISSING`、`W_MANIFEST_GHOST`、`W_MANIFEST_HASH`

（已删除：`E_DEPTH_OWNED`、`E_LINK_*`、`E_STRAY_META` —— 对应错误假设已撤回。`str codes` 是权威清单。）

## 9. AI 读取与写入协议（§8）

**渐进式披露**：① 读 `ROOT/._meta` 的 `name/title/summary` + `entries[role=node]` → ② 按 `type/tags/title/summary` 选目标分支 → ③ 读该分支 `._meta` → ④ 仅在需要时下钻 `entries[role=branch]` 或读 `payload` → ⑤ 需要横向关系时读 `refs[]`。
**禁止**未经筛选地递归读取整个 bundle 的所有 payload。裁剪上下文用 `str context <dir> [uuid] --depth n --budget c`（`[uuid]` 缺省为 ROOT）。

**写入约束**：① 不得修改/新建 UUID 目录名；② 在已有子目录内创建 `._meta` 会使其成为关联分支，必须同步把父级 `entries[]` 该项 `role` 由 `dir` 改为 `branch` 并补 `id`/`kind`；③ 可在任意深度新增文件/文件夹，但必须同步登记进 `entries[]`（或随后 `str sync`）；④ 修改后必须 `revision + 1`、更新 `updated_at`、按 §4.9 键序重排；⑤ 不得把深度 ≥2 的分支「提升」为独立节点；⑥ 删除他人 `owner` 的分支前必须显式确认；⑦ 字段语义不明时**必须提问**，禁止发明新字段（`ext` 除外）。

**对 AI 友好的分支**至少要有：`type`、`title`、`summary`，以及 `entries[]` 中每项的 `role`（分支项还应有 `summary` 与 `type`）。

## 10. `E_REVISION_STALE`：历史相关判定怎么落地

「`updated_at` 变化但 `revision` 未前进」**无法**只看一份 `._meta` 判定（文件不含「上一版」）。规范 §6.1.1 只要求「可判定」，不限定手段；参考实现的做法：

1. **写入端登记基线**：每次成功写盘后，把该分支的 `(revision, updated_at)` 快照写进 `._cache/revisions.json`（bundle 根下，`role = cache` 的派生数据：不入 `entries` 清单、`.gitignore` 已排除、可随时删除）；
2. **校验端比对**：`str validate` 读基线，`updated_at` 变了而 `revision` 未前进 → `E_REVISION_STALE`；
3. **`str sync` 只在 `revision` 确实前进时推进基线** —— 违规因此**跨 `sync` 持续可见**，直到有人真正修正 `revision`（若 sync 无条件覆盖，它会顺手把刚犯下的违规洗白）；
4. **无基线则跳过**：从未被工具写过的 bundle 该条件跳过，不误报；删掉 `._cache/` 即关闭这项历史检查。

另两个条件是自描述的，任何实现都必须直接判定：`revision` 是 ≥ 1 的整数、`updated_at` 不早于 `created_at`。
