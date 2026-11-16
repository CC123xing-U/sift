//! 查询语言（DESIGN §7）—— M4 完整语法。
//!
//! - 语法（§7.1）：空格 = AND，`|` = OR，`!` = NOT，`"..."` 短语，`(...)` / `<...>` 分组；
//!   优先级 NOT > AND > OR；term 含 `\` 或 `/` 时对**完整路径**匹配；
//!   `*`、`?` 仅在 term 含它们时启用通配符路径。
//! - 修饰符（§7.2 M4 必做）：`case:` `nocase:` `file:` `folder:` `ext:a;b` `path:`
//!   `regex:` `ww:`/`wholeword:` `startwith:` `endwith:` `size:` `dm:` `dc:` `da:`
//!   `parent:` `infolder:` `depth:` `attrib:`。
//! - 容错（§7.3）：不完整的输入（用户正在打字）尽量降级为字面子串或「无贡献」；
//!   只有**明确非法**的修饰符值（`size:abc`、`regex:(`、`dm:notadate`）才返回
//!   带位置的 [`ParseError`]。
//!
//! M7 再说：`content:` 族、`dupe:`、`runcount:`、`len:`。

use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::case::simple_lowercase;
use crate::content::ContentSpec;
use crate::ParseError;

/// Unix 纪元（1970-01-01）到 FILETIME 纪元（1601-01-01）的秒数差。
const UNIX_TO_FILETIME_SECS: i64 = 11_644_473_600;
/// FILETIME 单位：100ns。
const FILETIME_PER_SEC: i64 = 10_000_000;
/// 一天的 FILETIME 长度。
const DAY_FILETIME: i64 = 86_400 * FILETIME_PER_SEC;

// ───────────────────────── AST ─────────────────────────

/// 名字级匹配的种类（决定求值走哪条路径）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    /// 普通子串（`memmem` 快速路径）。
    Substring,
    /// `"..."` 短语：与子串同路径，只是允许含空格。
    Phrase,
    /// 通配符（含 `*` / `?`）。
    Glob,
    /// `regex:` 编译后的正则。
    Regex,
    /// `ww:` / `wholeword:`。
    WholeWord,
    /// `startwith:`。
    StartWith,
    /// `endwith:`。
    EndWith,
    /// 对**完整路径**匹配（`path:` 或 term 含 `\` / `/`）。
    Path,
    /// `ext:a;b;c`（名字后缀集合中任一）。
    Ext,
    /// `parent:foo`（父目录名子串）。
    Parent,
    /// `infolder:foo`（路径中某层目录名为 foo）。
    InFolder,
}

/// 一个名字级匹配项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameTerm {
    /// 原始文本（未做大小写折叠）。
    pub text: String,
    /// 在查询串中的字节区间 `[start, end)`。
    pub span: Range<usize>,
    pub kind: NameKind,
    /// `kind == Regex` 时为 [`Query::regexes`] 的下标。
    pub regex_idx: Option<usize>,
}

impl NameTerm {
    /// 按大小写策略得到 needle 字节（不敏感时做简单小写折叠，DESIGN §4.1）。
    pub(crate) fn needle(&self, case_sensitive: bool) -> Vec<u8> {
        if case_sensitive {
            self.text.clone().into_bytes()
        } else {
            simple_lowercase(self.text.as_bytes())
        }
    }

    /// 是否可走 `memmem` 大块扫描快速路径（子串 / 短语）。
    ///
    /// 其余种类需要逐条取名字（或现算路径）判定，只能全表或在候选集上校验。
    pub fn is_fast_substring(&self) -> bool {
        matches!(self.kind, NameKind::Substring | NameKind::Phrase)
    }

    /// 是否需要现算完整路径（求值成本最高，放最后）。
    pub fn needs_path(&self) -> bool {
        matches!(self.kind, NameKind::Path | NameKind::InFolder)
    }
}

/// 数值比较符（`size:` `depth:`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
}

impl CmpOp {
    fn test(self, v: u64, rhs: u64) -> bool {
        match self {
            CmpOp::Gt => v > rhs,
            CmpOp::Ge => v >= rhs,
            CmpOp::Lt => v < rhs,
            CmpOp::Le => v <= rhs,
            CmpOp::Eq => v == rhs,
        }
    }
}

/// 数值谓词：单值比较或闭区间。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumPred {
    Cmp { op: CmpOp, value: u64 },
    Range { lo: u64, hi: u64 },
}

impl NumPred {
    /// 判定数值是否满足谓词（`search.rs` 的求值器调用）。
    pub(crate) fn test(&self, v: u64) -> bool {
        match *self {
            NumPred::Cmp { op, value } => op.test(v, value),
            NumPred::Range { lo, hi } => v >= lo && v <= hi,
        }
    }
}

/// 日期字段（`dm:` 修改 / `dc:` 创建 / `da:` 访问）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateField {
    Modified,
    Created,
    Accessed,
}

/// 日期规格（求值时按 [`EvalContext`] 转成 FILETIME 区间）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateSpec {
    Today,
    Yesterday,
    /// `last7days`：最近 N 天（含今天）。
    RelativeDays {
        days: u32,
    },
    /// 单日。
    Day {
        year: i32,
        month: u32,
        day: u32,
    },
    /// 闭区间（含两端整天）。
    Range {
        from: (i32, u32, u32),
        to: (i32, u32, u32),
    },
}

/// 属性谓词（不依赖名字，走 SoA 列扫描或标志位）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Predicate {
    IsDir,
    IsFile,
    /// `attrib:` —— Win32 属性位掩码，命中 = 含其中任一位。
    Attrib(u32),
    Size(NumPred),
    Depth(NumPred),
    Date {
        field: DateField,
        spec: DateSpec,
    },
}

/// 查询表达式树。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// 无贡献（空查询、被忽略的空修饰符值）。
    Empty,
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Name(NameTerm),
    Pred(Predicate),
}

// ───────────────────────── 求值上下文 ─────────────────────────

/// 求值上下文（DESIGN §7.2：日期「统一按本地时区解释」）。
///
/// `ff-core` 是零平台依赖的纯逻辑 crate，标准库拿不到本地时区偏移，
/// 因此偏移**由调用方注入**：Windows 侧（`ff-engine`）用 Win32 取真实偏移，
/// 未注入时按 UTC 日界解释（见 DECISIONS）。
#[derive(Clone, Copy, Debug)]
pub struct EvalContext {
    /// 求值时「现在」的 FILETIME。
    pub now_filetime: i64,
    /// 本地时区相对 UTC 的偏移（分钟，东八区 = 480）。
    pub utc_offset_minutes: i32,
}

impl Default for EvalContext {
    fn default() -> Self {
        Self {
            now_filetime: now_filetime(),
            utc_offset_minutes: 0,
        }
    }
}

/// 当前的 FILETIME（`SystemTime` 换算；用于 `EvalContext::default`）。
pub fn now_filetime() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (secs + UNIX_TO_FILETIME_SECS) * FILETIME_PER_SEC
}

/// 把「本地日历日 00:00」换算成 FILETIME（UTC 时刻）。
///
/// `days_from_civil` 用 Howard Hinnant 的移位算法（无表、无依赖、跨平台一致）。
pub fn filetime_from_civil(year: i32, month: u32, day: u32, offset_minutes: i32) -> i64 {
    let days = days_from_civil(year as i64, month as i64, day as i64);
    let unix_secs = days * 86_400 - i64::from(offset_minutes) * 60;
    (unix_secs + UNIX_TO_FILETIME_SECS) * FILETIME_PER_SEC
}

/// 民用日期 → Unix 日数（1970-01-01 = 0）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// `DateSpec` → FILETIME 半开区间 `[start, end)`（按上下文换算）。
pub(crate) fn date_range(spec: DateSpec, ctx: EvalContext) -> (i64, i64) {
    let off = i64::from(ctx.utc_offset_minutes) * 60 * FILETIME_PER_SEC;
    // 「现在」所在本地日历日的 00:00（本地时刻）。
    let local_now = ctx.now_filetime + off;
    let today_local = local_now - local_now.rem_euclid(DAY_FILETIME);
    // 本地时刻 → FILETIME(UTC)
    let to_ft = |local: i64| local - off;
    match spec {
        DateSpec::Today => (to_ft(today_local), to_ft(today_local + DAY_FILETIME)),
        DateSpec::Yesterday => (to_ft(today_local - DAY_FILETIME), to_ft(today_local)),
        DateSpec::RelativeDays { days } => {
            let back = i64::from(days).saturating_mul(DAY_FILETIME);
            (
                to_ft(today_local.saturating_sub(back)),
                to_ft(today_local + DAY_FILETIME),
            )
        }
        DateSpec::Day { year, month, day } => {
            let s = filetime_from_civil(year, month, day, ctx.utc_offset_minutes);
            (s, s + DAY_FILETIME)
        }
        DateSpec::Range { from, to } => {
            let s = filetime_from_civil(from.0, from.1, from.2, ctx.utc_offset_minutes);
            let e = filetime_from_civil(to.0, to.1, to.2, ctx.utc_offset_minutes) + DAY_FILETIME;
            if s <= e {
                (s, e)
            } else {
                (e, s)
            }
        }
    }
}

// ───────────────────────── Query ─────────────────────────

/// 高亮用的扁平 term（与 M1 契约同名同义）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// 原始文本（未做大小写折叠；高亮 / 大小写敏感匹配用）。
    pub text: String,
    /// 在查询串中的字节区间 `[start, end)`（短语不含两侧引号）。
    pub span: Range<usize>,
    /// 是否来自 `"..."` 短语。
    pub phrase: bool,
}

impl Term {
    /// 按大小写策略得到 needle 字节。
    pub(crate) fn needle(&self, case_sensitive: bool) -> Vec<u8> {
        if case_sensitive {
            self.text.clone().into_bytes()
        } else {
            simple_lowercase(self.text.as_bytes())
        }
    }
}

/// 解析后的查询。
#[derive(Clone, Debug)]
pub struct Query {
    /// 表达式树（求值入口）。
    pub root: Expr,
    /// 可高亮的名字 term 扁平视图（**不含** NOT 子树内的 term，
    /// 也不含通配符 / 正则等非子串种类——它们的高亮区间不精确，M4 不高亮）。
    pub terms: Vec<Term>,
    /// 区分大小写（`case:` / `nocase:`；默认 false）。
    pub case_sensitive: bool,
    /// 求值上下文（日期用）。
    pub context: EvalContext,
    /// `regex:` 编译产物（下标见 [`NameTerm::regex_idx`]）。
    pub regexes: Vec<Regex>,
    /// `content:` 条件（M7，DECISIONS #95）。
    ///
    /// **不在 AST 里**：内容匹配需要读磁盘，成本与名字匹配差三个数量级，
    /// 塞进 AST 会让「全表扫描」路径（`VolumeIndex::search` / `matches_facts`）
    /// 误以为能就地判定。这里作为 `root` 之上的附加 AND 条件，由引擎在拿到
    /// 候选集后串行过滤（[`crate::content::ContentFilter`]）。
    /// 语义约束见 [`parse`]：必须同时有名字条件，且不支持 `!` / `|`。
    pub content: Vec<ContentSpec>,
}

impl Query {
    /// 是否空查询（无任何有效贡献）。
    pub fn is_empty(&self) -> bool {
        matches!(self.root, Expr::Empty) && self.content.is_empty()
    }

    /// 是否需要读文件内容（引擎据此决定是否走内容过滤，避免无谓开销）。
    pub fn has_content(&self) -> bool {
        !self.content.is_empty()
    }

    /// 求值是否需要「完整路径 / 父名 / 深度」上下文（P1-2 优化）。
    ///
    /// 仅当查询用到 `path:` / `infolder:` / `parent:` / `depth:` 时才需要为每条
    /// Overlay 增量条目现算路径上下文；普通名字子串查询不需要，可省掉这部分
    /// 每按键几十到上百毫秒的分配（实机 Overlay 积累到数万条时，P1-2 前的实现
    /// 每次搜索都对每条增量现算父名 / 拼完整路径 / 算深度）。
    pub fn needs_path_context(&self) -> bool {
        self.expr_needs_path(&self.root)
    }

    fn expr_needs_path(&self, e: &Expr) -> bool {
        match e {
            Expr::Empty => false,
            Expr::Name(t) => {
                matches!(
                    t.kind,
                    NameKind::Path | NameKind::InFolder | NameKind::Parent
                )
            }
            Expr::Pred(Predicate::Depth(_)) => true,
            Expr::Pred(_) => false,
            Expr::Not(i) => self.expr_needs_path(i),
            Expr::And(v) | Expr::Or(v) => v.iter().any(|x| self.expr_needs_path(x)),
        }
    }
}

impl Default for Query {
    fn default() -> Self {
        Self {
            root: Expr::Empty,
            terms: Vec::new(),
            case_sensitive: false,
            context: EvalContext::default(),
            regexes: Vec::new(),
            content: Vec::new(),
        }
    }
}

// ───────────────────────── 解析器 ─────────────────────────

/// Win32 文件属性位（`attrib:` 用；与 `FILE_ATTRIBUTE_*` 一致）。
pub mod win32_attr {
    pub const READONLY: u32 = 0x0001;
    pub const HIDDEN: u32 = 0x0002;
    pub const SYSTEM: u32 = 0x0004;
    pub const DIRECTORY: u32 = 0x0010;
    pub const ARCHIVE: u32 = 0x0020;
    pub const DEVICE: u32 = 0x0040;
    pub const NORMAL: u32 = 0x0080;
    pub const TEMPORARY: u32 = 0x0100;
    pub const SPARSE: u32 = 0x0200;
    pub const REPARSE: u32 = 0x0400;
    pub const COMPRESSED: u32 = 0x0800;
    pub const OFFLINE: u32 = 0x1000;
    pub const NOT_CONTENT_INDEXED: u32 = 0x2000;
    pub const ENCRYPTED: u32 = 0x4000;
}

/// `attrib:` 字母 → 属性位（大小写不敏感）。
fn attrib_bit(ch: char) -> Option<u32> {
    Some(match ch.to_ascii_uppercase() {
        'R' => win32_attr::READONLY,
        'H' => win32_attr::HIDDEN,
        'S' => win32_attr::SYSTEM,
        'D' => win32_attr::DIRECTORY,
        'A' => win32_attr::ARCHIVE,
        'C' => win32_attr::COMPRESSED,
        'E' => win32_attr::ENCRYPTED,
        'T' => win32_attr::TEMPORARY,
        'O' => win32_attr::OFFLINE,
        'I' => win32_attr::NOT_CONTENT_INDEXED,
        'N' => win32_attr::NORMAL,
        'L' => win32_attr::REPARSE,
        'P' => win32_attr::SPARSE,
        'V' => win32_attr::DEVICE,
        _ => return None,
    })
}

/// 已知修饰符名（小写比较）。
fn is_modifier(ident: &str) -> bool {
    matches!(
        ident.to_ascii_lowercase().as_str(),
        "case"
            | "nocase"
            | "file"
            | "folder"
            | "ext"
            | "path"
            | "regex"
            | "ww"
            | "wholeword"
            | "startwith"
            | "endwith"
            | "size"
            | "dm"
            | "dc"
            | "da"
            | "parent"
            | "infolder"
            | "depth"
            | "attrib"
            | "content"
    )
}

struct Parser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
    case_sensitive: bool,
    terms: Vec<Term>,
    regexes: Vec<Regex>,
    /// `content:` 条件（M7）。不进 AST，见 [`Query::content`]。
    contents: Vec<ContentSpec>,
    /// NOT 嵌套深度：>0 时不收集高亮 term（NOT 内的词不该被高亮）。
    not_depth: usize,
}

fn err(message: &str, span: Range<usize>) -> ParseError {
    ParseError {
        message: message.to_string(),
        span,
    }
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    /// OR 层：`a | b`（最低优先级）。
    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut parts = vec![self.parse_and()?];
        loop {
            self.skip_ws();
            if self.peek() == Some(b'|') {
                self.pos += 1;
                parts.push(self.parse_and()?);
            } else {
                break;
            }
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap_or(Expr::Empty)
        } else {
            Expr::Or(parts)
        })
    }

    /// AND 层：空白分隔的隐式 AND（也可能一个都没有 → `Empty`）。
    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut parts: Vec<Expr> = Vec::new();
        loop {
            self.skip_ws();
            if self.at_end() || matches!(self.peek(), Some(b'|' | b')' | b'>')) {
                break;
            }
            let e = self.parse_unary()?;
            if !matches!(e, Expr::Empty) {
                parts.push(e);
            }
        }
        Ok(match parts.len() {
            0 => Expr::Empty,
            1 => parts.pop().unwrap_or(Expr::Empty),
            _ => Expr::And(parts),
        })
    }

    /// NOT 层：`!x`（最高优先级）。
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        if self.peek() == Some(b'!') {
            let start = self.pos;
            self.pos += 1;
            self.not_depth += 1;
            let inner = self.parse_unary()?;
            self.not_depth -= 1;
            if matches!(inner, Expr::Empty) {
                // 容错：单独的 `!`（用户正在打 `!foo`）→ 降级为字面 `!`。
                return Ok(self.name(NameTerm {
                    text: self.input[start..start + 1].to_string(),
                    span: start..start + 1,
                    kind: NameKind::Substring,
                    regex_idx: None,
                }));
            }
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        self.skip_ws();
        if self.at_end() {
            return Ok(Expr::Empty);
        }
        match self.peek() {
            Some(b'(' | b'<') => self.parse_group(),
            Some(b'"') => self.parse_phrase(),
            // 防御性：`|` / `)` / `>` 由上层消费，不该到达这里。
            Some(b'|' | b')' | b'>') => Ok(Expr::Empty),
            _ => {
                let span = self.bare_span();
                self.pos = span.end;
                let text = self.input[span.clone()].to_string();
                self.term_from_text(text, span)
            }
        }
    }

    /// `(...)` / `<...>` 分组；**未闭合时容错降级**为字面开括号（§7.3）。
    fn parse_group(&mut self) -> Result<Expr, ParseError> {
        let open = self.peek().unwrap_or(b'(');
        let close = if open == b'(' { b')' } else { b'>' };
        let open_pos = self.pos;
        // 记录副作用长度：未闭合时要连带回滚（否则内层已收集的 term / 正则
        // 会残留，回退后外层又解析一遍 → 重复项）。
        let terms_len = self.terms.len();
        let regexes_len = self.regexes.len();
        self.pos += 1;
        let inner = self.parse_or()?;
        self.skip_ws();
        if self.peek() == Some(close) {
            self.pos += 1;
            Ok(inner)
        } else {
            // 未闭合：把开括号当字面字符，位置回退到它之后，剩余部分由
            // 外层 parse_and 继续解析（用户还在打字时不应报错）。
            self.terms.truncate(terms_len);
            self.regexes.truncate(regexes_len);
            self.pos = open_pos + 1;
            Ok(self.name(NameTerm {
                text: char::from(open).to_string(),
                span: open_pos..open_pos + 1,
                kind: NameKind::Substring,
                regex_idx: None,
            }))
        }
    }

    fn parse_phrase(&mut self) -> Result<Expr, ParseError> {
        let (text, span) = self.scan_quoted();
        if text.is_empty() {
            return Ok(Expr::Empty); // `""` 无贡献
        }
        Ok(self.name(NameTerm {
            text,
            span,
            kind: NameKind::Phrase,
            regex_idx: None,
        }))
    }

    /// 读 `"..."` 的内容（**不**登记高亮 term），`pos` 停在收尾引号之后。
    ///
    /// 未闭合时读到串尾返回（用户正在打字，§7.3 容错）。调用方负责把
    /// [`Self::pos`] 推到位——[`parse_phrase`] 与 `content:"..."` 都要用。
    fn scan_quoted(&mut self) -> (String, Range<usize>) {
        let start = (self.pos + 1).min(self.input.len());
        let mut j = start;
        while j < self.bytes.len() && self.bytes[j] != b'"' {
            j += 1;
        }
        let text = self.input[start..j].to_string();
        self.pos = if j < self.bytes.len() { j + 1 } else { j };
        (text, start..j)
    }

    /// 修饰符值的结束位置：**不**在 `>` `<` 处截断（它们是数值比较符）。
    ///
    /// `stop_at_pipe = true` 时（`size:` / `depth:`）遇 `|` 停止——那里是查询的
    /// OR；`regex:` 传 false，让 `|` 归正则。
    fn scan_value_end(&self, start: usize, stop_at_pipe: bool) -> usize {
        let mut j = start;
        while j < self.bytes.len() {
            let c = self.bytes[j];
            if c.is_ascii_whitespace() || c == b'"' || (stop_at_pipe && c == b'|') {
                break;
            }
            j += 1;
        }
        j
    }

    /// 裸 term 的字节区间（到空白或语法字符为止）。
    fn bare_span(&self) -> Range<usize> {
        let start = self.pos;
        let mut j = start;
        while j < self.bytes.len() {
            let c = self.bytes[j];
            if c.is_ascii_whitespace() || matches!(c, b'|' | b'(' | b')' | b'<' | b'>' | b'"') {
                break;
            }
            j += 1;
        }
        start..j
    }

    /// 裸 term → 修饰符或名字 term。
    fn term_from_text(&mut self, text: String, span: Range<usize>) -> Result<Expr, ParseError> {
        if let Some(colon) = text.find(':') {
            let ident = text[..colon].to_string();
            if is_modifier(&ident) {
                let value_start = span.start + colon + 1;
                // `size:` / `depth:` 的值可能以 `>` / `<` 开头、`regex:` 的值常含
                // `(` `|` 等——它们同时是查询语法字符（`bare_span` 会在此截断），
                // 故这三个修饰符单独放宽扫描范围。
                let vspan = match ident.to_ascii_lowercase().as_str() {
                    // 正则的 `|` 属于正则本身，不能在 `|` 处截断（否则
                    // `regex:(a|b)` 会被切成 `regex:(a` 而报非法正则）。
                    "regex" => {
                        let end = self.scan_value_end(value_start, false);
                        self.pos = end;
                        value_start..end
                    }
                    "size" | "depth" => {
                        let end = self.scan_value_end(value_start, true);
                        self.pos = end;
                        value_start..end
                    }
                    _ => value_start..span.end,
                };
                let input = self.input;
                return self.modifier(&ident, &input[vspan.clone()], span, vspan);
            }
        }
        let kind = if text.contains('\\') || text.contains('/') {
            NameKind::Path
        } else if text.contains('*') || text.contains('?') {
            NameKind::Glob
        } else {
            NameKind::Substring
        };
        Ok(self.name(NameTerm {
            text,
            span,
            kind,
            regex_idx: None,
        }))
    }

    /// 登记一个名字 term（顺带收集高亮 term）。
    fn name(&mut self, t: NameTerm) -> Expr {
        if self.not_depth == 0 && matches!(t.kind, NameKind::Substring | NameKind::Phrase) {
            self.terms.push(Term {
                text: t.text.clone(),
                span: t.span.clone(),
                phrase: t.kind == NameKind::Phrase,
            });
        }
        Expr::Name(t)
    }

    /// 各修饰符分支（`ident` 已确认在 [`is_modifier`] 白名单内）。
    #[allow(clippy::too_many_lines)]
    fn modifier(
        &mut self,
        ident: &str,
        value: &str,
        span: Range<usize>,
        vspan: Range<usize>,
    ) -> Result<Expr, ParseError> {
        let v = value.trim();
        match ident.to_ascii_lowercase().as_str() {
            // `case:` / `nocase:` 是全局开关；若冒号后还有内容（如 `case:READ`），
            // 那部分按独立子串 term 处理——否则用户写的搜索词会被当成开关的参数
            // 直接丢掉（与 `file:notes` 的处理保持一致）。
            "case" => {
                self.case_sensitive = true;
                Ok(self.simple_name(v, NameKind::Substring, vspan))
            }
            "nocase" => {
                self.case_sensitive = false;
                Ok(self.simple_name(v, NameKind::Substring, vspan))
            }
            "file" => Ok(self.file_folder(v, false, vspan)),
            "folder" => Ok(self.file_folder(v, true, vspan)),
            "ext" => Ok(if v.is_empty() {
                Expr::Empty
            } else {
                self.name(NameTerm {
                    text: v.to_ascii_lowercase(),
                    span,
                    kind: NameKind::Ext,
                    regex_idx: None,
                })
            }),
            "path" => Ok(if v.is_empty() {
                Expr::Empty
            } else {
                self.name(NameTerm {
                    text: v.to_string(),
                    span,
                    kind: NameKind::Path,
                    regex_idx: None,
                })
            }),
            "regex" => {
                if v.is_empty() {
                    return Ok(Expr::Empty);
                }
                let re = Regex::new(v).map_err(|e| {
                    err(
                        &format!("正则表达式非法：{}", first_line(&e.to_string())),
                        vspan.clone(),
                    )
                })?;
                let idx = self.regexes.len();
                self.regexes.push(re);
                Ok(self.name(NameTerm {
                    text: v.to_string(),
                    span,
                    kind: NameKind::Regex,
                    regex_idx: Some(idx),
                }))
            }
            "ww" | "wholeword" => Ok(self.simple_name(v, NameKind::WholeWord, vspan)),
            "startwith" => Ok(self.simple_name(v, NameKind::StartWith, vspan)),
            "endwith" => Ok(self.simple_name(v, NameKind::EndWith, vspan)),
            "parent" => Ok(self.simple_name(v, NameKind::Parent, vspan)),
            "infolder" => Ok(self.simple_name(v, NameKind::InFolder, vspan)),
            "size" => Ok(match parse_num(v, &vspan, "size", true)? {
                Some(p) => Expr::Pred(Predicate::Size(p)),
                None => Expr::Empty,
            }),
            "depth" => Ok(match parse_num(v, &vspan, "depth", false)? {
                Some(p) => Expr::Pred(Predicate::Depth(p)),
                None => Expr::Empty,
            }),
            "dm" => Ok(date_expr(DateField::Modified, v, &vspan)?),
            "dc" => Ok(date_expr(DateField::Created, v, &vspan)?),
            "da" => Ok(date_expr(DateField::Accessed, v, &vspan)?),
            "attrib" => {
                if v.is_empty() {
                    return Ok(Expr::Empty);
                }
                let mut mask = 0u32;
                for ch in v.chars() {
                    match attrib_bit(ch) {
                        Some(b) => mask |= b,
                        None => {
                            return Err(err(
                                &format!(
                                    "无法识别的属性字母 `{ch}`（可用 R H S D A C E T O I N L P V）"
                                ),
                                vspan.clone(),
                            ))
                        }
                    }
                }
                Ok(if mask == 0 {
                    Expr::Empty
                } else {
                    Expr::Pred(Predicate::Attrib(mask))
                })
            }
            // M7 内容搜索（DECISIONS #95）：值可为裸词（到空白为止）或
            // `content:"带空格的短语"`。该项**不进 AST**，收进 `self.contents`，
            // 由引擎在候选集上读文件过滤；这里返回 `Expr::Empty` 表示
            // 「对 AST 无贡献」，不影响 AND 里其它条件的求值。
            "content" => {
                let (text, tspan) = if v.is_empty() && self.peek() == Some(b'"') {
                    self.scan_quoted()
                } else {
                    (v.to_string(), vspan.clone())
                };
                if text.is_empty() {
                    return Ok(Expr::Empty); // `content:` 裸写：用户还在打字，容错
                }
                if self.not_depth > 0 {
                    return Err(err(
                        "content: 不支持取反（判定「不包含」需要读完整卷的文件）",
                        span,
                    ));
                }
                self.contents.push(ContentSpec {
                    needle: text,
                    span: tspan,
                    case_sensitive: self.case_sensitive,
                });
                Ok(Expr::Empty)
            }
            _ => unreachable!("is_modifier 已过滤"),
        }
    }

    fn simple_name(&mut self, v: &str, kind: NameKind, vspan: Range<usize>) -> Expr {
        if v.is_empty() {
            return Expr::Empty;
        }
        self.name(NameTerm {
            text: v.to_string(),
            span: vspan,
            kind,
            regex_idx: None,
        })
    }

    /// `file:` / `folder:` 带值时 = 子串 + 类型过滤；不带值时只做类型过滤。
    fn file_folder(&mut self, v: &str, is_dir: bool, vspan: Range<usize>) -> Expr {
        let pred = Expr::Pred(if is_dir {
            Predicate::IsDir
        } else {
            Predicate::IsFile
        });
        if v.is_empty() {
            return pred;
        }
        let name = self.name(NameTerm {
            text: v.to_string(),
            span: vspan,
            kind: NameKind::Substring,
            regex_idx: None,
        });
        Expr::And(vec![name, pred])
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

/// `dm:` / `dc:` / `da:` → 表达式（空值 = 无贡献，不报错）。
fn date_expr(field: DateField, v: &str, span: &Range<usize>) -> Result<Expr, ParseError> {
    Ok(match parse_date(v, span)? {
        Some(spec) => Expr::Pred(Predicate::Date { field, spec }),
        None => Expr::Empty,
    })
}

/// 解析 `size:` / `depth:` 的值（比较、范围、单位）。
///
/// 空值 → `None`（无贡献，容错）；单位 `b kb mb gb tb pb`（1024 进制）；
/// `depth:` 无单位。
fn parse_num(
    v: &str,
    span: &Range<usize>,
    name: &str,
    with_unit: bool,
) -> Result<Option<NumPred>, ParseError> {
    if v.is_empty() {
        return Ok(None);
    }
    if let Some(pos) = v.find("..") {
        let lo = parse_amount(&v[..pos], with_unit).ok_or_else(|| {
            err(
                &format!("{name}: 范围下界 `{}` 无法解析", &v[..pos]),
                span.clone(),
            )
        })?;
        let hi = parse_amount(&v[pos + 2..], with_unit).ok_or_else(|| {
            err(
                &format!("{name}: 范围上界 `{}` 无法解析", &v[pos + 2..]),
                span.clone(),
            )
        })?;
        return Ok(Some(if lo <= hi {
            NumPred::Range { lo, hi }
        } else {
            NumPred::Range { lo: hi, hi: lo }
        }));
    }
    let (op, rest) = match v.as_bytes() {
        [b'>', b'=', ..] => (CmpOp::Ge, &v[2..]),
        [b'<', b'=', ..] => (CmpOp::Le, &v[2..]),
        [b'>', ..] => (CmpOp::Gt, &v[1..]),
        [b'<', ..] => (CmpOp::Lt, &v[1..]),
        [b'=', ..] => (CmpOp::Eq, &v[1..]),
        _ => (CmpOp::Eq, v),
    };
    let value = parse_amount(rest, with_unit).ok_or_else(|| {
        err(
            &format!("{name}: `{rest}` 不是合法数值（例：10mb、1kb..1mb）"),
            span.clone(),
        )
    })?;
    Ok(Some(NumPred::Cmp { op, value }))
}

/// 数值 + 可选单位 → 字节数（1024 进制）。
fn parse_amount(s: &str, with_unit: bool) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    if num.is_empty() {
        return None;
    }
    let n: u64 = num.parse().ok()?;
    let unit = unit.trim();
    if !with_unit {
        return if unit.is_empty() { Some(n) } else { None };
    }
    let mul: u64 = match unit {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        "t" | "tb" => 1024 * 1024 * 1024 * 1024,
        "p" | "pb" => 1024 * 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };
    n.checked_mul(mul)
}

/// 解析 `dm:` / `dc:` / `da:` 的值；空值 → `None`（无贡献）。
fn parse_date(v: &str, span: &Range<usize>) -> Result<Option<DateSpec>, ParseError> {
    if v.is_empty() {
        return Ok(None);
    }
    let s = v.trim().to_ascii_lowercase();
    if s == "today" {
        return Ok(Some(DateSpec::Today));
    }
    if s == "yesterday" {
        return Ok(Some(DateSpec::Yesterday));
    }
    if let Some(rest) = s.strip_prefix("last").and_then(|r| r.strip_suffix("days")) {
        if let Ok(days) = rest.parse::<u32>() {
            return Ok(Some(DateSpec::RelativeDays { days }));
        }
    }
    if let Some(pos) = s.find("..") {
        if let (Some(from), Some(to)) = (parse_ymd(&s[..pos]), parse_ymd(&s[pos + 2..])) {
            return Ok(Some(DateSpec::Range { from, to }));
        }
        return Err(err(
            &format!("日期范围 `{v}` 无法解析（用 YYYY-MM-DD）"),
            span.clone(),
        ));
    }
    if let Some((year, month, day)) = parse_ymd(&s) {
        return Ok(Some(DateSpec::Day { year, month, day }));
    }
    Err(err(
        &format!("日期 `{v}` 无法解析（可用 today / yesterday / last7days / YYYY-MM-DD）"),
        span.clone(),
    ))
}

/// `YYYY-MM-DD` → `(year, month, day)`（只校验形状，不校验日期真实存在）。
fn parse_ymd(s: &str) -> Option<(i32, u32, u32)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return None;
    }
    let year: i32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// 解析查询串（DESIGN §7.3：错误带 byte span，干净返回、不 panic）。
pub fn parse(input: &str) -> Result<Query, ParseError> {
    let mut p = Parser {
        input,
        bytes: input.as_bytes(),
        pos: 0,
        case_sensitive: false,
        terms: Vec::new(),
        regexes: Vec::new(),
        contents: Vec::new(),
        not_depth: 0,
    };
    let root = p.parse_or()?;
    p.skip_ws();
    validate_content(&root, &p.contents)?;
    Ok(Query {
        root,
        terms: p.terms,
        case_sensitive: p.case_sensitive,
        context: EvalContext::default(),
        regexes: p.regexes,
        content: p.contents,
    })
}

/// `content:` 的语义约束（DECISIONS #95）。
///
/// 内容匹配的代价是「一次读文件」，所以必须保证：
/// 1. 有名字级条件先把候选集压下来（否则等于读整卷）；
/// 2. 只在前向 AND 语义下使用——`|` 的某个分支若只有 `content:`，
///    同样会退化成全表读文件；`!content:` 需要读完全部文件才能判定，
///    两者都在解析期直接拒绝并给出位置，让用户改写查询。
fn validate_content(root: &Expr, contents: &[ContentSpec]) -> Result<(), ParseError> {
    let Some(first) = contents.first() else {
        return Ok(());
    };
    if expr_has_or(root) {
        return Err(err(
            "content: 只能与其它条件用 AND 组合（不支持 | 分组）",
            first.span.clone(),
        ));
    }
    if !expr_has_name(root) {
        return Err(err(
            "content: 需要同时给出文件名或路径条件（会读文件内容，例如 *.txt content:foo）",
            first.span.clone(),
        ));
    }
    Ok(())
}

fn expr_has_or(e: &Expr) -> bool {
    match e {
        Expr::Or(_) => true,
        Expr::Not(i) => expr_has_or(i),
        Expr::And(v) => v.iter().any(expr_has_or),
        _ => false,
    }
}

fn expr_has_name(e: &Expr) -> bool {
    match e {
        Expr::Name(_) => true,
        Expr::Not(i) => expr_has_name(i),
        Expr::And(v) | Expr::Or(v) => v.iter().any(expr_has_name),
        _ => false,
    }
}

// ───────────────────────── 单元测试（语法层） ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> Query {
        parse(s).unwrap()
    }

    fn texts(query: &Query) -> Vec<String> {
        query.terms.iter().map(|t| t.text.clone()).collect()
    }

    #[test]
    fn m1_basics_still_work() {
        assert_eq!(texts(&q("a b")), vec!["a", "b"]);
        assert_eq!(texts(&q("\"a b\"")), vec!["a b"]);
        assert!(q("").is_empty());
        assert!(Query::default().is_empty());
    }

    #[test]
    fn or_and_not_parse() {
        assert!(matches!(q("a|b").root, Expr::Or(_)));
        assert!(matches!(q("!x").root, Expr::Not(_)));
        // NOT 优先级高于 AND
        match q("!a b").root {
            Expr::And(v) => assert!(matches!(v[0], Expr::Not(_))),
            other => panic!("期望 And，得到 {other:?}"),
        }
        // AND 优先级高于 OR
        match q("a b|c").root {
            Expr::Or(v) => assert!(matches!(v[0], Expr::And(_))),
            other => panic!("期望 Or，得到 {other:?}"),
        }
    }

    #[test]
    fn group_and_unclosed_group_falls_back() {
        assert!(matches!(q("(a|b) c").root, Expr::And(_)));
        // 未闭合 `(` → 降级为字面 "("，不报错
        let g = q("(a");
        assert_eq!(texts(&g), vec!["(", "a"]);
    }

    #[test]
    fn path_term_and_glob() {
        assert_eq!(
            q("src\\main").root,
            Expr::Name(NameTerm {
                text: "src\\main".to_string(),
                span: 0..8,
                kind: NameKind::Path,
                regex_idx: None,
            })
        );
        match &q("*.txt").root {
            Expr::Name(t) => assert_eq!(t.kind, NameKind::Glob),
            other => panic!("期望 Name，得到 {other:?}"),
        }
    }

    #[test]
    fn modifiers_parse() {
        match q("ext:txt;md").root {
            Expr::Name(t) => {
                assert_eq!(t.kind, NameKind::Ext);
                assert_eq!(t.text, "txt;md");
            }
            other => panic!("期望 Name，得到 {other:?}"),
        }
        assert!(matches!(q("folder:").root, Expr::Pred(Predicate::IsDir)));
        assert!(matches!(q("file:").root, Expr::Pred(Predicate::IsFile)));
        assert!(matches!(q("case:").root, Expr::Empty));
        assert!(q("case:Abc").case_sensitive);
        assert!(!q("nocase:Abc").case_sensitive);
        // 未知 `xxx:` 不当修饰符，降级为字面子串
        assert_eq!(texts(&q("http:foo")), vec!["http:foo"]);
    }

    #[test]
    fn size_units_and_ranges() {
        assert_eq!(
            q("size:>10mb").root,
            Expr::Pred(Predicate::Size(NumPred::Cmp {
                op: CmpOp::Gt,
                value: 10 * 1024 * 1024
            }))
        );
        assert_eq!(
            q("size:1kb..1mb").root,
            Expr::Pred(Predicate::Size(NumPred::Range {
                lo: 1024,
                hi: 1024 * 1024
            }))
        );
        assert_eq!(
            q("size:100").root,
            Expr::Pred(Predicate::Size(NumPred::Cmp {
                op: CmpOp::Eq,
                value: 100
            }))
        );
        assert!(matches!(
            q("depth:>3").root,
            Expr::Pred(Predicate::Depth(_))
        ));
    }

    #[test]
    fn date_forms() {
        assert!(matches!(
            q("dm:today").root,
            Expr::Pred(Predicate::Date {
                spec: DateSpec::Today,
                ..
            })
        ));
        assert!(matches!(
            q("dm:last7days").root,
            Expr::Pred(Predicate::Date {
                spec: DateSpec::RelativeDays { days: 7 },
                ..
            })
        ));
        assert!(matches!(
            q("dc:2024-01-01..2024-06-30").root,
            Expr::Pred(Predicate::Date {
                spec: DateSpec::Range { .. },
                ..
            })
        ));
        // 空值容错：不报错、无贡献
        for half in ["dm:", "size:", "attrib:", "ext:", "path:", "regex:"] {
            assert!(parse(half).is_ok(), "{half} 应容错");
            assert!(parse(half).unwrap().is_empty(), "{half} 应无贡献");
        }
    }

    #[test]
    fn errors_have_span() {
        for bad in ["size:abc", "regex:(", "dm:notadate", "attrib:Z", "depth:x"] {
            let e = parse(bad).unwrap_err();
            assert!(!e.message.is_empty(), "{bad} 应带消息");
            assert!(e.span.start < e.span.end, "{bad} span 非法");
            assert!(e.span.end <= bad.len(), "{bad} span 越界");
        }
    }

    #[test]
    fn filetime_roundtrip_known_day() {
        // 2024-01-01 00:00 UTC
        let ft = filetime_from_civil(2024, 1, 1, 0);
        assert_eq!(ft / FILETIME_PER_SEC - UNIX_TO_FILETIME_SECS, 1_704_067_200);
        // 东八区：本地 2024-01-01 00:00 = UTC 2023-12-31 16:00
        let ft8 = filetime_from_civil(2024, 1, 1, 480);
        assert_eq!(ft - ft8, 480 * 60 * FILETIME_PER_SEC);
    }

    #[test]
    fn content_parses_bare_and_phrase() {
        let c = q("*.txt content:hello");
        assert_eq!(c.content.len(), 1);
        assert_eq!(c.content[0].needle, "hello");
        assert!(!c.content[0].case_sensitive);
        assert!(!c.is_empty());
        // 短语形式（bare_span 在引号处截断，由 content 分支补扫）
        let c2 = q(r#"*.txt content:"hello world""#);
        assert_eq!(c2.content[0].needle, "hello world");
        // 多个 content = AND
        let c3 = q("*.log content:error content:timeout");
        assert_eq!(c3.content.len(), 2);
        // `case:` 写在前面才作用于后面的 content（解析期取值）
        let c4 = q("case: *.txt content:Hello");
        assert!(c4.content[0].case_sensitive);
        // 裸写 / 空短语容错，无贡献
        assert!(q("*.txt content:").content.is_empty());
        assert!(q(r#"*.txt content:"""#).content.is_empty());
    }

    #[test]
    fn content_rejects_expensive_shapes() {
        // 没有名字条件 → 等于读整卷
        let e = parse("content:foo").unwrap_err();
        assert!(e.message.contains("文件名"), "{}", e.message);
        // 取反
        let e = parse("!content:foo bar").unwrap_err();
        assert!(e.message.contains("取反"), "{}", e.message);
        // OR 分组
        let e = parse("a|b content:c").unwrap_err();
        assert!(e.message.contains("AND"), "{}", e.message);
        // 位置信息可用
        assert!(e.span.end <= "a|b content:c".len());
    }
}
