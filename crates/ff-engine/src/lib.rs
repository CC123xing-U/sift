//! ff-engine —— 把 `ff-ntfs` / `ff-core` / `ff-match` 粘合为对外 [`api::EngineClient`]
//! （DESIGN §2.2-1、§9）。
//!
//! 本轮（第一轮）只冻结 `api` 契约；引擎实现（卷管理、Base+Overlay、后台合并、
//! 落盘调度、`InProcess` 实现）属 M2，不在本轮范围。

pub mod api;
pub mod inprocess;

pub use api::{
    EngineClient, EngineError, EngineStatus, OpenAction, ParseError, RequestId, Result, ResultRow,
    SearchOptions, SearchRequest, SearchSummary, SortKey, SortSpec, VolumeId, VolumeState,
    VolumeStatus,
};

pub use inprocess::InProcessEngine;
