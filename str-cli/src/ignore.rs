//! 忽略名单（规范 4.7 `policies.ignore` + `.gitignore` 自动检测）。
//!
//! 语义与 gitignore 对齐：
//! - `#` 注释、空行跳过；`\#` / `\!` 转义；
//! - `!` 前缀 = 取消忽略（negation）；
//! - 尾随 `/` = 仅匹配目录；
//! - 模式含 `/`（尾随除外）= 锚定到该 ignore 文件所在目录，否则匹配任意深度的同名条目；
//! - 同一文件内「最后一条命中的模式」决定结果；
//! - 多层 ignore 文件（外层 git 仓库 → bundle 根 → 分支目录 → `policies.ignore`）
//!   由外向内叠加，**内层（更靠近条目 / 显式策略）的命中覆盖外层**。
//!
//! 所有匹配均以 **bundle 内相对路径**（`/` 分隔）进行：外层 ignore 文件的锚定模式
//! 天然只在其覆盖范围内生效（bundle 相对路径是其真路径的后缀），与 git 行为一致。

use globset::{GlobBuilder, GlobMatcher};

/// 单条忽略模式。
#[derive(Debug, Clone)]
struct Pattern {
    /// `!` 前缀：命中则取消忽略。
    negated: bool,
    /// 尾随 `/`：仅匹配目录（目录内部由遍历剪枝覆盖）。
    dir_only: bool,
    matcher: GlobMatcher,
}

/// 一份 ignore 来源（`.gitignore` 文件或 `policies.ignore`）的模式表。
#[derive(Debug, Clone)]
struct Layer {
    /// 作用域：bundle 内相对目录（`""` = bundle 根 / 外层仓库层）。分支目录层用。
    scope: String,
    /// 外层仓库层专用：bundle 根相对该 ignore 文件所在目录的路径
    /// （如 bundle 位于 `<repo>/bundles/x.str`、文件在 `<repo>/.gitignore` 时为
    /// `bundles/x.str`），用于把锚定模式正确翻译到 bundle 相对坐标。
    prefix: String,
    patterns: Vec<Pattern>,
}

/// 解析一份 ignore 文本为模式表（gitignore 语义）。
fn parse_patterns(text: &str) -> Vec<Pattern> {
    let mut out = Vec::new();
    for raw in text.lines() {
        // 去掉行尾空白（git：除非反斜杠转义，此处按常见实现简化为直接去除）。
        let line = raw.trim_end_matches([' ', '\t']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `\#` / `\!` 转义：去掉反斜杠，保留字面 `#` / `!` 起始的名字。
        let line = if line.starts_with("\\#") || line.starts_with("\\!") {
            &line[1..]
        } else {
            line
        };
        let (negated, line) = match line.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        // 尾随 `/` → 仅目录；`/` 去重。
        let (dir_only, line) = match line.strip_suffix('/') {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        if line.is_empty() {
            continue;
        }
        // 含 `/`（去掉尾随后）→ 锚定；否则匹配任意深度的同名条目。
        let anchored = line.contains('/');
        let line = line.strip_prefix('/').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let glob = if anchored {
            line.to_string()
        } else {
            format!("**/{line}")
        };
        if let Ok(m) = GlobBuilder::new(&glob)
            .literal_separator(true)
            .case_insensitive(false)
            .build()
        {
            out.push(Pattern {
                negated,
                dir_only,
                matcher: m.compile_matcher(),
            });
        }
    }
    out
}

/// bundle 全局的忽略判定器。
#[derive(Debug, Clone, Default)]
pub struct IgnoreSet {
    /// 由外向内叠加的 ignore 层（内层命中覆盖外层）。
    layers: Vec<Layer>,
}

impl IgnoreSet {
    /// 追加一层 bundle 内的 ignore（`scope` = 所在分支目录的 bundle 相对路径，
    /// bundle 根传 `""`），`text` 为文件内容。
    pub fn push_layer(&mut self, scope: &str, text: &str) {
        self.layers.push(Layer {
            scope: scope.trim_matches('/').to_string(),
            prefix: String::new(),
            patterns: parse_patterns(text),
        });
    }

    /// 追加一层 bundle 外的 ignore（如外层 git 仓库的 `.gitignore`）。
    /// `prefix` = bundle 根相对该文件所在目录的路径（ignore 目录为 bundle 祖先时
    /// 即中间路径；空串表示就是 bundle 根）。
    pub fn push_outer(&mut self, prefix: &str, text: &str) {
        self.layers.push(Layer {
            scope: String::new(),
            prefix: prefix.trim_matches('/').to_string(),
            patterns: parse_patterns(text),
        });
    }

    /// 是否没有任何生效模式（调用方可据此跳过匹配）。
    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(|l| l.patterns.is_empty())
    }

    /// 判定 bundle 内相对路径 `rel`（`/` 分隔）是否被忽略。
    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        let mut ignored = false;
        for layer in &self.layers {
            // 外层文件：锚定模式相对其所在目录解析 → 加回 bundle 根前缀；
            // 分支目录层：模式相对该目录解析 → 剥掉作用域前缀。
            let sub: String = if !layer.prefix.is_empty() {
                format!("{}/{}", layer.prefix, rel)
            } else if layer.scope.is_empty() {
                rel.to_string()
            } else {
                let scope = format!("{}/", layer.scope);
                match rel.strip_prefix(scope.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => continue,
                }
            };
            // git 语义：同一文件内「最后一条命中的模式」决定；内层文件覆盖外层。
            let mut decided: Option<bool> = None;
            for p in &layer.patterns {
                if p.dir_only && !is_dir {
                    continue;
                }
                if p.matcher.is_match(&sub) {
                    decided = Some(!p.negated);
                }
            }
            if let Some(d) = decided {
                ignored = d;
            }
        }
        ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(layers: &[(&str, &str)]) -> IgnoreSet {
        let mut s = IgnoreSet::default();
        for (scope, text) in layers {
            s.push_layer(scope, text);
        }
        s
    }

    #[test]
    fn basename_matches_any_depth() {
        let s = set(&[("", "*.log\nbuild\n")]);
        assert!(s.is_ignored("a.log", false));
        assert!(s.is_ignored("x/y/a.log", false));
        assert!(s.is_ignored("build", true));
        assert!(s.is_ignored("x/build", true));
        assert!(!s.is_ignored("builder", true));
    }

    #[test]
    fn anchored_and_dir_only() {
        let s = set(&[("", "logs/\n/foo\nsub/bar.txt\n")]);
        assert!(s.is_ignored("logs", true));
        assert!(s.is_ignored("a/logs", true));
        assert!(!s.is_ignored("logs", false)); // 目录专用模式不匹配文件
        assert!(s.is_ignored("foo", false));
        assert!(!s.is_ignored("x/foo", false)); // 锚定：仅 bundle 根下的 foo
        assert!(s.is_ignored("sub/bar.txt", false));
        assert!(!s.is_ignored("other/bar.txt", false));
    }

    #[test]
    fn last_match_and_negation() {
        let s = set(&[("", "*.log\n!keep.log\n")]);
        assert!(s.is_ignored("a.log", false));
        assert!(!s.is_ignored("keep.log", false));
    }

    #[test]
    fn inner_layer_overrides_outer() {
        // 外层忽略全部 .tmp；bundle 根的 .gitignore 取消忽略 final.tmp。
        let s = set(&[("", "*.tmp\n"), ("", "!final.tmp\n")]);
        assert!(s.is_ignored("a.tmp", false));
        assert!(!s.is_ignored("final.tmp", false));
    }

    #[test]
    fn scope_limits_layer() {
        // 作用域在分支 a/b/ 的层只影响其子树。
        let s = set(&[("a/b", "cache\n")]);
        assert!(s.is_ignored("a/b/cache", true));
        assert!(!s.is_ignored("a/cache", true));
        assert!(!s.is_ignored("cache", true));
    }

    #[test]
    fn comments_and_escapes() {
        let s = set(&[("", "# 注释\n\\#hash.txt\n")]);
        assert!(!s.is_ignored("hash.txt", false));
        assert!(s.is_ignored("#hash.txt", false));
    }

    #[test]
    fn outer_layer_anchoring() {
        let mut s = IgnoreSet::default();
        // bundle 在 <repo>/bundles/x.str；repo 根 .gitignore 有锚定 /target 与 basename *.log。
        s.push_outer("bundles/x.str", "/target\n*.log\n");
        // 锚定模式只作用于 <repo>/target，不影响 bundle 内的 target。
        assert!(!s.is_ignored("target", true));
        // basename 模式任意深度生效。
        assert!(s.is_ignored("a/b/c.log", false));
        // 锚定模式命中 bundle 内条目（真实路径 <repo>/bundles/x.str/foo）。
        let mut s2 = IgnoreSet::default();
        s2.push_outer("bundles/x.str", "/bundles/x.str/foo\n");
        assert!(s2.is_ignored("foo", false));
    }
}
