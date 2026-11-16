//! 当前用户 SID 获取与命名管道安全描述符（DESIGN §10.1）。
//!
//! 管道名 `\\.\pipe\sift-<SID>`：SID 来自调用进程的访问令牌（服务以 LocalSystem
//! 运行时是 SYSTEM 的 SID，但管道 ACL 里显式放行的是**登录用户**的 SID——由服务
//! 从「活动用户会话」取得；本模块提供两层能力：
//!   1. [`current_user_sid`]：当前进程令牌的 SID（UI 以普通权限运行时即登录用户）；
//!   2. [`pipe_acl_sddl`]：给定用户 SID，构造只允许该用户与 SYSTEM 访问的 SDDL。
//!
//! 服务侧应从客户端连接反查或由配置注入用户 SID（见 server.rs 的说明）；本模块
//! 不假定服务一定有交互式令牌，避免把 SYSTEM 的 SID 误当成登录用户。

use std::ffi::c_void;
use std::io;

use windows::core::PWSTR;
use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// 从当前进程令牌读取用户 SID，返回 `S-1-5-21-...` 字符串（UI 以普通权限运行时即登录用户）。
pub fn current_user_sid() -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| io::Error::other(format!("OpenProcessToken: {e}")))?;
        let _guard = HandleGuard(token);
        sid_from_token(token)
    }
}

/// 读取「活动控制台会话」登录用户的 SID。服务以 LocalSystem 运行时，
/// `current_user_sid()` 会得到 SYSTEM 的 SID，必须用本函数取真实登录用户，
/// 否则管道 ACL 会把 UI 进程挡在外面（DESIGN §10.1 的 `sift-<用户 SID>`）。
pub fn interactive_user_sid() -> io::Result<String> {
    unsafe {
        let session = WTSGetActiveConsoleSessionId();
        if session == u32::MAX {
            return Err(io::Error::other("no active console session"));
        }
        let mut token = HANDLE::default();
        WTSQueryUserToken(session, &mut token)
            .map_err(|e| io::Error::other(format!("WTSQueryUserToken({session}): {e}")))?;
        let _guard = HandleGuard(token);
        sid_from_token(token)
    }
}

/// 从令牌句柄读取用户 SID（令牌由调用方持有并在返回后关闭）。
unsafe fn sid_from_token(token: HANDLE) -> io::Result<String> {
    // 第一次调用取长度；returned_len 即所需大小。
    let mut needed = 0u32;
    let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
    if needed == 0 {
        return Err(io::Error::other("GetTokenInformation returned zero length"));
    }
    let mut buf = vec![0u8; needed as usize];
    GetTokenInformation(
        token,
        TokenUser,
        Some(buf.as_mut_ptr() as *mut c_void),
        needed,
        &mut needed,
    )
    .map_err(|e| io::Error::other(format!("GetTokenInformation(data): {e}")))?;

    let tu = &*(buf.as_ptr() as *const TOKEN_USER);
    sid_to_string(tu.User.Sid)
}

/// 把二进制 SID 转成字符串 SID，释放 API 分配的内存。
unsafe fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut pwstr = PWSTR::null();
    ConvertSidToStringSidW(sid, &mut pwstr)
        .map_err(|e| io::Error::other(format!("ConvertSidToStringSidW: {e}")))?;
    let result = pwstr
        .to_string()
        .map_err(|e| io::Error::other(format!("SID UTF-16: {e}")));
    let _ = LocalFree(Some(HLOCAL(pwstr.0 as *mut c_void)));
    result
}

/// 命名管道全名：`\\.\pipe\sift-<SID>`。
///
/// 完全由 ASCII（SID 字符 + 固定前缀）构成，便于在 ACL/日志/测试里稳定比较。
pub fn pipe_name(user_sid: &str) -> String {
    format!(r"\\.\pipe\sift-{user_sid}")
}

/// 构造只允许指定用户 SID 与 SYSTEM 完全控制、拒绝继承的 SDDL。
///
/// - `D:P`：受保护 DACL（禁止从父对象继承 ACE），避免权限被放宽；
/// - `(A;;GA;;;<sid>)`：当前用户完全控制；
/// - `(A;;GA;;;SY)`：SYSTEM 完全控制（服务自身与管理工具需要）；
/// - 无 `AU`/`WD` 等宽泛组，其他用户被隐式拒绝。
pub fn pipe_acl_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;{user_sid})(A;;GA;;;SY)")
}

/// 把 SDDL 解析成自相对安全描述符（调用方负责在用完后 [`free_security_descriptor`]）。
pub(crate) fn build_security_descriptor(sddl: &str) -> io::Result<*mut c_void> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Authorization::SDDL_REVISION_1;

    let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .map_err(|e| {
            io::Error::other(format!(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW: {e}"
            ))
        })?;
    }
    if sd.is_invalid() {
        return Err(io::Error::other("security descriptor is null"));
    }
    Ok(sd.0)
}

/// 释放 [`build_security_descriptor`] 返回的缓冲区（LocalAlloc 分配）。
pub(crate) unsafe fn free_security_descriptor(sd: *mut c_void) {
    if !sd.is_null() {
        let _ = LocalFree(Some(HLOCAL(sd)));
    }
}

/// 进程句柄 RAII 包装（仅用于 current_user_sid 内一次性关闭令牌句柄）。
struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_uses_sid() {
        assert_eq!(pipe_name("S-1-5-21-123"), r"\\.\pipe\sift-S-1-5-21-123");
    }

    #[test]
    fn sddl_allows_only_user_and_system() {
        let sddl = pipe_acl_sddl("S-1-5-21-999");
        assert!(sddl.starts_with("D:P("), "DACL 必须受保护禁止继承: {sddl}");
        assert!(sddl.contains("(A;;GA;;;S-1-5-21-999)"), "{sddl}");
        assert!(sddl.contains("(A;;GA;;;SY)"), "{sddl}");
        assert!(!sddl.contains("WD"), "不得包含 Everyone 宽泛组: {sddl}");
        assert!(!sddl.contains("AU"), "不得包含 Authenticated Users: {sddl}");
    }

    #[test]
    fn current_sid_looks_like_sid() {
        // 在 Windows 上（本 crate 仅 windows 编译）应能读到形如 S-1-... 的 SID。
        let sid = current_user_sid().expect("读取当前用户 SID");
        assert!(sid.starts_with("S-1-"), "意外 SID 格式: {sid}");
        assert!(sid.len() > 4);
    }

    #[test]
    fn built_security_descriptor_is_nonnull_and_freeable() {
        let sddl = pipe_acl_sddl(&current_user_sid().unwrap());
        let sd = build_security_descriptor(&sddl).expect("构造安全描述符");
        assert!(!sd.is_null());
        unsafe { free_security_descriptor(sd) };
    }
}
