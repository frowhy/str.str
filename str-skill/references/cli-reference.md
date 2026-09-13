# `str` CLI 参考（全部实测）

> 本文以**实现的实际行为**为准，与 `STR-FORMAT-PROMPT.md`（规范正文，v1.9.0）§9 的命令表对齐。
> 本文所有命令、输出与退出码均在 `str-cli` 的 `cargo build --release` 产物上实测取得（`str --version` = `str 0.3.0`）。
> 规范自身仍未闭合的少数点集中在文末「规范内部不一致」一节。

## 1. 获取与安装

`str` 是单二进制，无运行时依赖。

| 方式 | 命令 / 位置 | 产物 |
| --- | --- | --- |
| 装到 `PATH`（crates.io） | `cargo install str-format` | `~/.cargo/bin/str` |
| 装到 `PATH`（仓库源码） | `cargo install --path str-cli` | `~/.cargo/bin/str` |
| 仅在仓库内使用 | `cd str-cli && cargo build --release` | `str-cli/target/release/str` |
| 直接下预编译二进制 | GitHub Releases（5 平台） | 单文件 `str` / `str.exe` |
| 交给 Agent 自动获取 | `STR="$(sh str-skill/scripts/ensure-str.sh)"` | 打印可执行文件路径 |

`ensure-str.sh` 依次尝试，命中即返回：① `$STR_BIN` → ② `PATH` 上的 `str`（须 `--version` 输出 `str x.y.z`，否则继续）→ ③ `$STR_REPO` → ④ 从脚本目录、再从当前目录向上查找源码产物（发现 `str-cli/Cargo.toml` 即 `cargo build --release`）→ ⑤ **从 GitHub Releases 下载当前平台的预编译二进制** → ⑥ **用 `cargo install str-format` 从 crates.io 源码编译安装（隔离到缓存目录）**。第 ⑤ 步细节：

- **平台 → 资产名**（与 `.github/workflows/release.yml` 的矩阵一致）：
  `Darwin/arm64` → `aarch64-apple-darwin`；`Darwin/x86_64` → `x86_64-apple-darwin`；
  `Linux/aarch64` → `aarch64-unknown-linux-gnu`；`Linux/x86_64` → `x86_64-unknown-linux-gnu`；
  `MINGW*/MSYS*/CYGWIN*/Windows_NT` + `x86_64` → `x86_64-pc-windows-msvc`（资产为 `.zip`，二进制名 `str.exe`）。其余平台跳过下载。
- **版本**：`STR_VERSION`，默认 `latest`（经 GitHub API 解析 tag；API 不可用时**回退到脚本内置的默认版本**并告警，因此断网不会让版本解析失败）。
- **完整性（关键）**：**必须**取到同一 release 的 `SHA256SUMS.txt`，且其中含本资产条目，逐字节比对通过才使用；取不到校验和 / 缺条目 / 不匹配，一律**中止**且**不写入缓存** —— 不存在「未校验就用」的降级分支。
- **缓存**：`${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/<tag>/<target>/`，命中即复用、不重复下载。
- **其它变量**：`STR_RELEASE_REPO`（fork 时改 `owner/repo`）、`STR_DOWNLOAD_BASE`（镜像 / 网络受限）、`STR_CACHE_DIR`、`STR_NO_DOWNLOAD=1`（跳过本步）。

第 ⑥ 步细节：

- 命令为 `cargo install str-format --version <tag> --locked --root <缓存>`，**安装根是缓存目录**：`${XDG_CACHE_HOME:-$HOME/.cache}/str-skill/cargo/<tag>/bin/str`（Windows 为 `str.exe`）；**不写 `~/.cargo/bin`**，命中即复用、不重复编译。
- 排在下载之后：源码编译首次需数分钟，而第 ⑤ 步是预编译产物且强制 SHA-256 校验。
- 版本沿用第 ⑤ 步同一套解析（`STR_VERSION` → GitHub `latest` → 内置默认版本），因此两条路径拿到的版本一致；该 tag 若未发布到 crates.io，本步失败并落到最后的失败分支。
- 要求本机有 `cargo`（无则跳过）；`--locked` 用 crate 内随包发布的 `Cargo.lock`，与仓库 CI 验证过的依赖组合一致。
- **其它变量**：`STR_NO_CARGO_INSTALL=1`（跳过本步，离线时可与 `STR_NO_DOWNLOAD=1` 一起用）。

全部失败则打印安装指引并**非零退出**（退出码 1）；成功时 stdout **只**输出一行可执行文件路径，诊断一律走 stderr。

## 2. 全局约定

- 命令名 `str`。**除 `str codes` 外的每个命令都必须显式给出 bundle 目录**；`Bundle::new` 会把路径归一化为绝对路径，因此 `str validate .` 合法。
- 退出码：

| 码 | 含义 | 触发场景 |
| --- | --- | --- |
| `0` | 通过（可能含 warning） | 校验干净、写操作成功、`fmt --check` 无需规范化 |
| `1` | 存在 error | `validate` 有 error；`fmt --check` 有需规范化的文件 |
| `2` | 用法错误 | 路径不存在（`Error::NotFound`）、参数错误 / 找不到分支、关联线或条目（`Error::BadArg`） |
| `3` | I/O 或解析崩溃 | 读写失败等 |

- 错误信息一律走 **stderr**，前缀 `str: `：`str: 路径不存在：/x`、`str: 参数错误：找不到分支 id \`xxx\``。
- **stdout 只放结果**：`norm` / `export` / `context` / `validate --json` / `ls` / `show` / `tree` 的正文都在 stdout，可直接管道给 `jq`。
- 写操作成功时在 stdout 打印一行中文确认，例如 `已新增独立节点 <uuid>（深度 1）`。
- **写命令落盘的 `._meta` 一律已是 §4.9 规范形式**（键序 / 表序 / `entries`·`refs` 排序），因此「改完再 `fmt`」通常无事可做。
- 写命令还会把当前 `(revision, updated_at)` 登记进 `._cache/revisions.json`（bundle 根下的派生数据），这是 `E_REVISION_STALE` 历史判定的基线，见 §4。

## 3. 命令逐条

### 3.1 `str init [--name N] [--title T] [--summary S] [--id-version V] <DIR>`

- `<DIR>` 未以 `.str` 结尾时**自动追加** `.str`（`str init demo` → `demo.str`）。
- 目标已存在则 `Error::BadArg`（exit 2）。
- `--id-version`：`4` 或 `7`（缺省 `7`），写入 `policies.id_version`，并决定工具**后续生成**的 UUID 版本（`node add` / `branch add` 都读该策略）；其它取值直接拒绝（exit 2）。
- 生成：ROOT `._meta` + `._schema/`（写入 **3** 份 Schema：`root-meta` / `node-meta` / `branch-meta`）。
- 生成的 ROOT `._meta` 特点：`kind = "root"`、`revision = 1`、`tags = []`、**没有 `[[authors]]`**、`[policies]` 全为默认值（`id_version` = 你传入的值、`max_depth = 32`、`manifest = "strict"`、`sha256 = "required"`、`large_asset_bytes = 10485760`、`deep_tree_warn = 16`），并登记 `._schema` 为 `role = "schema"`（带 `count` 与 `note`）。
- 输出：

```
已创建 bundle：demo.str
  spec = 1.9.0  str = 1
  policies.id_version = 7
  ._schema 内已写入 3 份校验 Schema
```

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
- `--fix-manifest` **真正写盘**：先执行一次完整的 `str sync <dir>`（非 dry-run），再校验，因此「有未登记文件」这类问题会被就地修好。仍建议单独跑 `sync` 以便看到变更清单。
- **不检查书写顺序**：键序 / 表序 / `entries` 排序由 `str fmt --check` 兜底（§3.15），`validate` 只报错误码。

### 3.3 `str tree [--depth N] [--show-refs] [--ascii] <DIR>`

```
demo.str
├─ [1] 客户A  (crm.customer)
│  ⇢ 关联: <target-uuid>  --related--
│  └─ [1] 跟进记录  (crm.followup_log)
└─ [2] 标签体系  (crm.tag)
```

- `[n]` 是同级序号，括号内是 `type`。
- **子分支的呈现顺序 = 父级 `entries[]` 的 `(order, path)` 顺序**（与落盘顺序同源）：有 `order` 的按 `order` 升序在前，无 `order` 的在末尾按目录名/`path` 字典序；`entries[]` 里没登记的子目录附加在最后并标记。
- 关联线**只在 `--show-refs` 时输出**，且挂在**源分支**下方（未解析的目标显示为 `<uuid>（未解析）`）。
- `--ascii`：改用纯 ASCII 制表符（`|-- ` / `` `-- `` / `|   `，关联线箭头 `->`），便于字体缺字或纯文本环境。

### 3.4 `str ls [--raw] <DIR> [UUID]`

- 分支 id 既可作为**位置参数**，也可用 `--uuid <UUID>`（二者等价且互斥，规范 §9 用位置参数）。缺省列 ROOT。走清单（`._meta.entries`）：

```
01a09bf6-…-62b191  （清单）
  01a09bf6-…-5eba5  node  客户A
  profile.json                 payload  19B
  ._schema                     schema  bundle 级校验 Schema 存放处
```

第二列是 `role`，第三列是 `title`（若有），其后是 `size`（`19B`）与 `(optional)`。
**`path` 列就是子项名；对 `node`/`branch` 而言即子分支 UUID** —— 这是获取 UUID 的标准途径。

- `--raw` 直接读磁盘：首行 `<rel>  （磁盘原始）`，随后每行 `  - <文件>` / `  d <目录>`。

### 3.5 `str show [--full] <DIR> [UUID]`

打印该分支 `._meta` 的**归一化 JSON**（含 `ext: {}`、空表补 `[]`）。`[UUID]` 缺省为 ROOT。

`--full` 会在 JSON 之后追加各 `payload` / `asset` 的正文，形如：

```
── payload.txt ──
hello
```

### 3.6 `str node add [--type T] [--title X] [--summary S] <DIR>`

新增**独立节点**（深度 1，`kind = "node"`）：生成 UUID（版本取 ROOT 的 `policies.id_version`）目录 + `._meta`，并 `upsert` 进 ROOT 的 `entries[]`（`role = "node"`）。stdout 打印新 UUID。ROOT 的 `revision` 会 `+1`。

### 3.7 `str branch add [--type T] [--title X] [--summary S] [--order N] <DIR> [ANCHOR]`

在锚点分支下新增**关联分支**（`kind = "branch"`，深度 ≥ 2）。
锚点必须是 `node`/`branch`（深度 ≥ 1）；**`[ANCHOR]` 缺省为 ROOT**，而 ROOT 不是合法锚点，故省略时（与显式传 ROOT 一样）报 `参数错误：ROOT 的直接子分支应使用 \`str node add\`（role = node）；\`branch add\` 的锚点须是深度 ≥ 1 的分支`（exit 2）。`--order` 缺省为同级最大 `order + 1`。

### 3.8 `str branch rm --force [--recursive] <DIR> [UUID]`

- **删除总是递归**（`remove_dir_all`，含全部下级）；`--recursive` 为兼容规范 §9 的写法而接受，与缺省行为一致。
- 不加 `--force` 会以 exit 2 拒绝（提示会移除全部下级）；**`[UUID]` 缺省为 ROOT，而 ROOT 不可删除** —— 省略时以 `参数错误：不能删除 ROOT（\`<UUID>\` 缺省即为 ROOT，请显式给出要删除的分支 id）` 拒绝。
- 删除后会自动从父级 `entries[]` 移除该条目，父级 `revision + 1`；同时重扫一遍以清理该分支在 `._cache/revisions.json` 中的基线条目。

### 3.9 `str ref add --target <TARGET> [--rel R] [--title X] [--note N] <DIR> [UUID]`

在**源分支** `[UUID]`（缺省为 ROOT —— ROOT 持有 `refs[]` 是合法的）的 `._meta.refs[]` 追加一条关联线。`--rel` 缺省 `related`。
`[UUID]` 与 `<TARGET>` 都必须能在本 bundle 内解析，否则 exit 2。新 `refs[].id` 为 UUIDv7。

合法 `rel`（Schema 正则）：`related`、`depends_on`、`instance_of`、`derived_from`、`ref`、`x-<小写字母数字连字符>`。

### 3.10 `str ref rm <DIR> <REF_ID> [--uuid <SRC>] [--ref <REF_ID>]`

**`REF_ID` 是位置参数**（规范 §9 形式）。删除 `refs[]` 中 `id == REF_ID` 的关联线：

- 不给 `--uuid` 时，CLI 在**整棵分支树**上查找该关联线并定位其源分支；若在多处出现（UUID 冲突，实际不会发生）会要求用 `--uuid` 明确指定；
- `--ref <REF_ID>` 是同一位置参数的选项别名（旧写法），与位置参数互斥；
- 找不到则 exit 2。

### 3.11 `str meta set [--type T] [--title X] [--summary S] [--name N] [--tags a,b] <DIR> [UUID]`

写入**分支自身**（`._meta` 顶层）的元信息字段，缺省目标为 ROOT。
**空串表示移除该字段**；一个字段都不给会以 exit 2 拒绝（避免「空写」白白推进 `revision`）。`--tags` 逗号分隔且可重复，`--tags ""` 清空。
每次写入同样 `revision + 1` 并刷新 `updated_at`。

典型用途：`str sync` 把「含 `._meta` 的子目录」补登为分支后，用本命令补齐该分支的 `type` / `summary` —— 从而消掉 `W_NO_TYPE` / `W_NO_SUMMARY`。

### 3.12 `str entry set --path <P> [--type T] [--title X] [--summary S] [--note N] [--order N] <DIR> [UUID]`

写入**某分支** `entries[]` 中 `path == P` 那条目的字段；`[UUID]` 是**条目所在的分支**（缺省 ROOT）。空串表示移除该字段；找不到该 `path` 或一个字段都不给则 exit 2。

典型用途：补 `str sync` 补登行的 `type` / `title` / `summary`，或调整同层排序键 `--order`（改完落盘的条目顺序随之变化）。

> `meta set` 与 `entry set` **只能改字段值，不能造结构**：`path` / `role` / `id` / `kind` / `refs` 结构仍归 `node add` / `branch add` / `ref add` / `sync` 所有。

### 3.13 `str author add --id <ID> --role <R> [--name N] [--at T] <DIR> [UUID]` · `str author rm --id <ID> <DIR> [UUID]`

- `add` 按 `id` 去重（存在即覆盖），`--at` 缺省为当前时间（UTC，写成 TOML 原生 offset date-time）。
- `--role` 只接受 `owner` / `editor` / `viewer` / `agent`，其它值 exit 2；`--at` 必须带时区偏移，否则 exit 2。
- `rm` 按 `id` 删除，找不到则 exit 2。

### 3.14 `str sync [--dry-run] <DIR>`

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
- 补登的条目**不带** `title` / `summary` / `type` —— 用 §3.11 / §3.12 补齐。
- 非 dry-run 时会校准 `._cache/revisions.json` 基线：**只在该分支 `revision` 确实前进时推进**，因此绕过 CLI 的改动不会被「洗白」（见 §4）。

### 3.15 `str fmt [--check] [--strip-comments] <DIR>`

按规范 §4.9 重写 `._meta`：顶层裸键序、表序（`policies → authors → refs → entries → ext`）、各表内键序，以及 `entries` / `refs` 的 `(order, path|id)` 排序。

- 只调整**书写顺序**，不改值；注释随其所属键/表一起搬运，**不丢**。
- `--check`：`0` = 全部已规范；`1` = 有 N 份需要规范化（可直接当 CI 门禁）。
- `--strip-comments`：丢弃整行 `#` 注释（唯一允许丢注释的入口）。
- `fmt` **不修改** `revision` / `updated_at`。
- 幂等：连续执行第二次必定 `0`。

### 3.16 `str spec set <DIR> <VERSION> [--dry-run]`

把**整份 bundle** 的 `._meta.spec`（规范版本声明）统一改写为 `<VERSION>`。v1.9.0 新增：在此之前 `spec` 是唯一没有 CLI 写入路径的字段，bump 规范版本只能手改 `._meta`。

- **作用范围是整份 bundle**：`spec` 在 root / node / branch 三种档位里都是必填字段（§4.3），只改 ROOT 会让其余分支的声明与 ROOT 不一致 —— 文档就在说谎。子 bundle（目录名以 `.str` 结尾）是硬边界，不进入。
- **只改写有差异的分支 ⇒ 幂等**：第二次执行输出 `全部 `._meta` 的 `spec` 已是 1.9.0（6 份）`，一个字节都不写（`revision` / `updated_at` 同样不动）。
- `--dry-run` 只列出 `~ <rel>  spec: 1.9.0 → 1.8.0` 形式的计划，不落盘。
- 版本串必须是 `1.<minor>.<patch>`（与 `._schema` 的正则同源），可带 `v` 前缀；`2.0.0` / `1.9` / 空串一律 exit 2。
- 目标版本与本实现对应的规范版本不同时多打一行提示：`spec` 只供人类追溯，工具只强校验 `str` 主版本（规范 §13），因此**前向声明与降级声明都被允许**。
- **不写一半**：执行前先整体体检，任一份 `._meta` 解析失败就直接 exit 2，不做部分改写。
- 每个被改写的分支都会 `revision + 1` 并刷新 `updated_at`，同时推进 `._cache/revisions.json` 基线。

```sh
$S spec set demo.str 1.9.0                  # 幂等；已是目标版本时输出「已是 …（N 份）」
$S spec set demo.str --dry-run v1.10.0      # 只看计划（`v` 前缀会被规整掉）
$S spec set demo.str 9.9.9                  # exit 2：major 必须是 1
```

### 3.17 `str norm [--out <PATH|->] <DIR> [UUID]`

打印归一化 JSON（2 空格缩进，含 `ext: {}`、空表补 `[]`）。缺省为 ROOT。
`--out -`（或省略）写 stdout；给路径则写文件。

### 3.18 `str context <DIR> [UUID] [--depth N] [--budget C]`

输出 Markdown 上下文片段，下级分支按 `entries[]` 的 `(order, path)` 展开：

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

`--depth` 缺省 `2`，`--budget` 缺省 `8000`（字符）。`[UUID]` 缺省为 ROOT（即从 ROOT 起按深度展开整棵树）。超预算时截断并追加 `…（已按 --budget 截断）`。

### 3.19 `str export [--format json|toml] [--out <PATH|->] [--depth N] <DIR>`

只读导出整棵树：`{"path", "depth", "meta", "children": [...]}` 递归结构（子节点同样按 `(order, path)` 排列）。缺省 `--format json`。
`--out -`（或省略）写 stdout。产物是派生数据，**`--out` 若指向 bundle 内部会被拒绝（exit 2）**；`--format` 只接受 `json` / `toml`，其它值 exit 2。

> `--format toml` 用的是简易 JSON→TOML 序列化，仅供人读，不是 §4.9 的规范形式。

### 3.20 `str reveal <DIR>`

macOS：`SetFile -a B` 设 bundle 位 + `chflags nohidden` 取消各 `._meta` 的隐藏标记。缺少 `SetFile`（需 Xcode CLT）时只打印手动命令，不算失败。非 macOS 打印提示。

### 3.21 `str codes`

列出全部 **35** 个错误码（24 个 `E_*` + 11 个 `W_*`，含 `W_MANIFEST_*` 三个 advisory 变体）。规范 §9 未列此命令。

## 4. `E_REVISION_STALE` 与 `._cache/revisions.json`

规范 §6.1.1 的这条判定是**历史相关**的：单份 `._meta` 只含当前 `(revision, updated_at)`，没有「上一版」。实现做法：

1. **写入端**：每个写命令成功落盘后，把该分支的 `(revision, updated_at)` 登记进 `._cache/revisions.json`；
2. **校验端**：`str validate` 读基线，`updated_at` 变了而 `revision` 未前进 → `E_REVISION_STALE`；
3. **`str sync` / `branch rm` 只在 `revision` 确实前进时推进基线**（并清理已消失分支的条目）—— 违规因此跨 `sync` 持续可见；
4. **无基线则跳过**：从未被 CLI 写过的 bundle 该条件不参与判定，不误报。

该文件是派生数据：不进 `entries` 清单（`._cache` 是保留名）、`.gitignore` 已排除、删掉即关闭这项历史检查。

```
$ str sync demo.str && str validate demo.str --strict     # 基线建立，0 errors
$ sed -i '' 's/^updated_at = .*/updated_at = 2030-01-01T00:00:00+00:00/' demo.str/<uuid>/._meta
$ str validate demo.str --strict                           # ✗ E_REVISION_STALE：updated_at 变了但 revision 未前进
$ str sync demo.str && str validate demo.str --strict       # 仍是 ✗（sync 不会替它洗白）
$ # 把 revision +1 之后才恢复 0 errors
```

## 5. 已知限制

命令面与规范 §9 **已逐条对齐**（含参数形态）；下面列的是设计上的限制与平台差异，不是实现缺口。

| 项 | 现状 | 应对 |
| --- | --- | --- |
| `str validate` 不检查书写顺序 | 只报 §6 的错误码，键序/表序/集合顺序不参与判定（规范把这道门禁交给 `fmt --check`） | 用 `str fmt --check`（返回 0）当门禁；写命令本身已保证落盘即规范形式 |
| `E_REVISION_STALE` 依赖基线 | 需要 `._cache/revisions.json` 才生效 | 由写命令与 `str sync` 维护；删掉该目录即关闭此项检查 |
| `str reveal` 依赖 macOS `SetFile` | 缺失只提示，不改退出码 | 手动 `SetFile -a B` |
| `str export --format toml` | 简易序列化，非 §4.9 规范形式 | 需要规范形式请用 `str norm`（单分支）或 `json` |
