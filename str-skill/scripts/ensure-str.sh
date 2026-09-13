#!/bin/sh
# ensure-str.sh — 解析（必要时构建）STR 命令行工具 `str`，把可执行文件路径打印到 stdout。
#
# 用法：
#   STR="$(sh ensure-str.sh)" || exit 1
#   "$STR" validate mybundle.str
#
# 解析顺序：
#   1. $STR_BIN（显式指定的可执行文件路径）
#   2. PATH 上 `--version` 输出形如 "str x.y.z" 的 str
#   3. $STR_REPO 指向的仓库内 str-cli/target/release/str（必要时构建）
#   4. 从本脚本所在目录向上逐级查找
#   5. 从当前工作目录向上逐级查找
#   6. 以上任一位置若存在 str-cli/Cargo.toml 且本机有 cargo，则 cargo build --release 后复用
#   7. 全部失败 → 打印安装指引并以非零码退出
#
# 契约：
#   - 成功时 stdout 只输出一行可执行文件路径，不夹带任何其它内容。
#   - 诊断信息一律走 stderr。
#   - 不访问网络；除 str-cli/target/ 外不写入任何文件。
#   - 绝不「降级」为手工编辑 ._meta —— 找不到 CLI 就是失败。
set -eu

log() { printf '%s\n' "$*" >&2; }
die() { log "ensure-str: $*"; exit 1; }

# 判定是否为可用的 STR CLI（名字叫 str 的东西不止一个，必须验版本输出）。
is_str_bin() {
    [ -n "$1" ] || return 1
    [ -x "$1" ] || return 1
    _v=$("$1" --version 2>/dev/null || true)
    case "$_v" in
        str\ [0-9]*) return 0 ;;
        *) return 1 ;;
    esac
}

# 在给定目录里找产物；找不到就尝试用 cargo 构建一次。
try_build() {
    _repo=$1
    _bin="$_repo/str-cli/target/release/str"
    if is_str_bin "$_bin"; then
        printf '%s\n' "$_bin"
        return 0
    fi
    if [ -f "$_repo/str-cli/Cargo.toml" ] && command -v cargo >/dev/null 2>&1; then
        log "ensure-str: 在 $_repo 发现 str-cli 源码，正在 cargo build --release ..."
        if (cd "$_repo/str-cli" && cargo build --release >&2); then
            if is_str_bin "$_bin"; then
                printf '%s\n' "$_bin"
                return 0
            fi
        fi
    fi
    return 1
}

# 1 — 显式指定
if [ -n "${STR_BIN:-}" ]; then
    if is_str_bin "$STR_BIN"; then
        printf '%s\n' "$STR_BIN"
        exit 0
    fi
    die "STR_BIN=$STR_BIN 不是可用的 STR CLI（期望 \`--version\` 输出 str x.y.z）"
fi

# 2 — PATH
if command -v str >/dev/null 2>&1; then
    _found=$(command -v str)
    if is_str_bin "$_found"; then
        printf '%s\n' "$_found"
        exit 0
    fi
    log "ensure-str: PATH 上的 \`$_found\` 不是 STR CLI，继续查找"
fi

# 3 — STR_REPO
if [ -n "${STR_REPO:-}" ]; then
    if try_build "$STR_REPO"; then
        exit 0
    fi
    die "STR_REPO=$STR_REPO 下找不到可用的 str（既无产物也未能构建）"
fi

# 4 / 5 — 从脚本目录与 CWD 向上逐级查找
_here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P) || _here=""
_pwd=$(pwd -P)

for _start in "$_here" "$_pwd"; do
    [ -n "$_start" ] || continue
    _dir=$_start
    while [ "$_dir" != "/" ]; do
        if try_build "$_dir"; then
            exit 0
        fi
        _dir=$(dirname -- "$_dir")
    done
done

# 7 — 失败
die "未找到 STR CLI。请任选其一后重试：
  ① 在 STR 仓库执行：cargo install --path str-cli
  ② 或构建后指定：export STR_BIN=<仓库>/str-cli/target/release/str
  ③ 或指向源码：export STR_REPO=<仓库根>
注意：严禁以手工编辑 ._meta 代替 CLI —— 那是格式违规，不是降级方案。"
