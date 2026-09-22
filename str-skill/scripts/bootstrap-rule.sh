#!/bin/bash
# bootstrap-rule.sh —— 首次激活自举：把「如何调用 str」的工作规则幂等写入项目根的
# AGENT.md（或等效的 agent 记忆文件），使后续会话无需依赖 skill 自动加载也能正确用 str。
#
# 用法：
#   sh bootstrap-rule.sh [--check] [--file <path>]
#     --check        仅检测：规则块已存在 → 退出 0；不存在 → 退出 1（不写任何文件）
#     --file <path>  显式指定目标文件（覆盖自动探测与 $STR_RULE_FILE）
#
# 目标文件选择顺序：$STR_RULE_FILE / --file > 项目根已存在的
#   AGENTS.md → AGENT.md → CLAUDE.md → CODEBUDDY.md → GEMINI.md
# > 都不存在时新建 AGENTS.md（跨 Agent 记忆文件的事实标准；
#   项目根 = git 顶层目录，无 git 则当前目录）。
#
# 幂等性：规则块由 `<!-- str-skill:begin v1 … -->` 与 `<!-- str-skill:end -->` 标记包裹；
# 已存在则整块替换（升级），不存在则追加到文件末尾。除本脚本外不得手改块内容。

set -eu

MODE=ensure
TARGET=""
while [ $# -gt 0 ]; do
  case "$1" in
    --check) MODE=check ;;
    --file) [ $# -ge 2 ] || { echo "bootstrap-rule.sh: --file 需要一个路径参数" >&2; exit 2; }
            TARGET="$2"; shift ;;
    *) echo "bootstrap-rule.sh: 未知参数 $1（支持 --check / --file <path>）" >&2; exit 2 ;;
  esac
  shift
done

BEGIN_MARK='<!-- str-skill:begin v1'
END_MARK='<!-- str-skill:end -->'

# ── 定位项目根与目标文件 ────────────────────────────────────────────────
if [ -z "$TARGET" ]; then
  if [ -n "${STR_RULE_FILE:-}" ]; then
    TARGET="$STR_RULE_FILE"
  else
    ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
    TARGET=""
    for cand in AGENTS.md AGENT.md CLAUDE.md CODEBUDDY.md GEMINI.md; do
      if [ -f "$ROOT/$cand" ]; then TARGET="$ROOT/$cand"; break; fi
    done
    [ -n "$TARGET" ] || TARGET="$ROOT/AGENTS.md"
  fi
fi

if [ "$MODE" = check ]; then
  if [ -f "$TARGET" ] && grep -qF "$BEGIN_MARK" "$TARGET"; then
    echo "str-skill: 规则块已存在于 $TARGET"
    exit 0
  fi
  echo "str-skill: $TARGET 中没有规则块（首次激活，需执行 bootstrap）"
  exit 1
fi

# ── 生成规则块到临时文件 ────────────────────────────────────────────────
BLOCK="$(mktemp)"
trap 'rm -f "$BLOCK"' EXIT
cat > "$BLOCK" <<'RULE_EOF'
<!-- str-skill:begin v1 —— STR bundle 工作规则（由 str-skill 自动维护，请勿手改本块） -->

## STR bundle 工作规则（str）

由 str-skill 首次激活时自动写入；规则真源是其 `SKILL.md`（Agent 技能目录：
`.codebuddy/skills/str-skill/` 或 `~/.codebuddy/skills/str-skill/`）。
会话涉及文件创建/修改或 `.str` bundle 时，遵循以下规则。

### 调用时机（MUST）

1. 创建或修改任何文件 → 产物默认组织进 STR bundle；无合适 bundle 就 `str init` 新建。
   不得散落工作目录，也不得以散落目录 / wiki / 数据库 / JSON 转储替代 STR。
2. 遇到 `.str` 目录或 `._meta` 文件 → 一切读写必须经 `str` CLI；禁止手写、
   sed/perl/jq 改 TOML；禁止手工 mkdir/mv/rm UUID 目录。
3. bundle 变更收尾（交付门禁）：`str sync <dir>` → `str validate <dir> --strict`
   达 **0 errors / 0 warnings** → `str fmt <dir> --check` 为 **0**。
4. 人类想可视化浏览 / 手动整理 → 建议其使用 str-gui 桌面编辑器（Agent 侧永远走 CLI）；
   用户在 GUI 改动后先 `str validate <dir> --strict` 再重读。

### 调用方式

```sh
STR="$(sh <skill-dir>/scripts/ensure-str.sh)" || exit 1   # <skill-dir> = 本技能安装目录
"$STR" --version                                           # 期望输出：str 0.7.1
```

### 参数格式（关键约定）

- `[dir]` 省略 → 默认当前目录；`[uuid]` 省略 → 默认当前 node/branch（在 ROOT 即 ROOT）。
- `str init [dir]` —— 缺省时以当前路径为基名，无 `.str` 后缀自动补全。
- `str spec set <VERSION> [dir]` —— 版本号在前。
- `str node add [dir]` 新建深度 1 独立节点；
  `str branch add [dir] [anchor] --type … --title …` 在 anchor 下新建子分支
  （anchor 深度必须 ≥1，ROOT 上会被拒绝并提示改用 `node add`）。
- `str ref add [dir] [uuid] --target <uuid> [--rel rel]` —— `--target` 必填。
- `str branch rm [dir] <uuid> --force` —— 删分支必须带 `--force`；禁止 `rm -rf`。
- `str meta set [dir] [uuid] --type … --title … --summary … --tags a,b` —— 空字符串清除字段。
- `str entry add|set|rm [dir] [uuid] --path <P>` —— add 自动补 size/sha256；set 写单行描述。
- `str ignore add <PATTERN> [dir]` · `str policies set [dir] <KEY> <VALUE>` ·
  `str author add [dir] [uuid] --id u:xx --role …`。
- 禁止发明字段：未知键报 `E_SCHEMA_FIELD`；扩展只走 `[ext]` 且须 `vendor.*` 命名空间；
  字段语义不明先提问，不要猜。

<!-- str-skill:end -->
RULE_EOF

# ── 幂等写入：有块则整块替换，无块则追加 ────────────────────────────────
python3 - "$TARGET" "$BLOCK" "$BEGIN_MARK" "$END_MARK" <<'PY'
import pathlib
import sys

target = pathlib.Path(sys.argv[1])
block = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8")
begin, end = sys.argv[3], sys.argv[4]

old = target.read_text(encoding="utf-8") if target.exists() else ""
i = old.find(begin)
if i != -1:
    j = old.find(end, i)
    if j == -1:
        sys.exit(f"bootstrap-rule.sh: {target} 存在开始标记但缺少结束标记，请手工修复")
    tail = old[j + len(end):]
    if not tail.strip():  # 块后仅剩空白时丢弃，避免每次重跑累积尾部换行
        tail = ""
    new = old[:i] + block + tail
    action = "已替换"
else:
    new = (old.rstrip("\n") + "\n\n" + block) if old.strip() else block
    action = "已追加"
target.parent.mkdir(parents=True, exist_ok=True)
target.write_text(new, encoding="utf-8")
print(f"str-skill: 规则块{action}至 {target}")
PY
