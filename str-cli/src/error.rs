//! 错误码与校验报告。
//!
//! 错误码与规范第 6.1 章一一对应。清单类问题（`E_MANIFEST_*`）在
//! `policies.manifest = "advisory"` 时降级为对应的 `W_MANIFEST_*`。

use serde::Serialize;

/// 问题级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// 阻断性错误。
    Error,
    /// 非阻断性告警。
    Warn,
}

impl Level {
    /// 输出用字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
        }
    }
}

/// 规范 6.1 章的错误码常量。
pub mod code {
    // ── 结构 / 身份 ──────────────────────────────────────────
    /// `._meta` 非法 TOML / 编码非 UTF-8 / 含 BOM / 时间未带时区偏移。
    pub const PARSE: &str = "E_PARSE";
    /// 分支目录缺失 `._meta`。
    pub const META_MISSING: &str = "E_META_MISSING";
    /// `str` 主版本不受支持。
    pub const SPEC_UNSUPPORTED: &str = "E_SPEC_UNSUPPORTED";
    /// `kind` 非法。
    pub const KIND_INVALID: &str = "E_KIND_INVALID";
    /// `kind` 与所在深度不符。
    pub const KIND_DEPTH: &str = "E_KIND_DEPTH";
    /// 必填字段缺失 / 类型不符 / 未知字段。
    pub const SCHEMA_FIELD: &str = "E_SCHEMA_FIELD";
    /// `id` 与所在目录名不一致。
    pub const ID_MISMATCH: &str = "E_ID_MISMATCH";
    /// 分支目录名不是合法 UUID。
    pub const ID_NOT_UUID: &str = "E_ID_NOT_UUID";
    /// UUID 版本与 `policies.id_version` 不符。
    pub const ID_VERSION: &str = "E_ID_VERSION";
    /// 同一 bundle 内出现重复分支 `id`。
    pub const ID_DUP: &str = "E_ID_DUP";
    /// 父级 `entries[].role` 与子目录实际内容或深度不符。
    pub const ENTRY_ROLE_DEPTH: &str = "E_ENTRY_ROLE_DEPTH";
    /// `entries[].id` 与子目录名不一致。
    pub const ENTRY_ID_MISMATCH: &str = "E_ENTRY_ID_MISMATCH";
    /// 分支树深度超过 `max_depth`。
    pub const DEPTH_EXCEEDED: &str = "E_DEPTH_EXCEEDED";

    // ── 引用 ────────────────────────────────────────────────
    /// `refs[].target` 无法解析。
    pub const REF_NO_TARGET: &str = "E_REF_NO_TARGET";
    /// `refs[].target` 指向自身。
    pub const REF_SELF: &str = "E_REF_SELF";
    /// `refs` 关联图成环。
    pub const REF_CYCLE: &str = "E_REF_CYCLE";

    // ── 清单 ────────────────────────────────────────────────
    /// 磁盘存在但 `entries` 未登记。
    pub const MANIFEST_MISSING: &str = "E_MANIFEST_MISSING";
    /// `entries` 登记但磁盘不存在。
    pub const MANIFEST_GHOST: &str = "E_MANIFEST_GHOST";
    /// `size` / `sha256` 与实际不符。
    pub const MANIFEST_HASH: &str = "E_MANIFEST_HASH";
    /// 文件类条目缺少 `size` / `sha256`。
    pub const MANIFEST_DIGEST_MISSING: &str = "E_MANIFEST_DIGEST_MISSING";
    /// `entries[].path` 重复。
    pub const MANIFEST_DUP: &str = "E_MANIFEST_DUP";
    /// 业务条目以 `._` 开头。
    pub const RESERVED_NAME: &str = "E_RESERVED_NAME";
    /// `revision` 非递增整数，或 `updated_at` 早于 `created_at`。
    pub const REVISION_STALE: &str = "E_REVISION_STALE";
    /// payload 不满足其声明的 JSON Schema。
    pub const SCHEMA_FAIL: &str = "E_SCHEMA_FAIL";

    // ── 告警 ────────────────────────────────────────────────
    /// 根目录名未以 `.str` 结尾。
    pub const BUNDLE_SUFFIX: &str = "W_BUNDLE_SUFFIX";
    /// 出现非 `._meta` / `.lock` 的点文件。
    pub const DOTFILE: &str = "W_DOTFILE";
    /// ROOT 下出现既非分支目录也非保留名的条目。
    pub const ROOT_STRAY: &str = "W_ROOT_STRAY";
    /// 缺 `summary`。
    pub const NO_SUMMARY: &str = "W_NO_SUMMARY";
    /// 缺 `type`。
    pub const NO_TYPE: &str = "W_NO_TYPE";
    /// 分支树深度超过 `deep_tree_warn`。
    pub const DEEP_TREE: &str = "W_DEEP_TREE";
    /// 单文件超过 `large_asset_bytes`。
    pub const LARGE_ASSET: &str = "W_LARGE_ASSET";
    /// `optional: true` 的条目实际缺失。
    pub const OPTIONAL_MISSING: &str = "W_OPTIONAL_MISSING";
    /// `manifest = advisory` 时的未登记条目。
    pub const MANIFEST_MISSING_W: &str = "W_MANIFEST_MISSING";
    /// `manifest = advisory` 时的幽灵条目。
    pub const MANIFEST_GHOST_W: &str = "W_MANIFEST_GHOST";
    /// `manifest = advisory` 时的指纹不符。
    pub const MANIFEST_HASH_W: &str = "W_MANIFEST_HASH";

    /// 全部错误码（测试矩阵与 `--list-codes` 使用）。
    pub const ALL: &[&str] = &[
        PARSE,
        META_MISSING,
        SPEC_UNSUPPORTED,
        KIND_INVALID,
        KIND_DEPTH,
        SCHEMA_FIELD,
        ID_MISMATCH,
        ID_NOT_UUID,
        ID_VERSION,
        ID_DUP,
        ENTRY_ROLE_DEPTH,
        ENTRY_ID_MISMATCH,
        DEPTH_EXCEEDED,
        REF_NO_TARGET,
        REF_SELF,
        REF_CYCLE,
        MANIFEST_MISSING,
        MANIFEST_GHOST,
        MANIFEST_HASH,
        MANIFEST_DIGEST_MISSING,
        MANIFEST_DUP,
        RESERVED_NAME,
        REVISION_STALE,
        SCHEMA_FAIL,
        BUNDLE_SUFFIX,
        DOTFILE,
        ROOT_STRAY,
        NO_SUMMARY,
        NO_TYPE,
        DEEP_TREE,
        LARGE_ASSET,
        OPTIONAL_MISSING,
        MANIFEST_MISSING_W,
        MANIFEST_GHOST_W,
        MANIFEST_HASH_W,
    ];
}

/// 单条校验问题。
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    /// 错误码。
    pub code: String,
    /// 级别。
    pub level: Level,
    /// 定位路径（bundle 内相对路径）。
    pub path: String,
    /// 人类可读说明。
    pub message: String,
}

impl Issue {
    /// 构造一条问题。
    pub fn new(
        code: &'static str,
        level: Level,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            level,
            path: path.into(),
            message: message.into(),
        }
    }

    /// 构造一条 error。
    pub fn error(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, Level::Error, path, message)
    }

    /// 构造一条 warning。
    pub fn warn(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, Level::Warn, path, message)
    }
}

/// 统计信息。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    /// 独立节点数（深度 1 的分支）。
    pub nodes: usize,
    /// 关联分支数（深度 ≥2 的分支）。
    pub branches: usize,
    /// 已登记条目总数。
    pub entries: usize,
    /// 实测分支树最大深度。
    pub depth: usize,
}

/// 一次校验 / 遍历的完整报告。
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// bundle 名称。
    pub bundle: String,
    /// 全部问题，按产生顺序排列。
    pub issues: Vec<Issue>,
    /// 统计信息。
    pub stats: Stats,
}

impl Report {
    /// 追加一条问题。
    pub fn push(&mut self, issue: Issue) {
        self.issues.push(issue);
    }

    /// 全部错误。
    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Error)
    }

    /// 全部告警。
    pub fn warnings(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.level == Level::Warn)
    }

    /// 错误数。
    pub fn error_count(&self) -> usize {
        self.errors().count()
    }

    /// 告警数。
    pub fn warning_count(&self) -> usize {
        self.warnings().count()
    }

    /// 退出码：有 error 为 1，否则 0。
    pub fn exit_code(&self) -> i32 {
        if self.error_count() > 0 { 1 } else { 0 }
    }

    /// 规范 6.2 规定的 `--json` 结构。
    pub fn to_json(&self) -> serde_json::Value {
        let conv = |it: &Issue| {
            serde_json::json!({
                "code": it.code,
                "level": it.level.as_str(),
                "path": it.path,
                "message": it.message,
            })
        };
        serde_json::json!({
            "bundle": self.bundle,
            "errors": self.errors().map(conv).collect::<Vec<_>>(),
            "warnings": self.warnings().map(conv).collect::<Vec<_>>(),
            "stats": {
                "nodes": self.stats.nodes,
                "branches": self.stats.branches,
                "entries": self.stats.entries,
                "depth": self.stats.depth,
            },
        })
    }

    /// 人类可读输出（规范 6.2）。
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{}  {} nodes / {} branches / {} entries  depth={}\n",
            self.bundle, self.stats.nodes, self.stats.branches, self.stats.entries, self.stats.depth
        ));
        for it in &self.issues {
            let mark = match it.level {
                Level::Error => "✗",
                Level::Warn => "⚠",
            };
            out.push_str(&format!(
                "  {} {:<28} {}  {}\n",
                mark, it.code, it.path, it.message
            ));
        }
        out.push_str(&format!(
            "{} errors, {} warnings   exit={}",
            self.error_count(),
            self.warning_count(),
            self.exit_code()
        ));
        out
    }
}

/// 库级错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 路径不存在。
    #[error("路径不存在：{0}")]
    NotFound(String),
    /// 参数非法。
    #[error("参数错误：{0}")]
    BadArg(String),
    /// I/O 失败。
    #[error("I/O 错误（{path}）：{source}")]
    Io {
        /// 出错路径。
        path: String,
        /// 底层错误。
        source: std::io::Error,
    },
    /// 校验未通过。
    #[error("校验未通过：{0} 项错误")]
    Validation(usize),
    /// 其它。
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// 便捷构造 I/O 错误。
    pub fn io(path: impl AsRef<std::path::Path>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.as_ref().display().to_string(),
            source,
        }
    }
}

/// 库级结果类型。
pub type Result<T> = std::result::Result<T, Error>;
