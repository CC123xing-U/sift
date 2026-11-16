//! 拼音匹配（T2，DESIGN §8.1）。
//!
//! 生产环境应换用 `ib-pinyin`（匹配）+ `pinyin`（转换，如需）（DESIGN §8.1 / §0.5
//! 依赖许可证核对）。此处内置一份**常用汉字 → 拼音**的精简字典，足以演示 T2 分层
//! 与通过金标准用例（`zgr` → `中国人`、`pysousuo` → `拼音搜索Everything`）；
//! 未收录的汉字按「无拼音」处理（不影响 ASCII 查询与子串匹配）。替换生产库时只需
//! 改写 [`pinyin_of`] 与 [`name_pinyin`]，匹配逻辑不变。

use std::ops::Range;

use crate::matcher::{Matcher, Tier};

/// 单汉字 → (全拼, 首字母)。未收录汉字返回 `None`。
pub fn pinyin_of(c: char) -> Option<(&'static str, &'static str)> {
    Some(match c {
        '中' => ("zhong", "z"),
        '国' => ("guo", "g"),
        '人' => ("ren", "r"),
        '拼' => ("pin", "p"),
        '音' => ("yin", "y"),
        '搜' => ("sou", "s"),
        '索' => ("suo", "s"),
        '文' => ("wen", "w"),
        '件' => ("jian", "j"),
        '图' => ("tu", "t"),
        '视' => ("shi", "s"),
        '频' => ("pin", "p"),
        '电' => ("dian", "d"),
        '脑' => ("nao", "n"),
        '档' => ("dang", "d"),
        '案' => ("an", "a"),
        '设' => ("she", "s"),
        '置' => ("zhi", "z"),
        '复' => ("fu", "f"),
        '制' => ("zhi", "z"),
        '查' => ("cha", "c"),
        '看' => ("kan", "k"),
        '找' => ("zhao", "z"),
        '打' => ("da", "d"),
        '开' => ("kai", "k"),
        '字' => ("zi", "z"),
        '报' => ("bao", "b"),
        '告' => ("gao", "g"),
        '录' => ("lu", "l"),
        '影' => ("ying", "y"),
        '乐' => ("yue", "y"),
        '游' => ("you", "y"),
        '戏' => ("xi", "x"),
        '工' => ("gong", "g"),
        '具' => ("ju", "j"),
        '软' => ("ruan", "r"),
        '代' => ("dai", "d"),
        '码' => ("ma", "m"),
        '会' => ("hui", "h"),
        '议' => ("yi", "y"),
        '课' => ("ke", "k"),
        '程' => ("cheng", "c"),
        '数' => ("shu", "s"),
        '据' => ("ju", "j"),
        '库' => ("ku", "k"),
        '网' => ("wang", "w"),
        '页' => ("ye", "y"),
        '览' => ("lan", "l"),
        '器' => ("qi", "q"),
        '系' => ("xi", "x"),
        '统' => ("tong", "t"),
        '盘' => ("pan", "p"),
        '书' => ("shu", "s"),
        '照' => ("zhao", "z"),
        '片' => ("pian", "p"),
        _ => return None,
    })
}

/// 名字的拼音 token 序列。
///
/// `full` 与 `initials` 各自的长度等于其对应的 `byte_start` 长度：
/// - `full`：每个原字符可能展开为多字符（汉字全拼），每个全拼字符都记录「它来自
///   哪个原字符的字节起点」（`full_byte_start`），以便把拼音命中映射回原名字区间；
/// - `initials`：每个原字符恰好产出一个首字母字符，1:1 对齐（`initials_byte_start`）。
pub struct PinyinTokens {
    /// 全拼串（Latin 自身小写；汉字用其全拼逐字符展开；占位符 `\u{ffff}` 隔离）。
    pub full: String,
    /// 首字母串（Latin 自身小写；汉字取其首字母；占位符 `\u{ffff}` 隔离）。
    pub initials: String,
    /// 与 `full` 等长：每个全拼字符对应的原名字字节起点。
    pub full_byte_start: Vec<usize>,
    /// 与 `initials` 等长：每个首字母字符对应的原名字字节起点。
    pub initials_byte_start: Vec<usize>,
}

/// 把名字转成全拼 / 首字母 token 序列，并记录 token → 原字节起点的对齐。
pub fn name_pinyin(name: &str) -> PinyinTokens {
    let mut full = String::new();
    let mut initials = String::new();
    let mut full_byte_start = Vec::new();
    let mut initials_byte_start = Vec::new();
    let mut byte = 0usize;
    for c in name.chars() {
        let start = byte;
        byte += c.len_utf8();
        if c.is_ascii_alphabetic() {
            let lo = c.to_ascii_lowercase();
            full.push(lo);
            full_byte_start.push(start);
            initials.push(lo);
            initials_byte_start.push(start);
        } else if let Some((f, i)) = pinyin_of(c) {
            // 全拼可能多字符，每个全拼字符都回指同一个原字符起点。
            for fc in f.chars() {
                full.push(fc);
                full_byte_start.push(start);
            }
            initials.push_str(i);
            initials_byte_start.push(start);
        } else {
            // 不贡献拼音（数字、符号、未收录汉字）：用不会出现在查询里的占位符隔离，
            // 避免跨字符误连成子序列。
            full.push('\u{ffff}');
            full_byte_start.push(start);
            initials.push('\u{ffff}');
            initials_byte_start.push(start);
        }
    }
    PinyinTokens {
        full,
        initials,
        full_byte_start,
        initials_byte_start,
    }
}

/// T2 拼音匹配器（DESIGN §8.1）。
///
/// 触发条件（由 ff-core 在调用前判断）：查询为**纯 ASCII 小写字母**，且目标名字
/// 含 CJK 汉字。匹配方式：查询作为**子序列**出现在名字的全拼串或首字母串中即命中
/// （覆盖首字母 `zgr`→`中国人`、全拼/混合 `pysousuo`→`拼音搜索Everything`）。
pub struct PinyinMatcher {
    /// ASCII 小写查询字符（已折叠；非小写字母的字符在构造时被过滤掉）。
    query: Vec<char>,
}

impl PinyinMatcher {
    pub fn new(query: &str) -> Self {
        Self {
            query: query.chars().filter(|c| c.is_ascii_lowercase()).collect(),
        }
    }

    /// 在 `hay`（全拼或首字母串）上做子序列匹配；命中则返回覆盖到的原名字字节区间。
    ///
    /// 由于汉字全拼会展开为多字符，`hits` 的索引不能直接当字节区间；这里先收集被
    /// 命中字符所属的「原字符起点」集合，再据此切出每个被命中原字符的字节区间并合并
    /// （连续命中汉字会被合并成一个高亮区间）。
    fn match_tokens(
        &self,
        hay: &str,
        byte_start: &[usize],
        name_len: usize,
    ) -> Option<Vec<Range<u32>>> {
        if self.query.is_empty() || hay.chars().count() != byte_start.len() {
            return None;
        }
        let t: Vec<char> = hay.chars().collect();
        let mut hits = Vec::new();
        let mut ni = 0;
        for (i, &tc) in t.iter().enumerate() {
            // C3：占位符代表不贡献拼音的字符（数字/符号/未收录汉字）。它必须作为
            // 屏障阻断子序列——否则 `zr` 会跨过未收录汉字把前后两个字误连命中。
            // 跨越时重置本段进度，query 必须完整落在单个连续段内。
            if tc == '\u{ffff}' {
                ni = 0;
                hits.clear();
                continue;
            }
            if tc == self.query[ni] {
                hits.push(i);
                ni += 1;
                if ni == self.query.len() {
                    break;
                }
            }
        }
        if hits.len() != self.query.len() {
            return None;
        }
        // 被命中字符所属的原字符起点集合（去重）。
        let mut covered: Vec<usize> = hits.iter().map(|&i| byte_start[i]).collect();
        covered.sort_unstable();
        covered.dedup();
        let mut ranges = Vec::with_capacity(covered.len());
        for &s in &covered {
            // 该原字符的结束 = 下一个更大的原字符起点，或名字末尾。
            let end = byte_start
                .iter()
                .copied()
                .find(|&b| b > s)
                .unwrap_or(name_len);
            if end > s {
                ranges.push(s as u32..end as u32);
            }
        }
        // 输出非重叠、按起点升序的区间；相邻区间（如连续的汉字）不做合并，
        // 留给前端 NameHL 统一合并，与 SubstringMatcher 的约定保持一致。
        ranges.sort_by_key(|r| r.start);
        Some(ranges)
    }
}

impl PinyinMatcher {
    /// 零分配的必要条件剪枝：名字中是否存在能提供 needle **首字符** 的字符。
    ///
    /// `name_pinyin` 逐字符生成全拼串与首字母串；若没有任何字符能给出
    /// `query[0]`（Latin 自身、汉字拼音首字母或全拼首字符），子序列必然不匹配。
    /// 通过后才构造完整拼音 token（`PinyinTokens` 有 4 处堆分配），避免
    /// 在 59 万条 `cjk_ids` 上逐条构造。
    pub fn quick_may_match(&self, name: &[u8]) -> bool {
        let first = match self.query.first() {
            Some(c) => *c,
            None => return false,
        };
        let s = match std::str::from_utf8(name) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let fb = first as u8;
        for c in s.chars() {
            if c.is_ascii_alphabetic() {
                if c.to_ascii_lowercase() == first {
                    return true;
                }
            } else if let Some((full, initial)) = pinyin_of(c) {
                if initial.as_bytes().first() == Some(&fb) || full.as_bytes().first() == Some(&fb) {
                    return true;
                }
            }
        }
        false
    }
}

impl PinyinMatcher {
    /// 在 `full` 与 `initials` 两条拼音串上分别尝试子序列匹配，返回**覆盖原字符更多**
    /// 的那条结果。原因：全拼串拼接后可能产生「字符吸收」（`zhongguoren` 里 `g` 既来自
    /// 中 也来自 国），导致少覆盖某些汉字；首字母串是一字一字母的规范表示，覆盖更稳。
    /// 两者都命中时取覆盖更多者，金标准 `zgr`→`中国人` 因此能点亮全部三个汉字。
    fn try_match(&self, name: &str, name_len: usize) -> Option<Vec<Range<u32>>> {
        let tok = name_pinyin(name);
        let full = self.match_tokens(&tok.full, &tok.full_byte_start, name_len);
        let init = self.match_tokens(&tok.initials, &tok.initials_byte_start, name_len);
        match (full, init) {
            (Some(f), Some(i)) => Some(if i.len() >= f.len() { i } else { f }),
            (Some(f), None) => Some(f),
            (None, Some(i)) => Some(i),
            (None, None) => None,
        }
    }
}

impl Matcher for PinyinMatcher {
    fn tier(&self) -> Tier {
        Tier::Pinyin
    }

    fn matches(&self, name: &[u8]) -> bool {
        if self.query.is_empty() {
            return false;
        }
        let name = match std::str::from_utf8(name) {
            Ok(s) => s,
            Err(_) => return false,
        };
        self.try_match(name, name.len()).is_some()
    }

    fn find_ranges(&self, name: &[u8]) -> Vec<Range<u32>> {
        if self.query.is_empty() {
            return Vec::new();
        }
        let name = match std::str::from_utf8(name) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        self.try_match(name, name.len()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_zgr_hits_chinese() {
        // 金标准：zgr → 中国人
        let m = PinyinMatcher::new("zgr");
        assert!(m.matches("中国人".as_bytes()));
        let r = m.find_ranges("中国人".as_bytes());
        // 三个汉字各被命中一次（中/国/人 的首字母 z/g/r）
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn full_and_mixed_pysousuo_hits() {
        // 金标准（DESIGN §8.2）：`pysousuo` → `拼音搜索`（全拼「pinyin」+「sousuo」的
        // 子序列）；`Everything` 部分为拉丁字符，由 T1/T3 负责，此处只验证拼音段。
        let m = PinyinMatcher::new("pysousuo");
        assert!(m.matches("拼音搜索Everything".as_bytes()));
        // 也支持纯首字母：pys → 拼音搜索
        let m2 = PinyinMatcher::new("pys");
        assert!(m2.matches("拼音搜索Everything".as_bytes()));
    }

    #[test]
    fn pinyin_highlight_covers_full_hanzi() {
        // 多字符全拼（拼=p,i,n）应整体高亮为单一区间，而非塌成空区间。
        let m = PinyinMatcher::new("pin");
        let r = m.find_ranges("拼音搜索".as_bytes());
        assert_eq!(r, vec![0..3]); // 拼 = 3 字节
    }

    #[test]
    fn pinyin_does_not_false_hit_ascii() {
        // 纯 ASCII 名字不应被拼音匹配凭空命中（子序列隔离占位符）
        let m = PinyinMatcher::new("abc");
        assert!(!m.matches("report.txt".as_bytes()));
    }

    #[test]
    fn non_ascii_query_ignored() {
        // 大写或非字母查询被过滤：不会把 "ZGR" 当拼音
        let m = PinyinMatcher::new("ZGR");
        assert!(!m.matches("中国人".as_bytes()));
    }

    #[test]
    fn placeholder_blocks_cross_character_subsequence() {
        // C3：未收录汉字产生 \u{ffff} 占位符，必须阻断子序列，不要让 `zr` 跨过它
        // 把两个不相邻的字误连。「龘」未收录（字典无此项），夹在「中」「人」之间。
        let m = PinyinMatcher::new("zr");
        assert!(
            !m.matches("中龘人".as_bytes()),
            "zr 不得跨过未收录字把中与人的首字母误连"
        );
        // 对照：三个已收录字的首字母子序列仍正常命中。
        let m2 = PinyinMatcher::new("zr");
        assert!(m2.matches("中人国".as_bytes()));
    }

    #[test]
    fn quick_may_match_is_necessary_condition() {
        // 命中项必须通过快速剪枝（否则会产生漏匹配）。
        let zgr = PinyinMatcher::new("zgr");
        assert!(zgr.quick_may_match("中国人".as_bytes()));
        assert!(zgr.quick_may_match("zgr.txt".as_bytes()));
        // 没有任何字符能提供 'z' → 剪枝必须拒绝，且与 matches 结论一致。
        assert!(!zgr.quick_may_match("美国.txt".as_bytes()));
        assert!(!zgr.matches("美国.txt".as_bytes()));
        let abc = PinyinMatcher::new("abc");
        assert!(!abc.quick_may_match("report.txt".as_bytes()));
        // 空查询不通过剪枝。
        assert!(!PinyinMatcher::new("").quick_may_match("中国人".as_bytes()));
    }
}
