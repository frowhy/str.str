# 变更日志 · `str-format`（CLI / 库）

crate `str-format` 的版本变更 —— 可执行文件名 `str`，另含可复用的纯 Rust 库。
版本号真源见 [`../VERSIONS.toml`](../VERSIONS.toml)；本 crate 的版本是**独立于规范版本**的一条轴
（`0.x` 阶段允许非兼容改动，1.0.0 之后按常规 semver）。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，日期为 Asia/Shanghai。
发行动作由 `.github/workflows/publish.yml`（crates.io）与 `release.yml`（五平台二进制）承担，
两者都由 `v*` tag 触发，且强制 `tag == Cargo.toml version`。

---

## [0.9.0] — 2026-10-01（工作区，未发布）

对应规范 **v1.16.0**。

### 修复

- **`str sync` 不再移除软连接条目（现场缺陷）**：GUI 挂载产生的 `role = "link"` 条目
  是声明式挂载（`path` = 目标分支 id，位于 bundle 其它位置），此前 `sync` 按「磁盘无
  对应目录」将其当作已消失条目移除 —— 现场一次 `sync` 静默清除 86 条且 `validate`
  前后均 0 errors 0 warnings。现按规范 §4.6.1 规则 1 豁免：软连接行（非 `hard`）
  永不移除；硬链接目录真实存在，照常对账。

### 变更

- **`sync` 删除类变更告警 + 批量删除门禁**：移除条目数 > 0 时输出 `WARN:` 汇总行
  （`--dry-run` 亦然）；移除条目数 ≥ 10 且未带 `--yes` 时拒绝执行、一字不写
  （exit 1，指路 `--dry-run` / `--yes`）。新增 `sync --yes` flag；
  `validate --fix-manifest` 属显式修复意图、豁免门禁。
- 新增测试 `tests/sync_links.rs`（软连接不被 sync 删除 / 普通 path 对账不变 /
  门禁拒绝与放行）。

---

## [0.8.1] — 2026-10-01

对应规范 **v1.15.0**（无规范变更）。

### 变更

- 版本号随发行 `v0.8.1` 前进：**本 crate 无代码变更**（发行 tag 锚定 CLI 版本，GUI 0.5.2
  的平台编译回归修复需要一个新 tag 才能重新进入发布流水线）。`str --version` 现在输出
  `str 0.8.1`。

---

## [0.8.0] — 2026-09-29

对应规范 **v1.15.0**（命令面补齐：查询命令）。

### 变更

- **撤回硬链接「不得挂载自己的后代」校验（`link add` 与 `validate`）**：硬链接
  不渲染目标结构、内容只读，目标位于挂载点子树内不产生自嵌套；原规则使挂载点
  为 ROOT 时一切目标皆被禁（ROOT 无法挂硬链接）。`E_LINK_TARGET_INVALID` 保留
  「目标为 ROOT / 缺 `id` / `path ≠ id`」三类判定；成环仍由 `E_LINK_CYCLE` 把关。

### 新增

- **查询命令五件套（「定位 → 精读」链路，全部只读）**：
  - `str find [dir] [query]` —— 元数据检索：在分支自身字段（`title`/`summary`/`type`/`tags`）
    与实体条目行字段（`title`/`summary`/`note`/`path`）中做不区分大小写的字面子串匹配，
    或按 `--type` / `--tag`（交集）纯过滤；`--field` 限定检索字段、`--depth` 限深、
    `--limit` 限量、`--json` 机器可读（含 `matched` 数组）。命中按 `(depth, rel)` 排序；
    结构类条目行（`node`/`branch`/`link`）不参与条目字段检索（子分支会被单独访问）。
  - **检索范围（find / grep 共用）**：缺省 = **当前节点子树**（§9 `[uuid]` 缺省规则的
    沿用 —— `[dir]` 为 bundle 根即全 bundle，指向分支目录即只搜该分支及其全部后代）；
    `--scope <uuid|rel>` 可从任意位置显式指定（分支 id 或 bundle 相对路径，
    无法解析 → `BadArg`，不静默回退全 bundle）。
  - `str grep <PATTERN> [dir]` —— 正文全文检索：字面子串（非正则），
    `--ignore-case` / `--glob`（通配分支内相对路径）/ `--limit` / `--json`；二进制（含 NUL）
    与非 UTF-8 文件跳过，命中行截断到 200 字符，按分支分组输出并附所属分支上下文。
    **缺省遍历分支目录磁盘上的全部文本文件** —— 覆盖内容文件夹（`role = "dir"`）
    的未登记子项与未登记散落文件（命中带 `registered` 标注；JSON 恒含 `file`
    绝对路径），`--manifest-only` 退回「仅清单登记条目」口径；遍历不进入其它
    分支目录 / 子 bundle，跳过保留名 / 系统噪声 / `.lock`，并应用忽略名单
    （`.gitignore` + `policies.ignore`，与校验同源）。
  - `str tags [dir]` —— 全 bundle 分支级标签词表 + 计数（次数降序，再按名升序）。
  - `str where [dir] [uuid]` —— 面包屑定位：从 ROOT 到目标分支的完整链条
    （相对路径 + id + 标题 + `type`，目标行标注 `← 目标`）。
  - `str get [dir] [uuid] --path <P>` —— 精读单个**已登记**实体条目的正文（字节直出
    stdout）；`--path` 亦接受**多段路径**直读已登记内容目录内的未登记子项
    （`--info` 以 `registered: false` 标注、`role` 按扩展名推断、`size` / `sha256`
    按磁盘实算）。子分支 / 链接条目与其余未登记路径以 `BadArg` 拒绝并指路
    （`show` / 对目标再 `get` / `ls` / 先 `sync` 补登）。
  - `str find` / `str grep` 新增 `--real-path`：文本输出切换为**绝对路径**
    （JSON 恒含 `path_abs` / `file` 绝对路径字段），便于直接管道给其它命令。
- 五者均不推进 `revision`、不写任何 `._meta`；收集逻辑与打印层分离
  （`find_hits` / `grep_hits` / `tag_counts` / `where_json` / `get_bytes` / `get_info`
  为公开库 API，供测试与嵌入方复用）。新增集成测试 `tests/query.rs`（14 例，
  覆盖未登记子项检索 / 忽略名单剪枝 / 分支遍历边界 / 绝对路径输出）。

---

## [0.7.2] — 2026-09-23

对应规范 **v1.13.0**（无规范变更）。

### 变更

- 版本号随发行 `v0.7.2` 前进：**本 crate 无代码变更**（发行 tag 锚定 CLI 版本，GUI 0.3.2
  需要一个新 tag 才能发布）。`str --version` 现在输出 `str 0.7.2`。

---

## [0.7.1] — 2026-09-22

对应规范 **v1.13.0**（无规范变更）。

### 变更

- 版本号随发行 `v0.7.1` 前进：**本 crate 无代码变更**（发行 tag 锚定 CLI 版本，GUI 0.3.0
  需要一个新 tag 才能发布）。`str --version` 现在输出 `str 0.7.1`。

---

## [0.7.0] — 2026-09-19

对应规范 **v1.13.0**（命令面补齐）。

### 变更

- **`str entry add|rm`（实体登记闭环）**：`entry add` 向目标分支 `entries[]` 登记实体条目，
  自动补 `size` / `sha256` / `count` / `media_type`，`role` 缺省按磁盘对象推断；
  磁盘缺失须 `--optional` 占位；`entry rm` 只移除登记、**不删除磁盘文件**。
- **`str ignore add|rm|list`**：维护 ROOT `policies.ignore`（`add` 幂等；`list` 附带
  `policies.gitignore` 状态与检测到的 `.gitignore` 来源）。
- **`str policies set <KEY> <VALUE>`**：`[policies]` 标量键的 CLI 写入路径
  （含取值校验；`ignore` 数组键指引用 `str ignore add|rm`）—— 「任何字段都必须有
  CLI 写入路径」彻底闭合。
- `str --version` 现在输出 `str 0.7.0`。

---

## [0.6.0] — 2026-09-18

对应规范 **v1.13.0**。

### 变更

- **忽略名单（`policies.ignore` + `.gitignore` 自动检测）**：`[policies]` 新增 `ignore`
  （字符串数组，gitignore 语义：`*` / `**` / `!` 取反 / 尾随 `/` 仅目录 / 含 `/` 锚定，
  匹配 bundle 内相对路径）与 `gitignore`（布尔，默认 `true` = 自动检测并应用
  `.gitignore`，检测范围：外层 git 仓库（至 worktree 根）→ bundle 根 → 分支目录内，
  由外向内叠加、内层命中覆盖外层，`policies.ignore` 恒为最内层）。
  被忽略条目在分支遍历中被剪枝、不参与清单比对（不报 `E_MANIFEST_MISSING` /
  `W_DOTFILE`）、`str sync` 不补登；**已显式登记的条目不受影响**（登记仍强制、指纹仍校验）。
  **系统级忽略层**：`._meta` / `._schema/` / `._cache/` / `._` 保留命名空间 / `.lock` /
  OS 与 VCS 元数据恒为最内层、常开，用户 `!` 取反不能恢复（§3.4 豁免由该层统一实现）。
  实现新增 `ignore` 模块（globset 驱动），`Scan` 携带生效的 `IgnoreSet`（GUI 复用同一套语义）。
- `SPEC_VERSION` 1.12.0 → **1.13.0**；`str --version` 现在输出 `str 0.6.0`。

---

## [0.5.1] — 2026-09-16

对应规范 **v1.12.0**。

### 变更

- **`str init` 不再登记 `._schema`**：保留目录免登记（规范 §1.3 约束 5 / §4.8），生成的 ROOT
  `._meta` 不再含 `[[entries]] path = "._schema"`；`._schema/` 目录与三份 Schema 仍照常写入。
- `render_root_meta` 去掉仅服务该条目的 `schema_count` 形参（`0.x` 阶段允许非兼容改动）。
- `SPEC_VERSION` 1.11.0 → **1.12.0**；`str --version` 现在输出 `str 0.5.1`。

---

## [0.5.0] — 2026-09-14

对应规范 **v1.11.0**。

### 变更

- **`[uuid]` 缺省目标从 ROOT 细化为「当前节点」**：`[dir]` 为 bundle 根时省略 `<uuid>` 仍为 ROOT
  （v1.9.0 行为不变）；`[dir]` 指向 bundle 内某分支目录（或其内部子目录）时，省略 `<uuid>` 的
  `show` / `ls` / `context` / `norm` / `meta set` / `entry set` / `author add|rm` / `ref add` /
  `branch add` / `branch rm` 全部以**该分支**为目标 —— 在分支目录内执行
  `str branch add --title SUB` 即挂到当前分支下，`str branch rm --force` 即删除当前分支本身
  （父级 `entries[]` 由全树扫描同步修复）。实现上 `open()` 会向上解析真正的 bundle 根
  （`.str` 硬边界不被穿越），显式给出 `[uuid]` 的旧调用不受影响（规范 §9 条文，DoD 第 27 项）。
- **`node add` 在分支目录下执行 → 带原因拒绝**：独立节点只能挂 ROOT，报错指引改用
  `branch add`（避免「在分支里执行 node add 却把节点加到了 ROOT」的静默意外）。
- `SPEC_VERSION` 1.10.0 → **1.11.0**；`str --version` 现在输出 `str 0.5.0`。

---

## [Unreleased] — 0.4.0

对应规范 **v1.10.0**。

### 变更

- **`[dir]` 位置参数统一「省略即当前目录」**：全部子命令的 bundle 目录位置参数改为可省略
  （`str <cmd>` 等价于 `str <cmd> .`）。`init` 是唯一例外：省略时以当前路径为基准目标，仍按
  「未以 `.str` 结尾则追加」定名 —— 在 `foo/` 里执行 `str init` 创建的是**兄弟目录** `foo.str`。
  规则是**放宽**，显式路径的旧调用全部仍合法（规范 §9 条文，DoD 第 26 项）。
- **`spec set` 参数顺序调整为 `<VERSION> [dir]`**：可选位置参数不得排在必填位置参数之前
  （clap 的硬约束），故随 `<dir>` 缺省化连带调整；沿用旧顺序 `spec set <dir> <VERSION>` 的调用
  会在版本串校验处被拒并提示新顺序（该命令自 v1.9.0 才引入，兼容代价极小）。
- `SPEC_VERSION` 1.9.0 → **1.10.0**；`str --version` 现在输出 `str 0.4.0`。

### 新增

- `tests/dir_default.rs`：二进制级集成测试，覆盖 `[dir]` 缺省（读类 / 写类 / 门禁命令 +
  `init` 的基准目标语义 + 在 `.str` 目录中重复创建被拒）。

---

## [0.3.0] — 2026-09-14

对应规范 **v1.9.0**。

### 新增

- **`str spec set <dir> <VERSION>`**：把整份 bundle 的 `spec`（规范版本声明）递归改写为目标版本
  —— 子 bundle 除外（§3.5），只改有差异的分支（**幂等**，第二次输出「已是 …（N 份）」），
  任一份 `._meta` 解析失败即**整体拒绝**、不写出部分结果；目标版本须形如 `1.<minor>.<patch>`。
  这是此前**唯一只能手改**的字段，至此「不得手改 `._meta`」不再有例外（规范 DoD 第 25 项）。

### 变更

- **`str tree` 根节点与子节点同样式**：根行由裸 bundle 目录名改为 `[0] 标题  (type)`
  （标题依次取根 `._meta` 的 `title` → `name` → 目录名；根无 `type` 时以档位 `root` 兜底），
  不再输出首行 bundle 目录名，解析失败时追加 `! 解析失败`。
- **`str tree` 新增 `--show-entries`**：在分支行下方以 `· path  (role)  标题  字节数`
  列出该分支 `entries[]` 内容清单（保持落盘次序）；`node` / `branch` 结构行由树本身呈现，
  不重复输出。
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
