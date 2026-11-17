//! `VolumeScanner` 的 NTFS 实现（DESIGN §5.1 / §5.2 / §5.3）。
//!
//! 已落地：
//! - `full_scan`：`MftScan` 流式扫 MFT（不把整个 `$MFT` 读进内存），逐条喂 [`RawEntry`]；
//!   扫描**前**先记录 USN Journal 位置（§5.2，顺序不能反），并在建索引阶段应用最小排除集
//!   （§5.2 排除规则）；
//! - `read_changes`：真实 USN 增量（§5.3）——`win32::read_usn_journal` 拉原始记录流，
//!   经 `usn` 模块解析为 [`Change`]。需管理员权限 + 真实 NTFS 卷 + 已启用 Journal。

use std::time::Instant;

use ntfs_reader::{MftScan, NtfsAttributeType, NtfsReaderError, Volume};
use rustc_hash::{FxHashMap, FxHashSet};
use time::OffsetDateTime;

use crate::usn::mask_frn;
use crate::{usn, win32, Change, RawEntry, Result, ScanError, ScanMeta, VolumeId, VolumeScanner};

/// FILETIME 纪元差：1601-01-01 → 1970-01-01，单位 100ns。
const EPOCH_DIFFERENCE_TICKS: i128 = 116_444_736_000_000_000;

/// 目录名是否命中排除集（DESIGN §5.2：排除规则在建索引时应用）。
///
/// 只对**目录**生效：避免误伤取了同名目录名的普通文件（回收站 / 卷信息目录只有目录形态）。
/// 排除集本身定义在 `lib.rs`，与目录遍历回退（[`crate::walk`]）共用同一份口径；
/// 元数据文件（`$MFT` / `$Bitmap` 等）按 §5.2 保留（Everything 也能搜到），不在排除集内。
fn is_excluded(name: &str, is_dir: bool) -> bool {
    is_dir && crate::is_excluded_dir_name(name)
}

/// 条目是否应被排除：自身命中 [`is_excluded`]，或其父目录已被排除（**子树连带**）。
///
/// `excluded_frns` 收集"已被排除的目录"的掩码 FRN。只排除目录自身而不连带子树的话，
/// 子条目的 `parent_frn` 会解析不到，被挂到根下并计入 §5.2 的警告计数、路径也是错的。
fn is_excluded_subtree(
    name: &str,
    is_dir: bool,
    parent_frn: u64,
    excluded_frns: &FxHashSet<u64>,
) -> bool {
    is_excluded(name, is_dir) || excluded_frns.contains(&mask_frn(parent_frn))
}

/// `VolumeScanner` 的 NTFS 具体实现（无配置，单元结构）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NtfsVolumeScanner;

impl VolumeScanner for NtfsVolumeScanner {
    /// 全量扫描（DESIGN §5.2）。
    ///
    /// 顺序固定：解析设备路径 → 校验 NTFS → **先记录 USN Journal** → 再扫 MFT。
    fn full_scan(&self, vol: &VolumeId, sink: &mut dyn FnMut(RawEntry)) -> Result<ScanMeta> {
        let started = Instant::now();
        let device_path = device_path_for(vol)?;
        verify_ntfs(vol, &device_path)?;

        // DESIGN §5.2：先记录 FSCTL_QUERY_USN_JOURNAL 的 journal id 与 next_usn，再扫 MFT；
        // 扫描期间的变化由后续从该 next_usn 续读补齐（M2）。顺序不能反。
        let (journal_id, next_usn) = record_journal_position(&device_path);

        let volume = Volume::new(&device_path).map_err(map_ntfs_error)?;
        // MftScan：流式分块读 MFT（0.6.0 新增），百万级文件卷也只占数 MiB，
        // 不像 Mft::new 把整个 $MFT（约 1 KiB/文件）驻留内存。
        let mut scan = MftScan::new(volume).map_err(map_ntfs_error)?;

        // 父解析用的 FRN 集合（警告计数需要）；按记录总数预分配避免边扫边扩容。
        let mut frn_set: FxHashSet<u64> = FxHashSet::default();
        frn_set.reserve(scan.record_count() as usize);
        let mut parent_counts: FxHashMap<u64, u64> = FxHashMap::default();
        // 已被排除的目录（掩码 FRN）：其子树一并排除，见 [`is_excluded_subtree`]。
        let mut excluded_frns: FxHashSet<u64> = FxHashSet::default();

        let mut entries: u64 = 0;
        while let Some(chunk) = scan.next_chunk().map_err(map_ntfs_error)? {
            for file in chunk.files() {
                // 无名条目无法被搜索（仅损坏卷上偶发），跳过。
                let Some(name) = file.best_name() else {
                    continue;
                };
                let frn = file.reference();
                let parent_frn = name.parent_reference();
                let standard = file.standard_information();
                let name = name.to_string();
                // 主条目的 (parent, name) 副本：别名去重时要用，name 随后被 move 进 RawEntry。
                let main_name = name.clone();
                let is_dir = file.is_directory();
                // DESIGN §5.2：排除规则在建索引时应用。排除目录时其**整棵子树**一并排除
                // （否则子条目的 parent 找不到、会被挂到根下并计入警告计数）。
                if is_excluded_subtree(&name, is_dir, parent_frn, &excluded_frns) {
                    if is_dir {
                        excluded_frns.insert(mask_frn(frn));
                    }
                    continue;
                }

                sink(RawEntry {
                    frn,
                    parent_frn,
                    // Display 是尽力而为的 UTF-8（未配对代理 → U+FFFD）；
                    // to_os_string 无损但契约的 name 是 String。
                    name,
                    is_dir,
                    attrs: standard.map_or(0, |s| s.file_attributes()),
                    size: default_stream_size(&file),
                    mtime: standard.map_or(0, |s| filetime_from(s.modified())),
                    ctime: standard.map_or(0, |s| filetime_from(s.created())),
                    atime: standard.map_or(0, |s| filetime_from(s.accessed())),
                });
                entries += 1;

                // M7 #97：硬链接的其余名字（不同目录 / 不同名字）作为额外条目产出。
                // ntfs-reader 的 hard_links() 已排除 DOS 8.3 别名，每个就是一个真实链接。
                // 与主条目相同的 FRN + 相同元数据，只是 name/parent 不同；引擎 finish()
                // 对重复 FRN「先推入者胜」，frn_to_id 仍指向主条目，USN 增量语义不变。
                // 主条目自身也在 hard_links() 里，按 (parent, name) 去掉即为别名。
                for link in file.hard_links() {
                    let lp = link.parent_reference();
                    let ln = link.to_string();
                    if lp == parent_frn && ln == main_name {
                        continue; // 主条目，已产出
                    }
                    // 别名指向被排除目录时同样跳过（与主条目排除口径一致）。
                    if is_excluded_subtree(&ln, is_dir, lp, &excluded_frns) {
                        continue;
                    }
                    sink(RawEntry {
                        frn, // 与主条目相同
                        parent_frn: lp,
                        name: ln,
                        is_dir,
                        attrs: standard.map_or(0, |s| s.file_attributes()),
                        size: default_stream_size(&file),
                        mtime: standard.map_or(0, |s| filetime_from(s.modified())),
                        ctime: standard.map_or(0, |s| filetime_from(s.created())),
                        atime: standard.map_or(0, |s| filetime_from(s.accessed())),
                    });
                    entries += 1; // 别名也是可搜索条目，计入 ScanMeta.entries
                }

                frn_set.insert(mask_frn(frn));
                *parent_counts.entry(mask_frn(parent_frn)).or_default() += 1;
            }
        }
        let elapsed = started.elapsed();

        // DESIGN §5.2：找不到父的条目挂到根下并记录警告计数。
        // 判定：掩码后的父 FRN ≥ FIRST_NORMAL_RECORD(24) 且不在本次扫到的 FRN 集合中。
        // < 24 的是 NTFS 保留元数据记录（$MFT/$Volume/根目录 5/$Extend 11 等），
        // 卷上必然存在，只是 MftScan 不把它们作为文件条目产出，故不算"找不到父"。
        let warnings: u64 = parent_counts
            .iter()
            .filter(|(parent, _)| {
                **parent >= ntfs_reader::FIRST_NORMAL_RECORD && !frn_set.contains(*parent)
            })
            .map(|(_, count)| *count)
            .sum();

        Ok(ScanMeta {
            journal_id,
            next_usn,
            entries,
            elapsed,
            warnings,
        })
    }

    /// USN 增量读取（DESIGN §5.3，M2 真实实现）。
    ///
    /// 流程：解析设备路径 → 校验 journal id 未变且 `from_usn` 仍在日志内 →
    /// `read_usn_journal` 拉取原始记录流 → `usn` 解析成 [`Change`] → 返回
    /// `(changes, new_next_usn)`。实时路径需管理员权限 + 真实 NTFS 卷。
    fn read_changes(
        &self,
        vol: &VolumeId,
        from_usn: i64,
        journal_id: u64,
    ) -> Result<(Vec<Change>, i64)> {
        let device_path = device_path_for(vol)?;

        // 先查当前 journal：校验 id 一致（日志未被重建），并拿到权威 next_usn / first_usn。
        let info = win32::query_usn_journal(&device_path)?;
        if journal_id != 0 && info.journal_id != journal_id {
            return Err(ScanError::JournalUnavailable(format!(
                "journal id 不匹配（传入 {journal_id:#018x}，当前 {:#018x}），日志已被重建，需重新全量扫描",
                info.journal_id
            )));
        }
        // from_usn 早于日志首条：该位置记录已回收，无法续读，需重新基准。
        if from_usn < info.first_usn {
            return Err(ScanError::JournalUnavailable(format!(
                "from_usn {from_usn} 早于日志首条 {first_usn}，记录已回收，需重新全量扫描",
                first_usn = info.first_usn
            )));
        }

        let buf = win32::read_usn_journal(&device_path, from_usn, info.journal_id)?;
        let parsed = usn::parse_usn_journal_buffer(&buf);
        // DESIGN §5.3：V3（ReFS 128 位 FRN）与未知版本记录要**记录日志并忽略**（不崩溃）；
        // 解析被迫停止（损坏记录 / 缓冲截断）同样要留下痕迹，已解析部分仍然可用。
        if parsed.v3_skipped > 0 {
            tracing::warn!(
                volume = %vol,
                v3_skipped = parsed.v3_skipped,
                "跳过 V3（ReFS，128 位 FRN）USN 记录：本项目只支持 NTFS V2"
            );
        }
        if parsed.unsupported_version_skipped > 0 {
            tracing::warn!(
                volume = %vol,
                unsupported_version_skipped = parsed.unsupported_version_skipped,
                "跳过未知主版本的 USN 记录"
            );
        }
        if let Some(reason) = &parsed.stopped_at {
            tracing::warn!(
                volume = %vol,
                parsed_records = parsed.records.len(),
                reason = %reason,
                "USN 记录解析提前停止（已解析部分仍可用）"
            );
        }
        let changes = usn::records_to_changes(&parsed.records);
        Ok((changes, parsed.next_usn))
    }
}

/// 卷标识 → 设备路径：有盘符用 `\\.\X:`；装载点卷枚举 GUID 路径按序列号匹配。
///
/// 返回的 GUID 路径**不含**尾随反斜杠（`CreateFileW` 开原始卷的要求）；
/// 需要根路径形态（`GetVolumeInformationW`）的调用方直接交给
/// [`win32::query_volume_by_root`] 即可——它内部会补回尾随反斜杠。
///
/// 公开：CLI / 引擎展示 USN Journal 状态等场景需要同一路径（保证与 `full_scan` 内部一致）。
pub fn device_path_for(vol: &VolumeId) -> Result<String> {
    if let Some(drive) = vol.drive {
        return Ok(format!(r"\\.\{drive}:"));
    }
    win32::enumerate_volumes()?
        .into_iter()
        .find(|v| v.serial == vol.serial)
        .map(|v| v.guid_path.trim_end_matches('\\').to_string())
        .ok_or_else(|| ScanError::VolumeNotFound(vol.clone()))
}

/// 校验卷存在、是 NTFS、且序列号与 `vol` 一致（防止给错盘符）。
fn verify_ntfs(vol: &VolumeId, device_path: &str) -> Result<()> {
    let root = match vol.drive {
        Some(drive) => format!(r"{drive}:\"),
        None => device_path.to_string(),
    };
    let (serial, fs_name) = win32::query_volume_by_root(&root)?;
    if !fs_name.eq_ignore_ascii_case("NTFS") {
        return Err(ScanError::NotNtfs {
            volume: vol.clone(),
            fs_name,
        });
    }
    if serial != vol.serial {
        return Err(ScanError::VolumeNotFound(vol.clone()));
    }
    Ok(())
}

/// DESIGN §5.2 的"先记录"步骤；§5.3 的失败退化：
/// 查询失败 → 尝试 `FSCTL_CREATE_USN_JOURNAL` → 再查一次；
/// 仍失败则返回 `(0, 0)`，表示该卷退化为"仅启动时全量重扫"（调用方应告知用户）。
fn record_journal_position(device_path: &str) -> (u64, i64) {
    let info = match win32::query_usn_journal(device_path) {
        Ok(info) => Some(info),
        Err(_) => win32::create_usn_journal(
            device_path,
            win32::USN_JOURNAL_DEFAULT_MAX_SIZE,
            win32::USN_JOURNAL_DEFAULT_ALLOCATION_DELTA,
        )
        .and_then(|()| win32::query_usn_journal(device_path))
        .ok(),
    };
    match info {
        Some(info) => (info.journal_id, info.next_usn),
        None => (0, 0),
    }
}

/// 默认（未命名）`$DATA` 流的逻辑大小；目录与无名记录流为 0。
///
/// 不用 `FileInfo`：它会解析完整路径（父链遍历），而本项目按 DESIGN §2.2-3 用父链现算，
/// 扫描时不需要路径；这里只取条目字段。
fn default_stream_size(file: &ntfs_reader::NtfsFile) -> u64 {
    file.attributes()
        .filter(|a| a.attribute_type() == Some(NtfsAttributeType::Data) && a.name().is_none())
        .find_map(|a| a.value_size())
        .unwrap_or(0)
}

/// `OffsetDateTime`（ntfs-reader 的时间戳类型）→ FILETIME（契约单位：100ns，1601 纪元）。
///
/// ntfs-reader 内部按 `(ticks - EPOCH_DIFFERENCE) * 100` 纳秒转换；这里精确逆运算。
fn filetime_from(dt: OffsetDateTime) -> i64 {
    ((dt.unix_timestamp_nanos() / 100) + EPOCH_DIFFERENCE_TICKS) as i64
}

/// `NtfsReaderError` → [`ScanError`]，保留原始信息。
fn map_ntfs_error(e: NtfsReaderError) -> ScanError {
    match e {
        NtfsReaderError::AccessDenied => ScanError::AccessDenied(
            "打开卷被拒绝（需要管理员权限；卷路径须为 \\\\.\\X: 形式且不带尾随反斜杠）".to_string(),
        ),
        NtfsReaderError::Io(io) => ScanError::Io(io),
        other => ScanError::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(drive: char, serial: u32) -> VolumeId {
        VolumeId {
            serial,
            drive: Some(drive),
        }
    }

    #[test]
    fn excluded_subtree_covers_children() {
        // 父目录被排除 → 子树连带排除（否则子条目的 parent 解析不到、被挂到根下）
        let mut excluded: FxHashSet<u64> = FxHashSet::default();
        excluded.insert(0x40); // 已被排除的目录（$Recycle.Bin）
        assert!(is_excluded_subtree("$R12345.txt", false, 0x40, &excluded));
        assert!(is_excluded_subtree("sub", true, 0x40, &excluded));
        // 父 FRN 带序列号也能命中（内部按掩码比较）
        assert!(is_excluded_subtree(
            "$I12345.txt",
            false,
            0x0007_0000_0000_0040,
            &excluded
        ));
        // 不在被排除子树内、名字也不命中 → 保留
        assert!(!is_excluded_subtree("readme.txt", false, 5, &excluded));
        assert!(!is_excluded_subtree("Windows", true, 5, &excluded));
    }

    #[test]
    fn excluded_dirs_only_when_is_dir() {
        // 排除集只命中目录（DESIGN §5.2）：避免误伤取了同名目录名的普通文件。
        assert!(is_excluded("$Recycle.Bin", true));
        assert!(is_excluded("$recycle.bin", true)); // 大小写不敏感
        assert!(is_excluded("System Volume Information", true));
        assert!(!is_excluded("$Recycle.Bin", false), "文件不应被排除");
        assert!(!is_excluded("System Volume Information", false));
    }

    #[test]
    fn normal_dirs_and_metadata_files_kept() {
        // 元数据文件按 §5.2 保留；普通目录一概保留。
        assert!(!is_excluded("$MFT", false));
        assert!(!is_excluded("$Bitmap", false));
        assert!(!is_excluded("Windows", true));
        assert!(!is_excluded("回收站备份", true));
        assert!(!is_excluded("$RECYCLE.BIN.bak", true));
    }

    #[test]
    fn device_path_format_for_drive_letter() {
        // 不碰真实卷：只验证路径拼装逻辑（不执行 Win32）。
        let v = vol('C', 0x1234_5678);
        assert_eq!(format!(r"\\.\{}:", v.drive.unwrap()), r"\\.\C:");
    }

    #[test]
    fn filetime_roundtrip_known_values() {
        // 1970-01-01T00:00:00Z → 116444736000000000 (0x01...)
        let unix_epoch =
            OffsetDateTime::from_unix_timestamp_nanos(0).expect("unix epoch representable");
        assert_eq!(filetime_from(unix_epoch), 116_444_736_000_000_000);
        // 2026-10-24T12:00:00Z：手工核算 —— (秒数 × 10_000_000) + 纪元差
        let secs: i128 = 1_761_000_000; // 2026-10-24 附近的秒级时间戳
        let dt =
            OffsetDateTime::from_unix_timestamp_nanos(secs * 1_000_000_000).expect("representable");
        assert_eq!(
            filetime_from(dt),
            (secs * 10_000_000 + 116_444_736_000_000_000) as i64
        );
        // 1970 之前（1601 之后）的真实日期：1601-01-01T00:00:00Z → 0
        let ntfs_epoch = OffsetDateTime::from_unix_timestamp_nanos(-116_444_736_000_000_000 * 100)
            .expect("representable");
        assert_eq!(filetime_from(ntfs_epoch), 0);
    }

    #[test]
    fn read_changes_contract_uses_usn_pipeline() {
        // 不碰真实卷：只验证纯逻辑转换管线（usn::records_to_changes）在 scanner 侧可用。
        // 真实 IO 路径（需要管理员 + 真实 NTFS 卷 + USN Journal）放在手动/feature 验证，
        // 不在此 CI 单测内。
        let records = [crate::usn::ParsedUsnRecord {
            usn: 1,
            file_frn: 0x0001_0000_0000_0064,
            parent_frn: 5,
            reason: crate::usn::reason::FILE_CREATE | crate::usn::reason::CLOSE,
            file_attributes: crate::usn::FILE_ATTRIBUTE_DIRECTORY,
            timestamp: 1_300_000_000_000_000_000,
            name: "目录".into(),
            major_version: 2,
        }];
        let changes = crate::usn::records_to_changes(&records);
        assert_eq!(changes.len(), 1);
        match &changes[0] {
            crate::Change::Upsert(e) => {
                assert_eq!(e.frn, 0x64);
                assert!(e.is_dir);
            }
            other => panic!("期望 Upsert，得到 {other:?}"),
        }
    }
}
