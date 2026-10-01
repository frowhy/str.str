#!/usr/bin/env bash
# ══════════════════════════════════════════════════════════════════════════════
# 保留名迁移脚本：`._` 前缀 → `.str.` 前缀（规范 v1.17.0，破坏性变更）
# ══════════════════════════════════════════════════════════════════════════════
#
# 把既有 bundle 的格式保留名改为 v1.17.0 新名，并用新版 CLI 完成对账：
#
#   ._meta   → .str.toml     （元数据文件；**递归全部分支层级**，内容不变仅改名）
#   ._schema → .str.schema   （bundle 级 Schema 目录，仅根）
#   ._cache  → 删除           （派生缓存；规范 §7.4 可安全删除，旧基线对新名无意义，
#                              新 CLI 会重建 .str.cache）
#
# 子 bundle（bundle 内的 `*.str` 目录，规范 §3.5 硬边界）是**独立 bundle**，各自拥有
# 自己的保留名 —— 脚本会自动发现并作为独立 bundle 一并迁移（同样递归、独立对账）。
#
# 每份 bundle 改名后依次执行：`str spec set 1.17.0` → `str sync` →
# `str validate --strict` → `str fmt --check`，任一步失败即记为失败（已完成的改名
# 不回滚；脚本幂等，修复后重跑会自动续迁剩余部分）。
#
# 用法：
#   bash scripts/migrate-reserved-names.sh [选项] <bundle-dir> [more-dirs...]
# 选项：
#   --dry-run   只预览将执行的改名，不做任何变更
#   --no-check  跳过 sync / validate / fmt（不推荐）
# 说明：
#   - 目录参数指向 bundle 根（即原 `._meta` 所在目录，如 `notes.str`）；
#   - 已是新格式的 bundle / 分支自动跳过（幂等，可重跑）；
#   - 旧版 CLI（< 0.10.0）无法读取迁移后的 bundle，脚本会先校验 CLI 版本。
set -uo pipefail

DRY_RUN=0
NO_CHECK=0
DIRS=()

for arg in "$@"; do
  case "${arg}" in
    --dry-run) DRY_RUN=1 ;;
    --no-check) NO_CHECK=1 ;;
    -h|--help)
      sed -n '2,26p' "${BASH_SOURCE[0]}"; exit 0 ;;
    -*) echo "未知选项：${arg}（--help 查看用法）" >&2; exit 2 ;;
    *) DIRS+=("${arg}") ;;
  esac
done

[ "${#DIRS[@]}" -eq 0 ] && { echo "用法：bash scripts/migrate-reserved-names.sh [--dry-run] <bundle-dir> [more-dirs...]" >&2; exit 2; }

ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; }
ng()   { printf '  \033[31m✗\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }
skip() { printf '  \033[33m–\033[0m %s\n' "$1"; }
FAIL=0

# ── 解析 str CLI（≥ 0.10.0）─────────────────────────────────────────────────
STR_BIN_CMD="${STR_BIN:-}"
if [ -z "${STR_BIN_CMD}" ]; then
  if command -v str >/dev/null 2>&1; then
    STR_BIN_CMD="str"
  elif [ -f "${BASH_SOURCE[0]%/*}/../str-cli/target/release/str" ]; then
    STR_BIN_CMD="${BASH_SOURCE[0]%/*}/../str-cli/target/release/str"
  fi
fi
if [ -z "${STR_BIN_CMD}" ] || ! command -v "${STR_BIN_CMD}" >/dev/null 2>&1; then
  echo "找不到 str CLI：请安装 ≥ 0.10.0，或用 STR_BIN=<path> 指定。" >&2
  exit 1
fi
VER="$("${STR_BIN_CMD}" --version)"           # 期望输出：str 0.10.0
VMAJOR="$(printf '%s' "${VER}" | awk '{print int($2)}')"
VMINOR="$(printf '%s' "${VER}" | awk '{split($2, a, "."); print int(a[2])}')"
if [ "${VMAJOR}" -eq 0 ] && [ "${VMINOR}" -lt 10 ]; then
  echo "str CLI 过旧（${VER}）：迁移需要 ≥ 0.10.0（对应规范 v1.17.0）。" >&2
  exit 1
fi
echo "str CLI：${VER}（${STR_BIN_CMD}）"
[ "${DRY_RUN}" -eq 1 ] && echo "（--dry-run 预览模式，不做任何变更）"
echo

# ── 收集待迁移 bundle：显式参数 + 参数内部发现的子 bundle（独立硬边界）────────
# macOS bash 3.2 兼容：不用关联数组 / mapfile，用换行分隔字符串去重。
BUNDLES=""
add_bundle() { case "\n${BUNDLES}" in *"\n${1}\n"*) ;; *) BUNDLES="${BUNDLES}${1}
" ;; esac; }

for dir in "${DIRS[@]}"; do
  if [ ! -d "${dir}" ]; then
    ng "${dir}：目录不存在"
    continue
  fi
  root="$(cd "${dir}" && pwd)"
  # 参数本身须是 bundle（根上有 ._meta 或 .str.toml）
  if [ ! -e "${root}/._meta" ] && [ ! -e "${root}/.str.toml" ]; then
    ng "${dir}：根上没有 ._meta / .str.toml —— 不是 bundle（目录须指向 bundle 根）"
    continue
  fi
  add_bundle "${root}"
  # 内部嵌套的子 bundle（*.str 目录）是独立 bundle，一并纳入；prune 不深入
  while IFS= read -r sub; do
    [ -z "${sub}" ] && continue
    add_bundle "${sub}"
  done < <(find "${root}" -mindepth 1 -type d -name '*.str' -prune)
done

# ── 逐 bundle 迁移 ──────────────────────────────────────────────────────────
migrate_one() {
  local dir="$1"
  echo "▸ ${dir}"

  # 1) 递归改名全部层级的 ._meta（find 不进入子 bundle —— 子 bundle 已单独入列）
  local metas
  metas="$(find "${dir}" -mindepth 1 \( -type d -name '*.str' -prune \) -o -type f -name '._meta' -print)"
  local any_old=0 any_new=0 conflicts=0 p
  for p in ${metas}; do
    any_old=1
    if [ -e "${p%._meta}.str.toml" ]; then
      ng "${p}：同名 .str.toml 已存在，冲突请人工裁决"
      conflicts=1
      continue
    fi
    echo "  ${p#"$dir"/}  →  ${p%._meta}.str.toml"
  done
  if [ "${any_old}" -eq 1 ] && [ "${conflicts}" -eq 0 ]; then
    if [ "${DRY_RUN}" -eq 0 ]; then
      for p in ${metas}; do
        [ -e "${p%._meta}.str.toml" ] && continue
        mv "${p}" "${p%._meta}.str.toml" || ng "mv 失败：${p}"
      done
      ok "全部分支层级的 ._meta 已改名为 .str.toml"
    fi
  fi
  [ -e "${dir}/.str.toml" ] && any_new=1

  if [ "${any_old}" -eq 0 ] && [ "${any_new}" -eq 1 ]; then
    skip "已是新格式（无 ._meta 残留），跳过"
  fi

  # 2) 根级 ._schema / ._cache
  if [ -d "${dir}/._schema" ]; then
    echo "  ._schema → .str.schema"
    [ "${DRY_RUN}" -eq 0 ] && mv "${dir}/._schema" "${dir}/.str.schema"
  fi
  if [ -d "${dir}/._cache" ]; then
    echo "  ._cache  → 删除（派生数据，新 CLI 重建 .str.cache）"
    [ "${DRY_RUN}" -eq 0 ] && rm -rf "${dir}/._cache"
  fi

  # 3) 对账 + 校验（新 CLI）
  if [ "${NO_CHECK}" -eq 0 ] && [ "${DRY_RUN}" -eq 0 ] && [ "${conflicts}" -eq 0 ]; then
    if "${STR_BIN_CMD}" spec set 1.17.0 "${dir}" >/dev/null \
      && "${STR_BIN_CMD}" sync "${dir}" >/dev/null \
      && "${STR_BIN_CMD}" validate "${dir}" --strict >/dev/null \
      && "${STR_BIN_CMD}" fmt "${dir}" --check >/dev/null; then
      ok "对账与校验通过（0 errors / 0 warnings）"
    else
      ng "对账/校验失败 —— 请查看上方 CLI 输出（改名不回滚，修复后可重跑）"
    fi
  fi
}

FIRST=1
while IFS= read -r b; do
  [ -z "${b}" ] && continue
  [ "${FIRST}" -eq 0 ] && echo
  migrate_one "${b}"
  FIRST=0
done <<< "${BUNDLES}"

echo
if [ "${FAIL}" -ne 0 ]; then
  echo "迁移未全部成功：${FAIL} 处失败。" >&2
  exit 1
fi
echo "全部完成。"
