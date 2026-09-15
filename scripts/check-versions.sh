#!/usr/bin/env bash
# ══════════════════════════════════════════════════════════════════════════════
# 版本一致性门禁（drift gate）
# ══════════════════════════════════════════════════════════════════════════════
#
# `VERSIONS.toml` 是版本唯一真源（SSOT），但版本号在仓库里是**多点真相**：
# 光规范版本就散落在正文头部、仓库根 `._meta`、Rust 常量、Schema description、
# 示例生成脚本、README 等处。本脚本把「真源 ↔ 各声明点」逐点比对，任一处不一致
# 即失败并打印可定位的修复指引 —— 与 `tests/schema_sync.rs` 守卫 Schema 派生副本
# 是同一套思路：真源可以复制，但必须可验证。
#
# 用法：
#   bash scripts/check-versions.sh          # 全量校验，通过 exit 0，失败 exit 1
#
# 在 CI 中还会额外校验（仅在 tag 上下文中生效，本地运行自动跳过）：
#   推送的 tag `GITHUB_REF_NAME` 必须等于 `[release].tag`。
#
# 覆盖范围：**机械可判定的声明点**。散文里顺带提到的历史版本号（修订记录、
# CHANGELOG、迁移示例）刻意不纳入 —— 它们本就应当保留旧版本号。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

SSOT="VERSIONS.toml"
FAIL=0

ok() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
ng() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

# check <描述> <期望> <实际>
check() {
  if [ "$2" = "$3" ]; then
    ok "$1"
  else
    ng "$1（${SSOT} 期望 $2，实际「$3」）"
  fi
}

# check_set <描述> <期望> <实际（换行分隔的集合，已去重排序）>
check_set() {
  if [ "$2" = "$3" ]; then
    ok "$1"
  else
    ng "$1（期望集合 {$2}，实际 {$(printf '%s' "$3" | tr '\n' ' ')}）"
  fi
}

# has <描述> <文件> <必须出现的字面量>
has() {
  if grep -qF -- "$3" "$2"; then
    ok "$1"
  else
    ng "$1（$2 中找不到「$3」）"
  fi
}

# toml_get <文件> <section> <key>
toml_get() {
  awk -v sec="$2" -v key="$3" '
    $0 == "[" sec "]" { insec = 1; next }
    /^\[/ { insec = 0 }
    insec {
      line = $0
      sub(/#.*/, "", line)
      if (index(line, "=") == 0) next
      k = substr(line, 1, index(line, "=") - 1)
      gsub(/[ \t]/, "", k)
      if (k != key) next
      v = substr(line, index(line, "=") + 1)
      gsub(/^[ \t"]+/, "", v)
      gsub(/[ \t"]+$/, "", v)
      print v
      exit
    }
  ' "$1"
}

# 取文件的第一个匹配（sed 表达式由调用方给出）
x() { sed -n "$2" "$1" | head -n 1; }

# 唯一化一组版本号（去重排序后换行拼接）
uniq_set() { printf '%s\n' "$@" | sed '/^$/d' | sort -u | paste -sd, -; }

echo "版本一致性门禁 —— 真源：${SSOT}"
echo

# ── 读取真源 ─────────────────────────────────────────────────────────────────
REL_TAG="$(toml_get "${SSOT}" release tag)"
REL_STATUS="$(toml_get "${SSOT}" release status)"
REL_LAST="$(toml_get "${SSOT}" release last_published)"
SPEC_VER="$(toml_get "${SSOT}" spec version)"
SPEC_MAJOR="$(toml_get "${SSOT}" spec major)"
CLI_VER="$(toml_get "${SSOT}" cli version)"
SKILL_VER="$(toml_get "${SSOT}" skill version)"
SKILL_SLUG="$(toml_get "${SSOT}" skill slug)"
SKILL_DISPLAY="$(toml_get "${SSOT}" skill displayName)"

for pair in "release.tag:${REL_TAG}" "release.status:${REL_STATUS}" "spec.version:${SPEC_VER}" \
  "spec.major:${SPEC_MAJOR}" "cli.version:${CLI_VER}" "skill.version:${SKILL_VER}" \
  "skill.slug:${SKILL_SLUG}" "skill.displayName:${SKILL_DISPLAY}"; do
  if [ -z "${pair#*:}" ]; then
    ng "无法从 ${SSOT} 读取 ${pair%%:*}（文件缺失或 TOML 结构被改动）"
  fi
done
if [ "${FAIL}" -ne 0 ]; then
  echo
  echo "真源不可读，后续检查无意义。" >&2
  exit 1
fi

# ── 轴 A：规范版本 ───────────────────────────────────────────────────────────
echo "[A] 规范版本 = ${SPEC_VER}（str 主版本 = ${SPEC_MAJOR}）"

check "str-cli/src/lib.rs 的 SPEC_VERSION" "${SPEC_VER}" \
  "$(x str-cli/src/lib.rs 's/^pub const SPEC_VERSION: &str = "\(.*\)";$/\1/p')"
check "str-cli/src/lib.rs 的 STR_MAJOR" "${SPEC_MAJOR}" \
  "$(x str-cli/src/lib.rs 's/^pub const STR_MAJOR: i64 = \([0-9]*\);$/\1/p')"
check "仓库根 ._meta 的 spec" "${SPEC_VER}" \
  "$(x ._meta 's/^spec = "\([^"]*\)"$/\1/p')"
check "仓库根 ._meta 的 str" "${SPEC_MAJOR}" \
  "$(x ._meta 's/^str = \([0-9]*\)$/\1/p')"
check "STR-FORMAT-PROMPT.md 头部「规范版本」" "${SPEC_VER}" \
  "$(x STR-FORMAT-PROMPT.md 's/^| 规范版本 | \*\*v\([0-9.]*\)\*\*.*/\1/p')"
check "STR-FORMAT-PROMPT.md 头部「主版本号」" "${SPEC_MAJOR}" \
  "$(x STR-FORMAT-PROMPT.md 's/^| 规范版本 | .*主版本号 = `\([0-9]*\)`.*/\1/p')"
check "README.md「规范版本」" "${SPEC_VER}" \
  "$(x README.md 's/^- 规范版本：v\([0-9.]*\)（.*/\1/p')"
check "README.md「主版本号」" "${SPEC_MAJOR}" \
  "$(x README.md 's/^- 规范版本：.*主版本号 = `\([0-9]*\)`.*/\1/p')"

# README 只应当出现三条轴的当前版本号；多出第 4 个（典型的「忘了改的旧版本」）即失败。
# 例外：依赖 / 工具版本（如 str-gui 补丁针对的 winit-0.30.13）不是版本轴的声明点，
# 逐字列入白名单后从集合中剔除 —— 否则门禁会把它误判为漂移。
README_IGNORE_VERS="0.30.13"
README_VER_SET="$(grep -oE '[0-9]+\.[0-9]+\.[0-9]+' README.md \
  | grep -vxF -f <(printf '%s\n' ${README_IGNORE_VERS}) | sort -u | paste -sd, -)"
check_set "README.md 中出现的全部版本号（应为 spec / cli / skill 三者）" \
  "$(uniq_set "${SPEC_VER}" "${CLI_VER}" "${SKILL_VER}")" "${README_VER_SET}"

# 三份 Schema 的 description 写明「对应 STR 规范 vX」——真源在 ._schema/（派生的
# str-cli/schema/ 由 tests/schema_sync.rs 守卫逐字节一致）
SCHEMA_VER_SET="$(for f in ._schema/*.schema.json; do
  x "${f}" 's/.*对应 STR 规范 v\([0-9.]*\) 第 4 章.*/\1/p'
done | sort -u | paste -sd, -)"
check_set "._schema/*.json description 的规范版本" "${SPEC_VER}" "${SCHEMA_VER_SET}"

# 示例 bundle 是产物，其 spec 由生成脚本写入
check "examples/客户运营.str 的 spec" "${SPEC_VER}" \
  "$(x "examples/客户运营.str/._meta" 's/^spec = "\([^"]*\)"$/\1/p')"
BLD_VER_SET="$(grep -oE 'spec = "[0-9.]+"' scripts/build-example.sh \
  | sed 's/.*"\([0-9.]*\)"/\1/' | sort -u | paste -sd, -)"
check_set "scripts/build-example.sh 生成模板的 spec" "${SPEC_VER}" "${BLD_VER_SET}"

# 技能包内两份 reference 的头部声明（散文，用定点模式而非全文扫描）
check "str-skill/references/spec-digest.md 头部规范版本" "${SPEC_VER}" \
  "$(x str-skill/references/spec-digest.md 's/.*唯一真源\*\*，v\([0-9.]*\)）.*/\1/p')"
check "str-skill/references/cli-reference.md 头部规范版本" "${SPEC_VER}" \
  "$(x str-skill/references/cli-reference.md 's/.*规范正文，v\([0-9.]*\)）.*/\1/p')"
echo

# ── 轴 B：CLI 版本 ───────────────────────────────────────────────────────────
echo "[B] CLI 版本 = ${CLI_VER}（crate str-format）"

check "str-cli/Cargo.toml 的 version" "${CLI_VER}" \
  "$(toml_get str-cli/Cargo.toml package version)"
LOCK_VER="$(awk '/^name = "str-format"$/ { f = 1; next }
  f && /^version = / { gsub(/[^0-9.]/, "", $0); print; exit }' str-cli/Cargo.lock)"
check "str-cli/Cargo.lock 的 str-format 版本" "${CLI_VER}" "${LOCK_VER}"
check "str-skill/scripts/ensure-str.sh 的 DEFAULT_VERSION" "${CLI_VER}" \
  "$(x str-skill/scripts/ensure-str.sh 's/^DEFAULT_VERSION=v\([0-9.]*\)$/\1/p')"

has "str-skill/SKILL.md 的 --version 示例指向当前版本" str-skill/SKILL.md "str ${CLI_VER}"
has "str-skill/references/cli-reference.md 的实测版本指向当前版本" \
  str-skill/references/cli-reference.md "str ${CLI_VER}"
echo

# ── 轴 C：技能包版本 ─────────────────────────────────────────────────────────
echo "[C] 技能包版本 = ${SKILL_VER}"

check "str-skill/SKILL.md frontmatter 的 version" "${SKILL_VER}" \
  "$(x str-skill/SKILL.md 's/^version: \(.*\)$/\1/p')"
check "str-skill/README.md 声明的技能包版本" "${SKILL_VER}" \
  "$(x str-skill/README.md 's/^- 技能包版本：\*\*\([0-9.]*\)\*\*$/\1/p')"
# SkillHub 发布身份：slug 全网唯一且发布后不宜更改，因此与真源逐字比对
check "str-skill/SKILL.md frontmatter 的 slug（SkillHub 发布身份）" "${SKILL_SLUG}" \
  "$(x str-skill/SKILL.md 's/^slug: \(.*\)$/\1/p')"
check "str-skill/SKILL.md frontmatter 的 displayName" "${SKILL_DISPLAY}" \
  "$(x str-skill/SKILL.md 's/^displayName: \(.*\)$/\1/p')"
# slug 必须满足 SkillHub 的格式要求 —— 依据是 CLI 源码而非文档文案：
# `_SLUG_PATTERN = ^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$`，另有 `len < 2 or len > 128` 判定。
# ⚠ CLI 的报错文案写「必须 kebab-case 3-128 char」，但源码实际允许 2 字符（报错文案不准）；
# 门禁按**实现**取 2，免得把平台认可的合法 slug 误判为非法。
case "${SKILL_SLUG}" in
  [a-z0-9]*)
    if printf '%s' "${SKILL_SLUG}" | grep -qE '^[a-z0-9]+(-[a-z0-9]+)*$' &&
      [ "${#SKILL_SLUG}" -ge 2 ] && [ "${#SKILL_SLUG}" -le 128 ]; then
      ok "slug 满足 SkillHub 格式要求（kebab-case，${#SKILL_SLUG} 字符）"
    else
      ng "slug「${SKILL_SLUG}」不符合 kebab-case（仅小写字母 / 数字 / 单连字符，2~128 字符）"
    fi
    ;;
  *)
    ng "slug「${SKILL_SLUG}」不符合 kebab-case（仅小写字母 / 数字 / 单连字符，2~128 字符）"
    ;;
esac
echo

# ── 锚：发行 tag ─────────────────────────────────────────────────────────────
echo "[D] 发行 tag = ${REL_TAG}（status = ${REL_STATUS}，上一次发布 ${REL_LAST}）"

check "tag 必须等于 v + cli.version" "v${CLI_VER}" "${REL_TAG}"
case "${REL_STATUS}" in
  unreleased | released) ok "release.status 取值合法（${REL_STATUS}）" ;;
  *) ng "release.status 必须是 unreleased 或 released，实际「${REL_STATUS}」" ;;
esac
if [ "${REL_STATUS}" = "released" ]; then
  check "status = released 时 tag 必须等于 last_published" "${REL_LAST}" "${REL_TAG}"
fi
check ".github/workflows/release.yml 的 workflow_dispatch 默认 tag" "${REL_TAG}" \
  "$(x .github/workflows/release.yml 's/^ *default: "\(v[0-9.]*\)"$/\1/p')"

# CI tag 上下文：推送的 tag 必须就是真源里声明的那个（防止「打了 v0.2.1 但真源写 v0.3.0」）
if [ "${GITHUB_REF_TYPE:-}" = "tag" ]; then
  check "本次推送的 tag（${GITHUB_REF_NAME:-}）" "${REL_TAG}" "${GITHUB_REF_NAME:-}"
elif [ -n "${CHECK_TAG:-}" ]; then
  check "CHECK_TAG 指定的 tag" "${REL_TAG}" "${CHECK_TAG}"
else
  ok "非 tag 上下文，跳过「推送 tag ↔ 真源」比对（CI 的 tag 构建会自动启用）"
fi
echo

if [ "${FAIL}" -ne 0 ]; then
  cat >&2 <<EOF
版本一致性检查失败：${FAIL} 处不一致。
真源是 ${SSOT}。请二选一，不要两边各改一半：
  · 若真源已 bump：按上面每条报错同步对应声明点（改完仓库内 payload 记得 \`str sync .\`）；
  · 若是误改声明点：把它改回真源的值。
EOF
  exit 1
fi

echo "全部一致：spec ${SPEC_VER} · cli ${CLI_VER} · skill ${SKILL_VER} · release ${REL_TAG}"
