//! ff-engine —— 把 `ff-ntfs` / `ff-core` / `ff-match` 粘合为对外 [`api::EngineClient`]
//! （DESIGN §2.2-1、§9）。
//!
//! 已落地：`api` 契约 + `InProcess` 实现（`inprocess.rs`：卷管理、`search` / `range` /
//! `cancel` / `status` / `rescan` / `open`，跨平台可编译）。
//!
//! 仍未落地（DESIGN §4.3 / §6 / §13-M2）：Base+Overlay 增量层、后台合并（compaction）、
//! 索引落盘与载入、USN 增量接线（`ff_ntfs::VolumeScanner::read_changes` 已实现，但本
//! crate 尚无调用方）。`api::EngineStatus::last_saved` 因此恒为 `None`。

pub mod api;
pub mod inprocess;
pub mod overlay;

pub use api::{
    EngineClient, EngineError, EngineStatus, OpenAction, ParseError, RequestId, Result, ResultRow,
    SearchOptions, SearchRequest, SearchSummary, SortKey, SortSpec, VolumeId, VolumeState,
    VolumeStatus,
};
pub use overlay::{Overlay, OverlayEntry};

pub use inprocess::InProcessEngine;
