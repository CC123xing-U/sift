//! ff-ipc —— Sift M6 命名管道传输层（DESIGN §10.1 / §12）。
//!
//! 本 crate 仅在 Windows 编译（命名管道 / SID / ACL 均为 Win32）：非 Windows 下
//! 是空 crate，与 `ff-ntfs` 的平台隔离策略一致。
//!
//! 职责边界：
//! - 协议类型与编解码在 `ff-engine::ipc`（版本化消息 + 有界 JSON 帧），本 crate 不重复；
//! - 本 crate 提供 SID/SDDL ACL、HANDLE→Read/Write 封装、server accept 循环、
//!   `EngineClient over Pipe` 客户端；
//! - `Open` 只解析出 `ResolvedAction`，真实打开/剪贴板由 UI 进程执行（§12）。

#![cfg(windows)]

pub mod client;
pub mod pipe;
pub mod server;
pub mod sid;

pub use client::{resolve_volume_id, PipeClient};
pub use pipe::{connect, PipeListener, PipeStream};
pub use server::serve_forever;
pub use sid::{current_user_sid, interactive_user_sid, pipe_acl_sddl, pipe_name};
