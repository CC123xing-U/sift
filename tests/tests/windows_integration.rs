//! windows_integration —— 需要管理员权限与真实 NTFS 卷的集成测试（DESIGN §14）。
//!
//! 本文件所有测试默认 `#[ignore]`（涉及真实卷/权限/环境），手动运行：
//! `cargo test -p sift-tests -- --ignored`。
//!
//! 不实现 M2 的 USN 逻辑（DESIGN §13-M2 才是增量读写）。
//!
//! 依赖 Track A 的 `ff-ntfs` 公开 API。A 尚未提交卷枚举 API 时，
//! 相关测试以「最简可编译形式 + TODO(a-ntfs) 注释」占位，A 提交后适配。

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
        // M1 契约：本轮允许且应当返回 Unsupported（DESIGN §5.1 注释）。
        Err(ScanError::Unsupported("M1 未实现".to_string()))
    }
}

/// `VolumeScanner` 契约：`full_scan` 流式喂条目；`read_changes` 在 M1 返回 Unsupported。
/// 不依赖真实卷，随常规 `cargo test` 运行（与 B 的单元测试互补，不重复）。
#[test]
fn scanner_contract_full_scan_streams_and_read_changes_unsupported() {
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
        Err(ScanError::Unsupported(_)) => {} // M1 预期行为
        other => panic!("M1 的 read_changes 应返回 Unsupported，得到: {other:?}"),
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
/// TODO(a-ntfs)：Track A 提交卷枚举公开 API（预计 `ff_ntfs::list_ntfs_volumes()`
/// 或类似签名）后，将本测试切换为真实调用并断言：
/// - 返回值非空（至少 C:）；
/// - 每个 `VolumeId` 的 `Display` 含序列号；
/// - 对第一个卷调用 `full_scan`，条目数 > 0 且耗时 < 旧版基线 69s 的两倍（宽松）。
///
/// 当前 A 尚未提交该 API，本测试以最简形式保持编译通过。
#[test]
#[ignore = "需要管理员权限与真实 NTFS 卷（DESIGN §14）；卷枚举 API 待 Track A 提交后启用"]
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
