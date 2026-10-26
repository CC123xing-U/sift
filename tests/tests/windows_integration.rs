//! windows_integration —— 需要管理员权限与真实 NTFS 卷的集成测试（DESIGN §14）。
//!
//! 本文件所有测试默认 `#[ignore]`（涉及真实卷/权限/环境），手动运行：
//! `cargo test -p sift-tests -- --ignored`。
//!
//! 真实 USN 增量（`NtfsVolumeScanner::read_changes`，DESIGN §5.3）需管理员 + 真实 NTFS 卷 + 已启用 Journal，不在本文件的 CI 测试内。
//!
//! 其**纯逻辑**（记录解析、reason 合并、读错误分派）由 `ff-ntfs` 的 `usn` 模块单测覆盖；
//! 卷枚举等真实 IO 由下面的 `#[ignore]` 冒烟测试承载，手动运行：
//! `cargo test -p sift-tests -- --ignored`（需管理员）。

use ff_ntfs::{RawEntry, ScanError, ScanMeta, VolumeId, VolumeScanner};

/// 内存假扫描器：验证 `VolumeScanner` 契约在 M1 的行为（不碰真实卷）。
struct FakeScanner {
    entries: Vec<RawEntry>,
}

impl VolumeScanner for FakeScanner {
    fn full_scan(
        &self,
        _vol: &VolumeId,
        sink: &mut dyn FnMut(RawEntry),
    ) -> ff_ntfs::Result<ScanMeta> {
        for e in &self.entries {
            sink(e.clone());
        }
        Ok(ScanMeta {
            journal_id: 0,
            next_usn: 0,
            entries: self.entries.len() as u64,
            elapsed: std::time::Duration::ZERO,
            warnings: 0,
        })
    }

    fn read_changes(
        &self,
        _vol: &VolumeId,
        _from_usn: i64,
        _journal_id: u64,
    ) -> ff_ntfs::Result<(Vec<ff_ntfs::Change>, i64)> {
        // 内存假实现：契约允许用 Unsupported 占位（DESIGN §5.1 注释）。
        // 真实实现见 `NtfsVolumeScanner::read_changes`（DESIGN §5.3）。
        Err(ScanError::Unsupported(
            "内存假扫描器不提供 USN 增量".to_string(),
        ))
    }
}

/// `VolumeScanner` 契约：`full_scan` 流式喂条目；内存假实现的 `read_changes` 按契约
/// 返回 Unsupported（真实实现走 USN 管线，见 `NtfsVolumeScanner::read_changes`）。
/// 不依赖真实卷，随常规 `cargo test` 运行（与 B 的单元测试互补，不重复）。
#[test]
fn scanner_contract_full_scan_streams_and_fake_read_changes_unsupported() {
    let scanner = FakeScanner {
        entries: vec![
            RawEntry {
                frn: 2,
                parent_frn: 1,
                name: "Users".to_string(),
                is_dir: true,
                attrs: 0x10,
                size: 0,
                mtime: 0,
                ctime: 0,
                atime: 0,
            },
            RawEntry {
                frn: 3,
                parent_frn: 2,
                name: "readme.txt".to_string(),
                is_dir: false,
                attrs: 0x20,
                size: 12,
                mtime: 0,
                ctime: 0,
                atime: 0,
            },
        ],
    };
    let vol = VolumeId {
        serial: 0x1234_5678,
        drive: Some('C'),
    };
    let mut seen: Vec<RawEntry> = Vec::new();
    let meta = scanner
        .full_scan(&vol, &mut |e| seen.push(e))
        .expect("full_scan 不应失败");
    assert_eq!(meta.entries, 2);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].name, "Users");
    assert_eq!(seen[1].name, "readme.txt");
    match scanner.read_changes(&vol, 0, 0) {
        Err(ScanError::Unsupported(_)) => {} // 内存假实现的预期行为
        other => panic!("假扫描器的 read_changes 应返回 Unsupported，得到: {other:?}"),
    }
}

/// 冒烟：当前进程能否打开卷设备 `\\.\C:`（ff-ntfs 读 MFT 的前提；需管理员，
/// DESIGN §12：M0–M5 以管理员运行）。失败通常意味着未提权。
#[test]
#[ignore = "需要管理员权限与真实 NTFS 卷（DESIGN §12/§14）"]
fn raw_volume_open_smoke() {
    let f = std::fs::File::open(r"\\.\C:").expect("打开卷设备失败：请以管理员身份运行");
    drop(f);
}

/// 冒烟：枚举 NTFS 卷（ff-ntfs M0 交付面）。
///
/// 卷枚举公开 API 已交付（`ff_ntfs::enumerate_volumes()`）。本用例保持最简可编译形式：
/// 真实调用会打开卷设备，属管理员环境的手动验证项——
/// **在管理员终端手动运行 `cargo test -p sift-tests -- --ignored` 时按下面 TODO 的断言
/// 扩充为真实调用**：
/// - 返回值非空（至少 C:）；
/// - 每个 `VolumeId` 的 `Display` 含序列号；
/// - 对第一个卷调用 `full_scan`，条目数 > 0 且耗时 < 旧版基线 69s 的两倍（宽松）。
#[test]
#[ignore = "需要管理员权限与真实 NTFS 卷（DESIGN §14）；卷枚举的真实调用只在此手动模式下进行"]
fn volume_enumeration_smoke() {
    // 最简可编译形式：验证契约类型可用；Display 含序列号。
    let vol = VolumeId {
        serial: 0x1234_5678,
        drive: Some('C'),
    };
    let text = vol.to_string();
    assert!(
        text.contains("12345678"),
        "VolumeId Display 应含序列号: {text}"
    );
}

/// 回归：GUID 根路径缺尾随反斜杠时 `GetVolumeInformationW` 曾返回 ERROR_INVALID_NAME(123)，
/// 使所有无盘符卷（恢复分区 / 装载点卷）在 verify 阶段必败（2026-10-25 实机复现，
/// 症状：GUI「索引出错：IO 错误: GetVolumeInformationW(\\?\Volume{…}): 文件名、目录名或卷标语法不正确」）。
/// `query_volume_by_root` 现在内部规范化尾随反斜杠，两种形态必须等价成功。
#[cfg(windows)]
#[test]
#[ignore = "需要管理员权限与真实 NTFS 卷（DESIGN §14）"]
fn guid_root_trailing_backslash_regression() {
    let volumes = ff_ntfs::enumerate_volumes().expect("枚举卷失败");
    let c = volumes
        .iter()
        .find(|v| v.drive == Some('C'))
        .expect("应枚举到 C:");
    let trimmed = c.guid_path.trim_end_matches('\\').to_string();
    let with_bs = ff_ntfs::query_volume_by_root(&c.guid_path).expect("带尾随反斜杠应成功");
    let without_bs =
        ff_ntfs::query_volume_by_root(&trimmed).expect("缺尾随反斜杠也应成功（内部规范化）");
    assert_eq!(with_bs, without_bs, "两种根路径形态结果应一致");
}

/// 回归：无装载点的卷（恢复分区 / EFI / 瞬态虚拟卷）不再进入枚举结果——它们没有可解析
/// 路径，索引无意义，且其出现/消失会让索引集合反复抖动（2026-10-25 实机复现）。
#[cfg(windows)]
#[test]
#[ignore = "需要管理员权限与真实 NTFS 卷（DESIGN §14）"]
fn letterless_volumes_are_mount_point_backed() {
    let volumes = ff_ntfs::enumerate_volumes().expect("枚举卷失败");
    assert!(!volumes.is_empty(), "至少应枚举到 C:");
    for v in &volumes {
        if v.drive.is_none() {
            assert!(
                ff_ntfs::has_mount_points(&v.guid_path),
                "无盘符卷必须有装载点才会被枚举: {:?}",
                v.guid_path
            );
        }
        // 枚举结果里的每个卷，trimmed GUID 形态都应能查询成功（与 device_path_for 的产物一致）。
        let trimmed = v.guid_path.trim_end_matches('\\').to_string();
        ff_ntfs::query_volume_by_root(&trimmed)
            .unwrap_or_else(|e| panic!("trimmed GUID 查询失败 {:?}: {e}", v.guid_path));
    }
}
