//! 规范第 6 章全部错误码的实现。
//!
//! 覆盖四类规则：结构 / 身份、清单一致性、跨枝引用、JSON Schema。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value as JValue;

use crate::bundle::{Bundle, Scan, Visit};
use crate::error::{Issue, Report, Result, Stats, code};
use crate::meta::{Entry, Kind, ManifestPolicy, Meta, Policies, ShaPolicy};
use crate::util::{self, SCHEMA_DIR};

/// 校验入口。
pub fn validate(bundle: &Bundle) -> Result<Report> {
    let scan = bundle.scan()?;
    let mut checker = Checker::new(bundle, &scan);
    checker.run();
    checker.check_revision_history();
    Ok(checker.report)
}

/// 校验器内部状态。
struct Checker<'a> {
    bundle: &'a Bundle,
    scan: &'a Scan,
    report: Report,
    policies: Policies,
}

impl<'a> Checker<'a> {
    fn new(bundle: &'a Bundle, scan: &'a Scan) -> Self {
        let policies = scan
            .root_index
            .and_then(|i| scan.visits[i].meta.as_ref())
            .map(|m| m.policies.clone())
            .unwrap_or_default();
        let mut report = Report {
            bundle: bundle.name(),
            issues: Vec::new(),
            stats: Stats::default(),
        };
        report.issues.extend(scan.issues.iter().cloned());
        Self {
            bundle,
            scan,
            report,
            policies,
        }
    }

    // ── 基础输出 ────────────────────────────────────────────

    fn strict(&self) -> bool {
        self.policies.manifest == ManifestPolicy::Strict
    }

    fn warn(&mut self, code: &'static str, path: impl Into<String>, msg: impl Into<String>) {
        self.report.push(Issue::warn(code, path, msg));
    }

    fn err(&mut self, code: &'static str, path: impl Into<String>, msg: impl Into<String>) {
        self.report.push(Issue::error(code, path, msg));
    }

    /// 清单类问题：`manifest = advisory` 时降级为告警（规范 4.8）。
    fn manifest_issue(
        &mut self,
        e_code: &'static str,
        w_code: &'static str,
        path: impl Into<String>,
        msg: impl Into<String>,
    ) {
        let path = path.into();
        let msg = msg.into();
        if self.strict() {
            self.err(e_code, path, msg);
        } else {
            self.warn(w_code, path, msg);
        }
    }

    // ── 主流程 ─────────────────────────────────────────────

    fn run(&mut self) {
        let scan = self.scan;
        let name = self.bundle.name();
        if !name.ends_with(".str") {
            self.warn(
                code::BUNDLE_SUFFIX,
                ".",
                format!("根目录名 {name:?} 未以 `.str` 结尾"),
            );
        }

        let Some(root_idx) = scan.root_index else {
            return;
        };
        self.report.stats.depth = scan.max_depth;

        for v in &scan.visits {
            match v.depth {
                0 => {}
                1 => self.report.stats.nodes += 1,
                _ => self.report.stats.branches += 1,
            }
            if let Some(m) = &v.meta {
                self.report.stats.entries += m.entries.len();
            }
        }

        // 重复 id
        let dups: Vec<(String, String, usize)> = scan
            .by_id
            .iter()
            .filter(|(_, idxs)| idxs.len() > 1)
            .map(|(id, idxs)| {
                (
                    id.clone(),
                    scan.visits[idxs[0]].rel.clone(),
                    idxs.len(),
                )
            })
            .collect();
        for (id, path, n) in dups {
            self.err(code::ID_DUP, path, format!("id `{id}` 在 {n} 处重复出现"));
        }

        // ROOT 自身要求
        if let Some(m) = scan.visits[root_idx].meta.as_ref() {
            if m.kind != Some(Kind::Root) {
                self.err(
                    code::KIND_DEPTH,
                    ".",
                    format!("深度 0 的 `kind` 必须是 `root`，实为 {:?}", m.kind_raw),
                );
            }
            if m.name.is_none() {
                self.err(code::SCHEMA_FIELD, ".", "root 缺少必填字段 `name`");
            }
        }

        for i in 0..scan.visits.len() {
            self.check_branch(i);
        }

        self.check_ref_cycles();
        self.check_meta_schemas();
    }

    /// 单个分支目录的结构 / 身份 / 引用规则。
    fn check_branch(&mut self, idx: usize) {
        let scan = self.scan;
        let v: &Visit = &scan.visits[idx];
        let rel = v.rel.clone();
        let depth = v.depth;
        let Some(meta) = v.meta.as_ref() else {
            return; // 解析失败已在扫描期报出
        };

        // 档位 vs 深度
        let expected_kind = match depth {
            0 => Kind::Root,
            1 => Kind::Node,
            _ => Kind::Branch,
        };
        if let Some(k) = meta.kind {
            if k != expected_kind {
                self.err(
                    code::KIND_DEPTH,
                    rel.clone(),
                    format!(
                        "深度 {depth} 的 `kind` 必须是 `{}`，实为 `{}`",
                        expected_kind.as_str(),
                        k.as_str()
                    ),
                );
            }
        }

        if let Some(ver) = meta.str_version {
            if ver != crate::STR_MAJOR {
                self.err(
                    code::SPEC_UNSUPPORTED,
                    rel.clone(),
                    format!("`str` = {ver}，本实现只支持主版本 {}", crate::STR_MAJOR),
                );
            }
        }

        // 目录名 vs id
        let dir_name = v
            .dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if depth >= 1 {
            if !util::is_uuid(&dir_name) {
                self.err(
                    code::ID_NOT_UUID,
                    rel.clone(),
                    format!("分支目录名 {dir_name:?} 不是合法 UUID"),
                );
            } else if let Some(ver) = util::uuid_version(&dir_name) {
                if ver != self.policies.id_version {
                    self.err(
                        code::ID_VERSION,
                        rel.clone(),
                        format!(
                            "目录名 UUID 版本为 {ver}，`policies.id_version` 要求 {}",
                            self.policies.id_version
                        ),
                    );
                }
            }
            if let Some(id) = meta.id.as_deref() {
                if id != dir_name {
                    self.err(
                        code::ID_MISMATCH,
                        rel.clone(),
                        format!("`id` = {id:?} 与目录名 {dir_name:?} 不一致"),
                    );
                }
            }
        }

        // 深度
        if depth > self.policies.max_depth {
            self.err(
                code::DEPTH_EXCEEDED,
                rel.clone(),
                format!(
                    "深度 {depth} 超过 `policies.max_depth` = {}",
                    self.policies.max_depth
                ),
            );
        } else if depth > self.policies.deep_tree_warn {
            self.warn(
                code::DEEP_TREE,
                rel.clone(),
                format!(
                    "深度 {depth} 超过 `policies.deep_tree_warn` = {}，建议拆分",
                    self.policies.deep_tree_warn
                ),
            );
        }

        if depth >= 1 && meta.summary.is_none() {
            self.warn(
                code::NO_SUMMARY,
                rel.clone(),
                "建议补充 `summary`（AI 检索依据）",
            );
        }
        if depth >= 1 && meta.r#type.is_none() {
            self.warn(code::NO_TYPE, rel.clone(), "建议补充 `type`");
        }

        for issue in meta.revision_issues(&rel) {
            self.report.push(issue);
        }

        // 跨枝引用
        let own_id = meta.id.clone();
        let refs = meta.refs.clone();
        let mut bad_targets: Vec<(usize, String, String)> = Vec::new();
        for (n, r) in refs.iter().enumerate() {
            if Some(r.target.as_str()) == own_id.as_deref() {
                self.err(
                    code::REF_SELF,
                    format!("{rel} #refs[{n}]"),
                    "关联线指向自身",
                );
                continue;
            }
            if !r.target.is_empty() && scan.resolve(&r.target).is_none() {
                bad_targets.push((n, r.target.clone(), rel.clone()));
            }
        }
        for (n, target, rel) in bad_targets {
            self.err(
                code::REF_NO_TARGET,
                format!("{rel} #refs[{n}]"),
                format!("`target` = {target:?} 无法在本 bundle 内解析到任何分支"),
            );
        }

        // 清单
        self.check_manifest(idx);
    }

    /// 清单一致性（规范 4.8）与角色 / 深度一致性。
    fn check_manifest(&mut self, idx: usize) {
        let scan = self.scan;
        let bundle = self.bundle;
        let v: &Visit = &scan.visits[idx];
        let rel = v.rel.clone();
        let dir = v.dir.clone();
        let meta: &Meta = match v.meta.as_ref() {
            Some(m) => m,
            None => return,
        };
        let entries: Vec<Entry> = meta.entries.clone();

        // path 重复
        let mut seen: HashSet<String> = HashSet::new();
        let mut dups: Vec<String> = Vec::new();
        for e in &entries {
            if !seen.insert(e.path.clone()) {
                dups.push(e.path.clone());
            }
        }
        for p in dups {
            self.err(
                code::MANIFEST_DUP,
                format!("{rel} #entries"),
                format!("`entries[].path` = {p:?} 重复"),
            );
        }

        let actual = bundle.list_names(&dir).unwrap_or_default();

        // 实际侧 → 逐条比对
        for (name, is_dir) in &actual {
            let ep = format!("{rel}/{name}");
            if util::is_meta_file(name) || util::is_lock_file(name) {
                continue; // `._meta` / `.lock` 不参与清单
            }
            if util::is_reserved_name(name) {
                // 保留命名空间判定（规范 v1.12.0 §1.3 约束 5 / §4.8：保留目录免登记、不参与清单比对）：
                // - `._schema` / `._cache` 是已知保留目录 → **免登记**、放行（显式登记亦合法，属可选声明）；
                // - 业务**目录**以 `._` 开头 → 必然不是 AppleDouble（该机制只产生文件）→ `E_RESERVED_NAME`；
                // - 其余以 `._` 开头且**已登记**或为**目录**者 → 作者显式声明为业务内容 → `E_RESERVED_NAME`；
                // - 其余 `._*` **普通文件** → 视为 macOS AppleDouble 噪声，豁免（规范 3.4）。
                let known = name == crate::util::SCHEMA_DIR || name == crate::util::CACHE_DIR;
                let declared_here = entries.iter().any(|e| e.path == *name);
                if !known && (*is_dir || declared_here) {
                    self.err(
                        code::RESERVED_NAME,
                        ep.clone(),
                        "业务条目不得以 `._` 开头（`._` 为格式保留命名空间）",
                    );
                }
                continue;
            }
            if util::is_os_noise(name) {
                continue; // `.DS_Store` / `Thumbs.db` / `desktop.ini`
            }
            if util::is_other_dotfile(name) {
                self.warn(
                    code::DOTFILE,
                    ep.clone(),
                    "出现非 `._meta` / `.lock` 的点文件（不计入清单要求）",
                );
                continue;
            }
            // ROOT 允许承载任意内容（规范 3.3：任意目录可放任意文件），
            // 但「既非 UUID 命名的分支目录、又未登记进 entries」的条目属散落内容 → 告警。
            let declared_here = entries.iter().any(|e| e.path == *name);
            if v.depth == 0 && !util::is_uuid(name) && !declared_here {
                self.warn(
                    code::ROOT_STRAY,
                    ep.clone(),
                    "ROOT 下出现既非分支目录（UUID 命名）又未登记的条目",
                );
            }
            match entries.iter().find(|e| e.path == *name) {
                None => {
                    let what = if *is_dir { "目录" } else { "文件" };
                    self.manifest_issue(
                        code::MANIFEST_MISSING,
                        code::MANIFEST_MISSING_W,
                        ep.clone(),
                        format!("磁盘存在{what}但 `entries` 未登记"),
                    );
                }
                Some(e) => {
                    let e = e.clone();
                    self.check_entry(&dir, v.depth, meta, &e, &ep, *is_dir);
                }
            }
        }

        // 声明侧 → 幽灵条目
        let mut ghosts: Vec<(String, bool)> = Vec::new();
        for e in &entries {
            let path: PathBuf = dir.join(&e.path);
            if path.exists() {
                continue;
            }
            if e.optional {
                self.warn(
                    code::OPTIONAL_MISSING,
                    format!("{rel}/{}", e.path),
                    "`optional = true` 的条目当前缺失（允许）",
                );
            } else {
                ghosts.push((format!("{rel}/{}", e.path), false));
            }
        }
        for (ep, _) in ghosts {
            self.manifest_issue(
                code::MANIFEST_GHOST,
                code::MANIFEST_GHOST_W,
                ep,
                "`entries` 已登记但磁盘不存在",
            );
        }
    }

    /// 单条已登记条目 vs 磁盘实际。
    fn check_entry(
        &mut self,
        dir: &Path,
        parent_depth: usize,
        meta: &Meta,
        e: &Entry,
        ep: &str,
        is_dir: bool,
    ) {
        let bundle = self.bundle;
        let path: PathBuf = dir.join(&e.path);

        match e.role.as_str() {
            "node" | "branch" | "dir" => {
                if !is_dir {
                    self.err(
                        code::ENTRY_ROLE_DEPTH,
                        ep,
                        format!("`role = {:?}` 要求目录，但磁盘上是文件", e.role),
                    );
                    return;
                }
                let dir_name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let child_has_meta = bundle.has_meta(&path);
                if e.role == "dir" {
                    if util::is_sub_bundle(&dir_name) {
                        self.err(
                            code::ENTRY_ROLE_DEPTH,
                            ep,
                            "该目录名以 `.str` 结尾（独立子 bundle），应登记为 `role = \"bundle\"`",
                        );
                        return;
                    }
                    if child_has_meta {
                        self.err(
                            code::ENTRY_ROLE_DEPTH,
                            ep,
                            "该子目录含 `._meta`（已是分支），却被登记为 `role = \"dir\"`",
                        );
                    }
                    return;
                }
                if !child_has_meta {
                    self.err(code::META_MISSING, ep, "登记为分支但目录内缺少 `._meta`");
                    return;
                }
                let want_node = e.role == "node";
                if parent_depth == 0 && !want_node {
                    self.err(
                        code::ENTRY_ROLE_DEPTH,
                        ep,
                        "ROOT 的直接子分支必须是 `role = \"node\"`",
                    );
                } else if parent_depth >= 1 && want_node {
                    self.err(
                        code::ENTRY_ROLE_DEPTH,
                        ep,
                        "深度 ≥1 的子分支必须是 `role = \"branch\"`",
                    );
                }
                if let Some(id) = &e.id {
                    let dir_name = path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if *id != dir_name {
                        self.err(
                            code::ENTRY_ID_MISMATCH,
                            ep,
                            format!("`entries[].id` = {id:?} 与子目录名 {dir_name:?} 不一致"),
                        );
                    }
                }
                // 子分支自身的 `kind` 是否与父级 `role` 相符
                let child_kind = match bundle.read_meta(&path) {
                    Ok(crate::meta::MetaLoad::Ok(child, _)) => child.kind,
                    _ => None,
                };
                if let Some(k) = child_kind {
                    let expect = if want_node { Kind::Node } else { Kind::Branch };
                    if k != expect {
                        self.err(
                            code::ENTRY_ROLE_DEPTH,
                            ep,
                            format!(
                                "父级登记 `role = {:?}`，但子分支 `kind` = `{}`",
                                e.role,
                                k.as_str()
                            ),
                        );
                    }
                }
            }
            "payload" | "asset" => {
                if is_dir {
                    self.err(
                        code::ENTRY_ROLE_DEPTH,
                        ep,
                        format!("`role = {:?}` 要求文件，但磁盘上是目录", e.role),
                    );
                    return;
                }
                self.check_file_digest(e, ep, &path);
                if e.role == "payload" {
                    self.check_payload_schema(meta, e, ep, &path);
                }
            }
            "schema" | "cache" => {
                if !is_dir {
                    self.err(
                        code::ENTRY_ROLE_DEPTH,
                        ep,
                        format!("`role = {:?}` 要求目录", e.role),
                    );
                }
            }
            "bundle" => {
                // 独立子 bundle：目录名须以 `.str` 结尾；父 bundle 不进入其内部
                let dir_name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !is_dir {
                    self.err(
                        code::ENTRY_ROLE_DEPTH,
                        ep,
                        "`role = \"bundle\"` 要求目录",
                    );
                } else if !util::is_sub_bundle(&dir_name) {
                    self.err(
                        code::ENTRY_ROLE_DEPTH,
                        ep,
                        format!("`role = \"bundle\"` 要求目录名以 `.str` 结尾，实为 {dir_name:?}"),
                    );
                }
            }
            _ => {}
        }
    }

    /// 文件类条目的 `size` / `sha256` 强制与一致性。
    fn check_file_digest(&mut self, e: &Entry, ep: &str, path: &Path) {
        if self.policies.sha256 == ShaPolicy::Required && (e.size.is_none() || e.sha256.is_none()) {
            self.err(
                code::MANIFEST_DIGEST_MISSING,
                ep,
                "`policies.sha256 = \"required\"`：文件类条目必须同时提供 `size` 与 `sha256`",
            );
        }
        let Ok(md) = std::fs::metadata(path) else {
            return;
        };
        let real_size = md.len();
        if let Some(s) = e.size {
            if s < 0 || s as u64 != real_size {
                self.manifest_issue(
                    code::MANIFEST_HASH,
                    code::MANIFEST_HASH_W,
                    ep,
                    format!("`size` = {s} 与实际 {real_size} 不符"),
                );
            }
        }
        if self.policies.sha256 != ShaPolicy::Off {
            if let Some(declared) = e.sha256.clone() {
                match util::sha256_file(path) {
                    Ok(real) if real == declared => {}
                    Ok(real) => {
                        let d = &declared[..declared.len().min(12)];
                        let r = &real[..12];
                        self.manifest_issue(
                            code::MANIFEST_HASH,
                            code::MANIFEST_HASH_W,
                            ep,
                            format!("`sha256` 不符：声明 {d}…，实际 {r}…"),
                        );
                    }
                    Err(err) => {
                        self.manifest_issue(
                            code::MANIFEST_HASH,
                            code::MANIFEST_HASH_W,
                            ep,
                            format!("无法计算摘要：{err}"),
                        );
                    }
                }
            }
        }
        if real_size > self.policies.large_asset_bytes {
            self.warn(
                code::LARGE_ASSET,
                ep,
                format!(
                    "文件 {real_size} 字节，超过 `large_asset_bytes` = {}",
                    self.policies.large_asset_bytes
                ),
            );
        }
    }

    /// `E_REVISION_STALE` 的**历史**判定：`updated_at` 变了但 `revision` 没有前进（规范 §6.1）。
    ///
    /// 依据 `._cache/revisions.json`（由写入端登记，见 [`crate::baseline`]）。无基线时跳过 ——
    /// 单份 `._meta` 不含历史，无从判定；`str sync` 会把基线刷到当前状态。
    fn check_revision_history(&mut self) {
        let baseline = crate::baseline::load(self.bundle);
        if baseline.branches.is_empty() {
            return;
        }
        let mut found: Vec<(String, String, i64, i64, String)> = Vec::new();
        for v in &self.scan.visits {
            let Some(meta) = v.meta.as_ref() else {
                continue;
            };
            let Some(snap) = baseline.branches.get(&v.rel) else {
                continue;
            };
            let (Some(rev), Some(updated)) = (meta.revision, meta.updated_at.as_deref()) else {
                continue;
            };
            let changed = match (
                util::parse_rfc3339(&snap.updated_at),
                util::parse_rfc3339(updated),
            ) {
                (Some(a), Some(b)) => a != b,
                _ => snap.updated_at != updated,
            };
            if changed && rev <= snap.revision {
                found.push((
                    v.rel.clone(),
                    snap.updated_at.clone(),
                    snap.revision,
                    rev,
                    updated.to_string(),
                ));
            }
        }
        for (rel, was, old_rev, new_rev, now) in found {
            self.err(
                code::REVISION_STALE,
                rel,
                format!(
                    "`updated_at` 已由 {was} 变为 {now}，但 `revision` 未前进（{old_rev} → {new_rev}）；\
                     规范 §7.2 要求每次写入同时 +1 并刷新时间"
                ),
            );
        }
    }

    /// `refs` 关联图成环检测（迭代式 DFS）。
    fn check_ref_cycles(&mut self) {
        let scan = self.scan;
        let n = scan.visits.len();
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, v) in scan.visits.iter().enumerate() {
            if let Some(m) = &v.meta {
                for r in &m.refs {
                    if let Some(j) = scan.resolve(&r.target) {
                        edges[i].push(j);
                    }
                }
            }
        }

        let mut color = vec![0u8; n]; // 0 未访问 / 1 在栈 / 2 已完成
        let mut hits: Vec<String> = Vec::new();
        for start in 0..n {
            if color[start] != 0 {
                continue;
            }
            let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
            color[start] = 1;
            while !stack.is_empty() {
                let top = stack.len() - 1;
                let (node, k) = stack[top];
                if k < edges[node].len() {
                    let next = edges[node][k];
                    stack[top].1 += 1;
                    match color[next] {
                        0 => {
                            color[next] = 1;
                            stack.push((next, 0));
                        }
                        1 => hits.push(scan.visits[next].rel.clone()),
                        _ => {}
                    }
                } else {
                    color[node] = 2;
                    stack.pop();
                }
            }
        }
        hits.sort();
        hits.dedup();
        for path in hits {
            self.err(
                code::REF_CYCLE,
                path,
                "`refs` 关联图成环（追踪终点回到链上已有分支）",
            );
        }
    }

    /// 每个分支的 `._meta` 归一化后过 JSON Schema（规范 4.1 校验链路）。
    fn check_meta_schemas(&mut self) {
        let scan = self.scan;
        let bundle = self.bundle;
        let items: Vec<(String, JValue, &'static str)> = scan
            .visits
            .iter()
            .filter_map(|v| {
                let m = v.meta.as_ref()?;
                let name: &'static str = match v.depth {
                    0 => "root-meta.schema.json",
                    1 => "node-meta.schema.json",
                    _ => "branch-meta.schema.json",
                };
                Some((v.rel.clone(), m.to_json(), name))
            })
            .collect();

        for (rel, instance, name) in items {
            let schema = load_meta_schema(bundle, name);
            let Ok(validator) = jsonschema::validator_for(&schema) else {
                continue;
            };
            let msgs: Vec<String> = validator
                .iter_errors(&instance)
                .map(|e| format!("{e}（位于 {}）", e.instance_path()))
                .collect();
            for m in msgs {
                self.err(code::SCHEMA_FAIL, rel.clone(), m);
            }
        }
    }

    /// payload 的业务 Schema 校验（`entries[].schema` 或分支级 `schema`）。
    fn check_payload_schema(&mut self, meta: &Meta, e: &Entry, ep: &str, path: &Path) {
        let Some(schema_ref) = e.schema.clone().or_else(|| meta.schema.clone()) else {
            return;
        };
        let schema_path = self.bundle.root.join(&schema_ref);
        let Ok(schema_bytes) = std::fs::read(&schema_path) else {
            return;
        };
        let Ok(schema) = serde_json::from_slice::<JValue>(&schema_bytes) else {
            return;
        };
        let Ok(validator) = jsonschema::validator_for(&schema) else {
            return;
        };
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let Ok(instance) = serde_json::from_slice::<JValue>(&bytes) else {
            return; // 非 JSON payload 不做 Schema 校验
        };
        let msgs: Vec<String> = validator
            .iter_errors(&instance)
            .map(|err| format!("{err}（位于 {}）", err.instance_path()))
            .collect();
        for m in msgs {
            self.err(
                code::SCHEMA_FAIL,
                ep,
                format!("payload 不满足 `{schema_ref}`：{m}"),
            );
        }
    }
}

/// 取某档位的 `._meta` Schema：优先 bundle 内 `._schema/`，否则用内嵌兜底。
fn load_meta_schema(bundle: &Bundle, name: &str) -> JValue {
    let local = bundle.root.join(SCHEMA_DIR).join(name);
    if local.is_file() {
        if let Ok(bytes) = std::fs::read(&local) {
            if let Ok(v) = serde_json::from_slice::<JValue>(&bytes) {
                return v;
            }
        }
    }
    crate::EMBEDDED_SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, t)| serde_json::from_str(t).ok())
        .unwrap_or_else(|| JValue::Object(Default::default()))
}

/// 统计某目录下的条目数（`str sync` 用）。
pub fn count_dir_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| !util::is_meta_file(&e.file_name().to_string_lossy())).count())
        .unwrap_or(0)
}

/// 计算目录直接子项数（含点文件，排除 `._meta`）。
pub fn dir_child_count(dir: &Path) -> Option<i64> {
    std::fs::read_dir(dir).ok().map(|rd| {
        rd.flatten()
            .filter(|e| !util::is_meta_file(&e.file_name().to_string_lossy()))
            .count() as i64
    })
}

/// 收集目录内除了 `._meta` 与 OS 噪声之外的名字（`str sync` 用）。
pub fn real_entries(dir: &Path) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if util::is_meta_file(&name) || util::is_os_noise(&name) || util::is_lock_file(&name) {
            continue;
        }
        let is_dir = ent.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push((name, is_dir));
    }
    out.sort();
    out
}

/// 供测试与工具使用：一次扫描的 `id` → 相对路径映射。
pub fn id_index(scan: &Scan) -> HashMap<String, String> {
    scan.by_id
        .iter()
        .filter_map(|(id, idxs)| idxs.first().map(|i| (id.clone(), scan.visits[*i].rel.clone())))
        .collect()
}
