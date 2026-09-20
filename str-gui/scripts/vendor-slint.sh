#!/usr/bin/env bash
# ══════════════════════════════════════════════════════════════════════════════
# 生成 str-gui 依赖的 Slint 源码（上游 base rev + 本地补丁）
# ══════════════════════════════════════════════════════════════════════════════
#
# 为什么需要补丁
# ──────────────
# str-gui 的两项能力在 Slint 上游尚不存在：
#   1. 外部文件拖入：winit 事件（HoveredFile / DroppedFile）到 Slint `DropEvent`
#      的整条转换链路 —— 没有它，从 Finder 拖文件进窗口时 `.slint` 的 `dropped`
#      回调根本不会触发。落点取自后端缓存的 `cursor_pos`，而它由
#      `vendor-winit.sh` 打的补丁在拖拽悬停时补发 `CursorMoved` 维持，
#      两个补丁是配套关系，缺一不可。
#   2. `Window::set_color_scheme`：上游只有 Windows(muda) 与 Linux(xdg) 两条
#      实现分支，macOS 无通路（str-gui 现已改用 AppKit 的 NSAppearance 上报 OS，
#      见 src/main.rs，但此处的 Slint 侧改动仍是既有行为的一部分）。
#
# 为什么做成脚本
# ──────────────
# 早先这些改动散落在 `~/.cache/slint-src` 里就地修改（该目录甚至没有 .git），
# Cargo 指的还是绝对路径 —— 任何人 clone 本仓库都编不出同样结果。现在 base rev
# 与 tarball 的 sha256 都被钉死，补丁随仓库入库，执行本脚本即可复现出与原先
# **字节一致**的源码，产物落在 `str-gui/vendor/slint`（已 gitignore）。
#
# base rev 的来源：以未被本地改动的若干文件（object_tree.rs / properties.rs /
# interpreter/api.rs / Cargo.toml / items.rs）的 blob 指纹，在上游历史上反向
# 匹配得到 —— 除补丁涉及的 12 个文件外，其余 5077 个文件与该提交逐字节相同。
#
# 幂等：产物存在且 stamp 记录的 sha256 与预期一致时直接跳过。
#
# 用法
# ────
#   str-gui/scripts/vendor-slint.sh            # 首次/增量（已存在则跳过）
#   str-gui/scripts/vendor-slint.sh --force    # 强制重建
#   SLINT_TARBALL_URL=<镜像> str-gui/scripts/vendor-slint.sh
#
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

SLINT_REV="8fd54ede52e182d16917076e70af8061103f588d"
# codeload tarball 的 sha256 —— 改动此值即等于改动依赖来源
TARBALL_SHA256="1810ed5d233f21c90176ba76b008c9ac22eac2ba58f4ab9a0b9b47d47e1fdd0f"
TARBALL_URL="${SLINT_TARBALL_URL:-https://codeload.github.com/slint-ui/slint/tar.gz/${SLINT_REV}}"

PATCH="$ROOT/str-gui/patches/slint-$(echo "$SLINT_REV" | cut -c1-8)-macos-drag-and-color-scheme.patch"
# 追加补丁目录：目录内的 .patch 在主补丁之后按文件名顺序依次应用。
# 用于小粒度增量改动（如拖拽光标），避免频繁重造大补丁。
EXTRA_DIR="$ROOT/str-gui/patches/extra"
DEST="$ROOT/str-gui/vendor/slint"
STAMP="$DEST/.str-vendor-stamp"

# 补丁后必须存在的特征，用于断言补丁真的生效
ASSERT_FILE="internal/backends/winit/winitwindowadapter.rs"
ASSERT_PATTERN="DroppedFile"

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

    if [ "$force" -eq 0 ] && [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$TARBALL_SHA256" ]; then
        echo "✓ str-gui/vendor/slint 已是 rev ${SLINT_REV:0:8} 的修补版本（跳过；--force 可重建）"
        return 0
    fi

    # tmp 刻意不用 local：trap 直到脚本退出（main 返回之后）才触发，
    # 届时 local 变量已出作用域，配合 `set -u` 会报 unbound variable。
    tmp="$(mktemp -d)"
    trap 'rm -rf "${tmp:-}"' EXIT

    echo "↓ 下载 Slint ${SLINT_REV:0:8} 源码（约 11MB）"
    curl -fsSL "$TARBALL_URL" -o "$tmp/slint.tar.gz" || die "下载失败：$TARBALL_URL"

    local got
    got="$(sha256_of "$tmp/slint.tar.gz")"
    [ "$got" = "$TARBALL_SHA256" ] \
        || die "sha256 校验失败：期望 ${TARBALL_SHA256}，实际 ${got}。请核对下载源，或用 SLINT_TARBALL_URL 指定可信镜像"

    echo "✓ sha256 校验通过（${got:0:16}…）"

    mkdir -p "$tmp/x"
    # Windows runner 的 tar 无法创建 tarball 内的符号链接（需管理员/开发者模式；
    # GitHub 的 codeload tarball 含若干 README 软链）——缺的只是软链文件本身，
    # 不参与编译，容忍后靠特征断言兜底。macOS/Linux 正常解压、失败即中止。
    if uname -s | grep -qiE 'mingw|msys'; then
        tar -xzf "$tmp/slint.tar.gz" -C "$tmp/x" \
            || echo "注意：Windows 上跳过无法创建的符号链接文件（不影响编译）"
    else
        tar -xzf "$tmp/slint.tar.gz" -C "$tmp/x"
    fi
    local src="$tmp/x/slint-${SLINT_REV}"
    [ -d "$src" ] || die "tarball 内容异常：缺少 slint-${SLINT_REV}/ 目录"

    # 补丁必须在临时目录里打完再移动到位：str-gui/vendor/ 被 .gitignore 覆盖，
    # 在该目录内（含 staging 子目录）执行 `git apply` 会打印 "Skipped patch"
    # 并以 0 退出 —— 是假成功，务必靠下面的特征断言兜底。
    echo "↓ 应用补丁"
    (cd "$src" && git apply -p1 "$PATCH") \
        || die "补丁应用失败，请检查 $PATCH 是否与 rev ${SLINT_REV} 匹配"

    # 追加补丁（patches/extra/slint-*.patch，按文件名顺序）：每个都必须干净应用。
    # 命名约定：extra/ 下的补丁按目标源码加前缀——slint-*.patch 归本脚本、
    # winit-*.patch 归 vendor-winit.sh，避免把 winit 的补丁打到 Slint 源码上。
    if [ -d "$EXTRA_DIR" ]; then
        for extra in "$EXTRA_DIR"/slint-*.patch; do
            [ -f "$extra" ] || continue
            echo "↓ 应用追加补丁 $(basename "$extra")"
            (cd "$src" && git apply -p1 "$extra") \
                || die "追加补丁应用失败：$(basename "$extra")"
        done
    fi

    grep -q "$ASSERT_PATTERN" "$src/$ASSERT_FILE" \
        || die "补丁未生效：$ASSERT_FILE 中找不到 ${ASSERT_PATTERN}"

    # 从 /tmp 跨设备移动到仓库不是原子的；先拷进同级 staging，再用 rename 原子换入。
    local staging="$ROOT/str-gui/vendor/.slint-staging"
    rm -rf "$staging" 2>/dev/null || true
    mkdir -p "$staging"
    mv "$src" "$staging/slint"

    local old="$ROOT/str-gui/vendor/.slint-old"
    rm -rf "$old" 2>/dev/null || true
    if [ -d "$DEST" ]; then
        mv "$DEST" "$old" # 同级 rename，瞬时完成
    fi
    mkdir -p "$(dirname "$DEST")"
    mv "$staging/slint" "$DEST"
    rmdir "$staging" 2>/dev/null || true

    # 旧产物清理是善后动作：清不掉也只提示，不因此判定本次生成失败。
    rm -rf "$old" 2>/dev/null || echo "注意：旧产物 $old 未能自动删除，可手动清理"

    printf '%s' "$TARBALL_SHA256" >"$STAMP"
    echo "✓ 已生成 $(printf '%s' "$DEST" | sed "s|$ROOT/||")（rev ${SLINT_REV:0:8} + macOS 拖放/配色补丁）"
}

main "$@"
