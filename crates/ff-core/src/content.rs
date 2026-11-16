//! 内容搜索（DESIGN §7.2 M7 `content:` 族）—— 纯逻辑与 IO 抽象。
//!
//! `content:` 与文件名条件不同：它的候选集来自**磁盘文件内容**，代价比
//! 名字匹配高两三个数量级。因此本模块只提供三件东西，把 IO 交给调用方注入：
//!
//! 1. [`ContentSpec`]：一个 `content:` 条件的 needle 与大小写策略；
//! 2. [`ContentSource`]：打开并扫描一个文件的抽象（`ff-engine` 用 `std::fs` 实现，
//!    `ff-core` 单测用内存假实现，保持 DESIGN §0.5「零平台依赖 + 可单测」）；
//! 3. [`ContentFilter`]：候选集 → 命中集的过滤循环，含**硬上限**
//!    （[`MAX_CANDIDATES`] / [`MAX_FILE_BYTES`]）与跳过统计。
//!
//! 边界（DECISIONS #95）：
//! - 必须有名字级条件做候选集（否则等于读整卷），解析期就拒绝；
//! - 只在前向 AND 语义下有效（`!content:` / `a | content:b` 解析期拒绝）；
//! - 超过 [`MAX_FILE_BYTES`] 的文件、含 NUL 的二进制文件**跳过而不是报错**，
//!   统计进 [`ContentReport`]，由 UI 明确提示「结果可能不完整」。

use std::fmt;
use std::io::Read;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;
use crate::case::simple_lowercase_into;
use crate::entry::EntryId;
use crate::eval::contains;

/// 单个文件的内容扫描上限（32 MiB）。
///
/// 超过即跳过并计入 [`ContentReport::skipped_large`]：内容搜索是「找文件里的一段字」，
/// 而不是全文索引，读一整部蓝光镜像没有意义，还会把一次按键搜索拖到几十秒。
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// 一次内容搜索最多扫描的候选文件数（5000）。
///
/// `content:` 依赖名字条件先把候选压下来；候选仍有上万条时说明名字条件太宽
/// （例如只写了 `content:foo` 或 `file: content:foo`），此时扫前 5000 条并置
/// [`ContentReport::truncated`]，让 UI 提示用户补条件。
pub const MAX_CANDIDATES: usize = 5000;

/// 一次内容搜索最多读取的字节总量（256 MiB）。
///
/// 候选数与单文件上限之外还有一层：5000 个 30 MiB 的文件会读 150 GB，
/// 用户等到的只有「未响应」。按 SSD 顺序读 ~1 GB/s 计，256 MiB 约 0.3 s；
/// 机械盘上也落在数秒内。触顶即停并置 [`ContentReport::truncated`]。
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// 读文件的分块大小（64 KiB）。
pub const CHUNK: usize = 64 * 1024;

/// 一个 `content:` 条件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentSpec {
    /// 要查找的字节（原始大小写，折叠在 [`Matcher::new`] 里做）。
    pub needle: String,
    /// 在查询串中的字节区间（错误 / 提示定位用）。
    pub span: Range<usize>,
    /// 是否区分大小写（解析时的 `case:` 状态）。
    pub case_sensitive: bool,
}

impl ContentSpec {
    /// 空 needle（`content:` 裸写）在解析期已被丢弃，这里作为防御。
    pub fn is_empty(&self) -> bool {
        self.needle.is_empty()
    }
}

/// 单文件扫描结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentScan {
    /// 命中（读到 needle 即停，不读完整个文件）。
    Found,
    /// 读完了，没有命中。
    NotFound,
    /// 文件超过 [`MAX_FILE_BYTES`]，跳过。
    TooLarge,
    /// 含 NUL 字节，按二进制跳过。
    Binary,
    /// 打开 / 读取失败（权限、占用、路径失效）。
    Unreadable,
}

impl ContentScan {
    /// 是否命中。
    pub fn is_match(self) -> bool {
        matches!(self, ContentScan::Found)
    }
}

/// 内容源：把「打开文件并判断是否含 needle」抽象出来。
///
/// 实现必须**可被多线程共享**（`Sync`）；`ff-engine` 的实现是无状态的 `std::fs` 包装。
pub trait ContentSource: Sync {
    /// 扫描 `path` 是否包含 `spec.needle`。不得 panic。
    fn scan(&self, path: &str, spec: &ContentSpec) -> ContentScan;
}

/// 流式匹配器：跨块边界不丢命中。
///
/// 每次 [`feed`](Matcher::feed) 保留末尾 `needle.len() - 1` 字节与下一块拼接，
/// 因此 `needle` 跨 64 KiB 分块边界时仍能命中（单测覆盖）。
#[derive(Debug)]
pub struct Matcher {
    needle: Vec<u8>,
    case_sensitive: bool,
    /// 上一块末尾的重叠字节（已折叠）。
    carry: Vec<u8>,
    /// 拼接缓冲（carry + 本块，已折叠）。
    buf: Vec<u8>,
    /// 折叠临时缓冲（避免每块分配）。
    folded: Vec<u8>,
}

/// 喂一块数据的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    Match,
    Miss,
    /// 本块含 NUL → 判定二进制，停止扫描。
    Binary,
}

impl Matcher {
    /// 按 `spec` 建匹配器（needle 在此折叠一次）。
    pub fn new(spec: &ContentSpec) -> Self {
        let raw = spec.needle.as_bytes();
        let needle = if spec.case_sensitive {
            raw.to_vec()
        } else {
            let mut v = Vec::with_capacity(raw.len());
            simple_lowercase_into(raw, &mut v);
            v
        };
        Matcher {
            buf: Vec::with_capacity(CHUNK + needle.len()),
            folded: Vec::with_capacity(CHUNK),
            needle,
            case_sensitive: spec.case_sensitive,
            carry: Vec::new(),
        }
    }

    /// needle 的字节数（0 = 永不命中，调用方应提前判定）。
    pub fn needle_len(&self) -> usize {
        self.needle.len()
    }

    /// 喂一块数据。
    pub fn feed(&mut self, chunk: &[u8]) -> Feed {
        if chunk.is_empty() {
            return Feed::Miss;
        }
        if chunk.contains(&0) {
            return Feed::Binary;
        }
        if self.needle.is_empty() {
            return Feed::Miss;
        }
        self.buf.clear();
        self.buf.extend_from_slice(&self.carry);
        if self.case_sensitive {
            self.buf.extend_from_slice(chunk);
        } else {
            // 折叠进临时缓冲再拼（carry 已是折叠后的字节）。
            let mut folded = std::mem::take(&mut self.folded);
            simple_lowercase_into(chunk, &mut folded);
            self.buf.extend_from_slice(&folded);
            self.folded = folded;
        }
        if contains(&self.buf, &self.needle) {
            return Feed::Match;
        }
        let keep = self.needle.len().saturating_sub(1);
        self.carry.clear();
        if keep > 0 {
            let start = self.buf.len().saturating_sub(keep);
            let mut carry = std::mem::take(&mut self.carry);
            carry.extend_from_slice(&self.buf[start..]);
            self.carry = carry;
        }
        Feed::Miss
    }
}

/// 从流里读并匹配，最多读 `limit` 字节。
///
/// 读到的字节数超过 `limit` → [`ContentScan::TooLarge`]（调用方通常已按
/// 文件大小预判，这里是兜底：索引里的 size 可能过期）。
pub fn scan_stream<R: Read>(r: &mut R, spec: &ContentSpec, limit: u64) -> ContentScan {
    if spec.is_empty() {
        return ContentScan::NotFound;
    }
    let mut m = Matcher::new(spec);
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => return ContentScan::NotFound,
            Ok(n) => n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return ContentScan::Unreadable,
        };
        total += n as u64;
        if total > limit {
            return ContentScan::TooLarge;
        }
        match m.feed(&buf[..n]) {
            Feed::Match => return ContentScan::Found,
            Feed::Binary => return ContentScan::Binary,
            Feed::Miss => {}
        }
    }
}

/// 内容搜索的统计（随 `SearchSummary` 回传 UI，见 DECISIONS #95）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentReport {
    /// 生效的 `content:` 条件数（多个 = AND）。
    pub terms: usize,
    /// 名字条件给出的候选文件数（上限截断前）。
    pub candidates: u64,
    /// 实际打开扫描的文件数。
    pub scanned: u64,
    /// 命中数。
    pub matched: u64,
    /// 因超过 [`MAX_FILE_BYTES`] 跳过。
    pub skipped_large: u64,
    /// 因含 NUL 判定二进制跳过。
    pub skipped_binary: u64,
    /// 打不开 / 读失败。
    pub unreadable: u64,
    /// 目录条目（`content:` 只对文件生效）。
    pub skipped_dir: u64,
    /// 候选数或读取量触顶（[`MAX_CANDIDATES`] / [`MAX_TOTAL_BYTES`]），
    /// 只扫了前面一部分 —— 结果**不完整**。
    pub truncated: bool,
}

impl ContentReport {
    /// 是否发生了会影响完整性的跳过（UI 据此显示黄色提示）。
    pub fn incomplete(&self) -> bool {
        self.truncated || self.skipped_large > 0 || self.skipped_binary > 0 || self.unreadable > 0
    }

    fn add_scan(&mut self, s: ContentScan) {
        match s {
            ContentScan::Found => {
                self.scanned += 1;
                self.matched += 1;
            }
            ContentScan::NotFound => self.scanned += 1,
            ContentScan::TooLarge => self.skipped_large += 1,
            ContentScan::Binary => self.skipped_binary += 1,
            ContentScan::Unreadable => self.unreadable += 1,
        }
    }
}

impl fmt::Display for ContentReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "内容搜索：候选 {} 个 / 扫描 {} 个，命中 {}",
            self.candidates, self.scanned, self.matched
        )?;
        let mut skips: Vec<String> = Vec::new();
        if self.skipped_large > 0 {
            skips.push(format!("{} 个过大", self.skipped_large));
        }
        if self.skipped_binary > 0 {
            skips.push(format!("{} 个二进制", self.skipped_binary));
        }
        if self.unreadable > 0 {
            skips.push(format!("{} 个读取失败", self.unreadable));
        }
        if !skips.is_empty() {
            write!(f, "；跳过 {}", skips.join("、"))?;
        }
        if self.truncated {
            write!(f, "；候选数或读取量达上限，结果可能不完整")?;
        }
        Ok(())
    }
}

/// 候选集 → 命中集的过滤器。
///
/// 调用方提供「条目 → 完整路径」与「条目 → 大小」两个闭包：
/// `path_of` 返回 `None` 表示该条目不是可读文件（目录 / 无路径），计入
/// `skipped_dir` 并排除；`size_of` 返回 `None` 时不做大小预判，交给
/// [`ContentSource`] 自己判断。
pub struct ContentFilter<'a> {
    pub specs: &'a [ContentSpec],
    pub source: &'a dyn ContentSource,
    /// 最多扫多少个候选（默认 [`MAX_CANDIDATES`]）。
    pub max_candidates: usize,
    /// 最多读多少字节（默认 [`MAX_TOTAL_BYTES`]）。
    pub max_total_bytes: u64,
    /// 单文件上限（默认 [`MAX_FILE_BYTES`]）。
    pub max_file_bytes: u64,
}

impl<'a> ContentFilter<'a> {
    /// 用默认上限构造。
    pub fn new(specs: &'a [ContentSpec], source: &'a dyn ContentSource) -> Self {
        ContentFilter {
            specs,
            source,
            max_candidates: MAX_CANDIDATES,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_file_bytes: MAX_FILE_BYTES,
        }
    }

    /// 过滤候选（**保序**：输入升序则输出升序）。
    ///
    /// 串行执行：内容扫描是磁盘 IO，并行打开上千文件在机械盘上是负优化，
    /// 且会让取消响应变慢（每文件检查一次取消令牌已经足够快）。
    pub fn filter<F, G>(
        &self,
        ids: &[EntryId],
        path_of: F,
        size_of: G,
        cancel: &CancelToken,
    ) -> (Vec<EntryId>, ContentReport)
    where
        F: Fn(EntryId) -> Option<String>,
        G: Fn(EntryId) -> Option<u64>,
    {
        let mut report = ContentReport {
            terms: self.specs.len(),
            candidates: ids.len() as u64,
            ..ContentReport::default()
        };
        if self.specs.is_empty() {
            return (ids.to_vec(), report);
        }
        let mut out: Vec<EntryId> = Vec::new();
        // 读取量预算：按索引里的 size 估算（未知则不计），触顶即停。
        let mut budget: u64 = 0;
        for (i, &id) in ids.iter().enumerate() {
            if i >= self.max_candidates {
                report.truncated = true;
                break;
            }
            if budget >= self.max_total_bytes {
                report.truncated = true;
                break;
            }
            if cancel.is_cancelled() {
                return (Vec::new(), report);
            }
            let Some(path) = path_of(id) else {
                report.skipped_dir += 1;
                continue;
            };
            if let Some(size) = size_of(id) {
                if size > self.max_file_bytes {
                    report.skipped_large += 1;
                    continue;
                }
                budget = budget.saturating_add(size);
            }
            // 多个 `content:` 是 AND：任一条 NotFound 即可短路；跳过原因
            // 单独记下来，避免为了统计再读一遍文件（大文件二次读代价高）。
            let mut hit = true;
            let mut skip: Option<ContentScan> = None;
            for s in self.specs {
                match self.source.scan(&path, s) {
                    ContentScan::Found => {}
                    ContentScan::NotFound => {
                        hit = false;
                        break;
                    }
                    other => {
                        hit = false;
                        skip = Some(other);
                        break;
                    }
                }
            }
            match skip {
                Some(s) => report.add_scan(s),
                None => {
                    if hit {
                        out.push(id);
                    }
                    report.add_scan(if hit {
                        ContentScan::Found
                    } else {
                        ContentScan::NotFound
                    });
                }
            }
        }
        (out, report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(needle: &str) -> ContentSpec {
        ContentSpec {
            needle: needle.to_string(),
            span: 0..needle.len(),
            case_sensitive: false,
        }
    }

    #[test]
    fn matcher_cross_chunk_boundary() {
        // needle 被 4 字节一块切成两半，仍必须命中。
        let mut m = Matcher::new(&spec("needle"));
        assert_eq!(m.feed(b"xxne"), Feed::Miss);
        assert_eq!(m.feed(b"edle"), Feed::Match);
    }

    #[test]
    fn matcher_case_insensitive_by_default() {
        let mut m = Matcher::new(&spec("Hello"));
        assert_eq!(m.feed(b"...hELLo..."), Feed::Match);
    }

    #[test]
    fn matcher_case_sensitive_when_asked() {
        let s = ContentSpec {
            case_sensitive: true,
            ..spec("Hello")
        };
        let mut m = Matcher::new(&s);
        assert_eq!(m.feed(b"hello"), Feed::Miss);
        let mut m2 = Matcher::new(&s);
        assert_eq!(m2.feed(b"Hello"), Feed::Match);
    }

    #[test]
    fn matcher_detects_binary() {
        let mut m = Matcher::new(&spec("abc"));
        assert_eq!(m.feed(b"a\0c"), Feed::Binary);
    }

    #[test]
    fn matcher_cjk_needle() {
        let mut m = Matcher::new(&spec("季度报告"));
        assert_eq!(m.feed("2024 年度季度报告初稿".as_bytes()), Feed::Match);
    }

    struct MemSource(Vec<(String, ContentScan)>);

    impl ContentSource for MemSource {
        fn scan(&self, path: &str, _spec: &ContentSpec) -> ContentScan {
            self.0
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, s)| *s)
                .unwrap_or(ContentScan::Unreadable)
        }
    }

    fn ids(n: usize) -> Vec<EntryId> {
        (0..n as EntryId).collect()
    }

    #[test]
    fn filter_keeps_order_and_counts_skips() {
        let specs = vec![spec("x")];
        let src = MemSource(vec![
            ("a".into(), ContentScan::Found),
            ("b".into(), ContentScan::NotFound),
            ("c".into(), ContentScan::Binary),
            ("d".into(), ContentScan::TooLarge),
        ]);
        let f = ContentFilter::new(&specs, &src);
        let token = CancelToken::new();
        let (hit, rep) = f.filter(
            &ids(5),
            |id| {
                Some(
                    match id {
                        0 => "a",
                        1 => "b",
                        2 => "c",
                        3 => "d",
                        _ => "dir",
                    }
                    .to_string(),
                )
            },
            |_| None,
            &token,
        );
        assert_eq!(hit, vec![0]);
        assert_eq!(rep.scanned, 2);
        assert_eq!(rep.matched, 1);
        assert_eq!(rep.skipped_binary, 1);
        assert_eq!(rep.skipped_large, 1);
        assert!(rep.incomplete());
    }

    #[test]
    fn filter_truncates_candidates() {
        let specs = vec![spec("x")];
        let src = MemSource(Vec::new());
        let mut f = ContentFilter::new(&specs, &src);
        f.max_candidates = 2;
        let token = CancelToken::new();
        let (hit, rep) = f.filter(&ids(10), |_| Some("p".into()), |_| None, &token);
        assert!(hit.is_empty());
        assert!(rep.truncated);
        assert_eq!(rep.scanned, 0);
        assert_eq!(rep.unreadable, 2);
    }

    #[test]
    fn filter_respects_size_and_cancel() {
        let specs = vec![spec("x")];
        let src = MemSource(vec![("a".into(), ContentScan::Found)]);
        let f = ContentFilter::new(&specs, &src);
        let token = CancelToken::new();
        // size 超限 → 直接跳过，不打开文件
        let (hit, rep) = f.filter(&ids(1), |_| Some("a".into()), |_| Some(u64::MAX), &token);
        assert!(hit.is_empty());
        assert_eq!(rep.skipped_large, 1);
        // 取消 → 空结果
        token.cancel();
        let (hit2, _) = f.filter(&ids(1), |_| Some("a".into()), |_| None, &token);
        assert!(hit2.is_empty());
    }

    struct NotFoundSource;

    impl ContentSource for NotFoundSource {
        fn scan(&self, _path: &str, _spec: &ContentSpec) -> ContentScan {
            ContentScan::NotFound
        }
    }

    #[test]
    fn filter_stops_at_byte_budget() {
        // 每个候选 10 字节、预算 25 字节 → 进入第 4 个之前触顶，实际扫 3 个。
        let specs = vec![spec("x")];
        let src = NotFoundSource;
        let mut f = ContentFilter::new(&specs, &src);
        f.max_total_bytes = 25;
        let token = CancelToken::new();
        let (_hit, rep) = f.filter(&ids(5), |id| Some(format!("p{id}")), |_| Some(10), &token);
        assert!(rep.truncated, "预算触顶应标记不完整");
        assert_eq!(rep.scanned, 3, "0/10/20 时都在预算内，30 时停");
        assert_eq!(rep.candidates, 5);
        assert!(rep.incomplete());
    }

    #[test]
    fn scan_stream_size_limit() {
        let data = vec![b'a'; 100];
        let mut cur = std::io::Cursor::new(data);
        assert_eq!(
            scan_stream(&mut cur, &spec("zzz"), 10),
            ContentScan::TooLarge
        );
        let mut cur2 = std::io::Cursor::new(b"hello world".to_vec());
        assert_eq!(
            scan_stream(&mut cur2, &spec("world"), 1024),
            ContentScan::Found
        );
    }

    #[test]
    fn report_display_mentions_skips() {
        let r = ContentReport {
            terms: 1,
            candidates: 10,
            scanned: 8,
            matched: 2,
            skipped_binary: 2,
            truncated: true,
            ..ContentReport::default()
        };
        let s = r.to_string();
        assert!(s.contains("扫描 8 个"));
        assert!(s.contains("2 个二进制"));
        assert!(s.contains("达上限"));
    }
}
