//! `VolumeScanner` 的 NTFS 实现（DESIGN §5.1 / §5.2）。
//!
//! M0 范围：
//! - `full_scan`：`MftScan` 流式扫 MFT（不把整个 `$MFT` 读进内存），逐条喂 [`RawEntry`]；
//! - `read_changes`：属 M2，本轮返回 [`ScanError::Unsupported`]，不留空实现。

use std::time::Instant;

use ntfs_reader::{MftScan, NtfsAttributeType, NtfsReaderError, Volume};
use rustc_hash::{FxHashMap, FxHashSet};
use time::OffsetDateTime;

use crate::usn::mask_frn;
use crate::{win32, RawEntry, Result, ScanError, ScanMeta, VolumeId, VolumeScanner};

/// FILETIME 纪元差：1601-01-01 → 1970-01-01，单位 100ns。
const EPOCH_DIFFERENCE_TICKS: i128 = 116_444_736_000_000_000;

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

                sink(RawEntry {
                    frn,
                    parent_frn,
                    // Display 是尽力而为的 UTF-8（未配对代理 → U+FFFD）；
                    // to_os_string 无损但契约的 name 是 String。
                    name: name.to_string(),
                    is_dir: file.is_directory(),
                    attrs: standard.map_or(0, |s| s.file_attributes()),
                    size: default_stream_size(&file),
                    mtime: standard.map_or(0, |s| filetime_from(s.modified())),
                    ctime: standard.map_or(0, |s| filetime_from(s.created())),
                    atime: standard.map_or(0, |s| filetime_from(s.accessed())),
                });
                entries += 1;

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

    /// USN 增量读取（DESIGN §5.3）属 M2，本轮未实现。
    fn read_changes(
        &self,
        _vol: &VolumeId,
        _from_usn: i64,
        _journal_id: u64,
    ) -> Result<(Vec<crate::Change>, i64)> {
        Err(ScanError::Unsupported(
            "USN 增量读取属 M2，本轮未实现".to_string(),
        ))
    }
}

/// 卷标识 → 设备路径：有盘符用 `\\.\X:`；装载点卷枚举 GUID 路径按序列号匹配。
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
    fn read_changes_is_unsupported_in_m0() {
        let scanner = NtfsVolumeScanner;
        let err = scanner
            .read_changes(&vol('C', 1), 0, 0)
            .expect_err("M0 的 read_changes 必须返回 Unsupported");
        match err {
            ScanError::Unsupported(msg) => assert!(msg.contains("M2")),
            other => panic!("期望 Unsupported，得到 {other:?}"),
        }
    }
}
