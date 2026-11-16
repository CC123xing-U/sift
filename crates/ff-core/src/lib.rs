//! ff-core —— Sift 引擎核心（纯逻辑，零平台依赖）。
//!
//! 职责见 `docs/DESIGN.md` 第 4、6、7、8 节：条目表（SoA）与名字 arena、
//! 路径现算、查询解析与求值、匹配分层与评分、持久化格式。
//!
//! 当前范围：第 4.1/4.2 节数据模型与路径、第 7 节完整查询语法（M4 AST）、
//! 第 8.1 节 T1/T2/T3 分层匹配与第 8.2 节评分（M5）、第 6 节 FFIX 持久化
//! （M2-①）。§8.3 的用户规则 / PriorFolders 仍留接口、未实现。
//!
//! # 平台约束（DESIGN §0.5）
//! 本 crate 不得引用任何 Windows 专有 API，必须在所有平台可编译、可单测；
//! 因此禁用 `unsafe`（确需使用时向 Orchestrator 提交 `docs/REQUESTS.md`）。
#![forbid(unsafe_code)]

pub mod cancel;
pub mod case;
pub mod entry;
pub mod eval;
pub mod highlight;
pub mod import;
pub mod path;
pub mod persist;
pub mod query;
pub mod score;
pub mod search;

pub use cancel::CancelToken;
pub use case::simple_lowercase;
pub use entry::{attrs, flags, EntryId, EntryInput, IndexBuilder, VolumeIndex, VolumeInfo, NONE};
pub use ff_match::Tier;
pub use highlight::highlight_name;
pub use query::{parse, Query, Term};
pub use score::{
    match_overlay_name, match_tokens, score_name_bytes, MatchOptions, RankedHit, RankedResult,
    ScoreConfig,
};

use std::fmt;
use std::ops::Range;

use serde::{Deserialize, Serialize};

/// `std::ops::Range` 无内置 serde 实现，且 IPC 契约要求 `{start, end}` 对象形态
/// （TS 侧 `ParseError.span` 为 `{ start, end }`）。用本地 wire 适配器序列化，
/// 保持 Rust `Range` 类型与冻结契约不变。
mod serde_range {
    use super::Range;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wire<T> {
        start: T,
        end: T,
    }

    pub fn serialize<T, S>(r: &Range<T>, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize + Copy,
        S: Serializer,
    {
        Wire {
            start: r.start,
            end: r.end,
        }
        .serialize(s)
    }

    pub fn deserialize<'de, T, D>(d: D) -> Result<Range<T>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        let w = Wire::<T>::deserialize(d)?;
        Ok(w.start..w.end)
    }
}

/// 查询解析错误（DESIGN §7.3：错误必须带位置信息，UI 可展示但不崩溃）。
///
/// 跨 crate 契约类型（`ff-engine` 的 `SearchSummary::parse_error` 引用它），
/// 变更只能由 Orchestrator 进行并记录到 `docs/DECISIONS.md`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseError {
    /// 人类可读的错误描述（中文，面向 UI 展示）。
    pub message: String,
    /// 错误在查询串中的字节区间 `[start, end)`。
    #[serde(with = "serde_range")]
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
