//! bundle 遍历与索引。
//!
//! 「分支」的唯一判据是**该目录是否含 `._meta`**（规范 3.4 / 4.6）；
//! 层级关系由「目录结构 + 父级 `entries`」唯一决定（规范 5.1）。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::error::{Error, Issue, Result, code};
use crate::ignore::IgnoreSet;
use crate::meta::{Meta, MetaLoad, load};
use crate::util::{self, META_FILE};

/// 下钻硬上限（防止病态目录结构导致遍历失控；策略级 `max_depth` 由校验器判定）。
pub const HARD_MAX_DEPTH: usize = 128;

/// 一处被访问到的分支目录。
pub struct Visit {
    /// 目录绝对/原始路径。
    pub dir: PathBuf,
    /// bundle 内相对路径（ROOT 为 `.`）。
    pub rel: String,
    /// 深度（ROOT = 0）。
    pub depth: usize,
    /// 解析结果；`None` 表示 `._meta` 解析失败。
    pub meta: Option<Box<Meta>>,
    /// 父分支在 `visits` 中的下标。
    pub parent: Option<usize>,
    /// 原始 `._meta` 文本是否读取成功。
    pub readable: bool,
}

/// 一次完整扫描的结果。
pub struct Scan {
    /// bundle 根目录。
    pub root: PathBuf,
    /// 全部分支目录（下标 0 为 ROOT，若存在）。
    pub visits: Vec<Visit>,
    /// 扫描阶段产生的问题（解析层）。
    pub issues: Vec<Issue>,
    /// `id` → `visits` 下标列表（用于重复检测与引用解析）。
    pub by_id: HashMap<String, Vec<usize>>,
    /// ROOT 是否成功解析。
    pub root_index: Option<usize>,
    /// 实测最大深度。
    pub max_depth: usize,
    /// 本次扫描生效的忽略名单（`policies.ignore` + `.gitignore`，规范 4.7）。
    pub ignore: IgnoreSet,
}

impl Scan {
    /// 按 `id` 解析分支下标（首次命中）。
    pub fn resolve(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).and_then(|v| v.first().copied())
    }

    /// 分支的完整链条（从 ROOT 到自身）。
    pub fn ancestors(&self, mut idx: usize) -> Vec<usize> {
        let mut out = vec![idx];
        while let Some(p) = self.visits[idx].parent {
            out.push(p);
            idx = p;
        }
        out.reverse();
        out
    }
}

/// 一个 `.str` bundle。
#[derive(Debug, Clone)]
pub struct Bundle {
    /// 根目录。
    pub root: PathBuf,
}

impl Bundle {
    /// 构造并确认路径存在且为目录。
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.exists() {
            return Err(Error::NotFound(root.display().to_string()));
        }
        if !root.is_dir() {
            return Err(Error::BadArg(format!(
                "{} 不是目录（`.str` 是目录 bundle）",
                root.display()
            )));
        }
        // 归一化为绝对路径：否则以 `.` / `..` 形式传入时取不到目录名，
        // 会误报 `W_BUNDLE_SUFFIX`（`str validate .` 的实际场景）。
        let root = std::fs::canonicalize(&root).map_err(|e| Error::io(&root, e))?;
        Ok(Self { root })
    }

    /// bundle 目录名（不含路径）。
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    /// 某目录下 `._meta` 的路径。
    pub fn meta_path(&self, dir: &Path) -> PathBuf {
        dir.join(META_FILE)
    }

    /// 某目录是否含 `._meta`（即是否为分支）。
    pub fn has_meta(&self, dir: &Path) -> bool {
        self.meta_path(dir).is_file()
    }

    /// 相对路径显示。
    pub fn rel(&self, path: &Path) -> String {
        util::rel_display(&self.root, path)
    }

    /// 读取并解析某目录的 `._meta`。
    pub fn read_meta(&self, dir: &Path) -> Result<MetaLoad> {
        let p = self.meta_path(dir);
        let rel = self.rel(&p);
        load(&p, &rel)
    }

    /// 列出目录内全部条目 `(名字, 是否目录)`，按名字排序。
    pub fn list_names(&self, dir: &Path) -> Result<Vec<(String, bool)>> {
        let mut out = Vec::new();
        let rd = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
        for ent in rd {
            let ent = ent.map_err(|e| Error::io(dir, e))?;
            let name = ent.file_name().to_string_lossy().to_string();
            let is_dir = ent
                .file_type()
                .map(|t| t.is_dir())
                .unwrap_or(false);
            out.push((name, is_dir));
        }
        out.sort();
        Ok(out)
    }

    /// 仅列出子目录。
    pub fn child_dirs(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        Ok(self
            .list_names(dir)?
            .into_iter()
            .filter(|(_, is_dir)| *is_dir)
            .map(|(n, _)| dir.join(n))
            .collect())
    }

    /// 在**非分支**目录中查找是否藏有 `._meta`（层级异常，规范 4.6）。
    ///
    /// `rel_prefix` 为该目录的 bundle 相对路径（ROOT 传 `""`）；`ignore` 非空时，
    /// 被忽略的子项同样剪枝（与分支遍历语义一致）。
    fn contains_meta_deeper(&self, dir: &Path, rel_prefix: &str, ignore: &IgnoreSet) -> bool {
        for entry in walkdir::WalkDir::new(dir)
            .max_depth(8)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy().to_string();
                // 不进入独立子 bundle（`.str` 目录是硬边界，规范 3.5）
                if util::is_os_noise(&name) || util::is_sub_bundle(&name) {
                    return false;
                }
                if ignore.is_empty() {
                    return true;
                }
                let rel = match util::rel_display(dir, e.path()) {
                    s if s == "." => rel_prefix.to_string(),
                    s if rel_prefix.is_empty() => s,
                    s => format!("{rel_prefix}/{s}"),
                };
                let is_dir = e.file_type().is_dir();
                !ignore.is_ignored(&rel, is_dir)
            })
            .flatten()
        {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                continue;
            }
            if entry.path().join(META_FILE).is_file() {
                return true;
            }
        }
        false
    }

    /// 构建 bundle 全局的忽略名单（规范 4.7）。
    ///
    /// 叠加顺序（由外向内，内层命中覆盖外层）：
    /// ① 外层 git 仓库的 `.gitignore`（自最外层向 bundle 根收集，至 worktree 根为止）；
    /// ② bundle 根的 `.gitignore`；
    /// ③ ROOT `[policies].ignore`（显式配置，优先级最高）。
    /// `policies.gitignore = false` 时跳过 ①②（分支目录内的 `.gitignore` 亦不检测）。
    fn build_ignore(&self, root_meta: Option<&Meta>) -> IgnoreSet {
        let mut set = IgnoreSet::default();
        let policies = root_meta.map(|m| &m.policies);
        let auto = policies.map(|p| p.gitignore).unwrap_or(true);
        if auto {
            // 外层 `.gitignore`：自 bundle 父目录向上收集到 worktree 根（含 `.git` 的
            // 目录仍收集自身），越外层越先入栈 → 越靠近 bundle 的命中越优先。
            let mut chain: Vec<PathBuf> = Vec::new();
            let mut cur = self.root.parent();
            while let Some(d) = cur {
                chain.push(d.to_path_buf());
                if d.join(".git").exists() || chain.len() >= 32 {
                    break;
                }
                cur = d.parent();
            }
            for d in chain.into_iter().rev() {
                if let Ok(text) = std::fs::read_to_string(d.join(".gitignore")) {
                    // prefix = bundle 根相对该 ignore 目录的中间路径。
                    let prefix = self
                        .root
                        .strip_prefix(d)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    set.push_outer(&prefix, &text);
                }
            }
            if let Ok(text) = std::fs::read_to_string(self.root.join(".gitignore")) {
                set.push_layer("", &text);
            }
        }
        if let Some(p) = policies {
            if !p.ignore.is_empty() {
                set.push_layer("", &p.ignore.join("\n"));
            }
        }
        set
    }

    /// 扫描整棵分支树。
    pub fn scan(&self) -> Result<Scan> {
        let mut visits: Vec<Visit> = Vec::new();
        let mut issues: Vec<Issue> = Vec::new();
        let mut by_id: HashMap<String, Vec<usize>> = HashMap::new();

        let root_meta = self.meta_path(&self.root);
        if !root_meta.is_file() {
            issues.push(Issue::error(
                code::META_MISSING,
                ".",
                "bundle 根目录缺少 `._meta`",
            ));
            return Ok(Scan {
                root: self.root.clone(),
                visits,
                issues,
                by_id,
                root_index: None,
                max_depth: 0,
                ignore: IgnoreSet::default(),
            });
        }

        let (root_parsed, mut root_issues) = match self.read_meta(&self.root)? {
            MetaLoad::Ok(m, i) => (Some(m), i),
            MetaLoad::Failed(i) => (None, i),
        };
        let mut ignore = self.build_ignore(root_parsed.as_deref());
        issues.append(&mut root_issues);
        if let Some(m) = &root_parsed {
            if let Some(id) = &m.id {
                by_id.entry(id.clone()).or_default().push(0);
            }
        }
        visits.push(Visit {
            dir: self.root.clone(),
            rel: ".".to_string(),
            depth: 0,
            meta: root_parsed,
            parent: None,
            readable: true,
        });

        let mut queue: VecDeque<usize> = VecDeque::from([0usize]);
        let mut max_depth = 0usize;

        while let Some(cur) = queue.pop_front() {
            let dir = visits[cur].dir.clone();
            let depth = visits[cur].depth;
            if depth >= HARD_MAX_DEPTH {
                continue;
            }
            // 分支目录内的 `.gitignore` 动态入栈（BFS 深度序天然满足「内层覆盖外层」）。
            if depth >= 1 && !visits[cur].rel.is_empty() {
                if let Ok(text) = std::fs::read_to_string(dir.join(".gitignore")) {
                    ignore.push_layer(&visits[cur].rel, &text);
                }
            }
            for child in self.child_dirs(&dir)? {
                let name = child
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if util::is_os_noise(&name) {
                    continue;
                }
                if util::is_sub_bundle(&name) {
                    // 独立子 bundle：硬边界，父 bundle 不进入、不参与分支树
                    continue;
                }
                let rel = self.rel(&child);
                // 忽略名单命中（含目录专用模式）：不遍历、不参与清单比对（规范 4.7）。
                if !ignore.is_empty() && ignore.is_ignored(&rel, true) {
                    continue;
                }
                if !self.has_meta(&child) {
                    // 普通内容容器；若其中藏有分支则层级无法建立
                    if !util::is_reserved_name(&name) && self.contains_meta_deeper(&child, &rel, &ignore) {
                        issues.push(Issue::error(
                            code::META_MISSING,
                            rel,
                            "父目录不是分支（缺少 `._meta`），其内出现 `._meta`，无法建立分支层级",
                        ));
                    }
                    continue;
                }
                let d = depth + 1;
                let (parsed, mut iss) = match self.read_meta(&child)? {
                    MetaLoad::Ok(m, i) => (Some(m), i),
                    MetaLoad::Failed(i) => (None, i),
                };
                issues.append(&mut iss);
                let idx = visits.len();
                if let Some(m) = &parsed {
                    if let Some(id) = &m.id {
                        by_id.entry(id.clone()).or_default().push(idx);
                    }
                }
                visits.push(Visit {
                    dir: child.clone(),
                    rel,
                    depth: d,
                    meta: parsed,
                    parent: Some(cur),
                    readable: true,
                });
                max_depth = max_depth.max(d);
                queue.push_back(idx);
            }
        }

        Ok(Scan {
            root: self.root.clone(),
            visits,
            issues,
            by_id,
            root_index: Some(0),
            max_depth,
            ignore,
        })
    }
}
