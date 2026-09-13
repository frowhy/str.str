#!/usr/bin/env bash
# 同步 Schema 派生副本：仓库根 ._schema/（唯一真源）→ str-cli/schema/（crate 内副本）。
#
# 为什么需要副本：crates.io 只打包 crate 目录（str-cli/）内的文件，而 Schema 属
# **格式规范资产**，必须留在仓库根（与 examples/ 同级）。因此 crate 内保留一份
# **派生副本** —— 由本脚本生成，并由 str-cli/tests/schema_sync.rs 与 CI 守卫其与
# 真源逐字节一致（一旦漂移，cargo test 直接失败）。
#
# 用法：
#   bash scripts/sync-schema.sh           # 真源 → 副本（幂等）
#   bash scripts/sync-schema.sh --check   # 只校验，不一致则退出码 1
set -euo pipefail

ROOT="$(cd "$(dirname "${0}")/.." && pwd)"
SRC="${ROOT}/._schema"
DST="${ROOT}/str-cli/schema"

FILES=(
  root-meta.schema.json
  node-meta.schema.json
  branch-meta.schema.json
)

MODE="write"
if [ "${1:-}" = "--check" ]; then
  MODE="check"
fi

if [ "${MODE}" = "write" ]; then
  mkdir -p "${DST}"
fi

DRIFT=0
for f in "${FILES[@]}"; do
  if [ ! -f "${SRC}/${f}" ]; then
    echo "真源缺失：${SRC}/${f}" >&2
    exit 1
  fi
  if [ "${MODE}" = "write" ]; then
    cp "${SRC}/${f}" "${DST}/${f}"
    echo "已同步 ${f}"
  elif ! cmp -s "${SRC}/${f}" "${DST}/${f}"; then
    echo "不一致：${f}（${SRC}/${f} 与 ${DST}/${f}）" >&2
    DRIFT=1
  fi
done

if [ "${MODE}" = "check" ]; then
  if [ "${DRIFT}" -ne 0 ]; then
    echo "Schema 派生副本与真源不一致，请运行：bash scripts/sync-schema.sh" >&2
    exit 1
  fi
  echo "Schema 派生副本与真源一致（${#FILES[@]} 个文件）"
fi
