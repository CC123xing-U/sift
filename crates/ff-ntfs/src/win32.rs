//! Win32 封装（DESIGN §0.5 平台隔离 · §0.6 unsafe 注释与句柄 RAII）。
//!
//! 覆盖 M0 需要的三组 API（全部经 `windows` crate，当前 0.62）：
//! - 卷枚举：`GetLogicalDrives` + `GetVolumeInformationW` 判 NTFS，`FindFirstVolumeW` 取 GUID 路径；
//! - USN Journal：`FSCTL_QUERY_USN_JOURNAL`（DESIGN §5.2 扫描前先记录）、
//!   `FSCTL_CREATE_USN_JOURNAL`（§5.3 日志缺失时尝试创建）；
//! - 进程内存：`GetProcessMemoryInfo` 的 WorkingSet（M0 峰值内存实测）。
//!
//! 每个 `unsafe` 块都注明不变量；内核句柄一律经 [`VolumeHandle`] RAII 自动关闭。

use std::mem::size_of;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDriveTypeW,
    GetLogicalDrives, GetVolumeInformationW, FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_GENERIC_READ, FILE_SHARE_MODE,
};
use windows::Win32::System::Ioctl::{
    CREATE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL, FSCTL_QUERY_USN_JOURNAL, USN_JOURNAL_DATA_V0,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::System::IO::DeviceIoControl;

use crate::ScanError;

/// 打开卷设备路径用的共享模式：读/写共享，避免与其他进程冲突。
const VOLUME_SHARE_MODE: FILE_SHARE_MODE = FILE_SHARE_MODE(0x1 | 0x2); // READ | WRITE

/// `FSCTL_CREATE_USN_JOURNAL` 默认参数（`fsutil usn createjournal m=0x10000000 a=0x100000` 同量级）。
pub const USN_JOURNAL_DEFAULT_MAX_SIZE: u64 = 0x1000_0000; // 256 MiB
pub const USN_JOURNAL_DEFAULT_ALLOCATION_DELTA: u64 = 0x10_0000; // 1 MiB

/// 枚举到的一个卷的信息（M0 摸底展示用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// 卷序列号（`GetVolumeInformationW`）。
    pub serial: u32,
    /// 文件系统名（如 `NTFS` / `FAT32` / `exFAT`）。
    pub fs_name: String,
    /// 卷 GUID 路径（`FindFirstVolumeW`，形如 `\\?\Volume{...}\`）。
    pub guid_path: String,
    /// 盘符（装载点卷没有盘符）。
    pub drive: Option<char>,
}

/// `FSCTL_QUERY_USN_JOURNAL` 的关键结果（DESIGN §5.2：扫描 MFT 前先记录）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsnJournalInfo {
    pub journal_id: u64,
    pub first_usn: i64,
    pub next_usn: i64,
    pub max_size: u64,
    pub allocation_delta: u64,
}

/// RAII 卷句柄：Drop 自动 `CloseHandle`（DESIGN §0.6）。
struct VolumeHandle(HANDLE);

impl Drop for VolumeHandle {
    fn drop(&mut self) {
        // SAFETY: self.0 是本结构独占的合法内核句柄（或 INVALID_HANDLE_VALUE，CloseHandle 对其为 no-op 失败）。
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// UTF-8 → NUL 结尾 UTF-16（Win32 宽字符参数）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 缓冲 → String（截到 NUL 为止，其余 lossy）。
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// 把 `windows` crate 的错误转成 [`ScanError`]，保留**原始错误码**与系统消息。
///
/// `windows_core::Error` 的 HRESULT 对 Win32 失败是 `HRESULT_FROM_WIN32(code)`
/// （facility 7），低 16 位即 Win32 错误码；据此取 `io::Error` 的本地化消息。
fn win32_error(context: &str, e: windows::core::Error) -> ScanError {
    let win32 = (e.code().0 as u32) & 0xFFFF;
    let os_message = std::io::Error::from_raw_os_error(win32 as i32);
    let message = format!("{context}: {os_message}（原始 Win32 错误码 {win32}）");
    // ERROR_ACCESS_DENIED = 5
    if win32 == 5 {
        ScanError::AccessDenied(message)
    } else {
        ScanError::Io(std::io::Error::other(message))
    }
}

/// 枚举系统上的卷（盘符卷 + 无盘符的 GUID 装载点卷）。
///
/// - `GetLogicalDrives` 位掩码逐盘符查序列号 / 文件系统；
/// - `FindFirstVolumeW` / `FindNextVolumeW` 遍历 GUID 路径，按序列号与盘符关联。
pub fn enumerate_volumes() -> Result<Vec<VolumeInfo>, ScanError> {
    // 第一遍：所有 GUID 路径及其序列号 / 文件系统。
    let mut by_serial: Vec<(u32, String, String)> = Vec::new(); // (serial, fs_name, guid_path)
    {
        let mut buf = [0u16; 128];
        // SAFETY: buf 是合法的可变宽字符缓冲；返回的句柄立即交给 RAII 包装或关闭。
        let find = unsafe { FindFirstVolumeW(&mut buf) }
            .map_err(|e| win32_error("FindFirstVolumeW", e))?;
        let _find_guard = FindGuard(find);
        loop {
            let guid = from_wide(&buf);
            if let Ok(info) = query_volume_by_root(&guid) {
                by_serial.push((info.0, info.1, guid));
            }
            // SAFETY: 同一缓冲重复用于 FindNextVolumeW。
            if unsafe { FindNextVolumeW(find, &mut buf) }.is_err() {
                break;
            }
        }
    }

    // 第二遍：盘符 → 序列号 / 文件系统 / 关联 GUID。
    let mut volumes = Vec::new();
    // SAFETY: GetLogicalDrives 无指针参数，仅返回盘符位掩码。
    let drives = unsafe { GetLogicalDrives() };
    for bit in 0..26u8 {
        if drives & (1 << bit) == 0 {
            continue;
        }
        let drive = (b'A' + bit) as char;
        let root = format!("{drive}:\\");
        let root_wide = wide(&root);
        // DRIVE_FIXED = 3；过滤光驱、可移动盘与网络映射盘（DESIGN §5.4）。
        // SAFETY: root_wide 是 NUL 结尾宽串，调用期间保持有效。
        if unsafe { GetDriveTypeW(windows::core::PCWSTR(root_wide.as_ptr())) } != 3 {
            continue;
        }
        let Ok((serial, fs_name)) = query_volume_by_root(&root) else {
            continue;
        };
        let guid_path = by_serial
            .iter()
            .find(|(s, _, _)| *s == serial)
            .map(|(_, _, g)| g.clone())
            .unwrap_or_default();
        volumes.push(VolumeInfo {
            serial,
            fs_name,
            guid_path,
            drive: Some(drive),
        });
    }

    // 补上没有盘符的 GUID 卷（装载点）。
    for (serial, fs_name, guid) in by_serial {
        if !volumes.iter().any(|v| v.serial == serial) {
            volumes.push(VolumeInfo {
                serial,
                fs_name,
                guid_path: guid,
                drive: None,
            });
        }
    }

    volumes.sort_by_key(|v| v.drive);
    Ok(volumes)
}

/// `FindVolumeClose` 的 RAII 包装。
struct FindGuard(HANDLE);

impl Drop for FindGuard {
    fn drop(&mut self) {
        // SAFETY: self.0 是 FindFirstVolumeW 返回的合法查找句柄。
        unsafe {
            let _ = FindVolumeClose(self.0);
        }
    }
}

/// `GetVolumeInformationW` 取（序列号, 文件系统名）。根路径需带尾随反斜杠。
pub fn query_volume_by_root(root_path: &str) -> Result<(u32, String), ScanError> {
    let root = wide(root_path);
    let mut serial: u32 = 0;
    let mut fs_buf = [0u16; 32];
    // SAFETY: root 是 NUL 结尾宽串；输出缓冲均为本函数栈上可变数组。
    unsafe {
        GetVolumeInformationW(
            windows::core::PCWSTR(root.as_ptr()),
            None,
            Some(&mut serial),
            None,
            None,
            Some(&mut fs_buf),
        )
    }
    .map_err(|e| win32_error(&format!("GetVolumeInformationW({root_path})"), e))?;
    Ok((serial, from_wide(&fs_buf)))
}

/// 打开卷设备路径（`\\.\C:` 或 `\\?\Volume{...}\`）为可读句柄。
fn open_volume(device_path: &str) -> Result<VolumeHandle, ScanError> {
    let path = wide(device_path);
    // SAFETY: path 为 NUL 结尾宽串；其余参数均为常量或 None；返回值即刻交给 RAII。
    let handle = unsafe {
        CreateFileW(
            windows::core::PCWSTR(path.as_ptr()),
            FILE_GENERIC_READ.0,
            VOLUME_SHARE_MODE,
            None,
            FILE_CREATION_DISPOSITION(3), // OPEN_EXISTING
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .map_err(|e| win32_error(&format!("CreateFileW({device_path})"), e))?;
    Ok(VolumeHandle(handle))
}

/// `FSCTL_QUERY_USN_JOURNAL`：读当前 journal id / FirstUsn / NextUsn。
pub fn query_usn_journal(device_path: &str) -> Result<UsnJournalInfo, ScanError> {
    let handle = open_volume(device_path)?;
    let mut data = USN_JOURNAL_DATA_V0::default();
    let mut returned: u32 = 0;
    // SAFETY: data/returned 是本函数栈上变量，大小按结构体实际尺寸传入。
    unsafe {
        DeviceIoControl(
            handle.0,
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(&mut data as *mut USN_JOURNAL_DATA_V0 as *mut std::ffi::c_void),
            size_of::<USN_JOURNAL_DATA_V0>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| win32_error(&format!("FSCTL_QUERY_USN_JOURNAL({device_path})"), e))?;

    Ok(UsnJournalInfo {
        journal_id: data.UsnJournalID,
        first_usn: data.FirstUsn,
        next_usn: data.NextUsn,
        max_size: data.MaximumSize,
        allocation_delta: data.AllocationDelta,
    })
}

/// `FSCTL_CREATE_USN_JOURNAL`：日志不存在/被禁用时按 DESIGN §5.3 尝试创建。
pub fn create_usn_journal(
    device_path: &str,
    max_size: u64,
    allocation_delta: u64,
) -> Result<(), ScanError> {
    let handle = open_volume(device_path)?;
    let mut data = CREATE_USN_JOURNAL_DATA {
        MaximumSize: max_size,
        AllocationDelta: allocation_delta,
    };
    let mut returned: u32 = 0;
    // SAFETY: data/returned 为栈上变量；输入缓冲按结构体实际尺寸传入。
    unsafe {
        DeviceIoControl(
            handle.0,
            FSCTL_CREATE_USN_JOURNAL,
            Some(&mut data as *mut CREATE_USN_JOURNAL_DATA as *const std::ffi::c_void),
            size_of::<CREATE_USN_JOURNAL_DATA>() as u32,
            None,
            0,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| win32_error(&format!("FSCTL_CREATE_USN_JOURNAL({device_path})"), e))?;
    Ok(())
}

/// 当前进程的峰值 WorkingSet 字节数（`GetProcessMemoryInfo`，M0 内存实测）。
pub fn peak_working_set_bytes() -> Option<usize> {
    // GetCurrentProcess 返回伪句柄（-1），无需也不应关闭。
    let process = unsafe { GetCurrentProcess() };
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: counters 已初始化为本结构体大小；process 为当前进程伪句柄。
    unsafe { GetProcessMemoryInfo(process, &mut counters, counters.cb) }
        .ok()
        .map(|()| counters.PeakWorkingSetSize)
}

/// 当前进程的 WorkingSet 字节数（非峰值，用于对照）。
pub fn working_set_bytes() -> Option<usize> {
    let process = unsafe { GetCurrentProcess() };
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: 同上。
    unsafe { GetProcessMemoryInfo(process, &mut counters, counters.cb) }
        .ok()
        .map(|()| counters.WorkingSetSize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_roundtrip_and_nul_termination() {
        let w = wide(r"\\.\C:");
        assert_eq!(*w.last().unwrap(), 0);
        assert_eq!(from_wide(&w), r"\\.\C:");
    }

    #[test]
    fn from_wide_truncates_at_nul() {
        let buf = [b'N' as u16, b'T' as u16, 0, b'X' as u16];
        assert_eq!(from_wide(&buf), "NT");
    }

    #[test]
    fn win32_error_carries_raw_code() {
        // ERROR_ACCESS_DENIED = 5 → HRESULT_FROM_WIN32(5) = 0x8007_0005
        let err = windows::core::Error::from_hresult(windows::core::HRESULT(0x8007_0005u32 as i32));
        match win32_error("测试", err) {
            ScanError::AccessDenied(msg) => {
                assert!(msg.contains("原始 Win32 错误码 5"), "{msg}");
            }
            other => panic!("期望 AccessDenied，得到 {other:?}"),
        }
        // ERROR_PATH_NOT_FOUND = 3
        let err = windows::core::Error::from_hresult(windows::core::HRESULT(0x8007_0003u32 as i32));
        match win32_error("测试", err) {
            ScanError::Io(e) => assert!(e.to_string().contains("原始 Win32 错误码 3")),
            other => panic!("期望 Io，得到 {other:?}"),
        }
    }
}
