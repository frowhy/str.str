# `str` CLI 参考（v0.1.0 · 全部实测）

> 本文以**实现的实际行为**为准。规范正文 `STR-FORMAT-PROMPT.md` §9 的命令表存在若干未实现的参数，差异见「规范 ↔ 实现漂移」。
> 本文所有命令、输出与退出码均在 `str-cli` 的 `cargo build --release` 产物上实测取得（`str --version` = `str 0.1.0`）。

## 1. 获取与安装

`str` 是单二进制，无运行时依赖。

| 方式 | 命令 | 产物 |
| --- | --- | --- |
| 安装到 `PATH` | `cargo install --path str-cli` | `~/.cargo/bin/str` |
| 仅在仓库内使用 | `cd str-cli && cargo build --release` | `str-cli/target/release/str` |
| 交给 Agent 自动解析 | `STR="$(sh str-skill/scripts/ensure-str.sh)"` | 打印可执行文件路径 |

`ensure-str.sh` 依次尝试：`$STR_BIN` → `PATH` 上的 `str` → `$STR_REPO` → 从脚本目录与当前目录向上查找 `str-cli/target/release/str` → 发现 `str-cli/Cargo.toml` 就 `cargo build --release`。全部失败则打印指引并**非零退出**。

## 2. 全局约定

- 命令名 `str`。**除 `str codes` 外的每个命令都必须显式给出 bundle 目录**；`Bundle::new` 会把路径归一化为绝对路径，因此 `str validate .` 合法。
- 退出码：

| 码 | 含义 | 触发场景 |
| --- | --- | --- |
| `0` | 通过（可能含 warning） | 校验干净、写操作成功、`fmt --check` 无需规范化 |
| `1` | 存在 error | `validate` 有 error；`fmt --check` 有需规范化的文件 |
| `2` | 用法错误 | 路径不存在（`Error::NotFound`）、参数错误 / 找不到分支或关联线（`Error::BadArg`） |
| `3` | I/O 或解析崩溃 | 读写失败等 |

- 错误信息一律走 **stderr**，前缀 `str: `：`str: 路径不存在：/x`、`str: 参数错误：找不到分支 id \`xxx\``。
- **stdout 只放结果**：`norm` / `export` / `context` / `validate --json` / `ls` / `show` / `tree` 的正文都在 stdout，可直接管道给 `jq`。
- 写操作成功时在 stdout 打印一行中文确认，例如 `已新增独立节点 <uuid>（深度 1）`。

## 3. 命令逐条

### 3.1 `str init [--name N] [--title T] [--summary S] <DIR>`

- `<DIR>` 未以 `.str` 结尾时**自动追加** `.str`（`str init demo` → `demo.str`）。
- 目标已存在则 `Error::BadArg`（exit 2）。
- 生成：ROOT `._meta` + `._schema/`（写入 **3** 份 Schema：`root-meta` / `node-meta` / `branch-meta`）。
- 生成的 ROOT `._meta` 特点：`kind = "root"`、`revision = 1`、`tags = []`、**没有 `[[authors]]`**、`[policies]` 全为默认值（`id_version = 7`、`max_depth = 32`、`manifest = "strict"`、`sha256 = "required"`、`large_asset_bytes = 10485760`、`deep_tree_warn = 16`），并登记 `._schema` 为 `role = "schema"`（带 `count` 与 `note`）。

### 3.2 `str validate [--strict] [--json] [--fix-manifest] <DIR>`

- 文本输出首行统计 + 逐条 issue + 汇总行：

```
demo.str  2 nodes / 1 branches / 5 entries  depth=2
  ✗ E_MANIFEST_MISSING  <path>  <message>
  ⚠ W_NO_SUMMARY        <path>  <message>
1 errors, 0 warnings   exit=1
```

- `--json` 输出 `{ "bundle", "errors": [{code, level, path, message}], "warnings": [...], "stats": {nodes, branches, entries, depth} }`。
- `--strict` 把 warning 提升为 error（CI 用）。
- ⚠ **`--fix-manifest` 名不副实**：它调用的是 `sync(dir, dry_run = true)`，**只打印将要发生的变更、不会写盘**，随后照旧校验并因 `E_MANIFEST_MISSING` 失败（实测 exit 1）。**必须显式先跑 `str sync <dir>`。**

### 3.3 `str tree [--depth N] [--show-refs] <DIR>`

```
demo.str
├─ [1] 客户A  (crm.customer)
│  ⇢ 关联: <target-uuid>  --related--
│  └─ [1] 跟进记录  (crm.followup_log)
└─ [2] 标签体系  (crm.tag)
```

`[n]` 是同级序号，括号内是 `type`。关联线**只在 `--show-refs` 时输出**，且挂在**源分支**下方（未解析的目标显示为 `<uuid>（未解析）`）。

### 3.4 `str ls [--uuid U] [--raw] <DIR>`

- 缺省列 ROOT。走清单（`._meta.entries`）：

```
01a09bf6-…-62b191  （清单）
  01a09bf6-…-5eba5 branch  跟进记录
  profile.json                 payload  19B
  ._schema                     schema  bundle 级校验 Schema 存放处
```

第二列是 `role`，第三列是 `title`（若有），其后是 `size`（`19B`）与 `(optional)`。
**`path` 列就是子项名；对 `node`/`branch` 而言即子分支 UUID** —— 这是获取 UUID 的标准途径。

- `--raw` 直接读磁盘：首行 `<rel>  （磁盘原始）`，随后每行 `  - <文件>` / `  d <目录>`。

### 3.5 `str show [--full] <DIR> <UUID>`

打印该分支 `._meta` 的**归一化 JSON**（含 `ext: {}`、空表补 `[]`）。

`--full` 会在 JSON 之后追加各 `payload` / `asset` 的正文，形如：

```
── payload.txt ──
hello
```

### 3.6 `str node add [--type T] [--title X] [--summary S] <DIR>`

新增**独立节点**（深度 1，`kind = "node"`）：生成 UUIDv7 目录 + `._meta`，并 `upsert` 进 ROOT 的 `entries[]`（`role = "node"`）。stdout 打印新 UUID。ROOT 的 `revision` 会 `+1`。

### 3.7 `str branch add [--type T] [--title X] [--summary S] [--order N] <DIR> <ANCHOR>`

在锚点分支下新增**关联分支**（`kind = "branch"`，深度 ≥ 2）。
锚点必须是 `node`/`branch`（深度 ≥ 1）；对 ROOT 使用会报 `参数错误：ROOT 的直接子分支应使用 \`str node add\`（role = node）`。`--order` 缺省为同级最大 `order + 1`。

### 3.8 `str branch rm --force <DIR> <UUID>`

- **删除总是递归**（`remove_dir_all`，含全部下级）；规范 §9 里的 `--recursive` **不存在**。
- 不加 `--force` 会以 exit 2 拒绝（提示会移除全部下级）；不能删除 ROOT。
- 删除后会自动从父级 `entries[]` 移除该条目，父级 `revision + 1`。

### 3.9 `str ref add --target <TARGET> [--rel R] [--title X] [--note N] <DIR> <UUID>`

在**源分支** `<UUID>` 的 `._meta.refs[]` 追加一条关联线。`--rel` 缺省 `related`。
`<UUID>` 与 `<TARGET>` 都必须能在本 bundle 内解析，否则 exit 2。新 `refs[].id` 为 UUIDv7。

合法 `rel`（Schema 正则）：`related`、`depends_on`、`instance_of`、`derived_from`、`ref`、`x-<小写字母数字连字符>`。

### 3.10 `str ref rm --ref <REF_ID> <DIR> <UUID>`

**`--ref` 是选项，不是位置参数**。删除源分支 `refs[]` 中 `id == REF_ID` 的关联线；找不到则 exit 2。

### 3.11 `str sync [--dry-run] <DIR>`

用磁盘实际状态修正**全部**分支（始终递归）的 `entries[]` 与指纹。逐行打印：

```
+ <rel>/<name>  role=payload      # 补登（role 由扩展名推断，见下）
- <rel>/<name>                    # 移除（磁盘已不存在且非 optional）
~ <rel>/<name>  指纹更新           # size / sha256 变化
（dry-run）将更新 N 份 `._meta`     # 或：已更新 N 份 `._meta`
```

- 新目录补登为 `role = "dir"`（带 `count`）；若该目录**含 `._meta`** 则登记为 `node`（深度 0 时）或 `branch`。
- 新文件按扩展名推断 role：`json/toml/yaml/csv/md/txt` → `payload`，其余 → `asset`；同时补 `media_type`、`size`、`sha256`。
- 只有**确实发生变更**的分支才 `revision + 1` + 刷新 `updated_at`。
- 补登的条目**不带** `title` / `summary` / `type`（见「已知能力缺口」）。

### 3.12 `str fmt [--check] [--strip-comments] <DIR>`

- 设计意图：按 §4.9 键序 / 表序重写（保注释）。
- `--check`：`0` = 全部已规范；`1` = 有 N 份需要规范化。
- `--strip-comments`：丢弃整行 `#` 注释（唯一允许丢注释的入口）。
- **实测局限**：不重排顶层裸键；也不重排乱序的 `[[entries]]` / `[[refs]]`（见「已知缺陷」）。因此它实际只在 `--strip-comments` 时产生变更。`fmt` **不修改** `revision` / `updated_at`。

### 3.13 `str norm <DIR> [UUID]`

打印归一化 JSON（2 空格缩进，含 `ext: {}`）。缺省为 ROOT。**没有 `--out`**；需要文件就重定向。

### 3.14 `str context <DIR> <UUID> [--depth N] [--budget C]`

输出 Markdown 上下文片段：

```
# STR 上下文：demo.str

- `01a09bf6-…` **客户A** (crm.customer) depth=1
  示例客户
  tags: crm, demo
  内容：profile.json(payload)
  关联线 → 01a09bf6-… (related)
  - `01a09bf6-…` **跟进记录** (crm.followup_log) depth=2
    2026-09 跟进
```

`--depth` 缺省 `2`，`--budget` 缺省 `8000`（字符）。超预算时截断并追加 `…（已按 --budget 截断）`。

### 3.15 `str export [--format json|toml] [--depth N] <DIR>`

只读导出整棵树：`{"path", "depth", "meta", "children": [...]}` 递归结构。缺省 `--format json`。**没有 `--out`**；产物是派生数据，**不得**写回 bundle 内部。

### 3.16 `str reveal <DIR>`

macOS：`SetFile -a B` 设 bundle 位 + `chflags nohidden` 取消各 `._meta` 的隐藏标记。缺少 `SetFile`（需 Xcode CLT）时只打印手动命令，不算失败。非 macOS 打印提示。

### 3.17 `str codes`

列出全部 **35** 个错误码（24 个 `E_*` + 11 个 `W_*`，含 `W_MANIFEST_*` 三个 advisory 变体）。规范 §9 未列此命令。

## 4. 规范 ↔ 实现漂移

| 规范 §9 写法 | 实际实现 | 应对 |
| --- | --- | --- |
| `str init --id-version` | **不存在** | `init` 固定 UUIDv7，`policies.id_version = 7` |
| `str tree --ascii` | **不存在** | 只有 `--depth` / `--show-refs` |
| `str norm <dir> <uuid> --out -` | **无 `--out`** | 直接写 stdout，用重定向 |
| `str export --out -` | **无 `--out`** | 同上 |
| `str ls`（未写 uuid 传参形式） | 用 `--uuid <uuid>` | 用选项形式 |
| `str branch rm --force --recursive` | **无 `--recursive`** | 删除**总是**递归，`--force` 是确认位 |
| `str ref rm <dir> <ref-uuid>`（位置参数） | `--ref <REF_ID>` | 用 `--ref` |
| `str validate --fix-manifest` | 传 `dry_run = true`，**不写盘** | 显式 `str sync` |
| 规范未列 | 多出 `str codes` | — |

## 5. 已知缺陷与能力缺口（v0.1.0）

1. **`sort_collections()` 未生效**：`meta.rs` 中该函数负责按 `(order, path|id)` 重排 `entries` / `refs`，但实测**不产生重排**。可复现：手工把 ROOT 两个 `[[entries]]` 的 `order` 互换后，`str fmt` 仍报「全部 `._meta` 已是规范形式」且文件 md5 不变；随后 `str node add` 再次触发 `sort_collections()` 保存，新条目依然**追加在末尾**而非按 `order` 插入。后果：§4.9「确定性序列化」在参考实现中**未被强制执行**，磁盘顺序 = 创建顺序。
2. **校验不检查顺序**：`str validate --strict` 对顶层裸键乱序、`entries[]` 乱序均报 `0 errors, 0 warnings`。所以「键序 / 表序」目前只是**约定**，不能靠工具兜底。
3. **`validate --fix-manifest` 不修清单**（见 §3.2）。
4. **无 CLI 可写** `entries[].title` / `entries[].summary` / `entries[].note`、`tags`、`authors[]`。`str sync` 为分支目录补登出的 `entries[]` 行只有 `path` / `role` / `id`（无 `type`/`title`/`summary`）；而 `W_NO_TYPE` / `W_NO_SUMMARY` 判定的是**该分支自身 `._meta`** 的 `type` / `summary`。两者都只能人工补齐（走 SKILL.md 的「唯一例外」流程）。
5. **`E_REVISION_STALE` 判定比规范弱**：实现只校验 `revision` 是 `≥ 1` 的整数、且 `updated_at ≥ created_at`（`Meta::revision_issues`）。规范 §6.1 描述的「`updated_at` 变化但 `revision` 未前进」**未实现**，所以那个约束只能靠纪律（或 CI 额外检查）保证。
6. **`str reveal`** 依赖 macOS `SetFile`；缺失只提示，不改退出码。
