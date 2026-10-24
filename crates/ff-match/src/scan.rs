//! 名字 arena 的并行子串扫描（DESIGN §7.3-1）。
//!
//! 名字 arena 中条目以 `0x00` 分隔（`names` / `names_lower` 同布局）。
//! 本模块把 arena 按 `0x00` 边界切块交给 rayon 并行 `memmem`，
//! 命中偏移经二分 `name_off` 映射回 EntryId（`u32`）。

use rayon::prelude::*;
use std::ops::Range;

/// 每块的字节数目标（arena 按此粒度切块；块尾对齐到 `0x00` 之后，名字不跨块）。
const CHUNK_TARGET: usize = 64 * 1024;

/// 取消检查间隔（命中条数；DESIGN §7.3：长循环约每 4K 条检查一次）。
const CANCEL_CHECK_INTERVAL: usize = 4096;

/// 在 `haystack`（名字 arena）中并行查找 `needle`，返回命中的条目 id。
///
/// - `name_off`：各条目名字起点（升序），长度 = 条目数；命中偏移二分映射为 id。
/// - `cancelled`：协作取消回调（DESIGN §2.2-6），返回 true 时尽快收尾；
///   取消后返回**已完成的块**的部分结果（调用方应整体判定取消后丢弃）。
/// - 返回：命中的条目 id，**升序去重**（同一名字多次命中只记一次；
///   块边界对齐 `0x00`，名字不会跨块，故无需跨块去重）。
/// - `needle` 为空：语义上匹配所有条目（防御路径，解析层不会产生空 term）。
pub fn scan_names(
    haystack: &[u8],
    needle: &[u8],
    name_off: &[u32],
    cancelled: impl Fn() -> bool + Sync,
) -> Vec<u32> {
    if name_off.is_empty() {
        return Vec::new();
    }
    if needle.is_empty() {
        // 空 needle 匹配一切（防御路径）
        return (0..name_off.len() as u32).collect();
    }
    if cancelled() {
        return Vec::new();
    }
    let chunks = chunk_ranges(haystack);
    chunks
        .par_iter()
        .flat_map(|r| scan_chunk(haystack, needle, name_off, r, &cancelled))
        .collect()
}

/// 把 arena 切成不超过 [`CHUNK_TARGET`] 的块，块尾对齐到 `0x00` 分隔符之后
/// （保证任何名字都完整落在单个块内，命中不会跨块丢失）。
fn chunk_ranges(haystack: &[u8]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < haystack.len() {
        let mut end = (start + CHUNK_TARGET).min(haystack.len());
        if end < haystack.len() {
            match memchr::memchr(0, &haystack[end..]) {
                Some(p) => end += p + 1,
                None => end = haystack.len(),
            }
        }
        out.push(start..end);
        start = end;
    }
    out
}

/// 单块扫描：`memmem` 找全部命中 → 二分映射 EntryId → 去重（块内升序）。
fn scan_chunk<C: Fn() -> bool>(
    haystack: &[u8],
    needle: &[u8],
    name_off: &[u32],
    range: &Range<usize>,
    cancelled: &C,
) -> Vec<u32> {
    if cancelled() {
        return Vec::new();
    }
    let finder = memchr::memmem::Finder::new(needle);
    let chunk = &haystack[range.clone()];
    let mut out: Vec<u32> = Vec::new();
    let mut since_check = 0usize;
    for hit in finder.find_iter(chunk) {
        let id = offset_to_id(name_off, range.start + hit);
        if out.last() != Some(&id) {
            out.push(id);
        }
        since_check += 1;
        if since_check >= CANCEL_CHECK_INTERVAL {
            if cancelled() {
                return Vec::new();
            }
            since_check = 0;
        }
    }
    out
}

/// 命中字节偏移 → 条目 id（二分 `name_off`；偏移必落在某名字内部，
/// 因 needle 不含 `0x00`，不可能命中分隔符）。
fn offset_to_id(name_off: &[u32], offset: usize) -> u32 {
    debug_assert!(!name_off.is_empty());
    debug_assert!(offset >= name_off[0] as usize);
    // name_off[0] == 0 <= offset，故 partition_point 至少为 1
    (name_off.partition_point(|&o| (o as usize) <= offset) - 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 由名字列表构造 arena 与 name_off（与 IndexBuilder 的布局一致）。
    fn arena(names: &[&str]) -> (Vec<u8>, Vec<u32>) {
        let mut a = Vec::new();
        let mut off = Vec::new();
        for n in names {
            off.push(a.len() as u32);
            a.extend_from_slice(n.as_bytes());
            a.push(0);
        }
        (a, off)
    }

    #[test]
    fn finds_all_sorted_and_dedup() {
        let (a, off) = arena(&["alpha", "beta", "alpha", "gamma", "alphabet"]);
        // "alpha" 命中 0、2、4；"alpha" 在 4 号名字里只出现一次
        assert_eq!(scan_names(&a, b"alpha", &off, || false), vec![0, 2, 4]);
        // 同一名字多次命中只记一次
        let (a, off) = arena(&["xx", "abab", "yy"]);
        assert_eq!(scan_names(&a, b"ab", &off, || false), vec![1]);
    }

    #[test]
    fn no_match() {
        let (a, off) = arena(&["alpha", "beta"]);
        assert!(scan_names(&a, b"zzz", &off, || false).is_empty());
    }

    #[test]
    fn no_match_across_separator() {
        // 分隔符 0x00 两侧的 "a" 与 "b" 不构成 "ab"
        let (a, off) = arena(&["a", "b"]);
        assert!(scan_names(&a, b"ab", &off, || false).is_empty());
    }

    #[test]
    fn empty_needle_returns_all_defensive() {
        let (a, off) = arena(&["a", "b", "c"]);
        assert_eq!(scan_names(&a, b"", &off, || false), vec![0, 1, 2]);
    }

    #[test]
    fn empty_arena() {
        assert!(scan_names(b"", b"x", &[], || false).is_empty());
        assert!(scan_names(b"", b"", &[], || false).is_empty());
    }

    #[test]
    fn cancelled_before_start() {
        let (a, off) = arena(&["x"; 100]);
        assert!(scan_names(&a, b"x", &off, || true).is_empty());
    }

    #[test]
    fn cancelled_midway_returns_partial_or_empty() {
        // 第 3 次调用后取消：已完成的块保留、后续块为空（不 panic、可观测收尾）
        let (a, off) = arena(&["hit", "x", "x", "hit", "x", "hit"]);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let got = scan_names(&a, b"hit", &off, || {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            n > 2
        });
        assert!(got.len() <= 3);
    }

    #[test]
    fn spans_chunk_boundaries() {
        // 单字节/短名字 × 70000 ≈ 140KB arena → 至少 3 个 64KB 块；
        // 每 1000 个放一个 "hit"，覆盖块边界两侧。
        let names: Vec<String> = (0..70_000u32)
            .map(|i| {
                if i % 1000 == 0 {
                    "hit".to_string()
                } else {
                    "a".to_string()
                }
            })
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let (a, off) = arena(&refs);
        let want: Vec<u32> = (0..70_000u32).step_by(1000).collect();
        assert_eq!(scan_names(&a, b"hit", &off, || false), want);
    }

    #[test]
    fn unicode_names() {
        let (a, off) = arena(&["中文文件", "拼音", "search"]);
        assert_eq!(scan_names(&a, "文件".as_bytes(), &off, || false), vec![0]);
        assert_eq!(scan_names(&a, b"sea", &off, || false), vec![2]);
    }

    #[test]
    fn long_name_with_repeated_hits() {
        let (a, off) = arena(&[&"ab".repeat(3000), "x"]);
        // 3000×"ab"：命中 3000 次但同属条目 0
        assert_eq!(scan_names(&a, b"ab", &off, || false), vec![0]);
    }
}
