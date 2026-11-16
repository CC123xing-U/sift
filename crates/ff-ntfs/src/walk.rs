//! 非 NTFS 卷的目录遍历回退（DESIGN §5.4）。
//!
//! FAT / exFAT / ReFS / 其它无 USN Journal 的卷没有 MFT，也没有增量通道，
//! 只能靠系统 API 递归列目录建索引。DESIGN §5.4 原本写「`walkdir` 或 `ignore`」，
//! 实际**用 `std::fs::read_dir` 手写**：本项目一贯零新增依赖，而这里需要的
//! 只是「递归 + 跳过重解析点 + 复用排除集」，`walkdir` 能提供的额外价值不足以
//! 引入一个新 crate（理由与取舍见 DECISIONS #93）。
//!
//! 三条硬约束（DESIGN §5.4）：
//! 1. **不跟随符号链接 / 目录联接（junction）**：只索引链接自身，不递归进去，
//!    否则会绕环（`C:\link → C:\`）导致无限遍历；
//! 2. **排除集与 NTFS 路径一致**：回收站 / 系统卷信息目录不进索引（§5.2），
//!    共用 `lib.rs` 的 [`crate::EXCLUDED_DIR_NAMES`]；
//! 3. **无增量**：[`VolumeScanner::read_changes`] 明确返回 `Unsupported`，
//!    [`VolumeScanner::supports_incremental`] 返回 `false`，由引擎改走定期全量重走。
//!
//! 因为没有 FRN，条目用**路径哈希**当键（[`path_key`]）：同一路径每次扫描得到同一
//! 个键，索引持久化 / Overlay 语义与 NTFS 卷一致；键与 NTFS 的 FRN 一样经
//! `usn::mask_frn` 取低 48 位使用，两种键不会互相干扰（同一卷只会有一种键）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{is_excluded_dir_name, RawEntry, Result, ScanError, ScanMeta, VolumeId, VolumeScanner};

/// 目录递归深度上限。`ff-core` 的 parent 链上限是 512（DESIGN §4.2），
/// 这里留足余量取 96：真实的非 NTFS 卷（U 盘 / 移动硬盘 / 存储卡）极少超过 30 层，
/// 超过这个深度的树基本是异常数据，继续递归只会拖慢扫描。
const MAX_DEPTH: usize = 96;

/// FILETIME 纪元差：1601-01-01 → 1970-01-01，单位 100ns（与 `scanner.rs` 同值）。
const EPOCH_DIFFERENCE_TICKS: i128 = 116_444_736_000_000_000;

/// FNV-1a 64 位偏移基与质数（键哈希用；不引入额外依赖）。
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 目录遍历扫描器（无配置，单元结构）。
///
/// 与 [`crate::NtfsVolumeScanner`] 同签名实现 [`VolumeScanner`]，因此引擎、
/// 服务、CLI、测试假扫描器都不需要为它改接口。
#[derive(Debug, Clone, Copy, Default)]
pub struct DirWalkScanner;

impl VolumeScanner for DirWalkScanner {
    /// 全量遍历 `vol` 的目录树，流式喂给 `sink`。
    ///
    /// 与 NTFS 的 `full_scan` 对齐：先产出一个**根条目**（`parent_frn = 0`），
    /// 其余条目的 `parent_frn` 指向父目录的路径键。`ff-core` 的路径生成
    /// （`path.rs`）靠「第一个 `parent_frn == 0` 的条目即根」来跳过根名，
    /// 所以根条目必须先喂、且只能有一个。
    fn full_scan(&self, vol: &VolumeId, sink: &mut dyn FnMut(RawEntry)) -> Result<ScanMeta> {
        let started = Instant::now();
        let logical_root = logical_root(vol)?;
        let io_root = io_path(&logical_root);

        let root_meta = fs::metadata(&io_root).map_err(|e| {
            ScanError::Io(std::io::Error::new(
                e.kind(),
                format!("读取卷根 {logical_root} 失败: {e}"),
            ))
        })?;
        if !root_meta.is_dir() {
            return Err(ScanError::Other(format!("卷根不是目录: {logical_root}")));
        }

        let mut stats = WalkStats::default();
        let root_key = path_key(&logical_root);
        // 根条目：名字不参与路径拼接（`path.rs` 会跳过根名），但仍要存在——
        // `VolumeIndex::root_id()` 靠「第一个 `parent_frn == 0` 的条目」认定根，
        // 缺了它就会把某个顶层目录误当成根，那个条目的路径会丢掉自己的名字
        // （打开时就会打开到卷根而不是该目录）。名字取 "."：与 `ff-core` 测试
        // 夹具里的 NTFS 根条目（`dir(5, 0, ".", …)`）一致，用户也不会把它误认成文件。
        sink(RawEntry {
            frn: root_key,
            parent_frn: 0,
            name: ".".to_string(),
            is_dir: true,
            attrs: attrs_of(&root_meta, true),
            size: 0,
            mtime: filetime_from_system_time(root_meta.modified().unwrap_or(UNIX_EPOCH)),
            ctime: filetime_from_system_time(root_meta.created().unwrap_or(UNIX_EPOCH)),
            atime: filetime_from_system_time(root_meta.accessed().unwrap_or(UNIX_EPOCH)),
        });
        stats.entries += 1;

        walk_dir(&io_root, &logical_root, root_key, sink, 1, &mut stats);

        Ok(ScanMeta {
            // 目录遍历卷没有 USN Journal：两个字段保持 0，引擎据
            // `supports_incremental == false` 走定期重走，不会拿 0 去读日志。
            journal_id: 0,
            next_usn: 0,
            entries: stats.entries,
            elapsed: started.elapsed(),
            warnings: stats.warnings,
        })
    }

    /// 目录遍历卷没有增量通道（DESIGN §5.4）。显式返回 `Unsupported`，
    /// 让调用方看到「不是实现漏了，是这类卷本来就没有 USN」。
    fn read_changes(
        &self,
        vol: &VolumeId,
        _from_usn: i64,
        _journal_id: u64,
    ) -> Result<(Vec<crate::Change>, i64)> {
        Err(ScanError::Unsupported(format!(
            "卷 {vol} 无 USN Journal（非 NTFS），只能全量遍历"
        )))
    }

    fn supports_incremental(&self, _vol: &VolumeId) -> bool {
        false
    }
}

/// 遍历统计（条目数 / 警告数）。
#[derive(Default)]
struct WalkStats {
    entries: u64,
    /// 无法读取的目录、名字非 UTF-8、超过深度上限的目录等。
    warnings: u64,
}

/// 递归遍历一个目录（`io_dir` 是带 `\\?\` 前缀的 IO 路径，`logical` 是无前缀的逻辑路径）。
///
/// 子项按名字排序后处理：非 NTFS 卷没有 MFT 顺序，排序让同一棵树的扫描结果
/// 可复现（BENCH / 调试对比用），代价是每个目录多一次小排序。
fn walk_dir(
    io_dir: &Path,
    logical: &str,
    parent_key: u64,
    sink: &mut dyn FnMut(RawEntry),
    depth: usize,
    stats: &mut WalkStats,
) {
    if depth > MAX_DEPTH {
        stats.warnings += 1;
        return;
    }
    let read = match fs::read_dir(io_dir) {
        Ok(r) => r,
        Err(_) => {
            // 权限不足 / 目录已被删除 / 介质拔出：跳过整棵子树，计入警告。
            stats.warnings += 1;
            return;
        }
    };

    let mut children: Vec<(String, PathBuf, bool)> = Vec::new();
    for entry in read {
        let Ok(entry) = entry else {
            stats.warnings += 1;
            continue;
        };
        // `file_type()` 不跟随链接：Windows 上符号链接与目录联接（junction）
        // 都报 `is_symlink()`，正好满足「不跟随重解析点」的要求。
        let Ok(ft) = entry.file_type() else {
            stats.warnings += 1;
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            // 契约的 `name` 是 String；非 UTF-8 名字无法表达，跳过并计数。
            stats.warnings += 1;
            continue;
        };
        if ft.is_dir() && is_excluded_dir_name(&name) {
            continue;
        }
        // 链接自身进索引（用户能看到它），但不递归进去（§5.4 防环）。
        children.push((name, entry.path(), ft.is_symlink()));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, path, is_link) in children {
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => {
                stats.warnings += 1;
                continue;
            }
        };
        let is_dir = meta.is_dir();
        let logical_child = format!("{logical}\\{name}");
        let key = path_key(&logical_child);
        sink(RawEntry {
            frn: key,
            parent_frn: parent_key,
            name,
            is_dir,
            attrs: attrs_of(&meta, is_dir),
            size: if is_dir { 0 } else { meta.len() },
            mtime: filetime_from_system_time(meta.modified().unwrap_or(UNIX_EPOCH)),
            ctime: filetime_from_system_time(meta.created().unwrap_or(UNIX_EPOCH)),
            atime: filetime_from_system_time(meta.accessed().unwrap_or(UNIX_EPOCH)),
        });
        stats.entries += 1;

        if is_dir && !is_link {
            walk_dir(&path, &logical_child, key, sink, depth + 1, stats);
        }
    }
}

/// 卷根逻辑路径（`C:` → `C:\`）。
///
/// 无盘符的卷（仅挂载点的 GUID 卷）拿不到稳定路径：拒绝遍历而不是猜一个。
fn logical_root(vol: &VolumeId) -> Result<String> {
    match vol.drive {
        Some(c) => Ok(format!("{c}:\\")),
        None => Err(ScanError::Unsupported(format!(
            "卷 {vol} 没有盘符，目录遍历需要可解析的路径"
        ))),
    }
}

/// 给 IO 路径加 `\\?\` 前缀，绕开 `MAX_PATH`(260) 限制。
///
/// 只是 IO 用；索引里的逻辑路径（[`path_key`] 的输入）保持无前缀，
/// 否则键会随前缀写法变化而不稳定。
fn io_path(logical_root: &str) -> PathBuf {
    PathBuf::from(format!("\\\\?\\{logical_root}"))
}

/// 路径 → 48 位可用的稳定键（FNV-1a 64，路径按小写折叠）。
///
/// 大小写折叠让 `D:\Music` 与 `d:\music` 得到同一个键（Windows 路径不区分大小写）。
/// 返回 0 的极端情况改为 1：`0` 在索引里保留给「根之父」。
pub(crate) fn path_key(logical: &str) -> u64 {
    let mut h = FNV_OFFSET;
    for b in logical.to_lowercase().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    if h == 0 {
        1
    } else {
        h
    }
}

/// `SystemTime` → FILETIME（100ns，1601 纪元）。契约单位与 NTFS 路径一致。
fn filetime_from_system_time(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_nanos() as i128 / 100 + EPOCH_DIFFERENCE_TICKS) as i64,
        // 早于 1970 的文件时间（罕见但合法）：向前减。
        Err(e) => (EPOCH_DIFFERENCE_TICKS - (e.duration().as_nanos() as i128 / 100)) as i64,
    }
}

/// Win32 属性位；非 Windows 只给出「是否目录」。
#[cfg(windows)]
fn attrs_of(meta: &fs::Metadata, _is_dir: bool) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn attrs_of(_meta: &fs::Metadata, is_dir: bool) -> u32 {
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    if is_dir {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 建一棵临时目录树，返回 (根, 清理 guard)。
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "ff_walk_{tag}_{}_{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&p).unwrap();
            TempTree(p)
        }
        fn dir(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(&p).unwrap();
            p
        }
        fn file(&self, rel: &str, body: &str) {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, body).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 直接遍历一个临时目录树（绕过卷根，测试不需要真实卷）。
    fn walk_tree(root: &Path) -> (Vec<RawEntry>, ScanMeta) {
        let mut out = Vec::new();
        let logical = "T:";
        let root_key = path_key(logical);
        out.push(RawEntry {
            frn: root_key,
            parent_frn: 0,
            name: ".".to_string(),
            is_dir: true,
            attrs: 0,
            size: 0,
            mtime: 0,
            ctime: 0,
            atime: 0,
        });
        let mut stats = WalkStats {
            entries: 1,
            warnings: 0,
        };
        walk_dir(root, logical, root_key, &mut |e| out.push(e), 1, &mut stats);
        let meta = ScanMeta {
            journal_id: 0,
            next_usn: 0,
            entries: stats.entries,
            elapsed: Duration::ZERO,
            warnings: stats.warnings,
        };
        (out, meta)
    }

    #[test]
    fn path_key_is_stable_and_case_folded() {
        assert_eq!(path_key("D:\\Music"), path_key("d:\\music"));
        assert_ne!(path_key("D:\\Music"), path_key("D:\\Music2"));
        assert_eq!(path_key(""), FNV_OFFSET, "空串仍应是非零常量");
        assert_ne!(path_key(""), 0);
    }

    #[test]
    fn filetime_matches_known_epoch() {
        // 1970-01-01 就是 FILETIME 的纪元差常量。
        assert_eq!(
            filetime_from_system_time(UNIX_EPOCH),
            EPOCH_DIFFERENCE_TICKS as i64
        );
        // 1970-01-01 + 1s = 纪元差 + 10_000_000（100ns 单位）。
        assert_eq!(
            filetime_from_system_time(UNIX_EPOCH + Duration::from_secs(1)),
            EPOCH_DIFFERENCE_TICKS as i64 + 10_000_000
        );
        // 1969-12-31T23:59:59 走「早于 1970」分支。
        assert_eq!(
            filetime_from_system_time(UNIX_EPOCH - Duration::from_secs(1)),
            EPOCH_DIFFERENCE_TICKS as i64 - 10_000_000
        );
    }

    #[test]
    fn walk_emits_sorted_tree_with_correct_parents() {
        let t = TempTree::new("basic");
        t.file("a.txt", "aa");
        t.file("sub/b.txt", "bbb");
        t.dir("sub/deep");
        t.file("sub/deep/c.txt", "c");

        let (entries, meta) = walk_tree(&t.0);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // 根在首位，随后按名字排序：a.txt < sub；sub 内 b.txt < deep；deep 内 c.txt。
        assert_eq!(names, vec![".", "a.txt", "sub", "b.txt", "deep", "c.txt"]);
        assert_eq!(meta.entries, entries.len() as u64);
        assert_eq!(meta.warnings, 0);

        let root = &entries[0];
        assert_eq!(root.parent_frn, 0, "根条目的 parent 必须是 0");
        let a = entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(a.parent_frn, root.frn);
        assert!(!a.is_dir);
        assert_eq!(a.size, 2, "文件大小取自元数据");
        let deep = entries.iter().find(|e| e.name == "deep").unwrap();
        let sub = entries.iter().find(|e| e.name == "sub").unwrap();
        assert_eq!(deep.parent_frn, sub.frn);
        let c = entries.iter().find(|e| e.name == "c.txt").unwrap();
        assert_eq!(c.parent_frn, deep.frn);
        assert!(c.mtime > 0, "mtime 应转成 FILETIME");
    }

    #[test]
    fn walk_skips_excluded_dirs_and_their_subtrees() {
        let t = TempTree::new("excluded");
        t.file("keep.txt", "k");
        t.file("$RECYCLE.BIN/junk.txt", "j");
        t.dir("$RECYCLE.BIN/nested");
        t.file("System Volume Information/tracking.log", "x");
        t.file("Sub/System Volume Information/inner.txt", "y");

        let (entries, _) = walk_tree(&t.0);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"keep.txt"));
        assert!(
            !names.contains(&"junk.txt") && !names.contains(&"tracking.log"),
            "回收站 / 卷信息目录的子树必须整体排除: {names:?}"
        );
        // 大小写不同也要命中排除集（Windows 路径不区分大小写）。
        assert!(!names.contains(&"$RECYCLE.BIN"));
    }

    #[test]
    fn walk_does_not_recurse_into_symlink() {
        let t = TempTree::new("link");
        t.dir("real");
        t.file("real/inside.txt", "i");
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_dir;
            // 无权限创建符号链接时跳过（未开开发者模式 / 无 SeCreateSymbolicLinkPrivilege）。
            let link = t.0.join("link_to_real");
            if symlink_dir(t.0.join("real"), &link).is_err() {
                return;
            }
            let (entries, _) = walk_tree(&t.0);
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            // 链接自身进索引，但里面的 inside.txt 只能出现一次（经由 real 那条路径）。
            assert!(names.contains(&"link_to_real"), "{names:?}");
            assert_eq!(
                names.iter().filter(|n| **n == "inside.txt").count(),
                1,
                "不得顺着符号链接重复遍历: {names:?}"
            );
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::symlink;
            symlink(t.0.join("real"), t.0.join("link_to_real")).unwrap();
            let (entries, _) = walk_tree(&t.0);
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            assert!(names.contains(&"link_to_real"));
            assert_eq!(names.iter().filter(|n| **n == "inside.txt").count(), 1);
        }
    }

    #[test]
    fn walk_depth_cap_counts_warning() {
        let t = TempTree::new("deep");
        t.file("top.txt", "x");
        // 直接以「已超上限的深度」进入遍历，比造 96 层真实目录更精确：
        // 断言的是上限判定本身（不读目录、计一次警告），而非文件系统深度能力。
        let mut stats = WalkStats::default();
        let key = path_key("T:");
        let mut out: Vec<RawEntry> = Vec::new();
        walk_dir(
            &t.0,
            "T:",
            key,
            &mut |e| out.push(e),
            MAX_DEPTH + 1,
            &mut stats,
        );
        assert_eq!(stats.warnings, 1, "超深度必须计一次警告");
        assert_eq!(stats.entries, 0, "超深度不得产出任何条目");
        assert!(out.is_empty());

        // 边界：恰好等于上限时仍要正常遍历。
        let mut ok_stats = WalkStats::default();
        walk_dir(&t.0, "T:", key, &mut |_| {}, MAX_DEPTH, &mut ok_stats);
        assert_eq!(ok_stats.warnings, 0);
        assert_eq!(ok_stats.entries, 1, "top.txt 应被产出");
    }

    #[test]
    fn read_changes_is_explicitly_unsupported() {
        let sc = DirWalkScanner;
        let vol = VolumeId {
            serial: 0x1234,
            drive: Some('F'),
        };
        match sc.read_changes(&vol, 0, 0) {
            Err(ScanError::Unsupported(m)) => assert!(m.contains("USN")),
            other => panic!("目录遍历卷必须显式 Unsupported，实际 {other:?}"),
        }
        assert!(!sc.supports_incremental(&vol));
        #[cfg(windows)]
        assert!(
            crate::NtfsVolumeScanner.supports_incremental(&vol),
            "NTFS 默认有增量"
        );
    }

    #[test]
    fn volume_without_drive_is_rejected() {
        let sc = DirWalkScanner;
        let vol = VolumeId {
            serial: 0x1,
            drive: None,
        };
        let err = sc.full_scan(&vol, &mut |_| {}).unwrap_err();
        assert!(matches!(err, ScanError::Unsupported(_)), "{err:?}");
    }
}
