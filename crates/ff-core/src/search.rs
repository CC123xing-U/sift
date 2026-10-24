//! 查询求值（DESIGN §7.3）：arena 并行 `memmem` + AND 稀有词优先。

use ff_match::{scan_names, Matcher, SubstringMatcher};
use rayon::prelude::*;

use super::cancel::CancelToken;
use super::entry::{EntryId, VolumeIndex};
use super::query::Query;

/// AND 候选校验的并行批次大小（与取消检查间隔一致，DESIGN §7.3：约 4K）。
const VERIFY_CHUNK: usize = 4096;

impl VolumeIndex {
    /// 求值查询，返回命中条目 id（**升序去重**）。
    ///
    /// - 空查询（无 term）→ 空结果，不 panic；
    /// - 取消（`cancel` 已触发）→ 空结果（不返回半成品）；
    /// - 单 term：arena 并行 `memmem`，命中偏移二分 `name_off` 映射
    ///   （DESIGN §7.3-1）；
    /// - 多 term（AND）：**最长的 term**（稀有词启发式）先出候选集，
    ///   其余 term 只在候选上校验（DESIGN §7.3-2）；
    /// - 大小写：默认不敏感（匹配 `names_lower`）；`Query::case_sensitive`
    ///   为 true 时匹配原文 `names`。
    pub fn search(&self, q: &Query, cancel: &CancelToken) -> Vec<EntryId> {
        if q.terms.is_empty() || cancel.is_cancelled() {
            return Vec::new();
        }
        let candidates = self.driver_candidates(q, cancel);
        if cancel.is_cancelled() {
            return Vec::new(); // 取消：丢弃半成品候选
        }
        if q.terms.len() == 1 {
            return candidates;
        }
        let out = self.verify_and(candidates, q, cancel);
        if cancel.is_cancelled() {
            Vec::new() // 取消：丢弃部分校验结果
        } else {
            out
        }
    }

    /// 驱动词（最长 term，稀有词启发式）的候选集（DESIGN §7.3-2）。
    pub(crate) fn driver_candidates(&self, q: &Query, cancel: &CancelToken) -> Vec<EntryId> {
        let driver = driver_index(q);
        let needle = q.terms[driver].needle(q.case_sensitive);
        scan_names(
            self.haystack(q.case_sensitive),
            &needle,
            &self.name_off,
            || cancel.is_cancelled(),
        )
    }

    /// 在候选集上校验其余 term（并行；保序）。
    fn verify_and(
        &self,
        candidates: Vec<EntryId>,
        q: &Query,
        cancel: &CancelToken,
    ) -> Vec<EntryId> {
        let driver = driver_index(q);
        let matchers: Vec<SubstringMatcher> = q
            .terms
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != driver)
            .map(|(_, t)| SubstringMatcher::new(&t.needle(q.case_sensitive)))
            .collect();
        let haystack = self.haystack(q.case_sensitive);
        candidates
            .par_chunks(VERIFY_CHUNK)
            .flat_map(|chunk| {
                if cancel.is_cancelled() {
                    return Vec::new();
                }
                chunk
                    .iter()
                    .copied()
                    .filter(|&id| {
                        let name = self.name_in(haystack, id);
                        matchers.iter().all(|m| m.matches(name))
                    })
                    .collect::<Vec<EntryId>>()
            })
            .collect()
    }
}

/// 驱动词下标：字节数最长的 term（稀有词启发式；并列取最先）。
fn driver_index(q: &Query) -> usize {
    let mut driver = 0usize;
    for (i, t) in q.terms.iter().enumerate() {
        if t.text.len() > q.terms[driver].text.len() {
            driver = i;
        }
    }
    driver
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
        b.push(dir(100, 5, "Windows"));
        b.push(file(101, 100, "readme.txt"));
        b.push(dir(200, 5, "用户"));
        b.push(file(201, 200, "文档.txt"));
        b.push(file(202, 5, "README.MD"));
        b.finish()
    }

    fn q(s: &str) -> Query {
        parse(s).unwrap()
    }

    #[test]
    fn empty_query_empty_result() {
        let ix = sample();
        let empty = Query::default();
        assert!(ix.search(&empty, &CancelToken::new()).is_empty());
        assert!(ix.search(&q(""), &CancelToken::new()).is_empty());
        assert!(ix.search(&q("   "), &CancelToken::new()).is_empty());
        assert!(ix.search(&q("\"\""), &CancelToken::new()).is_empty());
    }

    #[test]
    fn substring_basic_sorted_dedup() {
        let ix = sample();
        // "read" 命中 readme.txt(2) 与 README.MD(5)（默认不区分大小写）
        assert_eq!(ix.search(&q("read"), &CancelToken::new()), vec![2, 5]);
        // 同一名字多次命中只记一次
        assert_eq!(ix.search(&q("e"), &CancelToken::new()), vec![2, 5]);
        assert_eq!(ix.search(&q("zzz"), &CancelToken::new()), Vec::<u32>::new());
    }

    #[test]
    fn unicode_names_searchable() {
        let ix = sample();
        assert_eq!(ix.search(&q("用户"), &CancelToken::new()), vec![3]);
        assert_eq!(ix.search(&q("文档"), &CancelToken::new()), vec![4]);
        // "txt" 命中 readme.txt(2) 与 文档.txt(4)
        assert_eq!(ix.search(&q("txt"), &CancelToken::new()), vec![2, 4]);
    }

    #[test]
    fn case_sensitivity() {
        let ix = sample();
        // 默认不敏感：大写查询命中 / 小写命中大写名
        assert_eq!(ix.search(&q("README"), &CancelToken::new()), vec![2, 5]);
        // 敏感：只命中原文
        let mut cs = q("README");
        cs.case_sensitive = true;
        assert_eq!(ix.search(&cs, &CancelToken::new()), vec![5]);
        let mut cs2 = q("readme");
        cs2.case_sensitive = true;
        assert_eq!(ix.search(&cs2, &CancelToken::new()), vec![2]);
    }

    #[test]
    fn and_requires_all_terms() {
        let ix = sample();
        assert_eq!(ix.search(&q("readme txt"), &CancelToken::new()), vec![2]);
        assert_eq!(ix.search(&q("readme md"), &CancelToken::new()), vec![5]);
        assert!(ix.search(&q("readme zzz"), &CancelToken::new()).is_empty());
        // 三词 AND：readme.txt 同时含 read / txt / me
        assert_eq!(ix.search(&q("read txt me"), &CancelToken::new()), vec![2]);
        // 三词 AND：缺一个即无命中
        assert!(ix.search(&q("read txt zz"), &CancelToken::new()).is_empty());
    }

    #[test]
    fn and_rare_word_first_prunes_candidates() {
        // 常见词 "a" 出现在全部名字；稀有词 "zz" 只在 1 个名字。
        // driver_candidates 必须只含稀有词命中（候选集被裁剪），
        // 而不是先扫出全部 "a" 的命中。
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(1, 5, "alpha"));
        b.push(file(2, 5, "zz-rare-a"));
        b.push(file(3, 5, "beta"));
        let ix = b.finish();
        let query = q("a zz");
        let candidates = ix.driver_candidates(&query, &CancelToken::new());
        assert_eq!(candidates, vec![2], "候选集应只含最长（最稀有）词的命中");
        assert_eq!(ix.search(&query, &CancelToken::new()), vec![2]);
    }

    #[test]
    fn and_driver_is_longest_term() {
        let ix = sample();
        // "readme.txt"(2) 含 "read" 与 "txt"；"README.MD"(5) 含 "read" 与 "md"
        let query = q("read md");
        // driver = "read"(4) 与 "md"(2) 并列时取先者；这里最长是 "read"
        assert_eq!(ix.search(&query, &CancelToken::new()), vec![5]);
        let query = q("readme md");
        assert_eq!(ix.search(&query, &CancelToken::new()), vec![5]);
    }

    #[test]
    fn cancel_before_search_returns_empty() {
        let ix = sample();
        let token = CancelToken::new();
        token.cancel();
        assert!(ix.search(&q("read"), &token).is_empty());
        assert!(ix.search(&q("a b"), &token).is_empty());
    }

    #[test]
    fn cancel_token_reset_allows_new_search() {
        let ix = sample();
        let token = CancelToken::new();
        token.cancel();
        assert!(ix.search(&q("read"), &token).is_empty());
        token.reset();
        assert_eq!(ix.search(&q("read"), &token), vec![2, 5]);
    }

    #[test]
    fn results_sorted_across_many_chunks() {
        // 12000 个名字（arena > 64KB，跨多块）：结果须全局升序
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        for i in 0..12_000u32 {
            b.push(file(1000 + i as u64, 5, &format!("item{i:05}-needle")));
        }
        // 每 7 个加一个干扰名
        for i in (0..12_000u32).step_by(7) {
            b.push(file(500_000 + i as u64, 5, &format!("item{i:05}-other")));
        }
        let ix = b.finish();
        let hits = ix.search(&q("needle"), &CancelToken::new());
        assert_eq!(hits.len(), 12_000);
        let mut sorted = hits.clone();
        sorted.sort_unstable();
        assert_eq!(hits, sorted, "跨块结果必须全局升序");
        assert_eq!(hits[0], 1);
        assert_eq!(*hits.last().unwrap(), 12_000);
    }

    #[test]
    fn and_over_many_chunks() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        for i in 0..12_000u32 {
            b.push(file(1000 + i as u64, 5, &format!("item{i:05}-needle-x")));
        }
        for i in (0..12_000u32).step_by(3) {
            b.push(file(600_000 + i as u64, 5, &format!("item{i:05}-needle-y")));
        }
        let ix = b.finish();
        let hits = ix.search(&q("needle x"), &CancelToken::new());
        assert_eq!(hits.len(), 12_000);
        let mut sorted = hits.clone();
        sorted.sort_unstable();
        assert_eq!(hits, sorted);
    }

    #[test]
    fn and_verify_after_common_driver() {
        // driver（最长词）很常见、另一词很稀有：校验必须过滤干净
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        for i in 0..500u32 {
            b.push(file(10 + i as u64, 5, &format!("common{i}")));
        }
        b.push(file(9999, 5, "common-unique"));
        let ix = b.finish();
        assert_eq!(
            ix.search(&q("common unique"), &CancelToken::new()),
            vec![501]
        );
    }
}
