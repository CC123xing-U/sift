//! ff-engine —— 把 `ff-ntfs` / `ff-core` / `ff-match` 粘合为对外 [`api::EngineClient`]
//! （DESIGN §2.2-1、§9）。
//!
//! 已落地：`api` 契约 + `InProcess` 实现（`inprocess.rs`：卷管理、`search` / `range` /
//! `cancel` / `status` / `rescan` / `open`；Base+Overlay 增量层、后台合并、索引落盘 /
//! 载入、USN 增量轮询、启动复用 `startup_reconcile`），跨平台可编译。
//! `api::EngineStatus::last_saved` 在确有落盘索引时反映最近保存时间。
//!
//! 目录清理（`persist_cleanup.rs`）由桌面与服务两个后端共用。

pub mod actions;
pub mod api;
pub mod inprocess;
pub mod ipc;
pub mod overlay;
pub mod persist_cleanup;
pub mod timeconv;

pub use persist_cleanup::prune_index_dir;

pub use actions::{execute_open, execute_open_checked, full_path};
pub use api::{
    EngineClient, EngineError, EngineStatus, OpenAction, ParseError, RequestId, Result, ResultRow,
    SearchOptions, SearchRequest, SearchSummary, SortKey, SortSpec, VolumeId, VolumeState,
    VolumeStatus,
};
pub use overlay::{Overlay, OverlayEntry};

pub use inprocess::{EngineSession, InProcessEngine};
