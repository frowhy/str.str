#!/bin/sh
# ensure-str.sh — 解析（必要时构建或下载）STR 命令行工具 `str`，把可执行文件路径打印到 stdout。
#
# 用法：
#   STR="$(sh ensure-str.sh)" || exit 1
#   "$STR" validate mybundle.str
#
# 解析顺序（命中即返回）：
#   1. $STR_BIN                     显式指定的可执行文件路径
#   2. PATH 上的 `str`              `--version` 必须输出 "str x.y.z"
#   3. $STR_REPO                    该源码仓库内 str-cli/target/release/str（必要时 cargo build）
#   4. 向上逐级查找源码仓库          从本脚本所在目录、再从 CWD 依次向上；命中即构建
#   5. GitHub Releases 预编译二进制  识别平台 → 下载 → 校验 SHA-256 → 解包到缓存 → 之后复用
#   6. 全部失败                      打印安装指引并以非零码退出
#
# 环境变量：
#   STR_BIN             显式指定可执行文件（最高优先级）
#   STR_REPO            源码仓库根（应含 str-cli/Cargo.toml）
#   STR_VERSION         版本 tag，如 v0.1.0；默认 latest，解析失败回退到内置默认版本
#   STR_RELEASE_REPO    发布仓库 `owner/repo`，默认 frowhy/str.str（fork 时改此值）
#   STR_DOWNLOAD_BASE   资产下载前缀，默认 https://github.com/<repo>/releases/download
#                       （网络受限时可指向镜像；`latest` 解析失败不影响它，因为版本已回退）
#   STR_NO_DOWNLOAD     置 1 则跳过第 5 步（离线 / 禁止网络时）
#   STR_CACHE_DIR       缓存根目录，默认 ${XDG_CACHE_HOME:-$HOME/.cache}/str-skill
#
# 契约：
#   - 成功时 stdout **只**输出一行可执行文件路径，不夹带任何其它内容；诊断一律走 stderr；
#   - 下载只取 GitHub Releases 资产，并**必须**用同一 release 的 SHA256SUMS.txt 逐字节校验，
#     取不到校验和或校验不通过即失败 —— 不允许「未校验就用」；
#   - 已缓存的版本直接复用，不重复下载；
#   - 除缓存目录与临时目录外不写入任何文件；
#   - 绝不「降级」为手工编辑 ._meta —— 找不到 CLI 就是失败。
#
# ⚠ 维护须知（本文件含大量中文文本，踩过一次）：
#   `$VAR` 后面若**紧跟非 ASCII 字符**（如「，」「（」「的」），部分 locale 下 bash 会把多字节
#   字符并入变量名 → 变量名不存在 → `set -u` 报 unbound variable，且**可能静默以 0 退出**。
#   规则：凡变量后紧跟中文的地方，一律写 `${VAR}`。改动后自查：
#     rg '\$[A-Za-z_][A-Za-z0-9_]*[^\x00-\x7F]' str-skill/scripts/ensure-str.sh   # 应为空
set -eu

RELEASE_REPO=${STR_RELEASE_REPO:-frowhy/str.str}
DEFAULT_VERSION=v0.1.0
STR_VERSION=${STR_VERSION:-latest}
DOWNLOAD_BASE=${STR_DOWNLOAD_BASE:-https://github.com/$RELEASE_REPO/releases/download}
CACHE_BASE=${STR_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/str-skill}
TMP_STR=""

log() { printf '%s\n' "$*" >&2; }
die() { log "ensure-str: $*"; exit 1; }

cleanup() {
    if [ -n "$TMP_STR" ]; then
        rm -rf "$TMP_STR"
    fi
    return 0
}
trap cleanup EXIT INT TERM

have_cmd() { command -v "$1" >/dev/null 2>&1; }

# ── 判定是否为可用的 STR CLI（叫 str 的其它工具不止一个，必须验版本输出）──────
is_str_bin() {
    [ -n "${1:-}" ] || return 1
    [ -f "$1" ] && [ -x "$1" ] || return 1
    _v=$("$1" --version 2>/dev/null || true)
    case "$_v" in
        str\ [0-9]*) return 0 ;;
        *) return 1 ;;
    esac
}

# ── 在源码仓库里找产物；找不到就用 cargo 构建一次 ───────────────────────────
try_build() {
    _repo=$1
    _out_bin="$_repo/str-cli/target/release/str"
    if is_str_bin "$_out_bin"; then
        printf '%s\n' "$_out_bin"
        return 0
    fi
    if [ -f "$_repo/str-cli/Cargo.toml" ] && have_cmd cargo; then
        log "ensure-str: 在 $_repo 发现 str-cli 源码，正在 cargo build --release ..."
        if (cd "$_repo/str-cli" && cargo build --release >&2); then
            if is_str_bin "$_out_bin"; then
                printf '%s\n' "$_out_bin"
                return 0
            fi
        fi
    fi
    return 1
}

# ── 网络 ────────────────────────────────────────────────────────────────
fetch_to() {
    _url=$1
    _out=$2
    if have_cmd curl; then
        curl -fsSL --retry 2 --connect-timeout 15 -o "$_out" "$_url"
    elif have_cmd wget; then
        wget -q -T 15 -O "$_out" "$_url"
    else
        return 127
    fi
}

fetch_stdout() {
    _url=$1
    if have_cmd curl; then
        curl -fsSL --retry 2 --connect-timeout 15 "$_url"
    elif have_cmd wget; then
        wget -q -T 15 -O - "$_url"
    else
        return 127
    fi
}

sha256_of() {
    if have_cmd sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif have_cmd shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        return 1
    fi
}

# ── 平台 → release 资产名里的 target triple（与 .github/workflows/release.yml 矩阵一致）──
detect_target() {
    _os=$(uname -s 2>/dev/null || echo unknown)
    _arch=$(uname -m 2>/dev/null || echo unknown)
    case "$_os" in
        Darwin)
            case "$_arch" in
                arm64|aarch64) printf '%s\n' aarch64-apple-darwin ;;
                x86_64|amd64) printf '%s\n' x86_64-apple-darwin ;;
                *) return 1 ;;
            esac ;;
        Linux)
            case "$_arch" in
                aarch64|arm64) printf '%s\n' aarch64-unknown-linux-gnu ;;
                x86_64|amd64) printf '%s\n' x86_64-unknown-linux-gnu ;;
                *) return 1 ;;
            esac ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            case "$_arch" in
                x86_64|amd64) printf '%s\n' x86_64-pc-windows-msvc ;;
                *) return 1 ;;
            esac ;;
        *) return 1 ;;
    esac
}

resolve_version() {
    if [ "$STR_VERSION" != "latest" ]; then
        printf '%s\n' "$STR_VERSION"
        return 0
    fi
    _tag=$(fetch_stdout "https://api.github.com/repos/$RELEASE_REPO/releases/latest" 2>/dev/null \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -1)
    if [ -n "$_tag" ]; then
        printf '%s\n' "$_tag"
    else
        log "ensure-str: 无法解析 latest（网络或 API 限流），回退到内置默认版本 $DEFAULT_VERSION"
        printf '%s\n' "$DEFAULT_VERSION"
    fi
}

# ── 第 5 步：从 GitHub Releases 下载预编译二进制 ──────────────────────────────
try_download() {
    if [ "${STR_NO_DOWNLOAD:-0}" = "1" ]; then
        log "ensure-str: STR_NO_DOWNLOAD=1，跳过下载"
        return 1
    fi

    _target=$(detect_target) || {
        log "ensure-str: 无法识别当前平台（$(uname -s 2>/dev/null || echo ?)/$(uname -m 2>/dev/null || echo ?)），跳过下载"
        return 1
    }

    case "$_target" in
        *windows*)
            _ext=zip
            _bin_name=str.exe ;;
        *)
            _ext=tar.gz
            _bin_name=str ;;
    esac

    _ver=$(resolve_version)
    _asset="str-${_ver}-${_target}.${_ext}"
    _cache_dir="$CACHE_BASE/$_ver/$_target"
    _cache_bin="$_cache_dir/$_bin_name"

    # 缓存命中：直接复用，不重复下载
    if is_str_bin "$_cache_bin"; then
        printf '%s\n' "$_cache_bin"
        return 0
    fi

    _base="$DOWNLOAD_BASE/$_ver"
    TMP_STR=$(mktemp -d 2>/dev/null || mktemp -d -t str-ensure) || die "无法创建临时目录"

    log "ensure-str: 从 $RELEASE_REPO 下载 $_ver 的 $_asset ..."
    if ! fetch_to "$_base/$_asset" "$TMP_STR/$_asset"; then
        log "ensure-str: 下载失败：$_base/$_asset"
        return 1
    fi

    # 完整性校验：必须拿到同一 release 的 SHA256SUMS.txt，且其中要有本资产的条目
    if ! fetch_to "$_base/SHA256SUMS.txt" "$TMP_STR/SHA256SUMS.txt"; then
        die "已下载二进制但取不到 SHA256SUMS.txt，拒绝在未校验的情况下使用"
    fi
    _want=$(grep -F "$_asset" "$TMP_STR/SHA256SUMS.txt" | head -1 | cut -d' ' -f1 || true)
    if [ -z "$_want" ]; then
        die "SHA256SUMS.txt 中找不到 $_asset 的条目，拒绝在未校验的情况下使用"
    fi
    _got=$(sha256_of "$TMP_STR/$_asset") || die "本机既无 sha256sum 也无 shasum，无法校验下载内容"
    if [ "$_want" != "$_got" ]; then
        die "SHA-256 不匹配（期望 ${_want}，实得 ${_got}），已中止"
    fi

    mkdir -p "$_cache_dir"
    case "$_ext" in
        zip)
            if have_cmd unzip; then
                unzip -q -o "$TMP_STR/$_asset" -d "$_cache_dir"
            else
                tar -xf "$TMP_STR/$_asset" -C "$_cache_dir"   # bsdtar 可直接解 zip
            fi ;;
        *)
            tar -xzf "$TMP_STR/$_asset" -C "$_cache_dir" ;;
    esac

    if [ ! -f "$_cache_bin" ]; then
        die "解包后未找到 ${_bin_name}（release 归档结构可能已变更）"
    fi
    chmod +x "$_cache_bin" 2>/dev/null || true
    is_str_bin "$_cache_bin" || die "下载的二进制无法运行：$_cache_bin"
    printf '%s\n' "$_cache_bin"
    return 0
}

# ─────────────────────────── 主流程 ───────────────────────────

# 1 — 显式指定
if [ -n "${STR_BIN:-}" ]; then
    if is_str_bin "$STR_BIN"; then
        printf '%s\n' "$STR_BIN"
        exit 0
    fi
    die "STR_BIN=$STR_BIN 不是可用的 STR CLI（期望 \`--version\` 输出 str x.y.z）"
fi

# 2 — PATH
if have_cmd str; then
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
    log "ensure-str: STR_REPO=$STR_REPO 下既无产物也未能构建，继续尝试下载"
fi

# 4 — 从脚本目录与 CWD 向上逐级查找源码仓库
_here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P) || _here=""
_cwd=$(pwd -P)
for _start in "$_here" "$_cwd"; do
    [ -n "$_start" ] || continue
    _dir=$_start
    while [ "$_dir" != "/" ]; do
        if try_build "$_dir"; then
            exit 0
        fi
        _dir=$(dirname -- "$_dir")
    done
done

# 5 — GitHub Releases 预编译二进制
if try_download; then
    exit 0
fi

# 6 — 全部失败
die "未找到 STR CLI，也无法自动获取。任选其一后重试：
  ① 下载预编译二进制：https://github.com/$RELEASE_REPO/releases
  ② 在源码仓库执行：cargo install --path str-cli
  ③ 显式指定：export STR_BIN=<路径>/str  或  export STR_REPO=<源码仓库根>
  ④ 检查网络 / 代理；也可设 STR_VERSION=<tag>、STR_RELEASE_REPO=<owner/repo>
  ⑤ 离线环境：若已手动放好二进制，用 STR_BIN 指过去，或设 STR_NO_DOWNLOAD=1 只走本地查找
注意：严禁以手工编辑 ._meta 代替 CLI —— 那是格式违规，不是降级方案。"
