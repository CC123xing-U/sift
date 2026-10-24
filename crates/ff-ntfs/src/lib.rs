//! ff-ntfs —— NTFS 卷读取（DESIGN §5）：MFT 全量扫描 + USN 增量。
//!
//! 封装 `ntfs-reader`（MFT）与 `windows` crate（Win32：卷枚举、USN Journal、权限）。
//! 输出 `RawEntry` / `Change` 供 `ff-engine` 消费；`ff-engine` 只依赖本 crate 的 trait。
//!
//! # 平台约束（DESIGN §0.5）
//! Windows 专有代码只允许存在于本 crate（及 `ff-service`、`app/src-tauri`），
//! 并以 `#[cfg(windows)]` 包裹；非 Windows 平台下编译通过但功能不可用。

use std::fmt;
use std::io;
use std::time::Duration;

/// 卷标识：以**卷序列号**为稳定键（DESIGN §5.3 重建条件：卷序列号变化 → 全量重建）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VolumeId {
    /// 卷序列号（`GetVolumeInformationW`）。
    pub serial: u32,
    /// 驱动器号（如 `'C'`）；装载点卷可能没有盘符，仅用于显示。
    pub drive: Option<char>,
}

impl fmt::Display for VolumeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.drive {
            Some(c) => write!(f, "{c}: (serial={:#010X})", self.serial),
            None => write!(f, "serial={:#010X}", self.serial),
        }
    }
}

/// 全量扫描产出的原始条目（DESIGN §5.1，契约冻结）。
///
/// 时间戳统一为 Windows FILETIME（i64，100ns 单位，1601 纪元）——
/// 见 `docs/DECISIONS.md`「时间戳单位」条；IPC/UI 边界再转换为 unix 毫秒。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawEntry {
    /// 文件引用号（MFT 记录号 + 序列号）。用作键时取低 48 位（DESIGN §5.3）。
    pub frn: u64,
    /// 父目录 FRN；卷根的父为 0。
    pub parent_frn: u64,
    pub name: String,
    pub is_dir: bool,
    /// Win32 文件属性位（`FILE_ATTRIBUTE_*`）。
    pub attrs: u32,
    pub size: u64,
    /// FILETIME（100ns，1601 纪元）。
    pub mtime: i64,
    /// FILETIME（100ns，1601 纪元）。
    pub ctime: i64,
    /// FILETIME（100ns，1601 纪元）。
    pub atime: i64,
}

/// USN 增量变更（DESIGN §5.1，契约冻结）。`read_changes` 属 M2，本轮返回 `Unsupported`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// 新建、重命名、移动、属性变化（同 frn 覆盖）。
    Upsert(RawEntry),
    /// 删除。
    Remove { frn: u64 },
}

/// 全量扫描元数据（DESIGN §5.1：`full_scan` 返回 journal id / next_usn）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanMeta {
    /// 扫描前记录的 USN Journal id（DESIGN §5.2：先记录再扫 MFT，顺序不能反）。
    pub journal_id: u64,
    /// 扫描前记录的 next_usn；扫描期间的变化由此续读补齐（M2）。
    pub next_usn: i64,
    /// 实际喂入 sink 的条目数。
    pub entries: u64,
    pub elapsed: Duration,
    /// 警告计数：父 FRN 无法解析而挂到根下的条目数（DESIGN §5.2）。
    pub warnings: u64,
}

/// 卷扫描错误。
#[derive(Debug)]
pub enum ScanError {
    Io(io::Error),
    VolumeNotFound(VolumeId),
    NotNtfs {
        volume: VolumeId,
        fs_name: String,
    },
    /// 权限不足（需要管理员 / SeManageVolumePrivilege）。
    AccessDenied(String),
    JournalUnavailable(String),
    /// 暂不支持（如 M2 之前的 `read_changes`、USN V3 记录）。
    Unsupported(String),
    Other(String),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Io(e) => write!(f, "IO 错误: {e}"),
            ScanError::VolumeNotFound(v) => write!(f, "卷不存在: {v}"),
            ScanError::NotNtfs { volume, fs_name } => {
                write!(f, "卷 {volume} 不是 NTFS（文件系统: {fs_name}）")
            }
            ScanError::AccessDenied(m) => write!(f, "权限不足: {m}"),
            ScanError::JournalUnavailable(m) => write!(f, "USN Journal 不可用: {m}"),
            ScanError::Unsupported(m) => write!(f, "暂不支持: {m}"),
            ScanError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScanError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ScanError {
    fn from(e: io::Error) -> Self {
        ScanError::Io(e)
    }
}

/// 本 crate 的 Result 别名。
pub type Result<T> = std::result::Result<T, ScanError>;

/// 卷扫描器（DESIGN §5.1，契约冻结）。
pub trait VolumeScanner {
    /// 全量扫描：枚举 `vol` 的 MFT，流式喂给 `sink`。
    ///
    /// 实现必须遵守 DESIGN §5.2 的顺序：**先**记录 `FSCTL_QUERY_USN_JOURNAL` 的
    /// journal id 与 next_usn，**再**扫 MFT；扫描期间的变化由后续从该 next_usn
    /// 续读补齐（M2）。
    fn full_scan(&self, vol: &VolumeId, sink: &mut dyn FnMut(RawEntry)) -> Result<ScanMeta>;

    /// USN 增量读取（DESIGN §5.3，M2 实现）。
    ///
    /// 本轮允许且应当返回 `Err(ScanError::Unsupported(..))`，不得留空实现。
    fn read_changes(
        &self,
        vol: &VolumeId,
        from_usn: i64,
        journal_id: u64,
    ) -> Result<(Vec<Change>, i64)>;
}
