//! `AutoVolumeScanner`：按**文件系统类型**在 NTFS 扫描与目录遍历之间分派
//! （M7，DESIGN §5.4）。
//!
//! 引入动机：`enumerate_volumes` 会把 exFAT / FAT32 的可移动卷也列出来，
//! 而引擎只有一个扫描器入口 `VolumeScanner`。若固定塞 `NtfsVolumeScanner`，
//! 这些卷每次全量扫描都返回 `NotNtfs` 错误，UI 上是一堆 Error 卷、搜索里
//! 一条结果都没有——用户会直接判定工具坏了。分派层让「同一台机器上不同
//! 文件系统的卷各自走能走的路径」这件事对引擎完全透明。
//!
//! 判定顺序（两条独立保险）：
//! 1. 先查 `GetVolumeInformationW` 的 fs_name（快，无副作用）；
//! 2. 名称查不到（无盘符 / 权限 / 瞬时失败）或名称说谎时，仍让 NTFS 扫描器
//!    试一次，只有拿到 `ScanError::NotNtfs` 才回退遍历。
//!    `verify_ntfs` 在任何条目喂进 sink **之前**执行，所以回退不会产生重复条目。

use crate::walk::DirWalkScanner;
use crate::{
    win32, Change, NtfsVolumeScanner, RawEntry, Result, ScanError, ScanMeta, VolumeId,
    VolumeScanner,
};

/// 自动分派扫描器（无配置，单元结构；引擎 / 服务 / CLI 直接 `Box::new` 它）。
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoVolumeScanner;

impl AutoVolumeScanner {
    /// 该卷是不是 NTFS。`None` = 查不出来（无盘符 / 查询失败），交给调用方按
    /// 「先试 NTFS、失败再回退」处理。
    fn is_ntfs(vol: &VolumeId) -> Option<bool> {
        let root = vol.drive.map(|c| format!("{c}:\\"))?;
        let (_, fs_name) = win32::query_volume_by_root(&root).ok()?;
        Some(fs_name.eq_ignore_ascii_case("NTFS"))
    }
}

impl VolumeScanner for AutoVolumeScanner {
    fn full_scan(&self, vol: &VolumeId, sink: &mut dyn FnMut(RawEntry)) -> Result<ScanMeta> {
        if Self::is_ntfs(vol) == Some(false) {
            return DirWalkScanner.full_scan(vol, sink);
        }
        match NtfsVolumeScanner.full_scan(vol, sink) {
            // 名称与实际不符（如 GetVolumeInformationW 被缓存 / 伪装卷）：回退遍历。
            Err(ScanError::NotNtfs { .. }) => DirWalkScanner.full_scan(vol, sink),
            other => other,
        }
    }

    fn read_changes(
        &self,
        vol: &VolumeId,
        from_usn: i64,
        journal_id: u64,
    ) -> Result<(Vec<Change>, i64)> {
        if Self::is_ntfs(vol) == Some(false) {
            return Err(ScanError::Unsupported(format!(
                "卷 {vol} 无 USN Journal（非 NTFS），只能全量遍历"
            )));
        }
        match NtfsVolumeScanner.read_changes(vol, from_usn, journal_id) {
            Err(ScanError::NotNtfs { .. }) => Err(ScanError::Unsupported(format!(
                "卷 {vol} 无 USN Journal（非 NTFS），只能全量遍历"
            ))),
            other => other,
        }
    }

    /// 非 NTFS 卷没有增量通道 → 引擎改走定期全量重走（见 `ff_engine::poll_volume`）。
    fn supports_incremental(&self, vol: &VolumeId) -> bool {
        Self::is_ntfs(vol) != Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无盘符的卷查不出文件系统：按 `None` 处理 → 仍尝试 NTFS 路径
    /// （`supports_incremental` 返回 true），而不是武断判成「非 NTFS」。
    #[test]
    fn unknown_filesystem_defaults_to_ntfs_attempt() {
        let vol = VolumeId {
            serial: 0,
            drive: None,
        };
        assert_eq!(AutoVolumeScanner::is_ntfs(&vol), None);
        assert!(AutoVolumeScanner.supports_incremental(&vol));
    }
}
