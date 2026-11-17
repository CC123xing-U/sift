//! 条目表与索引构建（DESIGN §4.1、§4.2 存储部分、§5.2 流式构建）。

use std::collections::hash_map::Entry as HashMapEntry;

use rustc_hash::FxHashMap;

use crate::case::{has_cjk, simple_lowercase};

/// 卷内条目索引（位置索引进 SoA 列）；`NONE` 表示“无”
/// （根条目的父、无效 id）。
pub type EntryId = u32;

/// [`EntryId`] 的“无”哨兵值。
pub const NONE: EntryId = u32::MAX;

/// FRN 用作哈希键时的掩码：屏蔽高 16 位序列号（DESIGN §4.1）。
pub const FRN_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// 条目标志位（手写常量；DESIGN §4.1 未引入 bitflags）。
pub mod flags {
    /// 目录。
    pub const IS_DIR: u16 = 1 << 0;
    /// 隐藏（`FILE_ATTRIBUTE_HIDDEN`）。
    pub const HIDDEN: u16 = 1 << 1;
    /// 系统（`FILE_ATTRIBUTE_SYSTEM`）。
    pub const SYSTEM: u16 = 1 << 2;
    /// 只读（`FILE_ATTRIBUTE_READONLY`）。
    pub const READONLY: u16 = 1 << 3;
    /// 名字含 CJK 汉字（建索引时设置；供 M5 拼音 tier 只遍历 `cjk_ids`）。
    pub const HAS_CJK: u16 = 1 << 4;
    /// 元数据未补齐（DESIGN §4.3；M2 增量条目设置，M1 不产生）。
    pub const STALE_META: u16 = 1 << 5;
}

/// Win32 文件属性位（`FILE_ATTRIBUTE_*`；`ff-ntfs` 的 `RawEntry.attrs` 原值）。
pub mod attrs {
    pub const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
    pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
    pub const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0004;
    pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
}

/// 卷元数据（DESIGN §4.1）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeInfo {
    /// 盘符（如 `'C'`）；装载点卷为 `None`（路径前缀退化为 GUID 路径）。
    pub drive: Option<char>,
    /// GUID 路径（`\\?\Volume{...}`），用于显示与挂载点卷。
    pub guid_path: String,
    /// 卷序列号（持久化按卷分文件，DESIGN §6）。
    pub serial: u32,
    /// USN Journal id（DESIGN §5.3 重建条件之一）。
    pub journal_id: u64,
    /// 下一个待读 USN（续读起点，DESIGN §5.2）。
    pub next_usn: i64,
}

impl VolumeInfo {
    /// 路径前缀字节（`C:\`；无盘符时用 GUID 路径，再不行退化为 `\`）。
    fn path_prefix(&self) -> Vec<u8> {
        if let Some(c) = self.drive {
            return format!("{c}:\\").into_bytes();
        }
        if !self.guid_path.is_empty() {
            let mut p = self.guid_path.clone().into_bytes();
            if p.last() != Some(&b'\\') {
                p.push(b'\\');
            }
            return p;
        }
        b"\\".to_vec()
    }
}

/// 索引构建输入（中立结构；`ff-ntfs` 的 `RawEntry` 直接映射进来）。
///
/// 时间统一为 FILETIME（i64，100ns，1601 纪元，与 `RawEntry` 一致）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryInput {
    /// 文件引用号（MFT 记录号 + 序列号；用作键时取低 48 位）。
    pub frn: u64,
    /// 父目录 FRN；卷根的父为 0（解析为 [`NONE`]）。
    pub parent_frn: u64,
    pub name: String,
    pub is_dir: bool,
    /// Win32 文件属性位（见 [`attrs`]）。
    pub attrs: u32,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
}

/// 流式索引构建器（DESIGN §5.2：边读边写列，`finish` 时整理）。
///
/// `EntryId` 按推入顺序分配（0, 1, 2, ...）；父条目可在子条目**之后**推入
/// （`finish` 统一解析 parent）。
pub struct IndexBuilder {
    volume: VolumeInfo,
    names: Vec<u8>,
    names_lower: Vec<u8>,
    name_off: Vec<u32>,
    parent_frn: Vec<u64>,
    flags: Vec<u16>,
    frn: Vec<u64>,
    size: Vec<u64>,
    mtime: Vec<i64>,
    ctime: Vec<i64>,
    atime: Vec<i64>,
    unresolved_parents: usize,
    duplicate_frns: usize,
}

impl IndexBuilder {
    pub fn new(volume: VolumeInfo) -> Self {
        Self {
            volume,
            names: Vec::new(),
            names_lower: Vec::new(),
            name_off: Vec::new(),
            parent_frn: Vec::new(),
            flags: Vec::new(),
            frn: Vec::new(),
            size: Vec::new(),
            mtime: Vec::new(),
            ctime: Vec::new(),
            atime: Vec::new(),
            unresolved_parents: 0,
            duplicate_frns: 0,
        }
    }

    /// 推入一个条目（字段语义同 `RawEntry`）。
    pub fn push(&mut self, e: EntryInput) {
        let name_bytes = e.name.as_bytes();
        // 名字 arena：各名字首尾相连，条目间以 0x00 分隔（DESIGN §4.1）
        self.name_off.push(self.names.len() as u32);
        self.names.extend_from_slice(name_bytes);
        self.names.push(0);
        // 小写副本：与 names 逐字节等长（simple_lowercase 保证，单元测试覆盖）
        self.names_lower
            .extend_from_slice(&simple_lowercase(name_bytes));
        self.names_lower.push(0);
        debug_assert_eq!(self.names.len(), self.names_lower.len());

        let mut f = 0u16;
        if e.is_dir || e.attrs & attrs::FILE_ATTRIBUTE_DIRECTORY != 0 {
            f |= flags::IS_DIR;
        }
        if e.attrs & attrs::FILE_ATTRIBUTE_HIDDEN != 0 {
            f |= flags::HIDDEN;
        }
        if e.attrs & attrs::FILE_ATTRIBUTE_SYSTEM != 0 {
            f |= flags::SYSTEM;
        }
        if e.attrs & attrs::FILE_ATTRIBUTE_READONLY != 0 {
            f |= flags::READONLY;
        }
        if has_cjk(&e.name) {
            f |= flags::HAS_CJK;
        }

        self.flags.push(f);
        self.parent_frn.push(e.parent_frn);
        self.frn.push(e.frn);
        self.size.push(e.size);
        self.mtime.push(e.mtime);
        self.ctime.push(e.ctime);
        self.atime.push(e.atime);
    }

    /// 收尾：构建 FRN 映射、解析 parent、汇总 `cjk_ids`，产出不可变索引。
    pub fn finish(mut self) -> VolumeIndex {
        // FRN → id（低 48 位；重复 FRN 保留先推入者并计数）
        let mut frn_map: FxHashMap<u64, EntryId> =
            FxHashMap::with_capacity_and_hasher(self.frn.len(), Default::default());
        for (i, &f) in self.frn.iter().enumerate() {
            match frn_map.entry(f & FRN_MASK) {
                HashMapEntry::Occupied(_) => self.duplicate_frns += 1,
                HashMapEntry::Vacant(v) => {
                    v.insert(i as EntryId);
                }
            }
        }
        // 根条目：第一个 parent_frn == 0 的条目（路径生成时其自身不出现）
        let root_id = self
            .parent_frn
            .iter()
            .position(|&p| p == 0)
            .map(|i| i as EntryId);
        // 解析 parent（顺序无关：子先父后亦可）；找不到父 → 挂到根下并计数
        let mut parent = Vec::with_capacity(self.parent_frn.len());
        for &pf in &self.parent_frn {
            if pf == 0 {
                parent.push(NONE);
                continue;
            }
            match frn_map.get(&(pf & FRN_MASK)) {
                Some(&pid) => parent.push(pid),
                None => {
                    self.unresolved_parents += 1;
                    parent.push(root_id.unwrap_or(NONE));
                }
            }
        }
        // cjk_ids（M5 拼音 tier 只遍历这部分）
        let cjk_ids: Vec<EntryId> = self
            .flags
            .iter()
            .enumerate()
            .filter(|(_, &f)| f & flags::HAS_CJK != 0)
            .map(|(i, _)| i as EntryId)
            .collect();
        let path_prefix = self.volume.path_prefix();
        VolumeIndex {
            volume: self.volume,
            names: self.names,
            names_lower: self.names_lower,
            name_off: self.name_off,
            parent,
            flags: self.flags,
            frn: self.frn,
            size: self.size,
            mtime: self.mtime,
            ctime: self.ctime,
            atime: self.atime,
            frn_map,
            cjk_ids,
            path_prefix,
            root_id,
            unresolved_parents: self.unresolved_parents,
            duplicate_frns: self.duplicate_frns,
        }
    }
}

/// 单卷内存索引（SoA，DESIGN §4.1）。
///
/// 列字段公开只读（引擎 / 测试直接读列）；FRN 映射、`cjk_ids` 等派生数据私有，
/// 经访问器读取。`frn_map` **不落盘**，载入后由 `finish` 重建（DESIGN §4.1）。
#[derive(Clone, Debug)]
pub struct VolumeIndex {
    /// 卷元数据。
    pub volume: VolumeInfo,
    /// UTF-8 名字 arena，条目间以 0x00 分隔。
    pub names: Vec<u8>,
    /// 同布局小写副本（匹配用；偏移与 `names` 完全一致）。
    pub names_lower: Vec<u8>,
    /// 名字在 `names` 中的起点（长度 = 条目数；名字到下一个 0x00 为止）。
    pub name_off: Vec<u32>,
    /// 父条目；根为 [`NONE`]。
    pub parent: Vec<u32>,
    /// 标志位（见 [`flags`]）。
    pub flags: Vec<u16>,
    /// 文件引用号（低 48 位为有效键，见 [`FRN_MASK`]）。
    pub frn: Vec<u64>,
    pub size: Vec<u64>,
    /// FILETIME（100ns，1601 纪元）。
    pub mtime: Vec<i64>,
    pub ctime: Vec<i64>,
    pub atime: Vec<i64>,

    // —— 派生数据（finish 时构建，不落盘）——
    frn_map: FxHashMap<u64, EntryId>,
    cjk_ids: Vec<EntryId>,
    path_prefix: Vec<u8>,
    root_id: Option<EntryId>,
    unresolved_parents: usize,
    duplicate_frns: usize,
}

impl VolumeIndex {
    /// 条目数（各 SoA 列长度）。
    pub fn entry_count(&self) -> usize {
        self.name_off.len()
    }

    /// 根条目 id（第一个 `parent_frn == 0` 的条目；无则为 `None`）。
    pub fn root_id(&self) -> Option<EntryId> {
        self.root_id
    }

    /// parent 解析失败（挂到根下）的条目数（DESIGN §5.2 警告计数）。
    pub fn unresolved_parents(&self) -> usize {
        self.unresolved_parents
    }

    /// 重复 FRN 的条目数（保留先推入者）。
    pub fn duplicate_frns(&self) -> usize {
        self.duplicate_frns
    }

    /// 含 CJK 汉字的条目列表（M5 拼音 tier 只遍历这部分，DESIGN §4.1）。
    pub fn cjk_ids(&self) -> &[EntryId] {
        &self.cjk_ids
    }

    /// FRN（自动屏蔽高 16 位序列号）→ 条目 id。
    pub fn frn_to_id(&self, frn: u64) -> Option<EntryId> {
        self.frn_map.get(&(frn & FRN_MASK)).copied()
    }

    /// 条目名字（UTF-8 字节，不含 0x00 分隔符）；无效 id 返回空。
    pub fn name(&self, id: EntryId) -> &[u8] {
        self.name_in(&self.names, id)
    }

    /// 条目名字的 `&str` 视图（非法 UTF-8 返回 `None`；索引输入恒为合法）。
    pub fn name_str(&self, id: EntryId) -> Option<&str> {
        std::str::from_utf8(self.name(id)).ok()
    }

    /// 从指定 arena（`names` 或 `names_lower`，偏移一致）取名字切片。
    pub(crate) fn name_in<'a>(&'a self, arena: &'a [u8], id: EntryId) -> &'a [u8] {
        let i = id as usize;
        if i >= self.name_off.len() {
            return &[];
        }
        let start = self.name_off[i] as usize;
        let end = match self.name_off.get(i + 1) {
            // 下一个条目前一个字节是 0x00 分隔符
            Some(&next) => (next as usize).saturating_sub(1),
            // 最后一个条目：arena 末尾是 0x00（容忍无尾分隔符的布局）
            None => {
                let mut e = arena.len();
                if e > start && arena[e - 1] == 0 {
                    e -= 1;
                }
                e
            }
        };
        // 防御性取切片：正常索引由 load / IndexBuilder 保证 start/end 合法，但
        // name_in 接收任意 arena，用 get() 确保畸形数据下退化为空串而非越界 panic。
        let hi = end.max(start);
        arena.get(start..hi).unwrap_or(&[])
    }

    /// 匹配用 arena（默认小写副本；大小写敏感时用原文）。
    pub(crate) fn haystack(&self, case_sensitive: bool) -> &[u8] {
        if case_sensitive {
            &self.names
        } else {
            &self.names_lower
        }
    }

    /// 路径前缀字节（`C:\` 等，`finish` 时由 [`VolumeInfo`] 预计算）。
    pub(crate) fn path_prefix_bytes(&self) -> &[u8] {
        &self.path_prefix
    }

    /// 由原始列 + 卷元数据重建索引（落盘载入共用，见 `persist` 模块）。
    ///
    /// 与 `IndexBuilder::finish` 不同：本函数**不重新解析 parent**（存储的 `parent`
    /// 已是解析后的 `EntryId`），且无法从列还原全量扫描期的诊断计数，故
    /// `unresolved_parents` / `duplicate_frns` 置 0（仅为诊断信息，不影响查询）。
    /// FRN 映射 / `cjk_ids` / `path_prefix` / `root_id` 由列重建。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_raw(
        volume: VolumeInfo,
        names: Vec<u8>,
        names_lower: Vec<u8>,
        name_off: Vec<u32>,
        parent: Vec<u32>,
        flags: Vec<u16>,
        frn: Vec<u64>,
        size: Vec<u64>,
        mtime: Vec<i64>,
        ctime: Vec<i64>,
        atime: Vec<i64>,
    ) -> VolumeIndex {
        let n = name_off.len();
        let mut frn_map: FxHashMap<u64, EntryId> =
            FxHashMap::with_capacity_and_hasher(n, Default::default());
        for (i, &f) in frn.iter().enumerate() {
            match frn_map.entry(f & FRN_MASK) {
                HashMapEntry::Occupied(_) => {}
                HashMapEntry::Vacant(v) => {
                    v.insert(i as EntryId);
                }
            }
        }
        let root_id = parent.iter().position(|&p| p == NONE).map(|i| i as EntryId);
        let cjk_ids: Vec<EntryId> = flags
            .iter()
            .enumerate()
            .filter(|(_, &f)| f & flags::HAS_CJK != 0)
            .map(|(i, _)| i as EntryId)
            .collect();
        let path_prefix = volume.path_prefix();
        VolumeIndex {
            volume,
            names,
            names_lower,
            name_off,
            parent,
            flags,
            frn,
            size,
            mtime,
            ctime,
            atime,
            frn_map,
            cjk_ids,
            path_prefix,
            root_id,
            unresolved_parents: 0,
            duplicate_frns: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> VolumeInfo {
        VolumeInfo {
            drive: Some('C'),
            ..Default::default()
        }
    }

    fn dir(frn: u64, parent: u64, name: &str) -> EntryInput {
        EntryInput {
            frn,
            parent_frn: parent,
            name: name.to_string(),
            is_dir: true,
            attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
            size: 0,
            mtime: 1,
            ctime: 1,
            atime: 1,
        }
    }

    fn file(frn: u64, parent: u64, name: &str) -> EntryInput {
        EntryInput {
            is_dir: false,
            attrs: 0,
            ..dir(frn, parent, name)
        }
    }

    /// C:\ (root, frn 5)
    ///  ├─ Windows\        (frn 100)
    ///  │   └─ readme.txt  (frn 101)
    ///  └─ 用户\            (frn 200)
    ///      └─ 文档.txt     (frn 201)
    fn sample() -> VolumeIndex {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(dir(100, 5, "Windows"));
        b.push(file(101, 100, "readme.txt"));
        b.push(dir(200, 5, "用户"));
        b.push(file(201, 200, "文档.txt"));
        b.finish()
    }

    #[test]
    fn columns_len_eq_entry_count() {
        let ix = sample();
        let n = ix.entry_count();
        assert_eq!(n, 5);
        assert_eq!(ix.name_off.len(), n);
        assert_eq!(ix.parent.len(), n);
        assert_eq!(ix.flags.len(), n);
        assert_eq!(ix.frn.len(), n);
        assert_eq!(ix.size.len(), n);
        assert_eq!(ix.mtime.len(), n);
        assert_eq!(ix.ctime.len(), n);
        assert_eq!(ix.atime.len(), n);
        // 两个 arena 逐字节等长
        assert_eq!(ix.names.len(), ix.names_lower.len());
    }

    #[test]
    fn names_and_lower_offsets_match() {
        let ix = sample();
        for id in 0..ix.entry_count() as EntryId {
            let orig = ix.name(id);
            let start = ix.name_off[id as usize] as usize;
            let low = &ix.names_lower[start..start + orig.len()];
            assert_eq!(low, &crate::case::simple_lowercase(orig)[..]);
        }
    }

    #[test]
    fn parent_resolution_child_before_parent() {
        // 子条目先推、父条目后推：finish 仍应正确解析
        let mut b = IndexBuilder::new(info());
        b.push(file(101, 100, "readme.txt")); // id 0
        b.push(dir(100, 5, "Windows")); // id 1
        b.push(dir(5, 0, ".")); // id 2
        let ix = b.finish();
        assert_eq!(ix.parent[0], 1);
        assert_eq!(ix.parent[1], 2);
        assert_eq!(ix.parent[2], NONE);
        assert_eq!(ix.root_id(), Some(2));
        assert_eq!(ix.unresolved_parents(), 0);
    }

    #[test]
    fn unresolved_parent_attaches_to_root() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(999, 12345, "orphan.txt")); // 父 12345 不存在
        let ix = b.finish();
        assert_eq!(ix.parent[1], 0); // 挂到根下
        assert_eq!(ix.unresolved_parents(), 1);
    }

    #[test]
    fn unresolved_parent_without_root_entry() {
        let mut b = IndexBuilder::new(info());
        b.push(file(999, 12345, "orphan.txt"));
        let ix = b.finish();
        assert_eq!(ix.parent[0], NONE);
        assert_eq!(ix.root_id(), None);
        assert_eq!(ix.unresolved_parents(), 1);
    }

    #[test]
    fn duplicate_frn_counted_first_wins() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(7, 5, "a.txt"));
        b.push(file(7, 5, "b.txt")); // 重复 FRN
        let ix = b.finish();
        assert_eq!(ix.duplicate_frns(), 1);
        assert_eq!(ix.frn_to_id(7), Some(1)); // 先推入者胜
        assert_eq!(ix.frn_to_id(0x0000_1234_5678_9ABC), None);
    }

    #[test]
    fn frn_to_id_masks_serial() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(0x0000_0000_DEAD_BEEF, 5, "x"));
        let ix = b.finish();
        // 高 16 位序列号被屏蔽
        assert_eq!(ix.frn_to_id(0x0001_0000_DEAD_BEEF), Some(1));
    }

    #[test]
    fn cjk_flag_and_ids() {
        let ix = sample();
        assert!(ix.flags[3] & flags::HAS_CJK != 0); // 用户
        assert!(ix.flags[4] & flags::HAS_CJK != 0); // 文档.txt
        assert!(ix.flags[2] & flags::HAS_CJK == 0); // readme.txt
        assert_eq!(ix.cjk_ids(), &[3, 4]);
    }

    #[test]
    fn flag_bits_from_attrs() {
        let mut b = IndexBuilder::new(info());
        b.push(EntryInput {
            frn: 1,
            parent_frn: 0,
            name: "sys.dat".into(),
            is_dir: false,
            attrs: attrs::FILE_ATTRIBUTE_HIDDEN
                | attrs::FILE_ATTRIBUTE_SYSTEM
                | attrs::FILE_ATTRIBUTE_READONLY,
            size: 1,
            mtime: 0,
            ctime: 0,
            atime: 0,
        });
        let ix = b.finish();
        let f = ix.flags[0];
        assert!(f & flags::IS_DIR == 0);
        assert!(f & flags::HIDDEN != 0);
        assert!(f & flags::SYSTEM != 0);
        assert!(f & flags::READONLY != 0);
    }

    #[test]
    fn is_dir_from_attrs_only() {
        // 扫描器只给 attrs 时也能识别目录
        let mut b = IndexBuilder::new(info());
        b.push(EntryInput {
            frn: 1,
            parent_frn: 0,
            name: "d".into(),
            is_dir: false,
            attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
            size: 0,
            mtime: 0,
            ctime: 0,
            atime: 0,
        });
        let ix = b.finish();
        assert!(ix.flags[0] & flags::IS_DIR != 0);
    }

    #[test]
    fn empty_index() {
        let ix = IndexBuilder::new(info()).finish();
        assert_eq!(ix.entry_count(), 0);
        assert!(ix.names.is_empty());
        assert!(ix.cjk_ids().is_empty());
        assert_eq!(ix.name(0), b"");
        // 无效 id → 空名字（合法 UTF-8，故为 Some("")）
        assert_eq!(ix.name_str(NONE), Some(""));
        assert_eq!(ix.name_str(0), Some(""));
    }

    #[test]
    fn unicode_names_searchable_bytes() {
        let ix = sample();
        assert_eq!(ix.name_str(3), Some("用户"));
        assert_eq!(ix.name_str(4), Some("文档.txt"));
        // 小写副本对中文原样保留
        assert_eq!(ix.name(4), "文档.txt".as_bytes());
    }
}
