//! 增量层（DESIGN §4.3，M2-②）。
//!
//! 位于 ff-engine（而非 ff-core）的原因：`OverlayEntry` 直接复用 `ff_ntfs::RawEntry`
//! 的字段，而 ff-core 是零平台依赖的纯逻辑 crate，不依赖 ff-ntfs（DESIGN §0.5）。
//! 搜索时 Base（VolumeIndex）+ Overlay 合并；Overlay 条目的父链解析先查 Overlay 再查 Base。

use ff_core::VolumeIndex;
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
    ///
    /// `base` 为当前卷的 Base 索引：USN 中不改名的内容/元数据变化记录不带名字，
    /// `read_changes` 转换出的 `RawEntry.name` 此时为空串（S1）。这里按 frn 从 Base
    /// 查回原名再插入 Overlay——否则空名条目会遮蔽 Base 命中，让文件从搜索结果中
    /// 消失，并在合并时被固化进 Base、落盘损坏（DESIGN §5.3「name 为 None 时保留
    /// 原条目名字」）。查不到（真正的新文件在后续记录会更名）时保留空串。
    pub fn apply(&mut self, changes: &[Change], base: &VolumeIndex) {
        for c in changes {
            match c {
                Change::Upsert(e) => {
                    self.deleted.remove(&e.frn);
                    let mut oe = OverlayEntry::from_raw(e.clone());
                    if oe.name.is_empty() {
                        // 空名 = 内容/元数据变化记录：size 恒为 0、时间不可靠（契约
                        // usn.rs::record_to_change），标 STALE_META 让 UI 显示占位而非
                        // 误导性的 "0 B"（S2）。有名的新建/改名记录不在此列。
                        oe.stale_meta = true;
                        // ① 先查**本 Overlay** 的已有条目。同批里「FILE_CREATE（带名）
                        //    → 紧接着写入（空名）」是常态：此时 Base 里还没有这个文件，
                        //    只查 Base 会拿到空名，随即把刚入表的带名条目覆盖成空名，
                        //    文件直接消失。所以自身优先，Base 只作回退。
                        if let Some(prev) = self.added_or_changed.get(&e.frn) {
                            if !prev.name.is_empty() {
                                oe.name = prev.name.clone();
                            }
                            // 上一版本若带有真实元数据（带名记录来自重扫补齐），
                            // 沿用它；本批记录本身不带 size/时间，不能拿 0 覆盖。
                            if !prev.stale_meta {
                                oe.size = prev.size;
                                oe.mtime = prev.mtime;
                                oe.ctime = prev.ctime;
                                oe.atime = prev.atime;
                                oe.stale_meta = false;
                            }
                        }
                        // ② 回退 Base：已存在文件的元数据变化 / 改名前的原名。
                        if oe.name.is_empty() {
                            if let Some(name) =
                                base.frn_to_id(e.frn).and_then(|id| base.name_str(id))
                            {
                                oe.name = name.to_string();
                            }
                        }
                    }
                    self.added_or_changed.insert(e.frn, oe);
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
