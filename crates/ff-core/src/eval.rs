//! 单条目 AST 判定（DESIGN §7.3）。
//!
//! 索引内条目走 [`VolumeIndex::search`](crate::VolumeIndex::search) 的
//! `memmem` 快速路径；**不在索引里的条目**（M2 的 Overlay 增量条目、以及任何
//! 只有元数据没有索引位置的场景）用本模块的 [`Query::matches_facts`] 按同一棵
//! AST 判定，保证两条路径语义一致——否则 M4 的 OR / NOT / 修饰符会在增量
//! 条目上漏匹配。

use super::case::simple_lowercase;
use super::query::{date_range, DateField, Expr, NameKind, NameTerm, Predicate, Query};

/// 一条待判定条目的已知事实（不依赖索引）。
///
/// 缺失的字段传 `None` / `0`：对应条件判定为 false（并在文档里说明限制）。
#[derive(Clone, Copy, Debug, Default)]
pub struct EntryFacts<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    /// Win32 文件属性位（`FILE_ATTRIBUTE_*`）。
    pub attrs: u32,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
    /// 父目录名（`parent:` 用）。
    pub parent_name: Option<&'a str>,
    /// 完整路径（路径类条件用）。
    pub path: Option<&'a str>,
    /// 目录深度（根 = 0）。
    pub depth: u32,
}

impl Query {
    /// 按 AST 判定一条已知事实的条目是否命中。
    pub fn matches_facts(&self, f: &EntryFacts<'_>) -> bool {
        if self.is_empty() {
            return false; // 空查询 → 空结果（与索引路径一致）
        }
        self.matches_expr(&self.root, f)
    }

    fn matches_expr(&self, e: &Expr, f: &EntryFacts<'_>) -> bool {
        match e {
            Expr::Empty => true,
            Expr::Name(t) => self.name_matches(t, f),
            Expr::Pred(p) => self.pred_matches(p, f),
            Expr::Not(i) => !self.matches_expr(i, f),
            Expr::And(v) => v.iter().all(|x| self.matches_expr(x, f)),
            Expr::Or(v) => v.iter().any(|x| self.matches_expr(x, f)),
        }
    }

    fn name_matches(&self, t: &NameTerm, f: &EntryFacts<'_>) -> bool {
        let lowered;
        let hay: &[u8] = if self.case_sensitive {
            f.name.as_bytes()
        } else {
            lowered = simple_lowercase(f.name.as_bytes());
            &lowered
        };
        let needle = t.needle(self.case_sensitive);
        match t.kind {
            NameKind::Substring | NameKind::Phrase => contains(hay, &needle),
            NameKind::Glob => glob_match(hay, &needle),
            NameKind::Regex => match t.regex_idx.and_then(|i| self.regexes.get(i)) {
                Some(re) => re.is_match(f.name),
                None => false,
            },
            NameKind::WholeWord => wholeword_match(hay, &needle),
            NameKind::StartWith => hay.starts_with(&needle),
            NameKind::EndWith => hay.ends_with(&needle),
            NameKind::Ext => ext_match(hay, &t.text),
            NameKind::Parent => match f.parent_name {
                Some(p) => {
                    let l;
                    let ph: &[u8] = if self.case_sensitive {
                        p.as_bytes()
                    } else {
                        l = simple_lowercase(p.as_bytes());
                        &l
                    };
                    contains(ph, &needle)
                }
                None => false,
            },
            NameKind::Path | NameKind::InFolder => match f.path {
                Some(p) => {
                    let (hp, np) = if self.case_sensitive {
                        (p.to_string(), t.text.clone())
                    } else {
                        (p.to_lowercase(), t.text.to_lowercase())
                    };
                    if t.kind == NameKind::Path {
                        hp.contains(&np)
                    } else {
                        hp.contains(&format!("\\{np}\\"))
                    }
                }
                None => false,
            },
        }
    }

    fn pred_matches(&self, p: &Predicate, f: &EntryFacts<'_>) -> bool {
        match *p {
            Predicate::IsDir => f.is_dir,
            Predicate::IsFile => !f.is_dir,
            // 这里拿到的是原始 Win32 属性位，因此 `attrib:` 的全部字母都能判定
            // （索引路径只保存了 R/H/S/D 四个派生位，见 DECISIONS）。
            Predicate::Attrib(mask) => mask != 0 && (f.attrs & mask) != 0,
            Predicate::Size(n) => n.test(f.size),
            Predicate::Depth(n) => n.test(u64::from(f.depth)),
            Predicate::Date { field, spec } => {
                let v = match field {
                    DateField::Modified => f.mtime,
                    DateField::Created => f.ctime,
                    DateField::Accessed => f.atime,
                };
                let (start, end) = date_range(spec, self.context);
                v >= start && v < end
            }
        }
    }
}

/// 子串包含（空 needle 视为不命中）。
pub(crate) fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// 通配符匹配（`*` = 任意长度，`?` = 单字节；回溯两指针法）。
pub(crate) fn glob_match(hay: &[u8], pat: &[u8]) -> bool {
    let (mut h, mut p) = (0usize, 0usize);
    let (mut star, mut star_h) = (None::<usize>, 0usize);
    while h < hay.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == hay[h]) {
            h += 1;
            p += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = Some(p);
            star_h = h;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            star_h += 1;
            h = star_h;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

/// 整词匹配：按 ASCII 非字母数字切分后逐词比较。
///
/// 名字含非 ASCII（中文等）时没有词边界概念，退化为「整个名字相等」。
pub(crate) fn wholeword_match(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return false;
    }
    if hay.iter().any(|b| !b.is_ascii_alphanumeric() && *b < 0x80) {
        return hay
            .split(|b| !b.is_ascii_alphanumeric())
            .any(|w| w == needle);
    }
    hay == needle
}

/// `ext:a;b;c`：名字最后一个 `.` 之后的部分是否属于集合（`exts` 已小写）。
pub(crate) fn ext_match(hay: &[u8], exts: &str) -> bool {
    let dot = match hay.iter().rposition(|&b| b == b'.') {
        Some(i) => i,
        None => return false,
    };
    let ext = simple_lowercase(&hay[dot + 1..]);
    exts.split(';')
        .map(str::trim)
        .any(|e| !e.is_empty() && ext == e.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::super::parse;
    use super::*;

    fn facts(name: &str) -> EntryFacts<'_> {
        EntryFacts {
            name,
            is_dir: false,
            attrs: 0,
            size: 1024,
            mtime: 0,
            ctime: 0,
            atime: 0,
            parent_name: Some("Docs"),
            path: Some("C:\\Docs\\report.pdf"),
            depth: 1,
        }
    }

    #[test]
    fn or_not_and_modifiers_on_single_entry() {
        assert!(parse("report|zzz")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
        assert!(!parse("x|y").unwrap().matches_facts(&facts("report.pdf")));
        assert!(parse("!x").unwrap().matches_facts(&facts("report.pdf")));
        assert!(!parse("!report")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
        assert!(parse("ext:pdf")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
        assert!(!parse("ext:zip")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
        assert!(parse("size:1kb")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
        assert!(parse("parent:Docs")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
        assert!(parse("depth:1")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
        assert!(parse("path:Docs")
            .unwrap()
            .matches_facts(&facts("report.pdf")));
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(!Query::default().matches_facts(&facts("report.pdf")));
        assert!(!parse("").unwrap().matches_facts(&facts("report.pdf")));
    }
}
