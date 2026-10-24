//! USN Journal 纯逻辑（DESIGN §5.3）：FRN 掩码 · 记录字节解析 · reason 合并规则。
//!
//! 本模块**不依赖 Windows、不需要管理员权限**，全部逻辑可完整单测。
//! `VolumeScanner::read_changes` 本体属 M2；按本轮任务要求，M2 需要的纯逻辑提前交付：
//!
//! - [`mask_frn`]：FRN 用作键时统一屏蔽高 16 位序列号（DESIGN §5.3）；
//! - [`parse_usn_journal_buffer`]：从 `FSCTL_READ_USN_JOURNAL` 的输出字节切片**独立解析**
//!   记录（V2 全字段；V3/ReFS 的 128 位 FRN 记录跳过并计数——DESIGN §5.3 要求忽略不崩溃；
//!   损坏记录报告后停止，已解析部分仍可用）；
//! - [`merge_usn_events`]：把解析出的记录按 reason 合并成 [`UsnEvent`]
//!   （只在 `CLOSE` 记录上产出事件；先判 `FILE_DELETE`；重命名以 `RENAME_NEW_NAME` 为准，
//!   `RENAME_OLD_NAME` 忽略）。
//!
//! M2 落地时二选一：ntfs-reader 的 `Journal`（支持 `NextUsn::Custom` 续读）或裸
//! `FSCTL_READ_USN_JOURNAL` + 本模块解析；无论哪条路，合并规则都是本模块这份。

use crate::{Change, RawEntry};

/// FRN 掩码：屏蔽高 16 位序列号，保留低 48 位记录号（DESIGN §5.3）。
pub const FRN_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// FRN 用作键时取低 48 位（屏蔽高 16 位序列号）。
pub const fn mask_frn(frn: u64) -> u64 {
    frn & FRN_MASK
}

/// USN reason 位（取值与 Win32 `USN_REASON_*` 一致）。
///
/// 在本模块内自行定义，保证跨平台可编译、可单测；
/// 与 `windows` crate `Win32_System_Ioctl` 的常量一一对应。
pub mod reason {
    pub const DATA_OVERWRITE: u32 = 0x0000_0001;
    pub const DATA_EXTEND: u32 = 0x0000_0002;
    pub const DATA_TRUNCATION: u32 = 0x0000_0004;
    pub const NAMED_DATA_OVERWRITE: u32 = 0x0000_0010;
    pub const NAMED_DATA_EXTEND: u32 = 0x0000_0020;
    pub const NAMED_DATA_TRUNCATION: u32 = 0x0000_0040;
    pub const FILE_CREATE: u32 = 0x0000_0100;
    pub const FILE_DELETE: u32 = 0x0000_0200;
    pub const EA_CHANGE: u32 = 0x0000_0400;
    pub const SECURITY_CHANGE: u32 = 0x0000_0800;
    pub const RENAME_OLD_NAME: u32 = 0x0000_1000;
    pub const RENAME_NEW_NAME: u32 = 0x0000_2000;
    pub const INDEXABLE_CHANGE: u32 = 0x0000_4000;
    pub const BASIC_INFO_CHANGE: u32 = 0x0000_8000;
    pub const HARD_LINK_CHANGE: u32 = 0x0001_0000;
    pub const COMPRESSION_CHANGE: u32 = 0x0002_0000;
    pub const ENCRYPTION_CHANGE: u32 = 0x0004_0000;
    pub const OBJECT_ID_CHANGE: u32 = 0x0008_0000;
    pub const REPARSE_POINT_CHANGE: u32 = 0x0010_0000;
    pub const STREAM_CHANGE: u32 = 0x0020_0000;
    pub const TRANSACTED_CHANGE: u32 = 0x0040_0000;
    pub const INTEGRITY_CHANGE: u32 = 0x0080_0000;
    pub const DESIRED_STORAGE_CLASS_CHANGE: u32 = 0x0100_0000;
    /// 句柄关闭时 NTFS 把累积的 reason 合并进这条记录；只认它上面的变化。
    pub const CLOSE: u32 = 0x8000_0000;
}

/// Win32 `FILE_ATTRIBUTE_DIRECTORY`（USN 记录里判断是否目录用）。
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;

/// V2（`USN_RECORD_V2`）记录固定头长度：不含 FileName。
pub const USN_V2_HEADER_LEN: usize = 60;

/// 从字节切片解析出的一条 USN 记录（V2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUsnRecord {
    /// 记录在日志中的位置（续读时以此为新的起点）。
    pub usn: i64,
    /// 发生变化的文件 FRN（未掩码；按键用时过 [`mask_frn`]）。
    pub file_frn: u64,
    /// 变化时所在目录的 FRN（未掩码）。
    pub parent_frn: u64,
    /// reason 位组合（见 [`reason`]）。
    pub reason: u32,
    /// 变化时的 `FILE_ATTRIBUTE_*` 标志。
    pub file_attributes: u32,
    /// FILETIME（100ns，1601 纪元）。
    pub timestamp: i64,
    /// 文件名（不含目录）。尽力而为的 UTF-8：未配对代理替换为 U+FFFD。
    /// 非改名事件的记录该字段通常为空。
    pub name: String,
    /// 记录主版本（恒为 2；V3/V4 不会进到 records 里）。
    pub major_version: u16,
}

/// [`parse_usn_journal_buffer`] 的结果。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct JournalParseResult {
    /// 输出缓冲前 8 字节：**下一条**待读记录的 USN（续读位置）。
    pub next_usn: i64,
    /// 成功解析出的 V2 记录，按日志顺序。
    pub records: Vec<ParsedUsnRecord>,
    /// 跳过的 V3（ReFS，128 位 FRN）记录数：DESIGN §5.3 要求记录并忽略，不崩溃。
    pub v3_skipped: u64,
    /// 跳过的其他未知主版本记录数。
    pub unsupported_version_skipped: u64,
    /// 解析被迫停止的原因（损坏记录 / 缓冲截断）；`None` 表示干净解析到缓冲末尾。
    /// 停止点之前的 `records` 与 `next_usn` 仍有效。
    pub stopped_at: Option<String>,
}

/// 解析 `FSCTL_READ_USN_JOURNAL` 的输出缓冲：前 8 字节为下一条 USN，其后是记录序列。
///
/// 规则：
/// - `RecordLength` 必须 ≥ 60（V2 固定头）、是 8 的倍数、不越界，否则视为损坏：
///   记录原因并**停止**（已解析部分保留，调用方可从最后一条有效记录的 USN 重试）；
/// - V3 / 其他主版本：按 `RecordLength` 跳过并计数，不崩溃（DESIGN §5.3）；
/// - 文件名按 UTF-16LE 解码，`String::from_utf16_lossy`（未配对代理 → U+FFFD）。
pub fn parse_usn_journal_buffer(buf: &[u8]) -> JournalParseResult {
    let mut result = JournalParseResult::default();
    if buf.len() < 8 {
        result.stopped_at = Some(format!(
            "输出缓冲不足 8 字节（无 next USN），实际 {} 字节",
            buf.len()
        ));
        return result;
    }
    result.next_usn = i64::from_le_bytes(buf[..8].try_into().unwrap());

    let mut offset = 8usize;
    let mut index = 0u64;
    while offset < buf.len() {
        let remaining = buf.len() - offset;
        if remaining < 8 {
            result.stopped_at = Some(format!(
                "记录 #{index}: 头部不完整（剩余 {remaining} 字节 < 8）"
            ));
            break;
        }
        let record_length = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
        let major_version = u16::from_le_bytes(buf[offset + 4..offset + 6].try_into().unwrap());
        let _minor_version = u16::from_le_bytes(buf[offset + 6..offset + 8].try_into().unwrap());

        if record_length < 8 || record_length % 8 != 0 {
            result.stopped_at = Some(format!(
                "记录 #{index}: RecordLength={record_length} 非法（< 8 或不是 8 的倍数），卷或缓冲已损坏"
            ));
            break;
        }
        let record_length = record_length as usize;
        if offset + record_length > buf.len() {
            result.stopped_at = Some(format!(
                "记录 #{index}: RecordLength={record_length} 超出缓冲剩余 {} 字节",
                buf.len() - offset
            ));
            break;
        }
        let record = &buf[offset..offset + record_length];

        match major_version {
            2 => {
                if record_length < USN_V2_HEADER_LEN {
                    result.stopped_at = Some(format!(
                        "记录 #{index}: V2 记录长度 {record_length} < 固定头 {USN_V2_HEADER_LEN}"
                    ));
                    break;
                }
                let file_frn = u64::from_le_bytes(record[8..16].try_into().unwrap());
                let parent_frn = u64::from_le_bytes(record[16..24].try_into().unwrap());
                let usn = i64::from_le_bytes(record[24..32].try_into().unwrap());
                let timestamp = i64::from_le_bytes(record[32..40].try_into().unwrap());
                let reason = u32::from_le_bytes(record[40..44].try_into().unwrap());
                let file_attributes = u32::from_le_bytes(record[52..56].try_into().unwrap());
                let name_length = u16::from_le_bytes(record[56..58].try_into().unwrap()) as usize;
                let name_offset = u16::from_le_bytes(record[58..60].try_into().unwrap()) as usize;

                let name_end = match name_offset.checked_add(name_length) {
                    Some(end) if end <= record_length => end,
                    _ => {
                        result.stopped_at = Some(format!(
                            "记录 #{index}: FileNameOffset={name_offset} + FileNameLength={name_length} 超出记录长度 {record_length}"
                        ));
                        break;
                    }
                };
                let units: Vec<u16> = record[name_offset..name_end]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .copied()
                    .map(u16::from_le_bytes)
                    .collect();

                result.records.push(ParsedUsnRecord {
                    usn,
                    file_frn,
                    parent_frn,
                    reason,
                    file_attributes,
                    timestamp,
                    name: String::from_utf16_lossy(&units),
                    major_version,
                });
            }
            3 => {
                // ReFS：128 位 FRN，本项目只支持 NTFS V2（DESIGN §5.3）。跳过不崩溃。
                result.v3_skipped += 1;
            }
            _other => {
                result.unsupported_version_skipped += 1;
            }
        }

        offset += record_length;
        index += 1;
    }
    result
}

/// 合并后的 USN 事件（M2 写入 Overlay 的输入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsnEvent {
    /// 新建 / 重命名 / 移动 / 元数据变化。
    ///
    /// USN 记录**不含**文件大小与时间戳：`size`/时间由 M2 后台补齐，
    /// 补齐前标记 `STALE_META`（DESIGN §4.3）。
    /// `name` 为 `None` 表示该记录没带名字（非改名事件），消费方应保留原条目的名字。
    Upsert {
        /// 掩码后的 FRN（键）。
        frn: u64,
        parent_frn: u64,
        name: Option<String>,
        is_dir: bool,
        attrs: u32,
    },
    /// 删除。
    Remove {
        /// 掩码后的 FRN（键）。
        frn: u64,
    },
}

/// 内容 / 元数据类 reason（非改名、非删建的 CLOSE 记录据此判为"需要刷新条目"）。
const METADATA_CHANGE_REASONS: u32 = reason::DATA_OVERWRITE
    | reason::DATA_EXTEND
    | reason::DATA_TRUNCATION
    | reason::NAMED_DATA_OVERWRITE
    | reason::NAMED_DATA_EXTEND
    | reason::NAMED_DATA_TRUNCATION
    | reason::EA_CHANGE
    | reason::SECURITY_CHANGE
    | reason::INDEXABLE_CHANGE
    | reason::BASIC_INFO_CHANGE
    | reason::HARD_LINK_CHANGE
    | reason::COMPRESSION_CHANGE
    | reason::ENCRYPTION_CHANGE
    | reason::OBJECT_ID_CHANGE
    | reason::REPARSE_POINT_CHANGE
    | reason::STREAM_CHANGE
    | reason::TRANSACTED_CHANGE
    | reason::INTEGRITY_CHANGE
    | reason::DESIRED_STORAGE_CLASS_CHANGE;

/// 把一条 USN 记录分类成一个可选事件（合并规则核心，纯函数）。
///
/// 顺序不能换（DESIGN §5.3 与 ntfs-reader journal 指南一致）：
/// 1. 没有 `CLOSE` 位的记录是中间态，其位会重现于关闭记录——不产出事件；
/// 2. 先判 `FILE_DELETE`：临时文件"创建→写→删"经一个句柄会合成一条
///    `FILE_CREATE | ... | FILE_DELETE | CLOSE` 记录，放最后判会误报成创建；
/// 3. `RENAME_NEW_NAME`：重命名以新名记录为准，`RENAME_OLD_NAME` 记录忽略；
/// 4. `FILE_CREATE`：新建；
/// 5. 其余内容 / 元数据类变化：刷新条目（`STALE_META`，M2 后台补 size/时间）。
pub fn classify_usn_record(record: &ParsedUsnRecord) -> Option<UsnEvent> {
    if record.reason & reason::CLOSE == 0 {
        return None;
    }
    if record.reason & reason::FILE_DELETE != 0 {
        return Some(UsnEvent::Remove {
            frn: mask_frn(record.file_frn),
        });
    }
    if record.reason & reason::RENAME_NEW_NAME != 0
        || record.reason & reason::FILE_CREATE != 0
        || record.reason & METADATA_CHANGE_REASONS != 0
    {
        let name = (!record.name.is_empty()).then(|| record.name.clone());
        return Some(UsnEvent::Upsert {
            frn: mask_frn(record.file_frn),
            parent_frn: record.parent_frn,
            name,
            is_dir: record.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            attrs: record.file_attributes,
        });
    }
    None
}

/// 按顺序把记录合并成事件序列（同一 frn 的多次变化由消费方按序应用，后写覆盖）。
pub fn merge_usn_events(records: &[ParsedUsnRecord]) -> Vec<UsnEvent> {
    records
        .iter()
        .filter_map(classify_usn_record)
        .collect::<Vec<_>>()
}

/// 把一条解析后的记录转换为契约 [`Change`]（reason 合并规则见 [`classify_usn_record`]）。
///
/// - 无事件（非 `CLOSE` 中间记录 / 关闭但无已知 reason）→ `None`；
/// - `FILE_DELETE` → `Change::Remove { frn }`（frn 已掩码）；
/// - 其余（创建 / 改名 / 写入 / 元数据）→ `Change::Upsert(RawEntry)`。
///
/// USN 记录**不含**文件大小与可靠时间戳：`size` 置 0，三时间戳统一用记录的
/// `timestamp`（FILETIME）。消费方应据此标记 `STALE_META` 并在后台补齐
/// （DESIGN §4.3）；`name` 为 `None` 时保留原条目名字（非改名事件不带名字）。
pub fn record_to_change(record: &ParsedUsnRecord) -> Option<Change> {
    match classify_usn_record(record)? {
        UsnEvent::Remove { frn } => Some(Change::Remove { frn }),
        UsnEvent::Upsert {
            frn,
            parent_frn,
            name,
            is_dir,
            attrs,
        } => Some(Change::Upsert(RawEntry {
            frn,
            parent_frn,
            name: name.unwrap_or_default(),
            is_dir,
            attrs,
            size: 0,
            mtime: record.timestamp,
            ctime: record.timestamp,
            atime: record.timestamp,
        })),
    }
}

/// 把解析后的记录序列按顺序转换为 [`Change`] 序列（过滤掉无事件的记录）。
///
/// 供 `scanner.rs::read_changes` 在 M2 落地时使用；与 [`merge_usn_events`] 共用同一份
/// reason 合并规则，只是输出类型不同（契约 [`Change`] vs 内部 [`UsnEvent`]）。
pub fn records_to_changes(records: &[ParsedUsnRecord]) -> Vec<Change> {
    records.iter().filter_map(record_to_change).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条 V2 记录的字节（含文件名）。
    ///
    /// 真实驱动的 `RecordLength` 会补齐到 8 字节倍数（ntfs-reader 的解析同样要求），
    /// 这里如实模拟：名字后补零，`FileNameLength` 仍是真实名字长度。
    fn v2_record(usn: i64, frn: u64, parent: u64, reason: u32, attrs: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_bytes = units.len() * 2;
        let record_len = (USN_V2_HEADER_LEN + name_bytes).next_multiple_of(8);
        let mut b = vec![0u8; record_len];
        b[0..4].copy_from_slice(&(record_len as u32).to_le_bytes());
        b[4..6].copy_from_slice(&2u16.to_le_bytes()); // MajorVersion
        b[6..8].copy_from_slice(&0u16.to_le_bytes()); // MinorVersion
        b[8..16].copy_from_slice(&frn.to_le_bytes());
        b[16..24].copy_from_slice(&parent.to_le_bytes());
        b[24..32].copy_from_slice(&usn.to_le_bytes());
        b[32..40].copy_from_slice(&1_300_000_000_000_000_000i64.to_le_bytes()); // TimeStamp
        b[40..44].copy_from_slice(&reason.to_le_bytes());
        b[44..48].copy_from_slice(&0u32.to_le_bytes()); // SourceInfo
        b[48..52].copy_from_slice(&0u32.to_le_bytes()); // SecurityId
        b[52..56].copy_from_slice(&attrs.to_le_bytes());
        b[56..58].copy_from_slice(&(name_bytes as u16).to_le_bytes());
        b[58..60].copy_from_slice(&(USN_V2_HEADER_LEN as u16).to_le_bytes());
        for (i, u) in units.into_iter().enumerate() {
            let at = USN_V2_HEADER_LEN + i * 2;
            b[at..at + 2].copy_from_slice(&u.to_le_bytes());
        }
        b
    }

    /// 构造一条 V3 记录的字节（128 位 FRN，本项目跳过）。
    fn v3_record(usn: i64, frn: u128, parent: u128, reason: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_bytes = units.len() * 2;
        let header_len = 76usize; // 60 - 16 + 32（两个 128 位引用）
        let record_len = (header_len + name_bytes).next_multiple_of(8);
        let mut b = vec![0u8; record_len];
        b[0..4].copy_from_slice(&(record_len as u32).to_le_bytes());
        b[4..6].copy_from_slice(&3u16.to_le_bytes());
        b[6..8].copy_from_slice(&0u16.to_le_bytes());
        b[8..24].copy_from_slice(&frn.to_le_bytes());
        b[24..40].copy_from_slice(&parent.to_le_bytes());
        b[40..48].copy_from_slice(&usn.to_le_bytes());
        b[48..56].copy_from_slice(&0i64.to_le_bytes());
        b[56..60].copy_from_slice(&reason.to_le_bytes());
        b[60..64].copy_from_slice(&0u32.to_le_bytes()); // SourceInfo
        b[64..68].copy_from_slice(&0u32.to_le_bytes()); // SecurityId
        b[68..72].copy_from_slice(&0u32.to_le_bytes()); // FileAttributes
        b[72..74].copy_from_slice(&(name_bytes as u16).to_le_bytes());
        b[74..76].copy_from_slice(&(header_len as u16).to_le_bytes());
        for (i, u) in units.into_iter().enumerate() {
            let at = header_len + i * 2;
            b[at..at + 2].copy_from_slice(&u.to_le_bytes());
        }
        b
    }

    /// 包成 FSCTL_READ_USN_JOURNAL 输出：next USN 前缀 + 记录序列。
    fn journal_buffer(next_usn: i64, records: &[Vec<u8>]) -> Vec<u8> {
        let mut b = next_usn.to_le_bytes().to_vec();
        for r in records {
            b.extend_from_slice(r);
        }
        b
    }

    #[test]
    fn mask_frn_clears_sequence_number() {
        assert_eq!(mask_frn(0x0000_0000_0000_0005), 5);
        assert_eq!(mask_frn(0x0001_0000_0000_0005), 5);
        assert_eq!(mask_frn(0xFFFF_FFFF_FFFF_FFFF), FRN_MASK);
        assert_eq!(mask_frn(0), 0);
        // 序列号只影响高 16 位
        assert_eq!(mask_frn(0x0002_DEAD_BEEF_CAFE), 0x0000_DEAD_BEEF_CAFE);
    }

    #[test]
    fn parse_v2_records_and_next_usn() {
        let buf = journal_buffer(
            777,
            &[
                v2_record(
                    100,
                    0x0001_0000_0000_0028,
                    5,
                    reason::FILE_CREATE | reason::CLOSE,
                    0x20,
                    "新建 文件.txt",
                ),
                v2_record(
                    200,
                    0x0000_0000_0000_0100,
                    0x0001_0000_0000_0028,
                    reason::DATA_EXTEND | reason::CLOSE,
                    0,
                    "ntdll.dll",
                ),
            ],
        );
        let parsed = parse_usn_journal_buffer(&buf);
        assert_eq!(parsed.stopped_at, None);
        assert_eq!(parsed.next_usn, 777);
        assert_eq!(parsed.records.len(), 2);
        assert_eq!(parsed.v3_skipped, 0);

        let first = &parsed.records[0];
        assert_eq!(first.usn, 100);
        assert_eq!(first.file_frn, 0x0001_0000_0000_0028);
        assert_eq!(first.parent_frn, 5);
        assert_eq!(first.reason, reason::FILE_CREATE | reason::CLOSE);
        assert_eq!(first.file_attributes, 0x20);
        assert_eq!(first.name, "新建 文件.txt");
        assert_eq!(first.major_version, 2);

        assert_eq!(parsed.records[1].name, "ntdll.dll");
        assert_eq!(parsed.records[1].file_frn, 0x0000_0000_0000_0100);
    }

    #[test]
    fn parse_stops_at_corrupt_record_and_keeps_prefix() {
        let mut bad = v2_record(300, 42, 5, reason::CLOSE, 0, "ok");
        // 把第二条记录的 RecordLength 改成非 8 倍数（损坏）
        let mut buf = journal_buffer(999, &[v2_record(100, 10, 5, reason::CLOSE, 0, "good")]);
        bad[0] = 7; // RecordLength = 7
        buf.extend_from_slice(&bad);
        buf.extend_from_slice(&v2_record(400, 11, 5, reason::CLOSE, 0, "after"));

        let parsed = parse_usn_journal_buffer(&buf);
        assert_eq!(parsed.records.len(), 1);
        assert_eq!(parsed.records[0].name, "good");
        assert_eq!(parsed.next_usn, 999);
        let stopped = parsed.stopped_at.expect("应报告损坏");
        assert!(stopped.contains("RecordLength=7"), "{stopped}");
    }

    #[test]
    fn parse_skips_v3_records_without_crashing() {
        let buf = journal_buffer(
            42,
            &[
                v2_record(100, 10, 5, reason::FILE_CREATE | reason::CLOSE, 0, "a.txt"),
                v3_record(
                    150,
                    0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10u128,
                    5,
                    reason::FILE_CREATE | reason::CLOSE,
                    "refs-only",
                ),
                v2_record(200, 11, 5, reason::FILE_DELETE | reason::CLOSE, 0, "b.txt"),
            ],
        );
        let parsed = parse_usn_journal_buffer(&buf);
        assert_eq!(parsed.stopped_at, None);
        assert_eq!(parsed.records.len(), 2);
        assert_eq!(parsed.v3_skipped, 1);
        assert_eq!(parsed.records[0].name, "a.txt");
        assert_eq!(parsed.records[1].name, "b.txt");
    }

    #[test]
    fn parse_rejects_truncated_buffer() {
        let parsed = parse_usn_journal_buffer(&[1, 2, 3, 4]);
        assert!(parsed.records.is_empty());
        assert!(parsed.stopped_at.is_some());

        // 只有 next USN、没有记录：干净结束
        let parsed = parse_usn_journal_buffer(&42i64.to_le_bytes());
        assert_eq!(parsed.stopped_at, None);
        assert_eq!(parsed.next_usn, 42);
    }

    #[test]
    fn merge_create_close_is_upsert() {
        let records = [
            ParsedUsnRecord {
                usn: 1,
                file_frn: 0x0001_0000_0000_0064,
                parent_frn: 5,
                reason: reason::FILE_CREATE,
                file_attributes: 0x20,
                timestamp: 0,
                name: "new.txt".into(),
                major_version: 2,
            },
            ParsedUsnRecord {
                usn: 2,
                file_frn: 0x0001_0000_0000_0064,
                parent_frn: 5,
                reason: reason::FILE_CREATE | reason::CLOSE,
                file_attributes: 0x20,
                timestamp: 0,
                name: "new.txt".into(),
                major_version: 2,
            },
        ];
        let events = merge_usn_events(&records);
        assert_eq!(events.len(), 1, "非 CLOSE 的中间记录不产出事件");
        assert_eq!(
            events[0],
            UsnEvent::Upsert {
                frn: 0x64,
                parent_frn: 5,
                name: Some("new.txt".into()),
                is_dir: false,
                attrs: 0x20,
            }
        );
    }

    #[test]
    fn merge_delete_wins_over_create_in_same_record() {
        // 临时文件：创建→写→删经一个句柄，合成一条记录。先判 DELETE。
        let records = [ParsedUsnRecord {
            usn: 1,
            file_frn: 0x0003_0000_0000_00C8,
            parent_frn: 5,
            reason: reason::FILE_CREATE | reason::DATA_EXTEND | reason::FILE_DELETE | reason::CLOSE,
            file_attributes: 0x20,
            timestamp: 0,
            name: "temp.tmp".into(),
            major_version: 2,
        }];
        assert_eq!(
            merge_usn_events(&records),
            vec![UsnEvent::Remove { frn: 0xC8 }]
        );
    }

    #[test]
    fn merge_rename_uses_new_name_record_only() {
        let records = [
            ParsedUsnRecord {
                usn: 1,
                file_frn: 0x200,
                parent_frn: 5,
                reason: reason::RENAME_OLD_NAME | reason::CLOSE,
                file_attributes: 0x20,
                timestamp: 0,
                name: "old.txt".into(),
                major_version: 2,
            },
            ParsedUsnRecord {
                usn: 2,
                file_frn: 0x200,
                parent_frn: 0x100, // 移到了新目录
                reason: reason::RENAME_NEW_NAME | reason::CLOSE,
                file_attributes: 0x20,
                timestamp: 0,
                name: "new.txt".into(),
                major_version: 2,
            },
        ];
        assert_eq!(
            merge_usn_events(&records),
            vec![UsnEvent::Upsert {
                frn: 0x200,
                parent_frn: 0x100,
                name: Some("new.txt".into()),
                is_dir: false,
                attrs: 0x20,
            }]
        );
    }

    #[test]
    fn merge_rename_old_name_alone_produces_nothing() {
        let records = [ParsedUsnRecord {
            usn: 1,
            file_frn: 7,
            parent_frn: 5,
            reason: reason::RENAME_OLD_NAME | reason::CLOSE,
            file_attributes: 0,
            timestamp: 0,
            name: "old.txt".into(),
            major_version: 2,
        }];
        assert!(merge_usn_events(&records).is_empty());
    }

    #[test]
    fn merge_data_change_close_is_upsert_without_name() {
        let records = [ParsedUsnRecord {
            usn: 1,
            file_frn: 9,
            parent_frn: 5,
            reason: reason::DATA_OVERWRITE | reason::BASIC_INFO_CHANGE | reason::CLOSE,
            file_attributes: 0x20,
            timestamp: 0,
            name: String::new(), // 非改名事件不带名字
            major_version: 2,
        }];
        assert_eq!(
            merge_usn_events(&records),
            vec![UsnEvent::Upsert {
                frn: 9,
                parent_frn: 5,
                name: None,
                is_dir: false,
                attrs: 0x20,
            }]
        );
    }

    #[test]
    fn merge_directory_attribute_sets_is_dir() {
        let records = [ParsedUsnRecord {
            usn: 1,
            file_frn: 0x1234,
            parent_frn: 5,
            reason: reason::FILE_CREATE | reason::CLOSE,
            file_attributes: FILE_ATTRIBUTE_DIRECTORY,
            timestamp: 0,
            name: "新目录".into(),
            major_version: 2,
        }];
        match &merge_usn_events(&records)[0] {
            UsnEvent::Upsert { is_dir: true, .. } => {}
            other => panic!("期望目录 Upsert，得到 {other:?}"),
        }
    }

    #[test]
    fn merge_close_without_known_reason_is_ignored() {
        let records = [ParsedUsnRecord {
            usn: 1,
            file_frn: 1,
            parent_frn: 5,
            reason: reason::CLOSE,
            file_attributes: 0,
            timestamp: 0,
            name: "x".into(),
            major_version: 2,
        }];
        assert!(merge_usn_events(&records).is_empty());
    }

    #[test]
    fn record_to_change_upsert_builds_raw_entry() {
        // 合成一条 V2 记录：目录、FILE_CREATE|CLOSE、带文件名。
        let buf = journal_buffer(
            0,
            &[v2_record(
                100,
                0x0001_0000_0000_0028,
                5,
                reason::FILE_CREATE | reason::CLOSE,
                FILE_ATTRIBUTE_DIRECTORY,
                "目录",
            )],
        );
        let parsed = parse_usn_journal_buffer(&buf);
        assert_eq!(parsed.stopped_at, None);
        assert_eq!(parsed.records.len(), 1);

        let changes = records_to_changes(&parsed.records);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            Change::Upsert(e) => {
                // frn 取低 48 位：高 16 位序列号被屏蔽。
                assert_eq!(e.frn, 0x28);
                assert_eq!(e.parent_frn, 5);
                assert_eq!(e.name, "目录");
                assert!(e.is_dir, "FILE_ATTRIBUTE_DIRECTORY 应判为目录");
                assert_eq!(e.attrs, FILE_ATTRIBUTE_DIRECTORY);
                assert_eq!(e.size, 0, "USN 记录不含 size");
                // 三时间戳统一用记录的 TimeStamp 字段。
                assert_eq!(e.mtime, 1_300_000_000_000_000_000);
                assert_eq!(e.ctime, 1_300_000_000_000_000_000);
                assert_eq!(e.atime, 1_300_000_000_000_000_000);
            }
            other => panic!("期望 Upsert，得到 {other:?}"),
        }
    }

    #[test]
    fn record_to_change_delete_is_remove() {
        let buf = journal_buffer(
            0,
            &[v2_record(
                100,
                0x0000_0000_0000_00C8,
                5,
                reason::FILE_DELETE | reason::CLOSE,
                0,
                "gone.txt",
            )],
        );
        let parsed = parse_usn_journal_buffer(&buf);
        let changes = records_to_changes(&parsed.records);
        assert_eq!(changes, vec![Change::Remove { frn: 0xC8 }]);
    }

    #[test]
    fn record_to_change_ignores_non_close_intermediate() {
        // 只有 FILE_CREATE 没有 CLOSE 的中间记录不应产出事件。
        let buf = journal_buffer(
            0,
            &[v2_record(100, 9, 5, reason::FILE_CREATE, 0x20, "x.txt")],
        );
        let parsed = parse_usn_journal_buffer(&buf);
        assert!(records_to_changes(&parsed.records).is_empty());
    }

    #[test]
    fn frn_mask_low_48_bits_matches_ff_core() {
        // 与 ff-core 一致：高 16 位序列号被屏蔽。
        assert_eq!(mask_frn(0x0001_0000_DEAD_BEEF), 0x0000_0000_DEAD_BEEF);
        // 0x0001_0000_DEAD_BEEF 低 48 位 == 0x0000_0000_DEAD_BEEF
        assert_eq!(mask_frn(0x0001_0000_DEAD_BEEF), 0xDEAD_BEEF);
    }

    #[test]
    fn parse_then_merge_end_to_end() {
        let buf = journal_buffer(
            1000,
            &[
                v2_record(
                    100,
                    0x0002_0000_0000_0064,
                    5,
                    reason::FILE_CREATE | reason::CLOSE,
                    0x20,
                    "报告.docx",
                ),
                v2_record(
                    200,
                    0x0002_0000_0000_0064,
                    0x50,
                    reason::RENAME_NEW_NAME | reason::CLOSE,
                    0x20,
                    "季度报告.docx",
                ),
                v2_record(
                    300,
                    0x0000_0000_0000_00C8,
                    5,
                    reason::FILE_DELETE | reason::CLOSE,
                    0x20,
                    "gone.txt",
                ),
            ],
        );
        let parsed = parse_usn_journal_buffer(&buf);
        assert_eq!(parsed.stopped_at, None);
        assert_eq!(
            merge_usn_events(&parsed.records),
            vec![
                UsnEvent::Upsert {
                    frn: 0x64,
                    parent_frn: 5,
                    name: Some("报告.docx".into()),
                    is_dir: false,
                    attrs: 0x20,
                },
                UsnEvent::Upsert {
                    frn: 0x64,
                    parent_frn: 0x50,
                    name: Some("季度报告.docx".into()),
                    is_dir: false,
                    attrs: 0x20,
                },
                UsnEvent::Remove { frn: 0xC8 },
            ]
        );
    }
}
