//! `._meta` 的**保注释写回**与新建模板渲染。
//!
//! 写回直接基于 `toml_edit::DocumentMut`，因此已有的 `#` 注释与键的书写顺序都会被保留
//! （规范硬性约束 6、DoD 16）。

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, value};

use crate::error::{Error, Result};
use crate::meta::{Author, Entry, Kind, Meta, RefItem, extract};
use crate::util;

/// TOML 基本字符串转义。
pub fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 从 TOML 文本构建 `Meta`（模板渲染后使用）。
pub fn meta_from_text(text: &str) -> Result<Meta> {
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| Error::Other(format!("内部模板 TOML 解析失败：{e}")))?;
    let (meta, _issues) = extract(doc, "<template>");
    Ok(meta)
}

/// 渲染 root `._meta` 模板。
pub fn render_root_meta(
    name: &str,
    title: Option<&str>,
    summary: Option<&str>,
    root_id: &str,
    created: &str,
    id_version: usize,
) -> String {
    let mut s = String::new();
    s.push_str("# ── STR bundle 根元数据 ──────────────────────────────────────────\n");
    s.push_str("# ROOT 的 [[entries]] 中 role = \"node\" 的条目即一级分支结构。\n");
    s.push_str(&format!("str = {}\n", crate::STR_MAJOR));
    s.push_str(&format!("spec = {}\n", toml_str(crate::SPEC_VERSION)));
    s.push_str("kind = \"root\"\n");
    s.push_str(&format!("id = {}\n", toml_str(root_id)));
    s.push_str(&format!("name = {}\n", toml_str(name)));
    if let Some(t) = title {
        s.push_str(&format!("title = {}\n", toml_str(t)));
    }
    if let Some(t) = summary {
        s.push_str(&format!("summary = {}\n", toml_str(t)));
    }
    s.push_str("tags = []\n");
    s.push_str("revision = 1\n");
    s.push_str(&format!("created_at = {created}\n"));
    s.push_str(&format!("updated_at = {created}\n"));
    s.push('\n');
    s.push_str("[policies]\n");
    s.push_str(&format!("id_version = {id_version}\n"));
    s.push_str("max_depth = 32\n");
    s.push_str("manifest = \"strict\"\n");
    s.push_str("sha256 = \"required\"\n");
    s.push_str("large_asset_bytes = 10485760\n");
    s.push_str("deep_tree_warn = 16\n");
    // v1.12.0：不再登记 `._schema` —— 保留目录免登记（规范 §1.3 约束 5 / §4.8）。
    // `._schema/` 目录本身仍由 init 创建并写入三份 Schema，只是不再出现在 `entries[]`。
    s.push('\n');
    s.push_str("[ext]\n");
    s
}

/// 渲染 node / branch `._meta` 模板。
pub fn render_branch_meta(
    kind: Kind,
    id: &str,
    type_: Option<&str>,
    title: Option<&str>,
    summary: Option<&str>,
    created: &str,
) -> String {
    let mut s = String::new();
    s.push_str(&format!("str = {}\n", crate::STR_MAJOR));
    s.push_str(&format!("spec = {}\n", toml_str(crate::SPEC_VERSION)));
    s.push_str(&format!("kind = {}\n", toml_str(kind.as_str())));
    s.push_str(&format!("id = {}\n", toml_str(id)));
    if let Some(t) = type_ {
        s.push_str(&format!("type = {}\n", toml_str(t)));
    }
    if let Some(t) = title {
        s.push_str(&format!("title = {}\n", toml_str(t)));
    }
    if let Some(t) = summary {
        s.push_str(&format!("summary = {}\n", toml_str(t)));
    }
    s.push_str("tags = []\n");
    s.push_str("revision = 1\n");
    s.push_str(&format!("created_at = {created}\n"));
    s.push_str(&format!("updated_at = {created}\n"));
    s.push('\n');
    s.push_str("[ext]\n");
    s
}

/// `[[entries]]` 元素 → TOML 表（按规范键序）。
pub fn entry_to_table(e: &Entry) -> Table {
    let mut t = Table::new();
    t.insert("path", value(e.path.clone()));
    t.insert("role", value(e.role.clone()));
    if let Some(v) = &e.id {
        t.insert("id", value(v.clone()));
    }
    if let Some(v) = &e.r#type {
        t.insert("type", value(v.clone()));
    }
    if let Some(v) = &e.title {
        t.insert("title", value(v.clone()));
    }
    if let Some(v) = &e.summary {
        t.insert("summary", value(v.clone()));
    }
    if let Some(v) = e.order {
        t.insert("order", value(v));
    }
    if let Some(v) = &e.media_type {
        t.insert("media_type", value(v.clone()));
    }
    if let Some(v) = e.size {
        t.insert("size", value(v));
    }
    if let Some(v) = &e.sha256 {
        t.insert("sha256", value(v.clone()));
    }
    if let Some(v) = e.count {
        t.insert("count", value(v));
    }
    if let Some(v) = &e.schema {
        t.insert("schema", value(v.clone()));
    }
    if e.optional {
        t.insert("optional", value(true));
    }
    if let Some(v) = &e.note {
        t.insert("note", value(v.clone()));
    }
    t
}

/// `[[refs]]` 元素 → TOML 表（按规范键序）。
pub fn ref_to_table(r: &RefItem) -> Table {
    let mut t = Table::new();
    t.insert("id", value(r.id.clone()));
    t.insert("target", value(r.target.clone()));
    t.insert("rel", value(r.rel.clone()));
    if let Some(v) = &r.title {
        t.insert("title", value(v.clone()));
    }
    if let Some(v) = r.order {
        t.insert("order", value(v));
    }
    if let Some(v) = &r.note {
        t.insert("note", value(v.clone()));
    }
    t
}

/// `[[authors]]` 元素 → TOML 表（按规范键序）。
pub fn author_to_table(a: &Author) -> Table {
    let mut t = Table::new();
    t.insert("id", value(a.id.clone()));
    if let Some(v) = &a.name {
        t.insert("name", value(v.clone()));
    }
    t.insert("role", value(a.role.clone()));
    if let Some(at) = &a.at
        && let Ok(dt) = at.parse::<toml_edit::Datetime>()
    {
        t.insert("at", value(dt));
    }
    t
}

/// 去掉整行 `#` 注释（不处理字符串内的 `#`）；`str fmt --strip-comments` 的唯一入口。
pub fn strip_line_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

impl Meta {
    /// 写操作后重新同步类型化视图（`doc` 为准）。
    pub fn resync(&mut self) {
        let doc = std::mem::replace(&mut self.doc, DocumentMut::new());
        let (meta, _issues) = extract(doc, "<resync>");
        *self = meta;
    }

    /// 按规范表序（`policies → authors → refs → entries → ext`）安置一张表 / 数组表。
    fn place_table(&mut self, key: &'static str, item: Item) {
        let order = crate::meta::TABLE_ORDER;
        let pos = order.iter().position(|k| *k == key).unwrap_or(order.len());
        let table = self.doc.as_table_mut();
        let mut moved: Vec<(String, Item)> = Vec::new();
        for k in order.iter().skip(pos + 1) {
            if let Some(it) = table.remove(k) {
                moved.push(((*k).to_string(), it));
            }
        }
        table.insert(key, item);
        for (k, it) in moved {
            table.insert(&k, it);
        }
    }

    /// `revision + 1` 并刷新 `updated_at`。
    ///
    /// `updated_at` 一律写成 TOML **原生 offset date-time**（规范 4.1），不得是字符串。
    pub fn touch(&mut self) {
        let next = self.revision.unwrap_or(0) + 1;
        let now = util::now_rfc3339();
        let table = self.doc.as_table_mut();
        table.insert("revision", value(next));
        if let Ok(dt) = now.parse::<toml_edit::Datetime>() {
            table.insert("updated_at", value(dt));
        }
        self.resync();
    }

    /// 设置顶层字符串字段（保注释）。
    pub fn set_str(&mut self, key: &str, v: &str) {
        self.doc.as_table_mut().insert(key, value(v.to_string()));
        self.resync();
    }

    /// 设置顶层字符串字段；`v` 为空串则**移除**该字段。
    pub fn set_str_or_remove(&mut self, key: &str, v: &str) {
        if v.is_empty() {
            self.doc.as_table_mut().remove(key);
        } else {
            self.doc.as_table_mut().insert(key, value(v.to_string()));
        }
        self.resync();
    }

    /// 设置顶层字符串数组字段（如 `tags`）。
    pub fn set_str_array(&mut self, key: &str, values: &[String]) {
        let arr: toml_edit::Array = values
            .iter()
            .map(|s| toml_edit::Value::from(s.clone()))
            .collect();
        self.doc.as_table_mut().insert(key, value(arr));
        self.resync();
    }

    /// 设置 `[policies]` 内的单个键（仅 root；保注释，键序随后由 `canonicalize()` 收口）。
    ///
    /// `None` 移除该键；`[policies]` 表不存在时创建。原本不存在且落完是空表时不产生改动。
    pub fn set_policies_value(&mut self, key: &str, v: Option<toml_edit::Item>) {
        let existing = self
            .doc
            .as_table()
            .get("policies")
            .and_then(|i| i.as_table().cloned());
        let created = existing.is_none();
        let mut pt = existing.unwrap_or_default();
        match v {
            Some(item) => {
                pt.insert(key, item);
            }
            None => {
                pt.remove(key);
            }
        }
        if created && pt.is_empty() {
            return;
        }
        self.place_table("policies", toml_edit::Item::Table(pt));
        self.resync();
    }

    /// 设置 `[policies]` 内的字符串数组键（如 `ignore`）；空数组即移除该键。
    pub fn set_policies_str_array(&mut self, key: &str, values: &[String]) {
        if values.is_empty() {
            self.set_policies_value(key, None);
            return;
        }
        let arr: toml_edit::Array = values
            .iter()
            .map(|s| toml_edit::Value::from(s.clone()))
            .collect();
        self.set_policies_value(key, Some(toml_edit::value(arr)));
    }

    /// 按 `id` 插入或替换一条 `[[authors]]`，返回 `true` 表示新增。
    pub fn upsert_author(&mut self, a: &Author) -> bool {
        let new_table = author_to_table(a);
        let mut aot = self
            .doc
            .as_table()
            .get("authors")
            .and_then(|i| i.as_array_of_tables())
            .cloned()
            .unwrap_or_default();
        let mut replaced = false;
        for t in aot.iter_mut() {
            if t.get("id").and_then(|v| v.as_str()) == Some(a.id.as_str()) {
                *t = new_table.clone();
                replaced = true;
                break;
            }
        }
        if !replaced {
            aot.push(new_table);
        }
        self.place_table("authors", Item::ArrayOfTables(aot));
        self.resync();
        !replaced
    }

    /// 按 `id` 删除 `[[authors]]`。
    pub fn remove_author(&mut self, id: &str) -> bool {
        let Some(aot) = self
            .doc
            .as_table_mut()
            .get_mut("authors")
            .and_then(|i| i.as_array_of_tables_mut())
        else {
            return false;
        };
        let before = aot.len();
        aot.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(id));
        let changed = aot.len() != before;
        if changed {
            self.resync();
        }
        changed
    }

    /// 在**指定分支**的 `entries[path]` 上设置字符串字段（保注释，键序由规范化收口）。
    ///
    /// `key` 限 `ENTRY_KEYS` 中的字符串字段；`v` 为空串则移除该键。
    /// 返回 `false` 表示该分支没有 `path` 对应的条目。
    pub fn set_entry_str(&mut self, path: &str, key: &str, v: &str) -> bool {
        self.with_entry(path, |t| {
            if v.is_empty() {
                t.remove(key);
            } else {
                t.insert(key, value(v.to_string()));
            }
        })
    }

    /// 在 `entries[path]` 上设置整数字段（如 `order`）。`None` 表示移除。
    pub fn set_entry_int(&mut self, path: &str, key: &str, v: Option<i64>) -> bool {
        self.with_entry(path, |t| match v {
            Some(n) => {
                t.insert(key, value(n));
            }
            None => {
                t.remove(key);
            }
        })
    }

    /// 对 `entries[path]` 施加一次就地编辑。
    fn with_entry(&mut self, path: &str, edit: impl FnOnce(&mut Table)) -> bool {
        let Some(aot) = self
            .doc
            .as_table_mut()
            .get_mut("entries")
            .and_then(|i| i.as_array_of_tables_mut())
        else {
            return false;
        };
        let mut hit = false;
        for t in aot.iter_mut() {
            if t.get("path").and_then(|v| v.as_str()) == Some(path) {
                edit(t);
                hit = true;
                break;
            }
        }
        if hit {
            self.resync();
        }
        hit
    }

    /// 插入或替换一条 `[[entries]]`，返回 `true` 表示新增。
    pub fn upsert_entry(&mut self, e: &Entry) -> bool {
        let new_table = entry_to_table(e);
        let mut aot = self
            .doc
            .as_table()
            .get("entries")
            .and_then(|i| i.as_array_of_tables())
            .cloned()
            .unwrap_or_default();

        let mut replaced = false;
        for t in aot.iter_mut() {
            if t.get("path").and_then(|v| v.as_str()) == Some(e.path.as_str()) {
                *t = new_table.clone();
                replaced = true;
                break;
            }
        }
        if !replaced {
            aot.push(new_table);
        }
        self.place_table("entries", Item::ArrayOfTables(aot));
        self.resync();
        !replaced
    }

    /// 删除指定 `path` 的 `[[entries]]` 元素。
    pub fn remove_entry_path(&mut self, path: &str) -> bool {
        let Some(aot) = self
            .doc
            .as_table_mut()
            .get_mut("entries")
            .and_then(|i| i.as_array_of_tables_mut())
        else {
            return false;
        };
        let before = aot.len();
        aot.retain(|t| t.get("path").and_then(|v| v.as_str()) != Some(path));
        let changed = aot.len() != before;
        if changed {
            self.resync();
        }
        changed
    }

    /// 追加一条 `[[refs]]`。
    pub fn push_ref(&mut self, r: &RefItem) {
        let mut aot = self
            .doc
            .as_table()
            .get("refs")
            .and_then(|i| i.as_array_of_tables())
            .cloned()
            .unwrap_or_default();
        aot.push(ref_to_table(r));
        self.place_table("refs", Item::ArrayOfTables(aot));
        self.resync();
    }

    /// 按 id 删除 `[[refs]]`。
    pub fn remove_ref(&mut self, id: &str) -> bool {
        let Some(aot) = self
            .doc
            .as_table_mut()
            .get_mut("refs")
            .and_then(|i| i.as_array_of_tables_mut())
        else {
            return false;
        };
        let before = aot.len();
        aot.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(id));
        let changed = aot.len() != before;
        if changed {
            self.resync();
        }
        changed
    }

    /// 规范 §4.9 归一化后的 TOML 文本（键序 / 表序固定，注释保留）。
    ///
    /// 属性是只读的派生视图：这里在**副本**上做规范化，不改动 `self.doc`。
    pub fn canonical_text(&self, strip_comments: bool) -> String {
        let mut doc = self.doc.clone();
        crate::meta::canonicalize_doc(&mut doc);
        let mut text = doc.to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if strip_comments {
            text = strip_line_comments(&text);
        }
        text
    }

    /// 保注释写回磁盘（写出的字节一律是 §4.9 规范形式）。
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = self.canonical_text(false);
        std::fs::write(path, text).map_err(|e| Error::io(path, e))
    }
}
