//! 通用工具：UUID v7、SHA-256、时间与命名判定。

use sha2::{Digest, Sha256};
use std::path::{Component, Path};

/// 元数据文件名。
pub const META_FILE: &str = "._meta";
/// bundle 级 Schema 目录。
pub const SCHEMA_DIR: &str = "._schema";
/// 派生缓存目录（可删、建议 gitignore）。
pub const CACHE_DIR: &str = "._cache";
/// 短期写入锁文件名。
pub const LOCK_FILE: &str = ".lock";

/// 生成 UUID v7（时间有序）。
pub fn new_uuid_v7() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// 解析 UUID，返回版本号。
pub fn uuid_version(s: &str) -> Option<usize> {
    uuid::Uuid::parse_str(s).ok().map(|u| u.get_version_num())
}

/// 是否为合法 UUID 字面量。
pub fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

/// 字节数组 → 小写十六进制。
pub fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(TABLE[(b >> 4) as usize] as char);
        out.push(TABLE[(b & 0x0f) as usize] as char);
    }
    out
}

/// 计算字节串 SHA-256。
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

/// 计算文件 SHA-256。
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut file, &mut h)?;
    Ok(hex(&h.finalize()))
}

/// 当前 UTC 时间，RFC3339 形式（统一 `+00:00` 偏移）。
pub fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// 解析 RFC3339（offset date-time）。
pub fn parse_rfc3339(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// `dir` 相对 `root` 的深度（ROOT 自身为 0）。
pub fn relative_depth(root: &Path, dir: &Path) -> usize {
    dir.strip_prefix(root)
        .map(|rel| {
            rel.components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .count()
        })
        .unwrap_or(0)
}

/// 是否为格式保留名（以 `._` 开头）。
pub fn is_reserved_name(name: &str) -> bool {
    name.starts_with("._")
}

/// 是否为 `._meta`。
pub fn is_meta_file(name: &str) -> bool {
    name == META_FILE
}

/// 是否为 `.lock`。
pub fn is_lock_file(name: &str) -> bool {
    name == LOCK_FILE
}

/// 是否为操作系统 / 工具元数据：一律豁免，不参与校验（规范 3.4）。
///
/// - `._*` 形式的**普通文件**是 macOS AppleDouble 伴生文件（`._meta` 本身不是噪声）；
/// - `.git` / `.gitignore` / `.hg` / `.svn` 等是版本控制元数据 —— 真实项目必然存在；
/// - `.github/` 是代码托管平台的元数据：GitHub Actions 的工作流**必须**位于
///   `.github/workflows/`（路径不可改名），同属「工具元数据」，故一并豁免。
pub fn is_os_noise(name: &str) -> bool {
    matches!(
        name,
        ".DS_Store"
            | "Thumbs.db"
            | "desktop.ini"
            | ".git"
            | ".gitignore"
            | ".gitattributes"
            | ".gitmodules"
            | ".gitkeep"
            | ".github"
            | ".hg"
            | ".hgignore"
            | ".svn"
            | ".jj"
    ) || (name.starts_with("._") && name != META_FILE)
}

/// 是否为**独立子 bundle**：目录名以 `.str` 结尾。
///
/// `.str` 目录是 bundle 的**硬边界**：父 bundle 不进入、不把它当作分支
/// （同 `.app` 嵌套语义）。父级 `entries` 中应表达为 `role = "bundle"`。
pub fn is_sub_bundle(name: &str) -> bool {
    name.ends_with(".str")
}

/// 是否为其它点文件（非 `._meta` / `.lock`）→ `W_DOTFILE`。
pub fn is_other_dotfile(name: &str) -> bool {
    name.starts_with('.') && !is_meta_file(name) && !is_lock_file(name)
}

/// 便捷：路径显示为 bundle 内相对路径。
pub fn rel_display(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let s = rel.display().to_string();
    if s.is_empty() { ".".to_string() } else { s }
}
