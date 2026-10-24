//! 进程内存测量（memprobe 使用；DESIGN §1.3：200 万条内存目标 ≤ 300MB）。
//!
//! 数据源：Win32 `GetProcessMemoryInfo`（PSAPI），与任务管理器"内存"列同源的
//! WorkingSet / PeakWorkingSet。仅 Windows 可用（bench 其余部分保持跨平台编译）。

//! SAFETY: `GetProcessMemoryInfo` 写入调用方提供的结构体。此处传入
//! `PROCESS_MEMORY_COUNTERS_EX` 并按 `size_of::<PROCESS_MEMORY_COUNTERS>()`
//! 声明长度，读取其前缀字段（WorkingSetSize / PeakWorkingSetSize），不越界。
#![cfg(windows)]

use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;

/// 当前进程 WorkingSet（字节）。失败返回 `None`。
pub fn working_set_bytes() -> Option<u64> {
    // SAFETY: 见模块级注释：零初始化 EX 结构体，按基类型长度传入，只读前缀字段。
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        let ok = GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::addr_of_mut!(pmc).cast::<PROCESS_MEMORY_COUNTERS>(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        );
        ok.ok().map(|()| pmc.WorkingSetSize as u64)
    }
}

/// 当前进程 PeakWorkingSet（字节）。失败返回 `None`。
pub fn peak_working_set_bytes() -> Option<u64> {
    // SAFETY: 同上。
    unsafe {
        let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        let ok = GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::addr_of_mut!(pmc).cast::<PROCESS_MEMORY_COUNTERS>(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        );
        ok.ok().map(|()| pmc.PeakWorkingSetSize as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_set_positive_and_peak_not_less() {
        let ws = working_set_bytes().expect("GetProcessMemoryInfo 失败");
        let peak = peak_working_set_bytes().expect("GetProcessMemoryInfo 失败");
        assert!(ws > 0, "WorkingSet 应 > 0，得到 {ws}");
        assert!(peak >= ws, "PeakWorkingSet({peak}) 应 >= WorkingSet({ws})");
    }
}
