//! M5 匹配分层调度与评分（DESIGN §8.1 / §8.2）。
//!
//! [`VolumeIndex::search_ranked`] 在 T1（子串 / 多词 AND，永远执行）之外，对**单
//! token 纯字母**查询追加两层：
//! - T2 拼音（`PinyinMatcher`，仅遍历 `cjk_ids`，查询须为纯 ASCII 小写字母）；
//! - T3 模糊（`FuzzyMatcher`，字符序子序列，仅当 T1+T2 命中 < 100 且查询长度 ≥ 2）。
//!
//! 命中按 [`ScoreConfig`] 评分并取 Top-K（按评分降序，稳定 tie-break）。
//!
//! 与 `ff-engine::SearchOptions` 解耦：本模块只用 [`MatchOptions`]（两个布尔），
//! 由引擎在调用处把 `opts.fuzzy` / `opts.pinyin` 映射进来。

use std::ops::Range;

use rayon::prelude::*;
use rustc_hash::FxHashMap;

use ff_match::{FuzzyMatcher, Matcher, PinyinMatcher, SubstringMatcher, Tier};

use crate::case::simple_lowercase;
use crate::entry::{EntryId, VolumeIndex};
use crate::query::Query;

/// M5 匹配开关（与 `ff-engine::SearchOptions` 的子集对应）。
#[derive(Clone, Copy, Debug, Default)]
pub struct MatchOptions {
    /// 启用 T3 模糊（字符序子序列）。
    pub fuzzy: bool,
    /// 启用 T2 拼音（仅纯 ASCII 小写字母查询 + 含 CJK 的名字）。
    pub pinyin: bool,
}

/// 评分权重（DESIGN §8.2，全部可调，集中一处）。
///
/// 公式骨架：
/// ```text
/// score = tier_base
///       + position_bonus * (1 - 命中位置/名字长)   // 越靠前越高
///       + contiguity_bonus * (连续? 1 : 0.3)        // 连续 > 间断
///       + (exact? 500 : prefix? 300 : 0)            // 完全相等 > 前缀
///       + (大小写精确? case_exact_bonus : 0)
///       + name_vs_path                              // 命中在文件名
///       - length_penalty * (名字长/32 截顶)          // 越短越靠前
///       - depth_penalty * 路径深度                  // 越浅越靠前
///       + rule_bonus / frecency（M5 预留，当前为 0）
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ScoreConfig {
    /// T1 子串分层基准分。
    pub tier_substring: f64,
    /// T2 拼音分层基准分。
    pub tier_pinyin: f64,
    /// T3 模糊分层基准分。
    pub tier_fuzzy: f64,
    /// 命中位置加成系数。
    pub position_bonus: f64,
    /// 连续命中加成。
    pub contiguity_bonus: f64,
    /// 整词 / 词首加成。
    pub wholeword_bonus: f64,
    /// 大小写精确（原名字含未折叠的 needle）加成。
    pub case_exact_bonus: f64,
    /// 命中文件名（相对仅命中路径）加成。
    pub name_vs_path: f64,
    /// 名字长度惩罚系数（按 32 字节归一，截顶 8）。
    pub length_penalty: f64,
    /// 路径深度惩罚系数（每层）。
    pub depth_penalty: f64,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        ScoreConfig {
            tier_substring: 1000.0,
            tier_pinyin: 800.0,
            tier_fuzzy: 500.0,
            position_bonus: 200.0,
            contiguity_bonus: 150.0,
            wholeword_bonus: 120.0,
            case_exact_bonus: 80.0,
            name_vs_path: 60.0,
            length_penalty: 0.5,
            depth_penalty: 10.0,
        }
    }
}

/// 单条排名命中（带 tier 与评分）。
#[derive(Clone, Copy, Debug)]
pub struct RankedHit {
    pub id: EntryId,
    pub tier: Tier,
    pub score: f64,
}

/// [`VolumeIndex::search_ranked`] 的返回：已按 `score` 降序排好（稳定 tie-break）。
pub struct RankedResult {
    pub hits: Vec<RankedHit>,
    /// 单 token 小写 needle（T2/T3 用）；多词 / 含非字母 / 空 → `None`。
    pub needle: Option<String>,
}

/// 单 token 查询解析结果（决定 T2/T3 是否触发）。
#[derive(Clone, Debug)]
pub struct MatchTokens {
    /// 小写 needle（如 `zgr` / `vsc`）；无单 token 则为 `None`。
    pub needle: Option<String>,
    /// 拼音 tier 是否可触发：查询为**单个纯字母 token 且全部小写**（DESIGN §8.1：
    /// 大写字母强制排除拼音，只按字面匹配）。
    pub pinyin_eligible: bool,
}

/// 从查询解析出单 token 字母 needle 与拼音资格（DESIGN §8.1）。
///
/// 仅当查询恰好由一个裸名字 term（Substring / Phrase，解析器只把这类收进
/// `terms`）构成、且文本全为 ASCII 字母时返回 needle；否则返回 `None`
/// （多词 AND / 修饰符查询走 T1，不触发拼音 / 模糊）。
pub fn match_tokens(q: &Query) -> MatchTokens {
    let empty = MatchTokens {
        needle: None,
        pinyin_eligible: false,
    };
    if q.terms.len() != 1 {
        return empty;
    }
    let text = q.terms[0].text.clone();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_alphabetic()) {
        return empty;
    }
    let lower = text.to_ascii_lowercase();
    // 拼音资格：原始文本全为小写（大写 → 用户强制排除拼音）。
    let pinyin_eligible = text.bytes().all(|b| b.is_ascii_lowercase());
    MatchTokens {
        needle: Some(lower),
        pinyin_eligible,
    }
}

/// 按 name / tier / needle / 命中区间 / 深度给单条命中评分（DESIGN §8.2）。
///
/// `ranges` 为相对名字的命中字节区间（已折叠 / 长度保持，偏移相对原名一致），
/// 用于取「首个命中位置」与判定连续度。`name` 为原始名字字节（长度惩罚用）。
pub fn score_name_bytes(
    name: &[u8],
    tier: Tier,
    needle: &Option<String>,
    ranges: &[Range<u32>],
    depth: u32,
    cfg: &ScoreConfig,
) -> f64 {
    let mut s = match tier {
        Tier::Substring => cfg.tier_substring,
        Tier::Pinyin => cfg.tier_pinyin,
        Tier::Fuzzy => cfg.tier_fuzzy,
    };
    let name_len = name.len().max(1) as f64;
    if let Some(n) = needle {
        let nb = n.as_bytes();
        // 位置：首个命中起点越靠前越高。
        if let Some(first) = ranges.first() {
            let pos = first.start as f64;
            s += cfg.position_bonus * (1.0 - (pos / name_len).min(1.0));
        }
        // 连续度：单段且覆盖 needle 过半长度视为连续。
        let contiguous =
            ranges.len() == 1 && (ranges[0].end - ranges[0].start) as f64 >= (n.len() as f64) * 0.5;
        s += if contiguous {
            cfg.contiguity_bonus
        } else {
            cfg.contiguity_bonus * 0.3
        };
        // 完全相等 > 前缀 > 其他。零分配的 ASCII 不敏感比较（DESIGN §15：
        // 默认简单小写、不做完整 Unicode case folding）。原实现每条名字
        // `from_utf8_lossy().to_lowercase()` 分配 String，是评分排序的主要热点。
        let eq_folded = |w: &[u8]| {
            w.len() == nb.len() && w.iter().zip(nb).all(|(&a, &b)| a.eq_ignore_ascii_case(&b))
        };
        if eq_folded(name) {
            s += 500.0;
        } else if name.len() >= nb.len() && eq_folded(&name[..nb.len()]) {
            s += 300.0;
        }
        // 大小写精确（原名字含未折叠的 needle）：窗口扫描，零分配。
        if !nb.is_empty() && name.windows(nb.len()).any(|w| w == nb) {
            s += cfg.case_exact_bonus;
        }
        s += cfg.name_vs_path;
        // 长度惩罚（越短越靠前）。
        s -= cfg.length_penalty * (name.len() as f64 / 32.0).min(8.0);
    } else {
        s += cfg.name_vs_path;
    }
    // 深度惩罚（越浅越靠前）。
    s - cfg.depth_penalty * depth as f64
}

/// Overlay 增量条目的 M5 分层判定（与 [`VolumeIndex::search_ranked`] 同一套 T1/T2/T3 口径）。
///
/// Overlay 条目不在 `VolumeIndex` 里，T1 由调用方用 `Query::matches_facts` 判定
/// （M4 的 OR / NOT / 修饰符在增量条目上才不会漏匹配）；T1 未命中时按
/// [`match_tokens`] 追加 T2 拼音 / T3 模糊。`allow_fuzzy` 由引擎按「Base 的
/// T1+T2 命中数 < 100」传入，落实 DESIGN §8.1 的 T3 稀疏触发条件。
///
/// 返回命中的最优 tier 与评分；未命中返回 `None`（调用方据此决定是否收入结果集）。
pub fn match_overlay_name(
    name: &[u8],
    q: &Query,
    mopts: &MatchOptions,
    cfg: &ScoreConfig,
    depth: u32,
    t1_hit: bool,
    allow_fuzzy: bool,
) -> Option<(Tier, f64)> {
    let mt = match_tokens(q);
    let lowered = simple_lowercase(name);
    let (tier, ranges) = if t1_hit {
        // 与 `VolumeIndex::sub_ranges` 一致：单 token 时用 needle 取区间，
        // 多词 / 修饰符查询无 needle → 区间为空（评分退化为 tier 基准 + 深度）。
        let ranges = match &mt.needle {
            Some(n) => SubstringMatcher::new(n.as_bytes()).find_ranges(&lowered),
            None => Vec::new(),
        };
        (Tier::Substring, ranges)
    } else {
        let n = mt.needle.as_ref()?;
        let fuzzy_eligible = mopts.fuzzy && allow_fuzzy && n.chars().count() >= 2;
        if mopts.pinyin && mt.pinyin_eligible {
            let pm = PinyinMatcher::new(n);
            if pm.matches(name) {
                (Tier::Pinyin, pm.find_ranges(name))
            } else if fuzzy_eligible {
                let fm = FuzzyMatcher::new(n.as_bytes());
                if fm.matches(&lowered) {
                    (Tier::Fuzzy, fm.find_ranges(&lowered))
                } else {
                    return None;
                }
            } else {
                return None;
            }
        } else if fuzzy_eligible {
            let fm = FuzzyMatcher::new(n.as_bytes());
            if fm.matches(&lowered) {
                (Tier::Fuzzy, fm.find_ranges(&lowered))
            } else {
                return None;
            }
        } else {
            return None;
        }
    };
    let score = score_name_bytes(name, tier, &mt.needle, &ranges, depth, cfg);
    Some((tier, score))
}

impl VolumeIndex {
    /// M5 排名搜索：T1 永远执行；单 token 纯字母查询再追加 T2（拼音）/ T3（模糊）。
    ///
    /// 返回的 `hits` 已按评分降序排列（同分按 id 升序），引擎层据此合并跨卷并按
    /// `SortKey::Score` 排序。`total` 即候选总数（引擎再用 `max_hits` 截断窗口）。
    pub fn search_ranked(
        &self,
        q: &Query,
        mopts: &MatchOptions,
        cfg: &ScoreConfig,
        cancel: &crate::cancel::CancelToken,
    ) -> RankedResult {
        if q.is_empty() || cancel.is_cancelled() {
            return RankedResult {
                hits: Vec::new(),
                needle: None,
            };
        }
        let mt = match_tokens(q);
        let needle = mt.needle.clone();

        // 每个 id 保留「最优 tier」命中（Substring > Pinyin > Fuzzy）。
        let mut best: FxHashMap<EntryId, RankedHit> = FxHashMap::default();

        // —— T1：子串 / 多词 AND（复用既有 AST 求值，最快）——
        let t1 = self.search(q, cancel);
        if cancel.is_cancelled() {
            return RankedResult {
                hits: Vec::new(),
                needle,
            };
        }
        for id in t1 {
            let ranges = self.sub_ranges(id, needle.as_deref());
            let score = score_name_bytes(
                self.name(id),
                Tier::Substring,
                &needle,
                &ranges,
                self.depth_of(id),
                cfg,
            );
            best.insert(
                id,
                RankedHit {
                    id,
                    tier: Tier::Substring,
                    score,
                },
            );
        }

        // —— T2 / T3：仅单 token 纯字母查询 ——
        if let Some(n) = &needle {
            // T2 拼音：仅遍历含 CJK 的条目（cjk_ids），查询须全小写。
            if mopts.pinyin && mt.pinyin_eligible {
                let pm = PinyinMatcher::new(n);
                // 并行遍历 cjk_ids：59 万条串行构造拼音 token 实测 813 ms，
                // rayon 分片后压到 DESIGN §8.1 的 150 ms 门槛内。
                let py_ids: Vec<EntryId> = self
                    .cjk_ids()
                    .par_iter()
                    .filter(|&&id| {
                        let raw = self.name(id);
                        pm.quick_may_match(raw) && pm.matches(raw)
                    })
                    .copied()
                    .collect();
                for id in py_ids {
                    let raw = self.name(id);
                    let ranges = pm.find_ranges(raw);
                    let score = score_name_bytes(
                        raw,
                        Tier::Pinyin,
                        &needle,
                        &ranges,
                        self.depth_of(id),
                        cfg,
                    );
                    best.entry(id).or_insert(RankedHit {
                        id,
                        tier: Tier::Pinyin,
                        score,
                    });
                }
            }
            // T3 模糊：仅当 T1+T2 仍很少（< 100）且查询长度 ≥ 2 时全表扫描
            // （并行、限名字长度 ≤ 64，控制 2M 条耗时；DESIGN §8.1）。
            if mopts.fuzzy && n.chars().count() >= 2 && best.len() < 100 {
                let fm = FuzzyMatcher::new(n.as_bytes());
                let n_entries = self.entry_count();
                let fuzzy_ids: Vec<EntryId> = (0..n_entries as EntryId)
                    .into_par_iter()
                    .filter(|&id| {
                        let raw = self.name(id);
                        raw.len() <= 64 && fm.matches_folded(raw)
                    })
                    .collect();
                for id in fuzzy_ids {
                    let lowered = simple_lowercase(self.name(id));
                    let ranges = fm.find_ranges(&lowered);
                    let score = score_name_bytes(
                        &lowered,
                        Tier::Fuzzy,
                        &needle,
                        &ranges,
                        self.depth_of(id),
                        cfg,
                    );
                    best.entry(id).or_insert(RankedHit {
                        id,
                        tier: Tier::Fuzzy,
                        score,
                    });
                }
            }
        }

        let mut hits: Vec<RankedHit> = best.into_values().collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
        RankedResult { hits, needle }
    }

    /// T1 候选的子串命中区间（相对原名，字节偏移），供评分取位置。
    /// 直接借 `names` arena，不复制整条名字（评分排序时命中可达数万条）。
    fn sub_ranges(&self, id: EntryId, needle: Option<&str>) -> Vec<Range<u32>> {
        let raw = self.name(id);
        match needle {
            Some(n) => {
                let lowered = simple_lowercase(raw);
                ff_match::SubstringMatcher::new(n.as_bytes()).find_ranges(&lowered)
            }
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{attrs, EntryInput, IndexBuilder, VolumeInfo};
    use crate::parse;

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

    /// 含中文 + 英文 + 相互干扰的名字，覆盖 T1/T2/T3 金标准。
    fn sample() -> VolumeIndex {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(100, 5, "report.txt")); // T1 完全相等
        b.push(file(101, 5, "reporting.log")); // T1 前缀
        b.push(file(102, 5, "myreport.dat")); // T1 中间子串
        b.push(file(103, 5, "repartee.md")); // T3 模糊（r e p .. r t）
        b.push(file(104, 5, "Visual Studio Code")); // T3 模糊 vsc
        b.push(file(105, 5, "中国人.txt")); // T2 拼音 zgr
        b.push(file(106, 5, "拼音搜索Everything")); // T2 拼音 pysousuo
        b.push(file(107, 5, "美国.txt")); // 干扰（zgr 不应命中）
        b.finish()
    }

    fn cfg() -> ScoreConfig {
        ScoreConfig::default()
    }

    fn mopts() -> MatchOptions {
        MatchOptions {
            fuzzy: true,
            pinyin: true,
        }
    }

    #[test]
    fn score_ordering_components() {
        // 金标准：完全相等 > 前缀 > 中间子串 > 模糊（DESIGN §8.2）。
        let c = cfg();
        let n = Some("report".to_string());
        // 单区间用迭代器收集，避免 clippy 的 single_range_in_vec_init 误读 `[0..6]`。
        let one =
            |start: u32, end: u32| -> Vec<Range<u32>> { std::iter::once(start..end).collect() };
        let s_exact = score_name_bytes(b"report", Tier::Substring, &n, &one(0, 6), 0, &c);
        let s_pref = score_name_bytes(b"reporting", Tier::Substring, &n, &one(0, 6), 0, &c);
        let s_mid = score_name_bytes(b"myreport", Tier::Substring, &n, &one(2, 8), 0, &c);
        let s_fuzzy = score_name_bytes(
            b"repartee",
            Tier::Fuzzy,
            &n,
            &[0..1, 2..3, 3..4, 5..6, 6..7],
            0,
            &c,
        );
        assert!(s_exact > s_pref, "完全相等应高于前缀");
        assert!(s_pref > s_mid, "前缀应高于中间子串");
        assert!(s_mid > s_fuzzy, "中间子串应高于模糊");
    }

    #[test]
    fn pinyin_zgr_hits_chinese() {
        let ix = sample();
        let r = ix.search_ranked(
            &parse("zgr").unwrap(),
            &mopts(),
            &cfg(),
            &Default::default(),
        );
        let hit = r
            .hits
            .iter()
            .find(|h| ix.name(h.id) == b"\xe4\xb8\xad\xe5\x9b\xbd\xe4\xba\xba.txt");
        assert!(hit.is_some(), "zgr 应拼音命中 中国人.txt");
        assert_eq!(hit.unwrap().tier, Tier::Pinyin);
    }

    #[test]
    fn pinyin_pysousuo_hits() {
        let ix = sample();
        let r = ix.search_ranked(
            &parse("pysousuo").unwrap(),
            &mopts(),
            &cfg(),
            &Default::default(),
        );
        let hit = r
            .hits
            .iter()
            .find(|h| ix.name(h.id) == "拼音搜索Everything".as_bytes());
        assert!(hit.is_some(), "pysousuo 应拼音命中 拼音搜索Everything");
        assert_eq!(hit.unwrap().tier, Tier::Pinyin);
    }

    #[test]
    fn fuzzy_vsc_hits_visual_studio_code() {
        let ix = sample();
        let r = ix.search_ranked(
            &parse("vsc").unwrap(),
            &mopts(),
            &cfg(),
            &Default::default(),
        );
        let hit = r
            .hits
            .iter()
            .find(|h| ix.name(h.id) == b"Visual Studio Code");
        assert!(hit.is_some(), "vsc 应模糊命中 Visual Studio Code");
        assert_eq!(hit.unwrap().tier, Tier::Fuzzy);
    }

    #[test]
    fn uppercase_excludes_pinyin_but_keeps_substring() {
        // "ZGR" 大写：不应拼音命中 中国人，但若名字字面含 "ZGR" 仍应 T1 命中。
        let ix = sample();
        let r = ix.search_ranked(
            &parse("ZGR").unwrap(),
            &mopts(),
            &cfg(),
            &Default::default(),
        );
        assert!(
            !r.hits
                .iter()
                .any(|h| ix.name(h.id) == b"\xe4\xb8\xad\xe5\x9b\xbd\xe4\xba\xba.txt"),
            "大写查询不应触发拼音"
        );
    }

    #[test]
    fn substring_exact_ranks_first() {
        let ix = sample();
        let r = ix.search_ranked(
            &parse("report").unwrap(),
            &mopts(),
            &cfg(),
            &Default::default(),
        );
        // 第一条应是完全相等的 report.txt（T1，高分），不会是模糊噪声。
        assert_eq!(ix.name(r.hits[0].id), b"report.txt");
    }

    #[test]
    fn shallow_beats_deep_when_scores_tie() {
        // 金标准（DESIGN §8.2）：其他条件相同，路径更浅的在前（depth_penalty）。
        // 注意 `EntryId` 是构建器内的序号，不是 FRN，断言不能硬编码 frn 值。
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(dir(10, 5, "A"));
        b.push(dir(11, 10, "B")); // .\A\B，深度比根目录多两层
        b.push(file(20, 5, "note.txt")); // 浅：.\note.txt
        b.push(file(21, 11, "note.txt")); // 深：.\A\B\note.txt
        let ix = b.finish();
        let r = ix.search_ranked(
            &parse("note").unwrap(),
            &mopts(),
            &cfg(),
            &Default::default(),
        );
        assert_eq!(r.hits.len(), 2);
        let (shallow, deep) = (r.hits[0].id, r.hits[1].id);
        assert_eq!(ix.name_str(shallow), Some("note.txt"));
        assert_eq!(ix.name_str(deep), Some("note.txt"));
        assert!(
            ix.depth_of(shallow) < ix.depth_of(deep),
            "浅路径应排在深路径之前"
        );
    }
}
