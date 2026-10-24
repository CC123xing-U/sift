//! ff-match —— 子串 / 模糊 / 拼音匹配器封装（统一 trait，纯逻辑）。
//!
//! 本轮范围：T1 子串 / 多词 AND（DESIGN §8.1）；
//! 模糊（T3，nucleo-matcher）与拼音（T2，ib-pinyin）属 M5，不在本轮。
//!
//! # 平台约束（DESIGN §0.5）
//! 纯逻辑 crate：任何平台可编译、可单测；禁用 `unsafe`。
#![forbid(unsafe_code)]

pub mod matcher;
pub mod scan;

pub use matcher::{Matcher, SubstringMatcher, Tier};
pub use scan::scan_names;
