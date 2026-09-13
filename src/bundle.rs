//! bundle 遍历与索引。
//!
//! 「分支」的唯一判据是**该目录是否含 `._meta`**（规范 3.4 / 4.6）；
//! 层级关系由「目录结构 + 父级 `entries`」唯一决定（规范 5.1）。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::error::{Error, Issue, Result, code};
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
    fn contains_meta_deeper(&self, dir: &Path) -> bool {
        for entry in walkdir::WalkDir::new(dir)
            .max_depth(8)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !util::is_os_noise(&e.file_name().to_string_lossy()))
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
            });
        }

        let (root_parsed, mut root_issues) = match self.read_meta(&self.root)? {
            MetaLoad::Ok(m, i) => (Some(m), i),
            MetaLoad::Failed(i) => (None, i),
        };
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
            for child in self.child_dirs(&dir)? {
                let name = child
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if util::is_os_noise(&name) {
                    continue;
                }
                let rel = self.rel(&child);
                if !self.has_meta(&child) {
                    // 普通内容容器；若其中藏有分支则层级无法建立
                    if !util::is_reserved_name(&name) && self.contains_meta_deeper(&child) {
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
        })
    }
}
