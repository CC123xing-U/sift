//! 高亮区间（DESIGN §9 `ResultRow.name_matches`；M5：T1 子串 + T2 拼音 + T3 模糊）。
//!
//! 引擎在 `range()` 取数时对窗口内行调用本函数填 `name_matches`；
//! 区间为 UTF-8 **字节偏移**（相对名字起点），与 `names` arena 布局一致。

use std::ops::Range;

use ff_match::{FuzzyMatcher, Matcher, PinyinMatcher, SubstringMatcher};

use super::entry::{EntryId, VolumeIndex, NONE};
use super::query::Query;
use super::score::match_tokens;

/// 名字内的 M5 命中区间（Base 与 Overlay 增量条目共用同一套 T1/T2/T3 判定）。
///
/// 返回 UTF-8 **字节**偏移（相对名字起点），与 `names` arena 布局一致；
/// `fuzzy` / `pinyin` 取自本次搜索的 `MatchOptions`，保证高亮与命中集合同开关。
/// 引擎在 `range()` 构造 `ResultRow` 时再按 UTF-16 转换给前端。
pub fn highlight_name(name: &[u8], q: &Query, fuzzy: bool, pinyin: bool) -> Vec<Range<u32>> {
    if q.terms.is_empty() {
        return Vec::new();
    }
    let lowered = crate::case::simple_lowercase(name);
    let haystack: &[u8] = if q.case_sensitive { name } else { &lowered };
    let mut ranges: Vec<Range<u32>> = Vec::new();
    for t in &q.terms {
        let m = SubstringMatcher::new(&t.needle(q.case_sensitive));
        ranges.extend(m.find_ranges(haystack));
    }
    // 子串未命中 → 尝试拼音 / 模糊（仅单 token 纯字母查询，见 match_tokens）。
    if ranges.is_empty() {
        let mt = match_tokens(q);
        if let Some(n) = &mt.needle {
            if pinyin && mt.pinyin_eligible {
                ranges.extend(PinyinMatcher::new(n).find_ranges(name));
            }
            if fuzzy && n.chars().count() >= 2 {
                ranges.extend(FuzzyMatcher::new(n.as_bytes()).find_ranges(&lowered));
            }
        }
    }
    ranges.sort_by_key(|a| (a.start, a.end));
    ranges.dedup();
    ranges
}

impl VolumeIndex {
    /// 条目 `id` 名字内各 term 的命中区间（委托 [`highlight_name`]）。
    ///
    /// - T1 子串：各 term 区间合并、按 `(start, end)` 排序、完全相同区间去重；
    /// - 当 T1 无命中且查询为单 token 时，按 `pinyin` / `fuzzy` 开关追加
    ///   T2 拼音 / T3 模糊区间（M5，DESIGN §8.1）；
    /// - 空查询 / 无效 id → 空；
    /// - 大小写策略同 [`search`](VolumeIndex::search)（默认不敏感）。
    pub fn highlight_ranges(
        &self,
        id: EntryId,
        q: &Query,
        fuzzy: bool,
        pinyin: bool,
    ) -> Vec<Range<u32>> {
        if q.terms.is_empty() || id == NONE || id as usize >= self.entry_count() {
            return Vec::new();
        }
        highlight_name(self.name(id), q, fuzzy, pinyin)
    }
}

#[cfg(test)]
mod tests {
    use super::super::entry::attrs;
    use super::super::entry::{EntryInput, IndexBuilder, VolumeInfo};
    use super::super::parse;
    use super::*;

    fn info() -> VolumeInfo {
        VolumeInfo {
            drive: Some('C'),
            ..Default::default()
        }
    }

    fn dir(frn: u64, parent: u64, name: &str) -> EntryInput {
        EntryInput {
            frn,
            parent_frn: parent,
            name: name.to_string(),
            is_dir: true,
            attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
            size: 0,
            mtime: 1,
            ctime: 1,
            atime: 1,
        }
    }

    fn file(frn: u64, parent: u64, name: &str) -> EntryInput {
        EntryInput {
            is_dir: false,
            attrs: 0,
            ..dir(frn, parent, name)
        }
    }

    fn sample() -> VolumeIndex {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(100, 5, "README.txt"));
        b.push(file(101, 5, "abab-cdab"));
        b.push(file(102, 5, "中文文件"));
        b.finish()
    }

    fn q(s: &str) -> Query {
        parse(s).unwrap()
    }

    #[test]
    fn single_term_multiple_hits() {
        let ix = sample();
        // "abab-cdab"：a0 b1 a2 b3 -4 c5 d6 a7 b8
        let r = ix.highlight_ranges(2, &q("ab"), false, false);
        assert_eq!(r, vec![0..2, 2..4, 7..9]);
    }

    #[test]
    fn no_hit_empty() {
        let ix = sample();
        assert!(ix.highlight_ranges(1, &q("zzz"), false, false).is_empty());
    }

    #[test]
    fn empty_query_empty() {
        let ix = sample();
        assert!(ix
            .highlight_ranges(1, &Query::default(), false, false)
            .is_empty());
        assert!(ix.highlight_ranges(1, &q(""), false, false).is_empty());
    }

    #[test]
    fn invalid_id_empty() {
        let ix = sample();
        assert!(ix
            .highlight_ranges(NONE, &q("read"), false, false)
            .is_empty());
        assert!(ix
            .highlight_ranges(999, &q("read"), false, false)
            .is_empty());
    }

    #[test]
    fn case_insensitive_default() {
        let ix = sample();
        // 查询小写命中大写名字
        assert_eq!(ix.highlight_ranges(1, &q("read"), false, false), vec![0..4]);
        // 整名匹配（"README" 折成 "readme" 命中名字前 6 字节）
        assert_eq!(
            ix.highlight_ranges(1, &q("README"), false, false),
            vec![0..6]
        );
    }

    #[test]
    fn case_sensitive_uses_original() {
        let ix = sample();
        let mut cs = q("read");
        cs.case_sensitive = true;
        assert!(ix.highlight_ranges(1, &cs, false, false).is_empty());
        let mut cs2 = q("READ");
        cs2.case_sensitive = true;
        assert_eq!(ix.highlight_ranges(1, &cs2, false, false), vec![0..4]);
    }

    #[test]
    fn and_merges_ranges_sorted() {
        let ix = sample();
        // "abab-cdab"：term "ab" → 0..2, 2..4, 7..9；term "cd" → 5..7
        let r = ix.highlight_ranges(2, &q("cd ab"), false, false);
        assert_eq!(r, vec![0..2, 2..4, 5..7, 7..9]);
        // 完全相同区间去重
        let r2 = ix.highlight_ranges(2, &q("ab ab"), false, false);
        assert_eq!(r2, vec![0..2, 2..4, 7..9]);
    }

    #[test]
    fn utf8_byte_offsets() {
        let ix = sample();
        // "中文文件"：中文 2 字 = 6 字节，"文件" 从字节 6 开始
        let r = ix.highlight_ranges(3, &q("文件"), false, false);
        assert_eq!(r, vec![6..12]);
        let r = ix.highlight_ranges(3, &q("中文 文件"), false, false);
        assert_eq!(r, vec![0..6, 6..12]);
    }

    // ───────────────────── M5：T2 拼音 / T3 模糊高亮 ─────────────────────

    #[test]
    fn pinyin_highlight_zgr() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(101, 5, "中国人"));
        let ix = b.finish();
        // 中国人：zgr 拼音命中三个汉字，各 3 字节区间（0..3, 3..6, 6..9）。
        let r = ix.highlight_ranges(1, &q("zgr"), false, true);
        assert_eq!(r, vec![0..3, 3..6, 6..9]);
    }

    #[test]
    fn fuzzy_highlight_vsc() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(101, 5, "Visual Studio Code"));
        let ix = b.finish();
        // "Visual Studio Code" 高亮 v/s/c 三字节（先转小写后匹配，偏移一致）。
        let r = ix.highlight_ranges(1, &q("vsc"), true, false);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].start, 0); // 'V' / 'v'
    }

    #[test]
    fn substring_takes_precedence_over_fuzzy() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(101, 5, "report.txt"));
        let ix = b.finish();
        // "report" 在 report.txt 已有子串命中，不应再叠模糊区间。
        let r = ix.highlight_ranges(1, &q("report"), true, false);
        assert_eq!(r, vec![0..6]);
    }
}
