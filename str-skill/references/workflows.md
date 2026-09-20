# 任务配方（全部实测）

> 每个配方都给出可直接复制的命令序列与期望结果。`$S` 代表已解析出的 CLI 路径：
> ```sh
> S="$(sh "$(dirname "$0")/../scripts/ensure-str.sh")" || exit 1   # 或 sh str-skill/scripts/ensure-str.sh
> ```
> 文中 `<B>` 代表 bundle 目录（如 `crm.str`）。

---

## 配方 1 · 新建 bundle 并从零建一棵树

```sh
$S init crm --name crm --title "客户运营" --summary "CRM 结构化数据束"
# → 已创建 bundle：crm.str / spec = 1.9.0  str = 1 / policies.id_version = 7 / ._schema 内已写入 3 份校验 Schema

$S node add crm.str --type crm.customer --title "客户A" --summary "示例客户"
# → 已新增独立节点 01a09bf1-0eba-7336-8366-b9499cbb240a（深度 1）

CUST=$($S ls crm.str | awk '{print $1}' | sed -n 2p)   # UUID 从 ls 的 path 列取得
$S branch add crm.str "$CUST" --type crm.followup_log --title "跟进记录" --summary "2026-09 跟进"
# → 已在 01a09bf1-… 下新增关联分支 01a09bf1-32e5-…（深度 2）

$S tree crm.str
# crm.str
# └─ [1] 客户A  (crm.customer)
#    └─ [1] 跟进记录  (crm.followup_log)
```

要点：
- `init` 会自动补 `.str` 后缀，并向 `._schema/` 写入 3 份 Schema；目标已存在会以 exit 2 拒绝。
- `--id-version 4|7`（缺省 `7`）决定 `policies.id_version` **与后续生成的 UUID 版本**；先想清楚再建，事后改策略会让既有目录名对不上（`E_ID_VERSION`）。
- **`node add` 用于深度 1，`branch add` 用于深度 ≥2**；对 ROOT 用 `branch add` 会被拒绝。
- UUID 一律由 CLI 生成，**永远不要自己造目录名**。

## 配方 2 · 新增 payload / asset 并登记

```sh
# 1) 用普通文件工具写内容文件（这不是 ._meta，允许直接写）
printf '%s\n' '{"name":"客户A","tags":["vip"]}' > crm.str/$CUST/profile.json

# 2) 登记 + 刷新指纹
$S sync crm.str --dry-run     # 先预览
# + 01a09bf1-…/profile.json  role=payload
# （dry-run）将更新 1 份 `._meta`

$S sync crm.str
# + 01a09bf1-…/profile.json  role=payload
# 已更新 1 份 `._meta`

# 3) 校验
$S validate crm.str --strict
# crm.str  1 nodes / 0 branches / 3 entries  depth=1
# 0 errors, 0 warnings   exit=0
```

要点：
- **跳过第 2 步是本格式最常见的失败**：文件在磁盘上但未登记 → `manifest = "strict"` 下报 `E_MANIFEST_MISSING`。
- `role` 由扩展名推断：`json/toml/yaml/csv/md/txt` → `payload`，其余 → `asset`；同时填 `media_type`、`size`、`sha256`。
- 只登记文件不需要动 `revision`——`str sync` 自己会 `revision + 1` 并刷新 `updated_at`。
- 也可以让 `$S validate <B> --fix-manifest` 顺手修（它内部真的跑一次 sync）。

## 配方 3 · 用 CLI 补齐描述性字段（不再手改 `._meta`）

`str sync` 只会补 `entries[]` 的行，**不会**写分支自身的 `type` / `title` / `summary`，也不会写新补登行的 `title` / `summary` / `type`。实测（`str sync` 补登一个含 `._meta` 的子目录）：

- 该分支自身的 `._meta` 缺 `type` / `summary` → `W_NO_TYPE` + `W_NO_SUMMARY`（`--strict` 下提升为 error）；
- 父级 `entries[]` 里新补的那行只有 `path` / `role` / `id`，缺 `title` / `type` / `summary` → 不报错，但 `str tree` 中该分支显示为空标题。

**这两处都用 CLI 补齐**：

```sh
# 1) 分支自身：type / title / summary / tags
$S meta set crm.str "$NEW" --type crm.attachment --title "合同扫描件" --summary "2024Q1 合同 PDF" --tags "contract,q1"

# 2) 父级 entries[] 里那一行：title / summary / type / note / order
$S entry set crm.str --path "$NEW" --type crm.attachment --title "合同扫描件" --summary "2024Q1 合同 PDF" --order 3

# 3) 协作者
$S author add crm.str --id "u:frowhy" --name "Frowhy" --role owner
$S author add crm.str --id "u:agent-001" --name "AI Agent" --role agent   # --at 缺省为当前时间

$S validate crm.str --strict   # 0 errors, 0 warnings
```

要点：
- **空串 = 移除字段**：`$S meta set crm.str "$CUST" --title ""` 会删掉 `title`；`--tags ""` 清空标签。
- 一个字段都不给会被拒绝（exit 2）—— 免得「空写」白白推进 `revision`。
- 这些命令**只写字段值，不造结构**：`path` / `role` / `id` / `kind` / `refs` 仍然只能由 `node add` / `branch add` / `ref add` / `sync` 改动。
- 于是**不再需要「手改 `._meta` 的唯一例外」**：一切写入都经 CLI。

## 配方 4 · 跨枝关联线

```sh
$S node add crm.str --type crm.tag --title "标签体系" --summary "标签"
TAG=$($S ls crm.str | awk '{print $1}' | tail -1)

$S ref add crm.str "$CUST" --target "$TAG" --rel related --title "同属客户域"
# → 已新增关联线 01a09bf1-354b-…：<CUST> → <TAG>

$S tree crm.str --show-refs
# crm.str
# ├─ [1] 客户A  (crm.customer)
# │  ⇢ 关联: <TAG>  --related--
# │  └─ [1] 跟进记录  (crm.followup_log)
# └─ [2] 标签体系  (crm.tag)

$S ref rm crm.str 01a09bf1-354b-72a6-a2bc-19c69cb9d504   # 位置参数：源分支由 CLI 定位
$S ref rm crm.str --uuid "$CUST" --ref 01a09bf1-354b-…    # 等价写法：显式指定源分支
```

要点：`rel` 只能是 `related` / `depends_on` / `instance_of` / `derived_from` / `ref` / `x-*`；禁止自关联与成环；**`refs` 不表达父子关系**。

## 配方 5 · 检索与读上下文（渐进披露）

```sh
$S tree crm.str --show-refs          # ① 先看全貌（不含 payload 正文）
$S ls crm.str                        # ② ROOT 清单，拿到一级节点 UUID
$S show crm.str                      # ②' 省略 UUID ⇒ ROOT（规范 §9：`[uuid]` 缺省即 ROOT）
$S ls crm.str "$CUST"                # ③ 单分支清单（等价：--uuid "$CUST"）
$S show crm.str "$CUST"              # ④ 单分支元信息（归一化 JSON）
$S show crm.str "$CUST" --full       # ⑤ 连带 payload 正文（谨慎，会变长）
$S context crm.str "$CUST" --depth 2 --budget 8000   # ⑥ 直接喂给模型
$S norm crm.str "$CUST" | jq '.summary'              # ⑦ 机器可读
$S norm crm.str "$CUST" --out /tmp/cust.json         # ⑧ 落盘（--out - 即 stdout）
$S tree crm.str --ascii              # ⑨ 终端字体缺字时用纯 ASCII 制表符
```

**禁止**用 `cat` / `grep` / `find` / `ls -R` 遍历 bundle 来代替上述命令。

## 配方 6 · 改内容后的收尾（每次写入都必须做）

```sh
$S sync <B>                    # 1. 让 entries[] 与磁盘一致
$S validate <B> --strict       # 2. 必须 0 errors, 0 warnings
$S fmt <B> --check             # 3. 期望「全部 `._meta` 已是规范形式」
```

三条全绿才能声称完成。注意：
- 写命令落盘的 `._meta` 已经是 §4.9 规范形式，所以第 3 步通常直接返回 0；一旦返回 1 说明有人（或别的工具）动过文件，跑 `$S fmt <B>` 修好。
- `validate --fix-manifest` 现在会**真的写盘**（内部跑一次完整 sync），但它只修清单，不替代上面的第 3 步。

## 配方 7 · 删分支

```sh
$S branch rm <B> <UUID>                      # → exit 2，提示会移除全部下级
$S branch rm <B> <UUID> --force               # → 已删除 <rel>
$S branch rm <B> <UUID> --force --recursive   # 同上（--recursive 为兼容规范 §9 而接受）
$S tree <B>                                   # 确认父级 entries 已同步（不会留 ghost）
```

`branch_rm` **总是递归**、总是要求 `--force`；删完父级 `entries[]` 自动修复、`revision + 1`，该分支在 `._cache/revisions.json` 的基线条目也会被清掉。**不要用 `rm -rf`**，否则父级会留 `E_MANIFEST_GHOST`。

## 配方 8 · 修订历史（`E_REVISION_STALE`）

```sh
$S sync <B>                       # 建立/校准基线（._cache/revisions.json）
$S validate <B> --strict          # 0 errors

# 有人绕过 CLI 改了 updated_at 却没推进 revision
$S validate <B> --strict
# ✗ E_REVISION_STALE  <rel>  `updated_at` 已由 … 变为 …，但 `revision` 未前进（3 → 3）

$S sync <B> && $S validate <B> --strict   # 仍报错：sync 不会替你洗白
# 正确修法：把 revision +1（并保留新的 updated_at），再 validate → 0 errors
```

要点：该判定依赖 `._cache/revisions.json`（派生数据，`.gitignore` 已排除）。删掉 `._cache/` 即关闭这项历史检查；从未被 CLI 写过的 bundle 不参与判定，因此不会误报。

## 配方 9 · 排障对照表

| 症状 | 错误码 | 动作 |
| --- | --- | --- |
| 写了文件但校验失败 | `E_MANIFEST_MISSING` | `$S sync <B>` |
| 删了目录但校验失败 | `E_MANIFEST_GHOST` | `$S sync <B>`（或改用 `branch rm --force`） |
| 改了 payload 内容 | `E_MANIFEST_HASH` | `$S sync <B>` 刷新指纹 |
| 分支目录被手工改名 | `E_ID_MISMATCH` / `E_ID_NOT_UUID` | 目录名改回，或用 CLI 新建后迁移内容 |
| 子目录加了 `._meta` 但父级没改 role | `E_ENTRY_ROLE_DEPTH` | 用 `$S sync <B>` 让其自动补 role；或手工把父级 `entries[].role` 改为 `branch` |
| `_meta` 里多了自造字段 | `E_SCHEMA_FIELD` | 删掉；扩展只能写进 `[ext]` 且键名为 `vendor.xxx` |
| 时间写成字符串 / 缺时区 | `E_PARSE` | 改为 TOML 原生 offset date-time，带偏移 |
| 缺少 `type` / `summary` | `W_NO_TYPE` / `W_NO_SUMMARY` | `$S meta set <B> [UUID] --type … --summary …` |
| 补登行没有标题 | — | `$S entry set <B> [UUID] --path <P> --title … --summary … --type …` |
| `updated_at` 变了但 revision 没动 | `E_REVISION_STALE` | 让写入方 `revision + 1`；见配方 8 |
| 根目录名没以 `.str` 结尾 | `W_BUNDLE_SUFFIX` | 重命名根目录（或在 CI 里接受该告警） |
| 出现业务点文件/目录 | `W_DOTFILE` / `E_RESERVED_NAME` | `._` 是格式保留命名空间，业务内容改名 |
| `fmt` 说需要规范化 | — | `$S fmt <B>`；顺序由写命令与 `fmt` 共同收口 |
| 想用 `--out` / `--ascii` / `--recursive` / `--id-version` | — | **都已支持**，见 `cli-reference.md` §3 |
