//! `E_REVISION_STALE` 的**历史基线**（`._cache/revisions.json`）。
//!
//! 规范 §6.1 的 `E_REVISION_STALE` 是「`updated_at` 变化但 `revision` 未前进」——这是
//! **历史相关**判定，单看一份 `._meta` 无从下手（`created_at` / `updated_at` / `revision`
//! 都是自描述的，文件本身不携带「上一版」）。
//!
//! 因此由**写入端**在每次成功写盘后登记当前快照，校验端只读比对：
//!
//! - 写入端：`str` 的全部写命令（经 `cmd::save_meta`）与 `str sync`（经 [`record_scan`]）；
//! - 校验端：[`crate::validate`] 读基线，`updated_at` 变了而 `revision` 未前进即报码。
//!
//! 基线文件是**派生数据**：位于 `._cache/`（格式保留名、不入 `entries` 清单、`.gitignore`
//! 已排除），删掉即关闭这项检查；`str sync` 会按当前扫描结果整份重建，因此不会积累陈旧条目。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bundle::{Bundle, Scan};
use crate::meta::Meta;
use crate::util::CACHE_DIR;

/// 基线文件名（位于 `._cache/` 下）。
const FILE: &str = "revisions.json";

/// 一个分支的基线快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snap {
    /// 上次写盘时的 `revision`。
    pub revision: i64,
    /// 上次写盘时的 `updated_at`（TOML 原生 offset date-time 的字符串形式）。
    pub updated_at: String,
}

/// 全 bundle 的基线：`._meta` 所在目录的相对路径 → 快照。
///
/// 键与 `Visit::rel` 一致：ROOT 为 `"."`，其余为 `"<uuid>/"` 形式。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Baseline {
    /// 各分支的基线快照。
    #[serde(default)]
    pub branches: BTreeMap<String, Snap>,
}

/// 基线文件路径。
fn file_path(bundle: &Bundle) -> PathBuf {
    bundle.root.join(CACHE_DIR).join(FILE)
}

/// 从 `._meta` 取快照；`revision` / `updated_at` 缺任一者则视为不可登记。
fn snap_of(meta: &Meta) -> Option<Snap> {
    Some(Snap {
        revision: meta.revision?,
        updated_at: meta.updated_at.clone()?,
    })
}

/// 读取基线。文件缺失 / 损坏一律退化为「无基线」：只关闭历史检查，不阻断主流程。
pub fn load(bundle: &Bundle) -> Baseline {
    std::fs::read_to_string(file_path(bundle))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// 写回基线。**尽力而为**：写失败只影响历史检查，不影响调用方的主流程。
fn store(bundle: &Bundle, baseline: &Baseline) {
    let path = file_path(bundle);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(baseline) {
        let _ = std::fs::write(path, format!("{text}\n"));
    }
}

/// 登记单个分支的当前状态（单个写命令用）。
pub fn record(bundle: &Bundle, dir: &Path, meta: &Meta) {
    let Some(snap) = snap_of(meta) else {
        return;
    };
    let mut baseline = load(bundle);
    baseline.branches.insert(bundle.rel(dir), snap);
    store(bundle, &baseline);
}

/// 以一次扫描的结果校准基线（`str sync` / `str branch rm` 用）。
///
/// 与 [`record`] 的关键差别：**只有 `revision` 确实前进才推进基线**。否则保留旧快照，
/// 让 §6.1 的违规在后续 `validate` 中**持续可见**，直到有人真正修好 `revision` ——
/// 若这里无条件覆盖，`str sync` 会把刚犯下的违规顺手洗白。
///
/// 同时丢弃已消失分支的陈旧条目（`str branch rm` 之后）。
pub fn record_scan(bundle: &Bundle, scan: &Scan) {
    let mut baseline = load(bundle);
    let mut seen = std::collections::BTreeSet::new();
    for v in &scan.visits {
        let Some(meta) = v.meta.as_ref() else {
            continue;
        };
        let Some(snap) = snap_of(meta) else {
            continue;
        };
        seen.insert(v.rel.clone());
        match baseline.branches.get(&v.rel) {
            None => {
                baseline.branches.insert(v.rel.clone(), snap);
            }
            Some(old) if snap.revision > old.revision => {
                baseline.branches.insert(v.rel.clone(), snap);
            }
            Some(_) => {}
        }
    }
    baseline.branches.retain(|rel, _| seen.contains(rel));
    store(bundle, &baseline);
}
