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
    GetLogicalDrives, GetVolumeInformationW, GetVolumePathNamesForVolumeNameW,
    FILE_CREATION_DISPOSITION, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_SHARE_MODE,
};
use windows::Win32::System::Ioctl::{
    CREATE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL, FSCTL_QUERY_USN_JOURNAL,
    FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::System::IO::DeviceIoControl;

use crate::usn::{classify_read_outcome, ReadOutcome};
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

/// 从 `windows` crate 的错误里取出**原始 Win32 错误码**。
///
/// `windows_core::Error` 对 Win32 失败包的是 `HRESULT_FROM_WIN32(code)`。依据
/// `windows-result 0.4.1`（`windows-core 0.62.2` 的依赖，`pub use windows_result::*`）
/// 的 `src/hresult.rs`：`from_win32(error)` = `(error & 0x0000_FFFF) | (7 << 16) | 0x8000_0000`
/// （`error as i32 <= 0` 时原样返回），即**低 16 位就是原始 Win32 码**。
/// 该版本只提供 `from_win32`，**没有反向 API**，故取低 16 位是唯一姿势；
/// 为免"靠掩码猜"，取完再用同一个 `from_win32` **反验**：不符即在 debug 下立刻断言失败。
fn win32_code(e: &windows::core::Error) -> u32 {
    let code = (e.code().0 as u32) & 0xFFFF;
    debug_assert_eq!(
        e.code(),
        windows::core::HRESULT::from_win32(code),
        "HRESULT 不是 HRESULT_FROM_WIN32 形态，Win32 码提取有误"
    );
    code
}

/// 把 `windows` crate 的错误转成 [`ScanError`]，保留**原始错误码**与系统消息。
fn win32_error(context: &str, e: windows::core::Error) -> ScanError {
    let win32 = win32_code(&e);
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
        // DRIVE_FIXED = 3 / DRIVE_REMOVABLE = 2（M7 §5.4：U 盘、存储卡这类
        // exFAT / FAT32 卷也要能索引，走目录遍历回退）。
        // 仍排除：光驱（5）、网络映射盘（4）、RAM 盘（6）。网络盘不是不想支持，
        // 而是遍历没有超时/取消，SMB 卡住会长期占住 scanner 锁、拖死其它卷的
        // 增量轮询——留到能中断的遍历做出来再开（DECISIONS #93）。
        // SAFETY: root_wide 是 NUL 结尾宽串，调用期间保持有效。
        let drive_type = unsafe { GetDriveTypeW(windows::core::PCWSTR(root_wide.as_ptr())) };
        if drive_type != 3 && drive_type != 2 {
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

    // 补上没有盘符的 GUID 卷（装载点）。无装载点的卷（恢复分区 / EFI / 瞬态虚拟卷）
    // 没有可解析路径，索引无意义且其出现/消失会让索引集合抖动，直接排除。
    for (serial, fs_name, guid) in by_serial {
        if volumes.iter().any(|v| v.serial == serial) {
            continue;
        }
        if !has_mount_points(&guid) {
            continue;
        }
        volumes.push(VolumeInfo {
            serial,
            fs_name,
            guid_path: guid,
            drive: None,
        });
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

/// 根路径规范化：`GetVolumeInformationW` 要求根路径带尾随反斜杠。
///
/// GUID 路径（`\\?\Volume{...}`）缺尾随反斜杠时 API 直接返回 ERROR_INVALID_NAME(123)
/// （实测：带 → 成功；不带 → 123）。`device_path_for` 为了给 `CreateFileW` 开原始卷会
/// trim 掉尾随反斜杠，因此这里统一补回，调用方可安全传入两种形态（NOTES.md 尾随反斜杠规则）。
fn normalize_root(root_path: &str) -> String {
    let trimmed = root_path.trim_end_matches('\\');
    let is_guid = trimmed
        .get(..11)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\Volume{"));
    let bytes = trimmed.as_bytes();
    let is_bare_drive = bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if is_guid || is_bare_drive {
        format!("{trimmed}\\")
    } else {
        root_path.to_string()
    }
}

/// `GetVolumeInformationW` 取（序列号, 文件系统名）。根路径需带尾随反斜杠（内部已规范化）。
pub fn query_volume_by_root(root_path: &str) -> Result<(u32, String), ScanError> {
    let normalized = normalize_root(root_path);
    let root = wide(&normalized);
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
    .map_err(|e| win32_error(&format!("GetVolumeInformationW({normalized})"), e))?;
    Ok((serial, from_wide(&fs_buf)))
}

/// 卷是否至少有一个装载点（盘符或目录挂载）。
///
/// 无装载点的卷（恢复分区 / EFI / 瞬态虚拟卷）没有可解析路径：索引它们无意义，
/// 且这类卷出现/消失不稳定，会让索引集合反复抖动。调用失败时 fail-open（视为有），
/// 避免因一次 API 抖动静默丢掉可索引的卷。
pub fn has_mount_points(guid_path: &str) -> bool {
    let wide_guid = wide(guid_path);
    let mut buf = [0u16; 512];
    let mut returned: u32 = 0;
    // SAFETY: wide_guid 是 NUL 结尾宽串；buf 为栈上可变缓冲（长度经 slice 传入）；
    // returned 为栈上可变 u32，接收实际写出的字符数。
    let res = unsafe {
        GetVolumePathNamesForVolumeNameW(
            windows::core::PCWSTR(wide_guid.as_ptr()),
            Some(&mut buf),
            &mut returned,
        )
    };
    match res {
        // 成功：装载点列表为双 NUL 结尾串列，空列表即首字符为 NUL。
        Ok(()) => {
            debug_assert!(returned as usize <= buf.len());
            buf[0] != 0
        }
        // ERROR_MORE_DATA（列表超长）或其他失败：保守视为有装载点，不排除（fail-open）。
        Err(_) => true,
    }
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

/// `FSCTL_READ_USN_JOURNAL`：从 `from_usn` 起增量读取原始 USN 记录流（DESIGN §5.3，M2）。
///
/// 返回缓冲布局与 `usn::parse_usn_journal_buffer` 约定一致：前 8 字节为**下一条**
/// 待读 USN（续读位置），其后是紧挨排列的 `USN_RECORD_V2/V3`（V3 由 `usn` 跳过）。
///
/// - `journal_id` 须与 `query_usn_journal` 一致；若日志被重建（id 变化），内核返回
///   `ERROR_JOURNAL_ENTRY_DELETED`，本函数转成 `ScanError::JournalUnavailable`，由调用方
///   按 DESIGN §5.3 重建条件重新基准（全量重建）；
/// - 记录多于输出缓冲时内核返回 `ERROR_MORE_DATA`：本函数自动按缓冲头部给出的
///   下一条 USN 续读、累积记录字节、翻倍缓冲，直到干净读完；日志异常庞大时设上限防死循环；
/// - 读到日志末尾时内核返回 `ERROR_HANDLE_EOF`：按 DESIGN §5.3 视为**正常结束**，
///   返回已累积的记录（可能为空）与最后已知的续读位置，不报错。这是增量轮询最常态的路径
///   （无新记录）。错误码分派见 [`usn::classify_read_outcome`]。
pub fn read_usn_journal(
    device_path: &str,
    from_usn: i64,
    journal_id: u64,
) -> Result<Vec<u8>, ScanError> {
    let handle = open_volume(device_path)?;
    // 单段缓冲上限：64 MiB。绝大多数增量批次远小于此；超限靠分块推进。
    const MAX_OUT: usize = 64 * 1024 * 1024;

    let mut start_usn = from_usn;
    // 仅累积记录字节（不含每段前缀的 8 字节 next USN）。
    let mut records_bytes: Vec<u8> = Vec::new();
    // 已确认的续读位置（下一条待读 USN）。初值取请求起点：若一段都没读到
    // （例如首段即 ERROR_HANDLE_EOF），就回退为该起点，语义是"没有新东西可读"。
    let mut next_usn = from_usn;

    // 初始缓冲 256 KiB，遇分块翻倍，封顶 MAX_OUT。
    let mut out = vec![0u8; 256 * 1024];
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 256 {
            return Err(ScanError::Other(
                "FSCTL_READ_USN_JOURNAL 超过 256 次分块仍未读完，USN 日志异常".to_string(),
            ));
        }
        let input = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: start_usn,
            ReasonMask: 0xFFFF_FFFF, // 所有 reason 都要
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: journal_id,
        };
        let mut returned: u32 = 0;
        // SAFETY: input 为本函数栈上变量，按结构体实际尺寸传入；out 是有效可变缓冲，
        // 尺寸按容量传入；returned 为栈上 u32；调用期间三者均保持有效。
        let res = unsafe {
            DeviceIoControl(
                handle.0,
                FSCTL_READ_USN_JOURNAL,
                Some(&input as *const READ_USN_JOURNAL_DATA_V0 as *const std::ffi::c_void),
                size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                Some(out.as_mut_ptr() as *mut std::ffi::c_void),
                out.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        match res {
            Ok(()) => {
                out.truncate(returned as usize);
                if returned < 8 {
                    return Err(ScanError::Other(
                        "FSCTL_READ_USN_JOURNAL 输出不足 8 字节（无 next USN）".to_string(),
                    ));
                }
                next_usn = i64::from_le_bytes(out[..8].try_into().unwrap());
                records_bytes.extend_from_slice(&out[8..]);
                break;
            }
            Err(e) => match classify_read_outcome(win32_code(&e)) {
                ReadOutcome::MoreData => {
                    // 缓冲不够：本段 [8-byte next_usn][records] 已有效，累积记录并续读。
                    out.truncate(returned as usize);
                    if returned < 8 {
                        return Err(ScanError::Other(
                            "FSCTL_READ_USN_JOURNAL 分块输出不足 8 字节（无 next USN）".to_string(),
                        ));
                    }
                    next_usn = i64::from_le_bytes(out[..8].try_into().unwrap());
                    records_bytes.extend_from_slice(&out[8..]);
                    start_usn = next_usn;
                    let new_len = (out.len() * 2).min(MAX_OUT);
                    out = vec![0u8; new_len];
                }
                ReadOutcome::EndOfJournal => {
                    // DESIGN §5.3：读到日志末尾 = **正常结束**。此时 out/returned 无意义，
                    // 只读已累积的记录字节与最后已知的续读位置。
                    return Ok(assemble_read_result(next_usn, &records_bytes));
                }
                ReadOutcome::JournalEntryDeleted => {
                    return Err(ScanError::JournalUnavailable(format!(
                        "USN 日志记录已被删除或重建（{device_path}），需重新基准（全量重建）"
                    )));
                }
                // Success 不应出现在 Err 分支；其余错误码走原错误路径（保留原始码）。
                ReadOutcome::Success | ReadOutcome::Failed(_) => {
                    return Err(win32_error(
                        &format!("FSCTL_READ_USN_JOURNAL({device_path})"),
                        e,
                    ));
                }
            },
        }
    }

    Ok(assemble_read_result(next_usn, &records_bytes))
}

/// 拼成 `usn::parse_usn_journal_buffer` 可直接解析的格式：`[next_usn][records...]`。
fn assemble_read_result(next_usn: i64, records_bytes: &[u8]) -> Vec<u8> {
    let mut result = next_usn.to_le_bytes().to_vec();
    result.extend_from_slice(records_bytes);
    result
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

/// 本地时区相对 UTC 的偏移（分钟；东八区 = 480，西五区 = -300）。
///
/// `ff-core` 是零平台依赖的纯逻辑 crate，标准库拿不到本地时区；这里用 Win32
/// 取到真实偏移后由 `ff-engine` 注入 [`ff_core::Query`] 的求值上下文，
/// 使 `dm:today` 等相对日期按**本地日历日**解释（DESIGN §7.2）。
///
/// `TIME_ZONE_INFORMATION.Bias` 的定义是「UTC = 本地时间 + Bias」，
/// 因此本地相对 UTC 的偏移 = `-Bias`。夏令时启用时再叠加 `-DaylightBias`
/// （返回值 `TIME_ZONE_ID_DAYLIGHT = 2`）。
pub fn local_utc_offset_minutes() -> i32 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

    // 不变量：`tzi` 是栈上的输出参数，`GetTimeZoneInformation` 只写它，
    // 返回值为时区 ID 或 `TIME_ZONE_ID_INVALID`（失败时返回 0xFFFFFFFF）。
    let mut tzi = TIME_ZONE_INFORMATION::default();
    let ret = unsafe { GetTimeZoneInformation(&mut tzi) };
    if ret == 0xFFFF_FFFFu32 {
        return 0; // 取不到就用 UTC，不把查询变成错误
    }
    let mut offset = -tzi.Bias;
    if ret == 2 {
        // TIME_ZONE_ID_DAYLIGHT
        offset -= tzi.DaylightBias;
    }
    offset
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
