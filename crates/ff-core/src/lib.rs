//! ff-core —— Sift 引擎核心（纯逻辑，零平台依赖）。
//!
//! 职责见 `docs/DESIGN.md` 第 4、6、7、8 节：条目表（SoA）与名字 arena、
//! 路径现算、查询解析与求值、匹配分层与评分、持久化格式。
//!
//! 本轮范围：第 4.1/4.2 节数据模型与路径、第 7 节查询子集
//! （子串 / 多词 AND）、第 8.1 节 T1 分层；评分（§8.2）、T2/T3（M5）、
//! 完整语法（M4）不在本轮。持久化格式见 `persist` 模块（M2-① 已实现）。
//!
//! # 平台约束（DESIGN §0.5）
//! 本 crate 不得引用任何 Windows 专有 API，必须在所有平台可编译、可单测；
//! 因此禁用 `unsafe`（确需使用时向 Orchestrator 提交 `docs/REQUESTS.md`）。
#![forbid(unsafe_code)]

pub mod cancel;
pub mod case;
pub mod entry;
pub mod highlight;
pub mod path;
pub mod persist;
pub mod query;
pub mod search;

pub use cancel::CancelToken;
pub use case::simple_lowercase;
pub use entry::{attrs, flags, EntryId, EntryInput, IndexBuilder, VolumeIndex, VolumeInfo, NONE};
pub use query::{parse, Query, Term};

use std::fmt;
use std::ops::Range;

/// 查询解析错误（DESIGN §7.3：错误必须带位置信息，UI 可展示但不崩溃）。
///
/// 跨 crate 契约类型（`ff-engine` 的 `SearchSummary::parse_error` 引用它），
/// 变更只能由 Orchestrator 进行并记录到 `docs/DECISIONS.md`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// 人类可读的错误描述（中文，面向 UI 展示）。
    pub message: String,
    /// 错误在查询串中的字节区间 `[start, end)`。
    pub span: Range<usize>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (位置 {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for ParseError {}
