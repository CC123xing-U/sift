//! 查询语言 M1 子集：空格 AND + `"..."` 短语 + 名字子串（DESIGN §7.1 子集）。
//!
//! 不支持（M4 实现，本轮干净返回 [`ParseError`]）：`|` OR、`!` NOT、
//! `()` / `<>` 分组、通配符 `*` `?`、`modifier:` 修饰符、含 `\` `/` 的
//! 路径 term（路径匹配）。

use std::ops::Range;

use crate::case::simple_lowercase;
use crate::ParseError;

/// 一个 AND 项（DESIGN §7.1：空格 = AND）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// 原始文本（未做大小写折叠；高亮 / 大小写敏感匹配用）。
    pub text: String,
    /// 在查询串中的字节区间 `[start, end)`（短语不含两侧引号）。
    pub span: Range<usize>,
    /// 是否来自 `"..."` 短语（可含空格；解析后与普通 term 同样按子串匹配）。
    pub phrase: bool,
}

impl Term {
    /// 按大小写策略得到 needle 字节（不敏感时做简单小写折叠，DESIGN §4.1）。
    pub(crate) fn needle(&self, case_sensitive: bool) -> Vec<u8> {
        if case_sensitive {
            self.text.clone().into_bytes()
        } else {
            simple_lowercase(self.text.as_bytes())
        }
    }
}

/// 解析后的查询（M1：仅 AND 子串；OR / NOT / 通配符 / 修饰符属 M4）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// AND 项；空 = 空查询（搜索返回空结果，不 panic）。
    pub terms: Vec<Term>,
    /// 区分大小写（引擎按 `SearchOptions::case_sensitive` 设置；默认 false，
    /// 即匹配 `names_lower`，DESIGN §4.1）。
    pub case_sensitive: bool,
}

impl Query {
    /// 是否空查询（无任何 term）。
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// 解析查询串（DESIGN §7.3：错误带 byte span，干净返回、不 panic）。
///
/// M1 语法：
/// - 空白（ASCII）分隔的 term 之间取 AND；
/// - `"..."` 为短语（可含空格，按子串匹配；未闭合引号容错为到串尾）；
/// - 每个 term 均按**名字子串**匹配。
pub fn parse(input: &str) -> Result<Query, ParseError> {
    let bytes = input.as_bytes();
    let mut terms: Vec<Term> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let (text, phrase, span, next) = if bytes[i] == b'"' {
            // 短语：到下一个 '"' 或串尾（容错未闭合引号，DESIGN §7.3）
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            let next = if j < bytes.len() { j + 1 } else { j };
            (input[start..j].to_string(), true, start..j, next)
        } else {
            // 普通 term：到空白或 '"' 为止
            let start = i;
            let mut j = i;
            while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'"' {
                j += 1;
            }
            (input[start..j].to_string(), false, start..j, j)
        };
        i = next;
        if text.is_empty() {
            continue; // `""` 空短语：匹配一切，视为无贡献
        }
        validate_term(&text, span.clone(), phrase)?;
        terms.push(Term { text, span, phrase });
    }
    Ok(Query {
        terms,
        case_sensitive: false,
    })
}

/// 检查 term 是否含 M1 不支持的语法；命中即返回带 span 的错误。
fn validate_term(text: &str, span: Range<usize>, phrase: bool) -> Result<(), ParseError> {
    for b in text.bytes() {
        // 路径分隔符：任何 term（含短语）含 \ 或 / → 路径匹配，属 M4
        if b == b'\\' || b == b'/' {
            return Err(err("路径匹配（term 含 \\ 或 /）尚未支持（计划 M4）", span));
        }
        if phrase {
            continue; // 短语内其余字符按字面（文件名可含 ( ) ! 等）
        }
        let msg = match b {
            b'|' => Some("布尔语法（| OR）尚未支持（计划 M4）"),
            b'!' => Some("布尔语法（! NOT）尚未支持（计划 M4）"),
            b'(' | b')' => Some("分组语法（()）尚未支持（计划 M4）"),
            b'<' | b'>' => Some("分组语法（<>）尚未支持（计划 M4）"),
            b'*' | b'?' => Some("通配符（* ?）尚未支持（计划 M4）"),
            b':' => Some("修饰符语法（如 size:）尚未支持（计划 M4）"),
            _ => None,
        };
        if let Some(m) = msg {
            return Err(err(m, span));
        }
    }
    Ok(())
}

fn err(message: &str, span: Range<usize>) -> ParseError {
    ParseError {
        message: message.to_string(),
        span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(q: &str) -> Vec<String> {
        parse(q)
            .unwrap()
            .terms
            .iter()
            .map(|t| t.text.clone())
            .collect()
    }

    #[test]
    fn single_term() {
        let q = parse("readme").unwrap();
        assert_eq!(q.terms.len(), 1);
        assert_eq!(q.terms[0].text, "readme");
        assert_eq!(q.terms[0].span, 0..6);
        assert!(!q.terms[0].phrase);
        assert!(!q.case_sensitive);
        assert!(!q.is_empty());
    }

    #[test]
    fn and_terms() {
        let q = parse("foo bar baz").unwrap();
        assert_eq!(q.terms.len(), 3);
        assert_eq!(terms("foo bar baz"), vec!["foo", "bar", "baz"]);
        assert_eq!(q.terms[1].span, 4..7);
    }

    #[test]
    fn multiple_spaces_and_tabs() {
        assert_eq!(terms("  foo \t bar  "), vec!["foo", "bar"]);
    }

    #[test]
    fn phrase_with_space() {
        let q = parse("\"my docs\"").unwrap();
        assert_eq!(q.terms.len(), 1);
        assert_eq!(q.terms[0].text, "my docs");
        assert!(q.terms[0].phrase);
        assert_eq!(q.terms[0].span, 1..8); // 不含引号
    }

    #[test]
    fn phrase_mixed_with_terms() {
        assert_eq!(terms("a \"b c\" d"), vec!["a", "b c", "d"]);
    }

    #[test]
    fn phrase_allows_literal_operators() {
        // 短语内 ( ) ! | * ? : 按字面（Windows 文件名可含 ( ) !）
        let q = parse("\"report (final) v2!\"").unwrap();
        assert_eq!(q.terms.len(), 1);
        assert_eq!(q.terms[0].text, "report (final) v2!");
    }

    #[test]
    fn empty_queries() {
        assert!(parse("").unwrap().terms.is_empty());
        assert!(parse("   \t ").unwrap().terms.is_empty());
        assert!(parse("\"\"").unwrap().terms.is_empty());
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn unterminated_quote_graceful() {
        let q = parse("\"abc").unwrap();
        assert_eq!(terms("\"abc"), vec!["abc"]);
        assert!(q.terms[0].phrase);
    }

    #[test]
    fn quote_starts_after_unquoted_text() {
        assert_eq!(terms("ab\"cd ef\""), vec!["ab", "cd ef"]);
    }

    #[test]
    fn unicode_terms() {
        assert_eq!(terms("中文 abc"), vec!["中文", "abc"]);
        assert_eq!(terms("\"拼音 搜索\""), vec!["拼音 搜索"]);
    }

    fn err_of(q: &str) -> ParseError {
        parse(q).unwrap_err()
    }

    #[test]
    fn error_or() {
        let e = err_of("a|b");
        assert!(e.message.contains('|'), "{}", e.message);
        assert_eq!(e.span, 0..3); // 整个 term
        let e = err_of("foo | bar");
        assert_eq!(e.span, 4..5); // 单独的 "|" term
    }

    #[test]
    fn error_not() {
        let e = err_of("!x");
        assert!(e.message.contains('!'));
        assert_eq!(e.span, 0..2);
    }

    #[test]
    fn error_grouping() {
        assert!(err_of("(x)").message.contains("分组"));
        assert!(err_of("<x>").message.contains("分组"));
    }

    #[test]
    fn error_wildcards() {
        let e = err_of("*.txt");
        assert!(e.message.contains("通配符"));
        assert_eq!(e.span, 0..5);
        assert!(err_of("a?b").message.contains("通配符"));
    }

    #[test]
    fn error_modifiers() {
        let e = err_of("size:>10mb");
        assert!(e.message.contains("修饰符"));
        assert_eq!(e.span, 0..10);
        assert!(err_of("dm:today").message.contains("修饰符"));
        assert!(err_of("regex:^a").message.contains("修饰符"));
    }

    #[test]
    fn error_path_separators() {
        let e = err_of("Windows\\System32");
        assert!(e.message.contains("路径"), "{}", e.message);
        assert_eq!(e.span, 0..16);
        assert!(err_of("a/b").message.contains("路径"));
        // 短语里的路径分隔符同样报错（路径匹配属 M4）
        assert!(err_of("\"a\\b\"").message.contains("路径"));
        assert!(err_of("\"C:\\dir\"").message.contains("路径"));
    }

    #[test]
    fn error_span_is_byte_offset() {
        // 中文 term 后跟不支持的语法：span 必须是字节偏移
        let e = err_of("中文 |");
        assert_eq!(e.span, 7..8); // "中文" 3+3 字节，空格 1
        let e = err_of("a 中文|");
        assert_eq!(e.span, 2..9); // term "中文|" = 3+3+1 字节
    }

    #[test]
    fn first_error_wins() {
        // 同一 term 内多个问题：按出现顺序报第一个
        let e = err_of("a|b?c");
        assert!(e.message.contains('|'));
    }

    #[test]
    fn error_display_has_position() {
        let e = err_of("|");
        let s = e.to_string();
        assert!(s.contains("位置 0..1"), "{s}");
    }

    #[test]
    fn colon_in_middle_errors() {
        assert!(err_of("note:important").message.contains("修饰符"));
    }

    #[test]
    fn literal_punctuation_ok_outside_quotes() {
        // Windows 文件名合法字符不应误报
        assert_eq!(terms("a-b_c.d,e[f]g'h"), vec!["a-b_c.d,e[f]g'h"]);
        assert_eq!(
            terms("#hash $dollar %pct &amp +plus =eq ~tilde"),
            vec!["#hash", "$dollar", "%pct", "&amp", "+plus", "=eq", "~tilde"]
        );
    }
}
