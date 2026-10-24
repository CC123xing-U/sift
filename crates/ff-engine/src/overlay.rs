//! 增量层（DESIGN §4.3，M2-②）。
//!
//! 位于 ff-engine（而非 ff-core）的原因：`OverlayEntry` 直接复用 `ff_ntfs::RawEntry`
//! 的字段，而 ff-core 是零平台依赖的纯逻辑 crate，不依赖 ff-ntfs（DESIGN §0.5）。
//! 搜索时 Base（VolumeIndex）+ Overlay 合并；Overlay 条目的父链解析先查 Overlay 再查 Base。

use std::collections::HashSet;

use ff_ntfs::{Change, RawEntry};
use rustc_hash::{FxHashMap, FxHashSet};

/// 增量条目（与 `ff_ntfs::RawEntry` 字段一致）。
#[derive(Clone, Debug)]
pub struct OverlayEntry {
    pub frn: u64,
    pub parent_frn: u64,
    pub name: String,
    pub is_dir: bool,
    pub attrs: u32,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
    /// size/mtime 尚未补齐（USN 记录不含大小，DESIGN §4.3）；显示为空，不影响搜索。
    pub stale_meta: bool,
}

impl OverlayEntry {
    pub fn from_raw(e: RawEntry) -> Self {
        OverlayEntry {
            frn: e.frn,
            parent_frn: e.parent_frn,
            name: e.name,
            is_dir: e.is_dir,
            attrs: e.attrs,
            size: e.size,
            mtime: e.mtime,
            ctime: e.ctime,
            atime: e.atime,
            stale_meta: false,
        }
    }

    /// 由 `attrs` + `is_dir` 推导的 `flags` 位（与 `ff_core::entry` 的 `push` 一致），
    /// 供搜索时的 `hide_system` / `hide_hidden` 过滤与 `is_dir` 判定复用。
    pub fn flags(&self) -> u16 {
        use ff_core::entry::attrs as A;
        use ff_core::entry::flags as F;
        let mut f = 0u16;
        if self.is_dir || self.attrs & A::FILE_ATTRIBUTE_DIRECTORY != 0 {
            f |= F::IS_DIR;
        }
        if self.attrs & A::FILE_ATTRIBUTE_HIDDEN != 0 {
            f |= F::HIDDEN;
        }
        if self.attrs & A::FILE_ATTRIBUTE_SYSTEM != 0 {
            f |= F::SYSTEM;
        }
        if self.attrs & A::FILE_ATTRIBUTE_READONLY != 0 {
            f |= F::READONLY;
        }
        f
    }
}

/// 单卷增量层（DESIGN §4.3）。
#[derive(Default, Clone)]
pub struct Overlay {
    /// 新建 / 改名 / 移动后的条目（完整字段，按 frn 索引；同 frn 覆盖）。
    pub added_or_changed: FxHashMap<u64, OverlayEntry>,
    /// 在 Base 中被删除的 frn。
    pub deleted: FxHashSet<u64>,
}

impl Overlay {
    /// 应用一批 [`Change`]（DESIGN §5.3：按 frn 合并；Upsert 覆盖、Remove 置删）。
    pub fn apply(&mut self, changes: &[Change]) {
        for c in changes {
            match c {
                Change::Upsert(e) => {
                    self.deleted.remove(&e.frn);
                    self.added_or_changed
                        .insert(e.frn, OverlayEntry::from_raw(e.clone()));
                }
                Change::Remove { frn } => {
                    self.added_or_changed.remove(frn);
                    self.deleted.insert(*frn);
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added_or_changed.is_empty() && self.deleted.is_empty()
    }

    /// 增量条目总数（用于合并触发判断）。
    pub fn len(&self) -> usize {
        self.added_or_changed.len() + self.deleted.len()
    }
}

/// 供测试 / 集合运算用的便捷别名。
pub type OverlaySet = HashSet<u64>;
