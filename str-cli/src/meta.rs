//! `._meta` 的解析、类型化提取与**归一化**（TOML → 规范 JSON）。
//!
//! 保注释写回与新建模板见 [`crate::meta_edit`]。

use std::path::Path;

use serde_json::{Map as JMap, Value as JValue};
use toml_edit::{DocumentMut, Item, Table};

use crate::error::{Error, Issue, Result, code};
use crate::util;

// ─────────────────────────── 规范 4.9：键序与表序 ───────────────────────────

/// 顶层裸键的规范顺序（必须写在任何表头之前）。
pub const TOP_BARE_KEYS: &[&str] = &[
    "str",
    "spec",
    "kind",
    "id",
    "name",
    "type",
    "title",
    "summary",
    "tags",
    "revision",
    "created_at",
    "updated_at",
    "schema",
];

/// 表 / 数组表的规范顺序。
pub const TABLE_ORDER: &[&str] = &["policies", "authors", "refs", "entries", "ext"];

/// `[policies]` 内键序。
pub const POLICIES_KEYS: &[&str] = &[
    "id_version",
    "max_depth",
    "manifest",
    "sha256",
    "large_asset_bytes",
    "deep_tree_warn",
];

/// `[[authors]]` 内键序。
pub const AUTHOR_KEYS: &[&str] = &["id", "name", "role", "at"];

/// `[[refs]]` 内键序。
pub const REF_KEYS: &[&str] = &["id", "target", "rel", "title", "order", "note"];

/// `[[entries]]` 内键序。
pub const ENTRY_KEYS: &[&str] = &[
    "path",
    "role",
    "id",
    "type",
    "title",
    "summary",
    "order",
    "media_type",
    "size",
    "sha256",
    "count",
    "schema",
    "optional",
    "note",
];

/// root 允许的顶层键。
pub const ROOT_ALLOWED: &[&str] = &[
    "str", "spec", "kind", "id", "name", "type", "title", "summary", "tags", "revision",
    "created_at", "updated_at", "schema", "policies", "authors", "refs", "entries", "ext",
];

/// node / branch 允许的顶层键（无 `policies`）。
pub const BRANCH_ALLOWED: &[&str] = &[
    "str", "spec", "kind", "id", "name", "type", "title", "summary", "tags", "revision",
    "created_at", "updated_at", "schema", "authors", "refs", "entries", "ext",
];

// ─────────────────────────── 模型 ───────────────────────────

/// `kind` 档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// ROOT（深度 0）。
    Root,
    /// 独立节点（深度 1）。
    Node,
    /// 关联分支（深度 ≥2）。
    Branch,
}

impl Kind {
    /// 字面量。
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Root => "root",
            Kind::Node => "node",
            Kind::Branch => "branch",
        }
    }

    /// 解析字面量。
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "root" => Some(Kind::Root),
            "node" => Some(Kind::Node),
            "branch" => Some(Kind::Branch),
            _ => None,
        }
    }

    /// root 与 node/branch 的允许键集。
    pub fn allowed_top_keys(self) -> &'static [&'static str] {
        match self {
            Kind::Root => ROOT_ALLOWED,
            _ => BRANCH_ALLOWED,
        }
    }
}

/// 清单策略（`policies.manifest`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestPolicy {
    /// 清单不一致为 error。
    Strict,
    /// 清单不一致仅告警。
    Advisory,
}

/// 指纹策略（`policies.sha256`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaPolicy {
    /// 文件类条目必须有 `size` + `sha256`。
    Required,
    /// 可选。
    Optional,
    /// 完全不校验。
    Off,
}

/// 校验策略（仅 root）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policies {
    /// UUID 版本要求。
    pub id_version: usize,
    /// 分支树最大深度。
    pub max_depth: usize,
    /// 清单策略。
    pub manifest: ManifestPolicy,
    /// 指纹策略。
    pub sha256: ShaPolicy,
    /// 大文件告警阈值。
    pub large_asset_bytes: u64,
    /// 深树告警阈值。
    pub deep_tree_warn: usize,
}

impl Default for Policies {
    fn default() -> Self {
        Self {
            id_version: 7,
            max_depth: 32,
            manifest: ManifestPolicy::Strict,
            sha256: ShaPolicy::Required,
            large_asset_bytes: 10 * 1024 * 1024,
            deep_tree_warn: 16,
        }
    }
}

/// `[[authors]]` 元素。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Author {
    /// 稳定标识符。
    pub id: String,
    /// 展示名。
    pub name: Option<String>,
    /// 角色。
    pub role: String,
    /// 参与时间。
    pub at: Option<String>,
}

/// `[[refs]]` 元素（跨枝关联线）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefItem {
    /// 关联线自身 id。
    pub id: String,
    /// 目标分支 id。
    pub target: String,
    /// 关联语义。
    pub rel: String,
    /// 标签。
    pub title: Option<String>,
    /// 排序键。
    pub order: Option<i64>,
    /// 备注。
    pub note: Option<String>,
}

/// `[[entries]]` 元素（本目录内容清单）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// 单段路径。
    pub path: String,
    /// 角色。
    pub role: String,
    /// 子分支 id（`node` / `branch`）。
    pub id: Option<String>,
    /// 子分支类型。
    pub r#type: Option<String>,
    /// 展示名。
    pub title: Option<String>,
    /// 摘要。
    pub summary: Option<String>,
    /// 排序键。
    pub order: Option<i64>,
    /// IANA 媒体类型。
    pub media_type: Option<String>,
    /// 字节数。
    pub size: Option<i64>,
    /// 内容指纹。
    pub sha256: Option<String>,
    /// 直接子项数（`dir`）。
    pub count: Option<i64>,
    /// 该文件遵循的 Schema。
    pub schema: Option<String>,
    /// 是否允许缺失。
    pub optional: bool,
    /// 备注。
    pub note: Option<String>,
}

impl Entry {
    /// 是否为分支条目（`node` / `branch`）。
    pub fn is_branch(&self) -> bool {
        self.role == "node" || self.role == "branch"
    }

    /// 是否为文件类条目（需要指纹）。
    pub fn is_file_like(&self) -> bool {
        self.role == "payload" || self.role == "asset"
    }
}

/// 一份 `._meta` 的完整内容：保序文档 + 类型化视图。
#[derive(Debug)]
pub struct Meta {
    /// 保注释、保顺序的 TOML 文档（写回用）。
    pub doc: DocumentMut,
    /// `str` 主版本。
    pub str_version: Option<i64>,
    /// 规范版本。
    pub spec: Option<String>,
    /// 档位。
    pub kind_raw: Option<String>,
    /// 解析出的档位。
    pub kind: Option<Kind>,
    /// 自身 id。
    pub id: Option<String>,
    /// bundle 短名（root）。
    pub name: Option<String>,
    /// 类型。
    pub r#type: Option<String>,
    /// 标题。
    pub title: Option<String>,
    /// 摘要。
    pub summary: Option<String>,
    /// 标签。
    pub tags: Vec<String>,
    /// 修订号。
    pub revision: Option<i64>,
    /// 创建时间。
    pub created_at: Option<String>,
    /// 更新时间。
    pub updated_at: Option<String>,
    /// payload schema 引用。
    pub schema: Option<String>,
    /// 策略。
    pub policies: Policies,
    /// 贡献者。
    pub authors: Vec<Author>,
    /// 跨枝关联。
    pub refs: Vec<RefItem>,
    /// 内容清单。
    pub entries: Vec<Entry>,
}

/// 加载结果。
pub enum MetaLoad {
    /// 解析成功（可能带有字段级问题）。
    Ok(Box<Meta>, Vec<Issue>),
    /// 解析失败（`E_PARSE`），无法构建模型。
    Failed(Vec<Issue>),
}

/// 从磁盘读取并解析一份 `._meta`。
pub fn load(path: &Path, rel: &str) -> Result<MetaLoad> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let mut issues = Vec::new();

    // 编码：UTF-8 无 BOM
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        issues.push(Issue::error(
            code::PARSE,
            rel,
            "文件含 UTF-8 BOM，规范要求 UTF-8 无 BOM",
        ));
        return Ok(MetaLoad::Failed(issues));
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(t) => t,
        Err(e) => {
            issues.push(Issue::error(
                code::PARSE,
                rel,
                format!("编码非 UTF-8：{e}"),
            ));
            return Ok(MetaLoad::Failed(issues));
        }
    };

    let doc: DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(e) => {
            issues.push(Issue::error(code::PARSE, rel, format!("TOML 解析失败：{e}")));
            return Ok(MetaLoad::Failed(issues));
        }
    };

    let (meta, mut field_issues) = extract(doc, rel);
    issues.append(&mut field_issues);
    Ok(MetaLoad::Ok(Box::new(meta), issues))
}

/// 从已解析的文档构建类型化视图。
pub fn extract(doc: DocumentMut, rel: &str) -> (Meta, Vec<Issue>) {
    let mut cx = Ctx {
        rel: rel.to_string(),
        issues: Vec::new(),
    };
    let table = doc.as_table().clone();

    let str_version = cx.opt_int(&table, "str");
    let spec = cx.opt_str(&table, "spec");
    let kind_raw = cx.opt_str(&table, "kind");
    let kind = match kind_raw.as_deref() {
        Some(s) => match Kind::parse(s) {
            Some(k) => Some(k),
            None => {
                cx.err(
                    code::KIND_INVALID,
                    format!("`kind` = {s:?} 非法（应为 root / node / branch）"),
                );
                None
            }
        },
        None => {
            cx.err(code::SCHEMA_FIELD, "缺少必填字段 `kind`");
            None
        }
    };

    let allowed = kind.map(|k| k.allowed_top_keys()).unwrap_or(BRANCH_ALLOWED);
    cx.check_unknown(&table, allowed);

    // 注意：TOML 无法表达空的数组表，故 `entries` / `refs` 为空时整表省略，
    // 归一化时补 `[]`（规范 4.1 / 4.9）。
    for key in ["id", "revision", "created_at", "updated_at"] {
        if table.get(key).is_none() {
            cx.err(code::SCHEMA_FIELD, format!("缺少必填字段 `{key}`"));
        }
    }

    let id = cx.opt_str(&table, "id");
    let name = cx.opt_str(&table, "name");
    let r#type = cx.opt_str(&table, "type");
    let title = cx.opt_str(&table, "title");
    let summary = cx.opt_str(&table, "summary");
    let tags = cx.opt_str_array(&table, "tags").unwrap_or_default();
    let revision = cx.opt_int(&table, "revision");
    let created_at = cx.opt_dt(&table, "created_at");
    let updated_at = cx.opt_dt(&table, "updated_at");
    let schema = cx.opt_str(&table, "schema");

    // `[policies]`（仅 root）
    let mut policies = Policies::default();
    if let Some(item) = table.get("policies") {
        match item.as_table() {
            Some(pt) => {
                if kind != Some(Kind::Root) {
                    cx.err(
                        code::SCHEMA_FIELD,
                        "`[policies]` 只能出现在 root 的 `._meta` 中",
                    );
                }
                cx.check_unknown(pt, POLICIES_KEYS);
                if let Some(v) = cx.opt_int(pt, "id_version") {
                    policies.id_version = v.max(0) as usize;
                }
                if let Some(v) = cx.opt_int(pt, "max_depth") {
                    policies.max_depth = v.max(1) as usize;
                }
                if let Some(v) = cx.opt_str(pt, "manifest") {
                    policies.manifest = match v.as_str() {
                        "strict" => ManifestPolicy::Strict,
                        "advisory" => ManifestPolicy::Advisory,
                        _ => {
                            cx.err(
                                code::SCHEMA_FIELD,
                                format!("`policies.manifest` = {v:?} 非法（strict / advisory）"),
                            );
                            policies.manifest
                        }
                    };
                }
                if let Some(v) = cx.opt_str(pt, "sha256") {
                    policies.sha256 = match v.as_str() {
                        "required" => ShaPolicy::Required,
                        "optional" => ShaPolicy::Optional,
                        "off" => ShaPolicy::Off,
                        _ => {
                            cx.err(
                                code::SCHEMA_FIELD,
                                format!("`policies.sha256` = {v:?} 非法（required / optional / off）"),
                            );
                            policies.sha256
                        }
                    };
                }
                if let Some(v) = cx.opt_int(pt, "large_asset_bytes") {
                    policies.large_asset_bytes = v.max(0) as u64;
                }
                if let Some(v) = cx.opt_int(pt, "deep_tree_warn") {
                    policies.deep_tree_warn = v.max(1) as usize;
                }
            }
            None => cx.err(code::SCHEMA_FIELD, "`policies` 必须是表 `[policies]`"),
        }
    }

    // `[[authors]]`
    let mut authors = Vec::new();
    if let Some(aot) = table.get("authors").and_then(|i| i.as_array_of_tables()) {
        for (i, t) in aot.iter().enumerate() {
            cx.set_rel(&format!("{rel} #authors[{i}]"));
            cx.check_unknown(t, AUTHOR_KEYS);
            let aid = cx.req_str(t, "id").unwrap_or_default();
            let role = cx.req_str(t, "role").unwrap_or_default();
            let aname = cx.opt_str(t, "name");
            let at = cx.opt_dt(t, "at");
            authors.push(Author {
                id: aid,
                name: aname,
                role,
                at,
            });
        }
        cx.set_rel(rel);
    } else if table.get("authors").is_some() {
        cx.err(code::SCHEMA_FIELD, "`authors` 必须是数组表 `[[authors]]`");
    }

    // `[[refs]]`
    let mut refs = Vec::new();
    if let Some(aot) = table.get("refs").and_then(|i| i.as_array_of_tables()) {
        for (i, t) in aot.iter().enumerate() {
            cx.set_rel(&format!("{rel} #refs[{i}]"));
            cx.check_unknown(t, REF_KEYS);
            refs.push(RefItem {
                id: cx.req_str(t, "id").unwrap_or_default(),
                target: cx.req_str(t, "target").unwrap_or_default(),
                rel: cx.req_str(t, "rel").unwrap_or_default(),
                title: cx.opt_str(t, "title"),
                order: cx.opt_int(t, "order"),
                note: cx.opt_str(t, "note"),
            });
        }
        cx.set_rel(rel);
    } else if table.get("refs").is_some() {
        cx.err(code::SCHEMA_FIELD, "`refs` 必须是数组表 `[[refs]]`");
    }

    // `[[entries]]`
    let mut entries = Vec::new();
    if let Some(aot) = table.get("entries").and_then(|i| i.as_array_of_tables()) {
        for (i, t) in aot.iter().enumerate() {
            cx.set_rel(&format!("{rel} #entries[{i}]"));
            cx.check_unknown(t, ENTRY_KEYS);
            let path = cx.req_str(t, "path").unwrap_or_default();
            let role = cx.req_str(t, "role").unwrap_or_default();
            if path.contains('/') {
                cx.err(
                    code::SCHEMA_FIELD,
                    format!("`entries[].path` = {path:?} 必须是单段路径（不含 `/`）"),
                );
            }
            let e = Entry {
                path,
                role,
                id: cx.opt_str(t, "id"),
                r#type: cx.opt_str(t, "type"),
                title: cx.opt_str(t, "title"),
                summary: cx.opt_str(t, "summary"),
                order: cx.opt_int(t, "order"),
                media_type: cx.opt_str(t, "media_type"),
                size: cx.opt_int(t, "size"),
                sha256: cx.opt_str(t, "sha256"),
                count: cx.opt_int(t, "count"),
                schema: cx.opt_str(t, "schema"),
                optional: cx.opt_bool(t, "optional").unwrap_or(false),
                note: cx.opt_str(t, "note"),
            };
            entries.push(e);
        }
        cx.set_rel(rel);
    } else if table.get("entries").is_some() {
        cx.err(code::SCHEMA_FIELD, "`entries` 必须是数组表 `[[entries]]`");
    }

    let meta = Meta {
        doc,
        str_version,
        spec,
        kind_raw,
        kind,
        id,
        name,
        r#type,
        title,
        summary,
        tags,
        revision,
        created_at,
        updated_at,
        schema,
        policies,
        authors,
        refs,
        entries,
    };
    (meta, cx.issues)
}

// ─────────────────────────── 归一化（TOML → 规范 JSON）───────────────────────────

impl Meta {
    /// 归一化为**规范 JSON**：顶层裸键与各表按 4.9 的键序/表序输出，
    /// 未知键原样保留，使其可被 JSON Schema 校验器与 AI 稳定消费。
    pub fn to_json(&self) -> JValue {
        let table = self.doc.as_table();
        let mut map = JMap::new();

        for key in TOP_BARE_KEYS {
            if let Some(item) = table.get(key) {
                map.insert((*key).to_string(), item_to_json(item));
            }
        }
        if let Some(item) = table.get("policies") {
            match item.as_table() {
                Some(t) => {
                    map.insert("policies".into(), table_to_json(t, POLICIES_KEYS));
                }
                None => {
                    map.insert("policies".into(), item_to_json(item));
                }
            }
        }
        if let Some(aot) = table.get("authors").and_then(|i| i.as_array_of_tables()) {
            map.insert(
                "authors".into(),
                JValue::Array(
                    aot.iter()
                        .map(|t| table_to_json(t, AUTHOR_KEYS))
                        .collect(),
                ),
            );
        }
        // `refs` 为空（TOML 无法表达空数组表）时补 `[]`
        map.insert(
            "refs".into(),
            match table.get("refs").and_then(|i| i.as_array_of_tables()) {
                Some(aot) => JValue::Array(
                    aot.iter()
                        .map(|t| table_to_json(t, REF_KEYS))
                        .collect(),
                ),
                None => JValue::Array(Vec::new()),
            },
        );
        map.insert(
            "entries".into(),
            match table.get("entries").and_then(|i| i.as_array_of_tables()) {
                Some(aot) => JValue::Array(
                    aot.iter()
                        .map(|t| table_to_json(t, ENTRY_KEYS))
                        .collect(),
                ),
                None => JValue::Array(Vec::new()),
            },
        );
        if let Some(item) = table.get("ext") {
            map.insert("ext".into(), item_to_json(item));
        }
        // 未知键原样保留（前向兼容）
        for (k, v) in table.iter() {
            if !map.contains_key(k) {
                map.insert(k.to_string(), item_to_json(v));
            }
        }
        JValue::Object(map)
    }

    /// 按规范键序对 `entries` / `refs` 做稳定排序。
    pub fn sort_collections(&mut self) {
        use toml_edit::{ArrayOfTables, Item};
        for key in ["entries", "refs"] {
            let Some(aot) = self
                .doc
                .as_table()
                .get(key)
                .and_then(|i| i.as_array_of_tables())
            else {
                continue;
            };
            let mut items: Vec<Table> = aot.iter().cloned().collect();
            let idx = |t: &Table| {
                t.get("order")
                    .and_then(|i| i.as_integer())
                    .unwrap_or(i64::MAX)
            };
            items.sort_by(|a, b| {
                let ka = (
                    idx(a),
                    a.get("path")
                        .or_else(|| a.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string(),
                );
                let kb = (
                    idx(b),
                    b.get("path")
                        .or_else(|| b.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string(),
                );
                ka.cmp(&kb)
            });
            let mut new_aot = ArrayOfTables::new();
            for t in items {
                new_aot.push(t);
            }
            self.doc
                .as_table_mut()
                .insert(key, Item::ArrayOfTables(new_aot));
        }
    }

    /// 校验序号等基础合法性（`E_REVISION_STALE` 的可判定部分）。
    pub fn revision_issues(&self, rel: &str) -> Vec<Issue> {
        let mut out = Vec::new();
        match self.revision {
            None => {}
            Some(r) if r < 1 => out.push(Issue::error(
                code::REVISION_STALE,
                rel,
                format!("`revision` = {r}，必须为 ≥ 1 的整数"),
            )),
            Some(_) => {}
        }
        if let (Some(c), Some(u)) = (self.created_at.as_deref(), self.updated_at.as_deref()) {
            match (util::parse_rfc3339(c), util::parse_rfc3339(u)) {
                (Some(cd), Some(ud)) if ud < cd => out.push(Issue::error(
                    code::REVISION_STALE,
                    rel,
                    format!("`updated_at`（{u}）早于 `created_at`（{c}）"),
                )),
                _ => {}
            }
        }
        out
    }
}

/// TOML `Item` → JSON。
fn item_to_json(item: &Item) -> JValue {
    match item {
        Item::None => JValue::Null,
        Item::Value(v) => value_to_json(v),
        Item::Table(t) => {
            let mut m = JMap::new();
            for (k, v) in t.iter() {
                m.insert(k.to_string(), item_to_json(v));
            }
            JValue::Object(m)
        }
        Item::ArrayOfTables(aot) => JValue::Array(
            aot.iter()
                .map(|t| {
                    let mut m = JMap::new();
                    for (k, v) in t.iter() {
                        m.insert(k.to_string(), item_to_json(v));
                    }
                    JValue::Object(m)
                })
                .collect(),
        ),
    }
}

/// TOML `Value` → JSON。
fn value_to_json(v: &toml_edit::Value) -> JValue {
    if let Some(s) = v.as_str() {
        return JValue::String(s.to_string());
    }
    if let Some(i) = v.as_integer() {
        return JValue::from(i);
    }
    if let Some(f) = v.as_float() {
        return serde_json::Number::from_f64(f)
            .map(JValue::Number)
            .unwrap_or(JValue::Null);
    }
    if let Some(b) = v.as_bool() {
        return JValue::Bool(b);
    }
    if let Some(d) = v.as_datetime() {
        return JValue::String(d.to_string());
    }
    if let Some(a) = v.as_array() {
        return JValue::Array(a.iter().map(value_to_json).collect());
    }
    if let Some(t) = v.as_inline_table() {
        let mut m = JMap::new();
        for (k, val) in t.iter() {
            m.insert(k.to_string(), value_to_json(val));
        }
        return JValue::Object(m);
    }
    JValue::Null
}

/// 表 → JSON（先按 `order` 输出已知键，再补未知键）。
fn table_to_json(table: &Table, order: &[&str]) -> JValue {
    let mut m = JMap::new();
    for k in order {
        if let Some(v) = table.get(k) {
            m.insert((*k).to_string(), item_to_json(v));
        }
    }
    for (k, v) in table.iter() {
        if !m.contains_key(k) {
            m.insert(k.to_string(), item_to_json(v));
        }
    }
    JValue::Object(m)
}

// ─────────────────────────── 提取辅助 ───────────────────────────

/// 提取上下文：累积字段级问题。
struct Ctx {
    rel: String,
    issues: Vec<Issue>,
}

impl Ctx {
    fn set_rel(&mut self, rel: &str) {
        self.rel = rel.to_string();
    }

    fn err(&mut self, code: &'static str, message: impl Into<String>) {
        self.issues
            .push(Issue::error(code, self.rel.clone(), message));
    }

    fn check_unknown(&mut self, t: &Table, allowed: &[&str]) {
        for (k, _) in t.iter() {
            if !allowed.contains(&k) {
                self.err(
                    code::SCHEMA_FIELD,
                    format!("未知字段 `{k}`（扩展请放入 `[ext]`）"),
                );
            }
        }
    }

    fn req_str(&mut self, t: &Table, key: &str) -> Option<String> {
        match t.get(key) {
            None => {
                self.err(code::SCHEMA_FIELD, format!("缺少必填字段 `{key}`"));
                None
            }
            Some(item) => match item.as_str() {
                Some(s) => Some(s.to_string()),
                None => {
                    self.err(code::SCHEMA_FIELD, format!("`{key}` 必须是字符串"));
                    None
                }
            },
        }
    }

    fn opt_str(&mut self, t: &Table, key: &str) -> Option<String> {
        match t.get(key) {
            None => None,
            Some(item) => match item.as_str() {
                Some(s) => Some(s.to_string()),
                None => {
                    self.err(code::SCHEMA_FIELD, format!("`{key}` 必须是字符串"));
                    None
                }
            },
        }
    }

    fn opt_int(&mut self, t: &Table, key: &str) -> Option<i64> {
        match t.get(key) {
            None => None,
            Some(item) => match item.as_integer() {
                Some(i) => Some(i),
                None => {
                    self.err(code::SCHEMA_FIELD, format!("`{key}` 必须是整数"));
                    None
                }
            },
        }
    }

    fn opt_bool(&mut self, t: &Table, key: &str) -> Option<bool> {
        match t.get(key) {
            None => None,
            Some(item) => match item.as_bool() {
                Some(b) => Some(b),
                None => {
                    self.err(code::SCHEMA_FIELD, format!("`{key}` 必须是布尔值"));
                    None
                }
            },
        }
    }

    /// 读取 TOML 原生 offset date-time；写成字符串或缺少时区偏移均报 `E_PARSE`。
    fn opt_dt(&mut self, t: &Table, key: &str) -> Option<String> {
        let Some(item) = t.get(key) else {
            return None;
        };
        if item.as_str().is_some() {
            self.err(
                code::PARSE,
                format!("`{key}` 必须使用 TOML 原生 offset date-time，不得写成字符串"),
            );
            return None;
        }
        match item.as_datetime() {
            Some(d) => {
                if d.date.is_none() || d.time.is_none() || d.offset.is_none() {
                    self.err(
                        code::PARSE,
                        format!("`{key}` 必须是带时区偏移的 offset date-time（如 2026-09-14T10:03:11+08:00）"),
                    );
                    return None;
                }
                Some(d.to_string())
            }
            None => {
                self.err(
                    code::PARSE,
                    format!("`{key}` 必须是 offset date-time"),
                );
                None
            }
        }
    }

    fn opt_str_array(&mut self, t: &Table, key: &str) -> Option<Vec<String>> {
        let Some(item) = t.get(key) else {
            return None;
        };
        let Some(arr) = item.as_array() else {
            self.err(code::SCHEMA_FIELD, format!("`{key}` 必须是字符串数组"));
            return None;
        };
        let mut out = Vec::new();
        for v in arr.iter() {
            match v.as_str() {
                Some(s) => out.push(s.to_string()),
                None => {
                    self.err(code::SCHEMA_FIELD, format!("`{key}` 的元素必须是字符串"));
                }
            }
        }
        Some(out)
    }
}
