//! 高亮区间（DESIGN §9 `ResultRow.name_matches`；M5 前仅 T1 子串）。
//!
//! 引擎在 `range()` 取数时对窗口内行调用本函数填 `name_matches`；
//! 区间为 UTF-8 **字节偏移**（相对名字起点），与 `names` arena 布局一致。

use std::ops::Range;

use ff_match::{Matcher, SubstringMatcher};

use super::entry::{EntryId, VolumeIndex, NONE};
use super::query::Query;

impl VolumeIndex {
    /// 条目 `id` 名字内各 term 的命中区间。
    ///
    /// - AND 多 term：合并各 term 区间后按 `(start, end)` 排序；
    ///   **完全相同**的区间去重，**重叠**保留（DESIGN 任务说明）；
    /// - 空查询 / 无效 id → 空；
    /// - 大小写策略同 [`search`](VolumeIndex::search)（默认不敏感，
    ///   匹配小写副本；区间仍相对**原名**，偏移一致故可直接用）。
    pub fn highlight_ranges(&self, id: EntryId, q: &Query) -> Vec<Range<u32>> {
        if q.terms.is_empty() || id == NONE || id as usize >= self.entry_count() {
            return Vec::new();
        }
        let name = self.name(id);
        let lowered;
        let haystack: &[u8] = if q.case_sensitive {
            name
        } else {
            lowered = crate::case::simple_lowercase(name);
            &lowered
        };
        let mut ranges: Vec<Range<u32>> = Vec::new();
        for t in &q.terms {
            let m = SubstringMatcher::new(&t.needle(q.case_sensitive));
            ranges.extend(m.find_ranges(haystack));
        }
        ranges.sort_by_key(|a| (a.start, a.end));
        ranges.dedup();
        ranges
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
        let r = ix.highlight_ranges(2, &q("ab"));
        assert_eq!(r, vec![0..2, 2..4, 7..9]);
    }

    #[test]
    fn no_hit_empty() {
        let ix = sample();
        assert!(ix.highlight_ranges(1, &q("zzz")).is_empty());
    }

    #[test]
    fn empty_query_empty() {
        let ix = sample();
        assert!(ix.highlight_ranges(1, &Query::default()).is_empty());
        assert!(ix.highlight_ranges(1, &q("")).is_empty());
    }

    #[test]
    fn invalid_id_empty() {
        let ix = sample();
        assert!(ix.highlight_ranges(NONE, &q("read")).is_empty());
        assert!(ix.highlight_ranges(999, &q("read")).is_empty());
    }

    #[test]
    fn case_insensitive_default() {
        let ix = sample();
        // 查询小写命中大写名字
        assert_eq!(ix.highlight_ranges(1, &q("read")), vec![0..4]);
        // 整名匹配（"README" 折成 "readme" 命中名字前 6 字节）
        assert_eq!(ix.highlight_ranges(1, &q("README")), vec![0..6]);
    }

    #[test]
    fn case_sensitive_uses_original() {
        let ix = sample();
        let mut cs = q("read");
        cs.case_sensitive = true;
        assert!(ix.highlight_ranges(1, &cs).is_empty());
        let mut cs2 = q("READ");
        cs2.case_sensitive = true;
        assert_eq!(ix.highlight_ranges(1, &cs2), vec![0..4]);
    }

    #[test]
    fn and_merges_ranges_sorted() {
        let ix = sample();
        // "abab-cdab"：term "ab" → 0..2, 2..4, 7..9；term "cd" → 5..7
        let r = ix.highlight_ranges(2, &q("cd ab"));
        assert_eq!(r, vec![0..2, 2..4, 5..7, 7..9]);
        // 完全相同区间去重
        let r2 = ix.highlight_ranges(2, &q("ab ab"));
        assert_eq!(r2, vec![0..2, 2..4, 7..9]);
    }

    #[test]
    fn utf8_byte_offsets() {
        let ix = sample();
        // "中文文件"：中文 2 字 = 6 字节，"文件" 从字节 6 开始
        let r = ix.highlight_ranges(3, &q("文件"));
        assert_eq!(r, vec![6..12]);
        let r = ix.highlight_ranges(3, &q("中文 文件"));
        assert_eq!(r, vec![0..6, 6..12]);
    }
}
