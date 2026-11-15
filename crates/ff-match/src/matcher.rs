//! 统一匹配 trait 与 T1 子串 / T3 模糊匹配器（DESIGN §8.1）。
//!
//! T2 `PinyinMatcher`（`pinyin.rs`）与 T3 `FuzzyMatcher` 实现同一 [`Matcher`]
//! trait，由 ff-core 的匹配分层（T1→T2→T3）调度。当前用自实现的精简拼音
//! 字典与有序子序列；生产库 `ib-pinyin` / `nucleo-matcher` 的替换条件见
//! `docs/DECISIONS.md` #70。

use std::ops::Range;

/// 匹配分层（DESIGN §8.1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// T1：子串 / 多词 AND（本轮实现；永远执行，最快）。
    Substring,
    /// T2：拼音（M5，`ib-pinyin`；仅遍历 `HAS_CJK` 条目）。
    Pinyin,
    /// T3：模糊（M5，`nucleo-matcher`）。
    Fuzzy,
}

/// 名字级匹配器统一 trait（DESIGN §3：ff-match 的职责）。
///
/// 约定：
/// - 匹配在**单个名字的 UTF-8 字节**上进行；大小写折叠由调用方（ff-core）
///   在构造匹配器前完成，匹配器本身不做大小写处理；
/// - needle 为空时不命中任何名字（空串不构成有效匹配；解析层不会产生空 term）。
pub trait Matcher: Send + Sync {
    /// 所属分层。
    fn tier(&self) -> Tier;

    /// `name` 中是否存在命中。
    fn matches(&self, name: &[u8]) -> bool;

    /// `name` 内全部命中区间（UTF-8 字节偏移，相对 `name` 起点，非重叠、升序）。
    fn find_ranges(&self, name: &[u8]) -> Vec<Range<u32>>;
}

/// T1 子串匹配器：`memchr::memmem` 封装（DESIGN §2.1：不自写 Boyer-Moore）。
#[derive(Clone, Debug)]
pub struct SubstringMatcher {
    needle: Vec<u8>,
}

impl SubstringMatcher {
    /// `needle` 必须已经过大小写折叠（ff-core 负责）。
    pub fn new(needle: &[u8]) -> Self {
        Self {
            needle: needle.to_vec(),
        }
    }

    /// 原始 needle 字节（已折叠）。
    pub fn needle(&self) -> &[u8] {
        &self.needle
    }
}

impl Matcher for SubstringMatcher {
    fn tier(&self) -> Tier {
        Tier::Substring
    }

    fn matches(&self, name: &[u8]) -> bool {
        !self.needle.is_empty() && memchr::memmem::find(name, &self.needle).is_some()
    }

    fn find_ranges(&self, name: &[u8]) -> Vec<Range<u32>> {
        if self.needle.is_empty() {
            return Vec::new();
        }
        let nlen = self.needle.len() as u32;
        memchr::memmem::Finder::new(&self.needle)
            .find_iter(name)
            .map(|p| p as u32..p as u32 + nlen)
            .collect()
    }
}

/// T3 模糊匹配器（DESIGN §8.1）。
///
/// 用**有序子序列**判定（字符序模糊，等价于 `nucleo-matcher` 的基本行为）：
/// `needle` 的字符按序出现在 `name` 中即命中。`vsc` 能在 `Visual Studio Code`
/// 中命中即此原理。命中位置用于高亮。生产可换用 `nucleo-matcher` 以获得更优的
/// 连续度评分与预过滤，但匹配语义一致。
///
/// 约定：与 `SubstringMatcher` 相同，`needle` 已由调用方大小写折叠。
#[derive(Clone, Debug)]
pub struct FuzzyMatcher {
    needle: Vec<u8>,
}

impl FuzzyMatcher {
    pub fn new(needle: &[u8]) -> Self {
        Self {
            needle: needle.to_vec(),
        }
    }

    /// 零分配的大小写不敏感子序列判定（T3 全表预过滤用，DESIGN §8.1）。
    ///
    /// `needle` 已折叠为 ASCII 小写；逐字节把 `name` 做 ASCII 小写比较，
    /// 避免为 200 万条名字各分配一份小写副本和命中位置向量。
    pub fn matches_folded(&self, name: &[u8]) -> bool {
        if self.needle.is_empty() {
            return false;
        }
        let mut ni = 0;
        for &b in name {
            if b.to_ascii_lowercase() == self.needle[ni] {
                ni += 1;
                if ni == self.needle.len() {
                    return true;
                }
            }
        }
        false
    }

    /// 返回命中的字节位置（升序）；非子序列返回 `None`。
    fn positions(&self, name: &[u8]) -> Option<Vec<usize>> {
        if self.needle.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        let mut ni = 0;
        for (i, &b) in name.iter().enumerate() {
            if b == self.needle[ni] {
                out.push(i);
                ni += 1;
                if ni == self.needle.len() {
                    return Some(out);
                }
            }
        }
        None
    }
}

impl Matcher for FuzzyMatcher {
    fn tier(&self) -> Tier {
        Tier::Fuzzy
    }

    fn matches(&self, name: &[u8]) -> bool {
        self.positions(name).is_some()
    }

    fn find_ranges(&self, name: &[u8]) -> Vec<Range<u32>> {
        match self.positions(name) {
            Some(p) => p
                .into_iter()
                .map(|i| (i as u32)..((i + 1) as u32))
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_is_substring() {
        assert_eq!(SubstringMatcher::new(b"ab").tier(), Tier::Substring);
    }

    #[test]
    fn empty_needle_matches_nothing() {
        let m = SubstringMatcher::new(b"");
        assert!(!m.matches(b"abc"));
        assert!(!m.matches(b""));
        assert!(m.find_ranges(b"abc").is_empty());
    }

    #[test]
    fn find_ranges_multiple_non_overlapping() {
        let m = SubstringMatcher::new(b"ab");
        assert_eq!(m.find_ranges(b"abxab"), vec![0..2, 3..5]);
        assert_eq!(
            SubstringMatcher::new(b"aa").find_ranges(b"aaaa"),
            vec![0..2, 2..4]
        );
    }

    #[test]
    fn matches_basic() {
        let m = SubstringMatcher::new(b"Cod");
        assert!(m.matches(b"Visual Studio Code"));
        // 匹配器本身不做大小写折叠（由调用方预处理）
        assert!(!m.matches(b"cod"));
        assert!(!m.matches(b""));
    }

    #[test]
    fn matches_utf8_bytes() {
        // 中文按 UTF-8 字节匹配
        let m = SubstringMatcher::new("搜索".as_bytes());
        assert!(m.matches("拼音搜索Everything".as_bytes()));
        assert!(!m.matches("拼听搜查".as_bytes()));
    }

    #[test]
    fn fuzzy_subsequence_vsc_hits_vsc() {
        // 金标准：vsc → Visual Studio Code。
        // 注意：匹配器按字节比较、大小写不折叠（由调用方用 names_lower 折叠），
        // 故此处用全小写串（与引擎 names_lower 一致）。
        let m = FuzzyMatcher::new(b"vsc");
        assert!(m.matches(b"visual studio code"));
        let r = m.find_ranges(b"visual studio code");
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn fuzzy_no_match_when_out_of_order() {
        let m = FuzzyMatcher::new(b"xyz");
        assert!(!m.matches(b"Visual Studio Code"));
    }

    #[test]
    fn fuzzy_empty_needle_no_match() {
        let m = FuzzyMatcher::new(b"");
        assert!(!m.matches(b"abc"));
    }
}
