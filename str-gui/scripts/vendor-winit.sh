#!/usr/bin/env bash
# ══════════════════════════════════════════════════════════════════════════════
# 生成 macOS 拖放修补版 winit，供 str-gui 的 `[patch.crates-io]` 使用
# ══════════════════════════════════════════════════════════════════════════════
#
# 为什么需要补丁
# ──────────────
# winit 0.30.x 的 macOS 后端没有实现 NSDraggingDestination 的
# `draggingUpdated:` —— 该回调必须返回 YES，AppKit 才会批准（accept）落下操作；
# 同时它也不在拖拽悬停时上报光标位置。而 str-gui 的树形拖放（拖到行上半区 /
# 下半区 / 分支节点，以及从 Finder 拖文件入树）恰恰依赖这两件事。
# 补丁本体见 `str-gui/patches/winit-0.30.13-macos-dragndrop.patch`
# （仅 `src/platform_impl/macos/window_delegate.rs` 一个文件、30 行新增）。
#
# 为什么做成脚本
# ──────────────
# 早先的做法是把改好的源码放在 `~/.cache/winit-0.30.13`（既非 git 仓库、
# 也没有 workflow），Cargo 指向的绝对路径只对本人这台机器成立 —— 任何人 clone
# 本仓库都构建不出同样结果。现在：官方 tarball 以 sha256 钉死 + 补丁随仓库入库，
# 执行本脚本即可复现出与原先**字节一致**的源码，产物落在 `str-gui/vendor/winit`
# （已 gitignore），不再依赖任何 ~/.cache 路径。
#
# 幂等：产物存在且 stamp 记录的 sha256 与预期一致时直接跳过。
#
# 用法
# ────
#   str-gui/scripts/vendor-winit.sh            # 首次/增量（已存在则跳过）
#   str-gui/scripts/vendor-winit.sh --force    # 强制重建
#   WINIT_CRATE_URL=<镜像> str-gui/scripts/vendor-winit.sh   # 下载源不可达时改用镜像
#
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

WINIT_VERSION="0.30.13"
# static.crates.io 官方 tarball 的 sha256 —— 改动此值即等于改动依赖来源，需同步更新
# str-gui/patches/ 下的补丁适用性。
CRATE_SHA256="a6755fa58a9f8350bd1e472d4c3fcc25f824ec358933bba33306d0b63df5978d"
CRATE_URL="${WINIT_CRATE_URL:-https://static.crates.io/crates/winit/winit-${WINIT_VERSION}.crate}"

PATCH="$ROOT/str-gui/patches/winit-${WINIT_VERSION}-macos-dragndrop.patch"
DEST="$ROOT/str-gui/vendor/winit"
STAMP="$DEST/.str-vendor-stamp"

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

die() { printf '✗ %s\n' "$1" >&2; exit 1; }

main() {
    local force=0
    if [ "${1:-}" = "--force" ]; then
        force=1
    fi

    [ -f "$PATCH" ] || die "找不到补丁：$PATCH"

    if [ "$force" -eq 0 ] && [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$CRATE_SHA256" ]; then
        echo "✓ str-gui/vendor/winit 已是 winit ${WINIT_VERSION} 的修补版本（跳过；--force 可重建）"
        return 0
    fi

    # tmp 刻意不用 local：trap 直到脚本退出（main 返回之后）才会触发，
    # 届时 local 变量已出作用域，配合 `set -u` 会报 unbound variable。
    tmp="$(mktemp -d)"
    trap 'rm -rf "${tmp:-}"' EXIT

    echo "↓ 下载 winit ${WINIT_VERSION}"
    curl -fsSL "$CRATE_URL" -o "$tmp/winit.crate" || die "下载失败：$CRATE_URL"

    local got
    got="$(sha256_of "$tmp/winit.crate")"
    [ "$got" = "$CRATE_SHA256" ] \
        || die "sha256 校验失败：期望 ${CRATE_SHA256}，实际 ${got}。请核对下载源，或用 WINIT_CRATE_URL 指定可信镜像"

    echo "✓ sha256 校验通过（${got:0:16}…）"

    mkdir -p "$tmp/x"
    tar -xzf "$tmp/winit.crate" -C "$tmp/x"
    [ -d "$tmp/x/winit-${WINIT_VERSION}" ] || die "tarball 内容异常：缺少 winit-${WINIT_VERSION}/ 目录"

    # 补丁必须在临时目录里打完再移动到位：str-gui/vendor/ 被 .gitignore 覆盖，
    # 在该目录内执行 `git apply` 会打印 "Skipped patch" 并以 0 退出（假成功），
    # 产物会静默退回未打补丁的官方源码。
    echo "↓ 应用 macOS 拖放补丁"
    local patched_file="src/platform_impl/macos/window_delegate.rs"
    (cd "$tmp/x/winit-${WINIT_VERSION}" && git apply -p1 "$PATCH") \
        || die "补丁应用失败，请检查 $PATCH 是否与 winit ${WINIT_VERSION} 匹配"

    # 追加补丁目录：目录内的 .patch 在主补丁之后按文件名顺序依次应用。
    if [ -d "$ROOT/str-gui/patches/extra" ]; then
        for extra in "$ROOT"/str-gui/patches/extra/winit-*.patch; do
            [ -f "$extra" ] || continue
            echo "↓ 应用追加补丁 $(basename "$extra")"
            (cd "$tmp/x/winit-${WINIT_VERSION}" && git apply -p1 "$extra") \
                || die "追加补丁应用失败：$(basename "$extra")"
        done
    fi

    # 补丁后断言：宁可失败，也不要带着未修补的产物继续（上面那次假成功的教训）。
    grep -q "draggingUpdated" "$tmp/x/winit-${WINIT_VERSION}/$patched_file" \
        || die "补丁未生效：$patched_file 中找不到 draggingUpdated"

    rm -rf "$DEST"
    mkdir -p "$(dirname "$DEST")"
    mv "$tmp/x/winit-${WINIT_VERSION}" "$DEST"

    printf '%s' "$CRATE_SHA256" >"$STAMP"
    echo "✓ 已生成 $(printf '%s' "$DEST" | sed "s|$ROOT/||")（含 macOS 拖放补丁）"
}

main "$@"
