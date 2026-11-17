//! 命名管道 server/client（DESIGN §10.1）。
//!
//! 传输层只负责把 Win32 `HANDLE` 包装成 `Read`/`Write`，帧编解码与请求分发复用
//! `ff_engine::ipc` 的 `serve_connection` / `read_response` / `write_request`——
//! 协议只有一份实现，管道不重复造轮子。
//!
//! 所有 pipe 都是**字节模式同步 I/O**（`PIPE_TYPE_BYTE | PIPE_READMODE_BYTE |
//! PIPE_WAIT`），与长度前缀 JSON 天然契合；实例数 `PIPE_UNLIMITED_INSTANCES`
//! 支持多客户端并发，每条连接由服务侧分配独立 `EngineSession`。

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, SetNamedPipeHandleState, NMPWAIT_USE_DEFAULT_WAIT,
    PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::GetCurrentProcess;

use crate::sid::{build_security_descriptor, free_security_descriptor};

const BUFFER_SIZE: u32 = 64 * 1024;

/// 拥有的管道句柄；`Read`/`Write` 走同步 ReadFile/WriteFile，Drop 关闭句柄。
///
/// `HANDLE` 本身不是 `Send`（裸指针语义），但同步管道句柄在**单线程专有**使用时
/// 是安全的：我们从不跨线程共享同一个 `PipeStream`，accept 后直接 move 给工作线程。
unsafe impl Send for PipeStream {}

pub struct PipeStream {
    handle: HANDLE,
    closed: AtomicBool,
}

impl PipeStream {
    /// 从已有句柄接管所有权（调用方不得再关闭）。
    pub(crate) unsafe fn from_handle(handle: HANDLE) -> Self {
        Self {
            handle,
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn handle(&self) -> HANDLE {
        self.handle
    }

    /// 复制句柄（指向同一个管道端），用于把 reader/writer 分给 `serve_connection`
    /// 的两个 `&mut` 参数。两份句柄各自 Drop 关闭，引用计数由内核管理。
    pub fn try_clone(&self) -> io::Result<Self> {
        let mut dup = HANDLE::default();
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle,
                GetCurrentProcess(),
                &mut dup,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .map_err(io::Error::other)?;
        Ok(unsafe { PipeStream::from_handle(dup) })
    }

    /// 刷新内核管道缓冲，确保对端在断开前读到全部字节。
    pub fn flush_buffers(&self) -> io::Result<()> {
        unsafe { FlushFileBuffers(self.handle) }.map_err(io::Error::other)
    }

    fn ensure_open(&self) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pipe already closed",
            ))
        } else {
            Ok(())
        }
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.ensure_open()?;
        if buf.is_empty() {
            return Ok(0);
        }
        let mut read = 0u32;
        if let Err(e) = unsafe { ReadFile(self.handle, Some(buf), Some(&mut read), None) } {
            // windows-core 0.62 把 Win32 失败码封进 HRESULT 低 16 位（同 ff-ntfs::win32_code）。
            // 客户端已写完并关闭时 ReadFile 报 ERROR_BROKEN_PIPE(109) / ERROR_NO_DATA(232)，
            // 这是「干净断开」而非连接错误：转成 EOF(Ok(0))，让上层 read_message 能区分
            // 干净断开与截断帧，也避免服务侧把每次正常退出误记成错误日志。
            let win32 = (e.code().0 as u32) & 0xFFFF;
            if win32 == 109 || win32 == 232 {
                return Ok(0);
            }
            return Err(io::Error::other(e));
        }
        Ok(read as usize)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.ensure_open()?;
        if buf.is_empty() {
            return Ok(0);
        }
        let mut written = 0u32;
        unsafe { WriteFile(self.handle, Some(buf), Some(&mut written), None) }
            .map_err(io::Error::other)?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = unsafe { CloseHandle(self.handle) };
        }
    }
}

/// 服务端监听端：持有一个「模板」命名管道实例，accept 时阻塞等待客户端连接。
///
/// 每接受一条连接就新建下一实例（用相同名字与 ACL），因此天然支持多客户端。
pub struct PipeListener {
    name: String,
    sddl: String,
}

impl PipeListener {
    pub fn new(name: String, sddl: String) -> Self {
        Self { name, sddl }
    }

    /// 用 ACL 创建一个管道实例并阻塞等待客户端；返回已连接的流。
    pub fn accept(&self) -> io::Result<PipeStream> {
        let wide: Vec<u16> = self.name.encode_utf16().chain(std::iter::once(0)).collect();
        let sd = build_security_descriptor(&self.sddl)?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: windows::Win32::Foundation::FALSE,
        };
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER_SIZE,
                BUFFER_SIZE,
                NMPWAIT_USE_DEFAULT_WAIT,
                Some(&sa),
            )
        };
        unsafe { free_security_descriptor(sd) };
        if handle.is_invalid() {
            return Err(io::Error::other(format!(
                "CreateNamedPipeW failed: {}",
                io::Error::last_os_error()
            )));
        }
        let stream = unsafe { PipeStream::from_handle(handle) };

        // 阻塞等客户端连接。若客户端已在 CreateNamedPipe 与 ConnectNamedPipe 之间
        // 连上，Win32 返回 ERROR_PIPE_CONNECTED——这也是「连接成功」，不能当错误。
        if let Err(e) = unsafe { ConnectNamedPipe(stream.handle(), None) } {
            const ERROR_PIPE_CONNECTED: i32 = 535;
            if e.code().0 != ERROR_PIPE_CONNECTED {
                return Err(io::Error::other(format!("ConnectNamedPipe: {e}")));
            }
        }
        Ok(stream)
    }
}

/// 客户端连接服务端。仅允许本用户 / SYSTEM 访问由服务端 ACL 保证。
pub fn connect(name: &str) -> io::Result<PipeStream> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }?;
    if handle.is_invalid() {
        return Err(io::Error::other("CreateFileW returned invalid handle"));
    }
    // 字节读模式显式设置一次，避免服务端实例模式残留。
    let mode = PIPE_READMODE_BYTE;
    unsafe { SetNamedPipeHandleState(handle, Some(&mode), None, None) }
        .map_err(io::Error::other)?;
    Ok(unsafe { PipeStream::from_handle(handle) })
}
