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
# → 已创建 bundle：crm.str / spec = 1.7.0  str = 1 / ._schema 内已写入 3 份校验 Schema

$S node add crm.str --type crm.customer --title "客户A" --summary "示例客户"
# → 已新增独立节点 01a09bf1-0eba-7336-8366-b9499cbb240a（深度 1）

CUST=$($S ls crm.str | awk '/ node /{print $1}')      # UUID 从 ls 的 path 列取得
$S branch add crm.str "$CUST" --type crm.followup_log --title "跟进记录" --summary "2026-09 跟进"
# → 已在 01a09bf1-… 下新增关联分支 01a09bf1-32e5-…（深度 2）

$S tree crm.str
# crm.str
# └─ [1] 客户A  (crm.customer)
#    └─ [1] 跟进记录  (crm.followup_log)
```

要点：
- `init` 会自动补 `.str` 后缀，并向 `._schema/` 写入 3 份 Schema；目标已存在会以 exit 2 拒绝。
- **`node add` 用于深度 1，`branch add` 用于深度 ≥2**；对 ROOT 用 `branch add` 会被拒绝。
- UUID 一律由 CLI 生成 UUIDv7，**永远不要自己造目录名**。

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

## 配方 3 · 补齐 CLI 无法设置的字段（唯一例外流程）

`str sync` 只会补 `entries[]` 的行，**不会**写分支自身的 `type` / `title` / `summary`。实测（`str sync` 补登一个含 `._meta` 的子目录）：

- 该分支自身的 `._meta` 缺 `type` / `summary` → `W_NO_TYPE` + `W_NO_SUMMARY`（`--strict` 下提升为 error）；
- 父级 `entries[]` 里新补的那行只有 `path` / `role` / `id`，缺 `title` / `type` / `summary` → 不报错，但 `str tree` 中该分支显示为空标题。

`v0.1.0` 没有设置这些字段的命令，因此允许**受限例外**：

1. 只在 `._meta` 中**增补** `type` / `title` / `summary`（或 `tags` / `authors[]`），插到 §4.9 规定的位置；
2. 同一次编辑里 `revision` 加 1、并把 `updated_at` 刷成带时区偏移的 TOML 原生时间（如 `2026-09-14T10:03:11+08:00`，**不要加引号**）；
3. 立即 `$S fmt <B>` 与 `$S validate <B> --strict`。

**绝不**用这条例外去改结构字段（`path` / `role` / `id` / `kind` / `refs`）。

## 配方 4 · 跨枝关联线

```sh
$S node add crm.str --type crm.tag --title "标签体系" --summary "标签"
TAG=$($S ls crm.str | awk '/ node /{print $1}' | tail -1)

$S ref add crm.str "$CUST" --target "$TAG" --rel related --title "同属客户域"
# → 已新增关联线 01a09bf1-354b-…：<CUST> → <TAG>

$S tree crm.str --show-refs
# crm.str
# ├─ [1] 客户A  (crm.customer)
# │  ⇢ 关联: <TAG>  --related--
# │  └─ [1] 跟进记录  (crm.followup_log)
# └─ [2] 标签体系  (crm.tag)

$S ref rm crm.str "$CUST" --ref 01a09bf1-354b-72a6-a2bc-19c69cb9d504   # 注意是 --ref，不是位置参数
```

要点：`rel` 只能是 `related` / `depends_on` / `instance_of` / `derived_from` / `ref` / `x-*`；禁止自关联与成环；**`refs` 不表达父子关系**。

## 配方 5 · 检索与读上下文（渐进披露）

```sh
$S tree crm.str --show-refs          # ① 先看全貌（不含 payload 正文）
$S ls crm.str                        # ② ROOT 清单，拿到一级节点 UUID
$S ls crm.str --uuid "$CUST"         # ③ 单分支清单
$S show crm.str "$CUST"              # ④ 单分支元信息（归一化 JSON）
$S show crm.str "$CUST" --full       # ⑤ 连带 payload 正文（谨慎，会变长）
$S context crm.str "$CUST" --depth 2 --budget 8000   # ⑥ 直接喂给模型
$S norm crm.str "$CUST" | jq '.summary'              # ⑦ 机器可读
```

**禁止**用 `cat` / `grep` / `find` / `ls -R` 遍历 bundle 来代替上述命令。

## 配方 6 · 改内容后的收尾（每次写入都必须做）

```sh
$S sync <B>                    # 1. 让 entries[] 与磁盘一致
$S validate <B> --strict       # 2. 必须 0 errors, 0 warnings
$S fmt <B> --check             # 3. 期望「全部 `._meta` 已是规范形式」
```

三条全绿才能声称完成。注意 **`validate --fix-manifest` 不会修清单**（只预览），必须显式 `sync`。

## 配方 7 · 删分支

```sh
$S branch rm <B> <UUID>            # → exit 2，提示会移除全部下级
$S branch rm <B> <UUID> --force    # → 已删除 <rel>
$S tree <B>                        # 确认父级 entries 已同步（不会留 ghost）
```

`branch rm` **总是递归**，没有 `--recursive`；删完的父级 `entries[]` 会自动修复、`revision + 1`。**不要用 `rm -rf`**，否则父级会留 `E_MANIFEST_GHOST`。

## 配方 8 · 排障对照表

| 症状 | 错误码 | 动作 |
| --- | --- | --- |
| 写了文件但校验失败 | `E_MANIFEST_MISSING` | `$S sync <B>` |
| 删了目录但校验失败 | `E_MANIFEST_GHOST` | `$S sync <B>`（或改用 `branch rm --force`） |
| 改了 payload 内容 | `E_MANIFEST_HASH` | `$S sync <B>` 刷新指纹 |
| 分支目录被手工改名 | `E_ID_MISMATCH` / `E_ID_NOT_UUID` | 目录名改回，或用 CLI 新建后迁移内容 |
| 子目录加了 `._meta` 但父级没改 role | `E_ENTRY_ROLE_DEPTH` | 用 `$S sync <B>` 让其自动补 role；或手工把父级 `entries[].role` 改为 `branch` |
| `_meta` 里多了自造字段 | `E_SCHEMA_FIELD` | 删掉；扩展只能写进 `[ext]` 且键名为 `vendor.xxx` |
| 时间写成字符串 / 缺时区 | `E_PARSE` | 改为 TOML 原生 offset date-time，带偏移 |
| 缺少 `type` / `summary` | `W_NO_TYPE` / `W_NO_SUMMARY` | 走配方 3 的例外流程补齐 |
| 根目录名没以 `.str` 结尾 | `W_BUNDLE_SUFFIX` | 重命名根目录（或在 CI 里接受该告警） |
| 出现业务点文件/目录 | `W_DOTFILE` / `E_RESERVED_NAME` | `._` 是格式保留命名空间，业务内容改名 |
| `fmt` 后顺序没变 | — | 已知实现局限，见 `cli-reference.md` §5；不要期待 `fmt` 排序 |
| 想用 `--out` / `--ascii` / `--recursive` | — | 这些参数不存在，见 `cli-reference.md` §4 |
