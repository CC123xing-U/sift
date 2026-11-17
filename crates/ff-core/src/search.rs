//! 查询求值（DESIGN §7.3）—— M4：AST 求值（AND / OR / NOT）+ 属性过滤
//! + 路径 / 通配符 / 正则 / 词边界。
//!
//! 求值顺序按成本递增（§7.3）：
//! 1. 可名字级快速匹配的 term（子串 / 短语）走 **一次大 `memmem` 扫描**出候选；
//! 2. 其余条件（属性、路径、通配、正则）只在候选集上校验；
//! 3. 没有任何快速 term 时（纯属性 / 纯路径查询）才全表并行扫描。
//!
//! 结果一律 **升序去重**；取消令牌每 ~4K 条目检查一次。

use rayon::prelude::*;

use std::cell::RefCell;

use super::cancel::CancelToken;
use super::case::{simple_lowercase, simple_lowercase_into};
use super::entry::{flags, EntryId, VolumeIndex, NONE};
use super::eval::{contains, ext_match, glob_match, wholeword_match};
use super::query::{date_range, DateField, Expr, NameKind, NameTerm, Predicate, Query};
use ff_match::scan_names;

/// 候选校验 / 全表扫描的并行批次大小（与取消检查间隔一致，DESIGN §7.3：约 4K）。
const VERIFY_CHUNK: usize = 4096;

// 大小写折叠的复用缓冲（按线程各一份；rayon 工作线程互不干扰）。
// 全表扫描类查询每条名字都要折叠一次，复用容量可省掉这部分分配。
thread_local! {
    static HAY_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static NEEDLE_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn take_hay_buf() -> Vec<u8> {
    HAY_BUF.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

fn give_hay_buf(v: Vec<u8>) {
    HAY_BUF.with(|b| *b.borrow_mut() = v)
}

fn take_needle_buf() -> Vec<u8> {
    NEEDLE_BUF.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

fn give_needle_buf(v: Vec<u8>) {
    NEEDLE_BUF.with(|b| *b.borrow_mut() = v)
}
/// 深度上溯的层数上限（防御 parent 环；`path.rs` 另有环检测）。
const DEPTH_MAX: u32 = 4096;

/// 通配符 / 扩展名的**字面探针**：匹配成功的名字必然包含其中某段文本，
/// 因此可先用它们走 `memmem` 出候选集再逐条验证（剪枝）。
///
/// - Glob：按 `*` `?` 切分后取最长段（`*.log` → `.log`）；
/// - Ext：**每个**扩展名各出一个 `.ext` 探针（多扩展名取并集）。
fn literal_probes(t: &NameTerm) -> Vec<String> {
    match t.kind {
        NameKind::Glob => {
            let mut best = "";
            for seg in t.text.split(['*', '?']) {
                if seg.len() > best.len() {
                    best = seg;
                }
            }
            if best.is_empty() {
                Vec::new()
            } else {
                vec![best.to_string()]
            }
        }
        NameKind::Ext => t
            .text
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|e| format!(".{e}"))
            .collect(),
        _ => Vec::new(),
    }
}

impl VolumeIndex {
    /// 求值查询，返回命中条目 id（**升序去重**）。
    ///
    /// - 空查询 → 空结果，不 panic；
    /// - 取消（已触发）→ 空结果（不返回半成品）。
    pub fn search(&self, q: &Query, cancel: &CancelToken) -> Vec<EntryId> {
        if q.is_empty() || cancel.is_cancelled() {
            return Vec::new();
        }
        let out = self.eval(&q.root, q, cancel);
        if cancel.is_cancelled() {
            Vec::new()
        } else {
            out
        }
    }

    /// AST 求值（返回升序去重结果）。
    fn eval(&self, e: &Expr, q: &Query, cancel: &CancelToken) -> Vec<EntryId> {
        match e {
            Expr::Empty => Vec::new(),
            Expr::Name(t) => self.eval_name(t, q, cancel),
            Expr::Pred(p) => self.all_where(|id| self.pred_matches(id, p, q), cancel),
            Expr::Not(inner) => {
                let sub = self.eval(inner, q, cancel);
                if cancel.is_cancelled() {
                    return Vec::new();
                }
                self.complement(&sub, cancel)
            }
            Expr::Or(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    if cancel.is_cancelled() {
                        return Vec::new();
                    }
                    out.extend(self.eval(p, q, cancel));
                }
                out.sort_unstable();
                out.dedup();
                out
            }
            Expr::And(parts) => self.eval_and(parts, q, cancel),
        }
    }

    /// AND：选一个「驱动词」出候选，其余条件只在候选上校验（§7.3-2）。
    fn eval_and(&self, parts: &[Expr], q: &Query, cancel: &CancelToken) -> Vec<EntryId> {
        // 驱动词 = 最长的可快速子串 term（稀有词启发式）。
        let mut driver: Option<(usize, usize)> = None;
        for (i, p) in parts.iter().enumerate() {
            if let Expr::Name(t) = p {
                if t.is_fast_substring() {
                    let len = t.text.len();
                    if driver.is_none_or(|(_, best)| len > best) {
                        driver = Some((i, len));
                    }
                }
            }
        }
        let ids = match driver {
            Some((i, _)) => match &parts[i] {
                Expr::Name(t) => self.eval_name(t, q, cancel),
                _ => self.all_ids(cancel),
            },
            None => self.all_ids(cancel),
        };
        if cancel.is_cancelled() {
            return Vec::new();
        }
        let rest: Vec<&Expr> = parts
            .iter()
            .enumerate()
            .filter(|(i, _)| driver.is_none_or(|(d, _)| *i != d))
            .map(|(_, e)| e)
            .collect();
        if rest.is_empty() {
            return ids;
        }
        self.filter_ids(ids, &rest, q, cancel)
    }

    /// 单个名字 term 求值。
    fn eval_name(&self, t: &NameTerm, q: &Query, cancel: &CancelToken) -> Vec<EntryId> {
        if t.is_fast_substring() {
            let needle = t.needle(q.case_sensitive);
            return scan_names(
                self.haystack(q.case_sensitive),
                &needle,
                &self.name_off,
                || cancel.is_cancelled(),
            );
        }
        // 剪枝：通配符 / 单扩展名可以先用一个**字面段**走 `memmem` 出候选集，
        // 再在候选上逐条验证（匹配成功的名字必然包含该字面段，故不会漏）。
        // 实测 88 万条卷：`ext:pdf` 与 `*.log` 由全表 67 ms 降到毫秒级。
        let probes = literal_probes(t);
        if !probes.is_empty() {
            let mut cand: Vec<EntryId> = Vec::new();
            for lit in probes {
                let needle = if q.case_sensitive {
                    lit.clone().into_bytes()
                } else {
                    simple_lowercase(lit.as_bytes())
                };
                cand.extend(scan_names(
                    self.haystack(q.case_sensitive),
                    &needle,
                    &self.name_off,
                    || cancel.is_cancelled(),
                ));
                if cancel.is_cancelled() {
                    return Vec::new();
                }
            }
            cand.sort_unstable();
            cand.dedup();
            return cand
                .into_iter()
                .filter(|&id| self.name_matches(id, t, q))
                .collect();
        }
        // 其余种类（正则 / 路径 / 多扩展名）无快速路径 → 全表。
        self.all_where(|id| self.name_matches(id, t, q), cancel)
    }

    /// 在候选集上按条件过滤（并行；保序）。
    fn filter_ids(
        &self,
        ids: Vec<EntryId>,
        parts: &[&Expr],
        q: &Query,
        cancel: &CancelToken,
    ) -> Vec<EntryId> {
        ids.par_chunks(VERIFY_CHUNK)
            .flat_map(|chunk| {
                if cancel.is_cancelled() {
                    return Vec::new();
                }
                chunk
                    .iter()
                    .copied()
                    .filter(|&id| parts.iter().all(|p| self.matches(id, p, q)))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// 全表并行扫描，保留满足 `f` 的 id（升序）。
    fn all_where<F>(&self, f: F, cancel: &CancelToken) -> Vec<EntryId>
    where
        F: Fn(EntryId) -> bool + Sync,
    {
        let n = self.entry_count();
        let mut out: Vec<EntryId> = Vec::new();
        // 分块以便每 ~4K 检查一次取消（DESIGN §7.3）。
        let mut start = 0usize;
        while start < n {
            if cancel.is_cancelled() {
                return Vec::new();
            }
            let end = (start + VERIFY_CHUNK).min(n);
            let ids: Vec<EntryId> = (start as EntryId..end as EntryId)
                .into_par_iter()
                .filter(|&id| f(id))
                .collect();
            out.extend(ids);
            start = end;
        }
        out
    }

    /// 全表 id（升序）。
    fn all_ids(&self, cancel: &CancelToken) -> Vec<EntryId> {
        self.all_where(|_| true, cancel)
    }

    /// 补集（全集 \ `sub`；`sub` 必须升序）。
    fn complement(&self, sub: &[EntryId], cancel: &CancelToken) -> Vec<EntryId> {
        let n = self.entry_count();
        let mut out = Vec::with_capacity(n.saturating_sub(sub.len()));
        let mut j = 0usize;
        for id in 0..n as EntryId {
            if cancel.is_cancelled() {
                return Vec::new();
            }
            while j < sub.len() && sub[j] < id {
                j += 1;
            }
            if j < sub.len() && sub[j] == id {
                continue;
            }
            out.push(id);
        }
        out
    }

    /// 判定条目是否满足子表达式（在候选集 / 全表上逐条调用）。
    fn matches(&self, id: EntryId, e: &Expr, q: &Query) -> bool {
        match e {
            Expr::Empty => true,
            Expr::Name(t) => self.name_matches(id, t, q),
            Expr::Pred(p) => self.pred_matches(id, p, q),
            Expr::Not(i) => !self.matches(id, i, q),
            Expr::And(v) => v.iter().all(|x| self.matches(id, x, q)),
            Expr::Or(v) => v.iter().any(|x| self.matches(id, x, q)),
        }
    }

    /// 名字级条件判定（按 [`NameKind`] 分派）。
    ///
    /// 大小折叠用 thread-local 缓冲复用（见 [`case::simple_lowercase_into`]）：
    /// 全表扫描类查询会对每条名字调用本函数，每次新建 `Vec` 的实测代价是
    /// 88 万条卷上 `ext:pdf` 要 77 ms。
    fn name_matches(&self, id: EntryId, t: &NameTerm, q: &Query) -> bool {
        let raw = self.name(id);
        let mut hay_buf: Option<Vec<u8>> = None;
        let hay: &[u8] = if q.case_sensitive {
            raw
        } else {
            let mut v = take_hay_buf();
            simple_lowercase_into(raw, &mut v);
            hay_buf = Some(v);
            hay_buf.as_ref().expect("刚赋值")
        };
        let mut needle_buf: Option<Vec<u8>> = None;
        let needle: &[u8] = if q.case_sensitive {
            t.text.as_bytes()
        } else {
            let mut v = take_needle_buf();
            simple_lowercase_into(t.text.as_bytes(), &mut v);
            needle_buf = Some(v);
            needle_buf.as_ref().expect("刚赋值")
        };
        let hit = self.name_matches_hay(id, hay, raw, needle, t, q);
        if let Some(v) = hay_buf {
            give_hay_buf(v);
        }
        if let Some(v) = needle_buf {
            give_needle_buf(v);
        }
        hit
    }

    /// 名字级条件判定（已备好折叠后的 haystack / needle 与原始名字）。
    fn name_matches_hay(
        &self,
        id: EntryId,
        hay: &[u8],
        raw: &[u8],
        needle: &[u8],
        t: &NameTerm,
        q: &Query,
    ) -> bool {
        match t.kind {
            NameKind::Substring | NameKind::Phrase => contains(hay, needle),
            NameKind::Glob => glob_match(hay, needle),
            NameKind::Regex => match t.regex_idx.and_then(|i| q.regexes.get(i)) {
                Some(re) => match std::str::from_utf8(raw) {
                    Ok(s) => re.is_match(s),
                    Err(_) => false,
                },
                None => false,
            },
            NameKind::WholeWord => wholeword_match(hay, needle),
            NameKind::StartWith => hay.starts_with(needle),
            NameKind::EndWith => hay.ends_with(needle),
            NameKind::Ext => ext_match(hay, &t.text),
            NameKind::Parent => {
                let p = self.parent.get(id as usize).copied().unwrap_or(NONE);
                if p == NONE {
                    false
                } else {
                    let pname = self.name(p);
                    let plowered;
                    let phay: &[u8] = if q.case_sensitive {
                        pname
                    } else {
                        plowered = simple_lowercase(pname);
                        &plowered
                    };
                    contains(phay, needle)
                }
            }
            // 需要现算完整路径（成本最高，§7.3-4）。
            NameKind::Path | NameKind::InFolder => match self.path_string(id) {
                Some(p) => {
                    let (hay_sp, needle_sp) = if q.case_sensitive {
                        (p.clone(), t.text.clone())
                    } else {
                        (p.to_lowercase(), t.text.to_lowercase())
                    };
                    if t.kind == NameKind::Path {
                        hay_sp.contains(&needle_sp)
                    } else {
                        // infolder：路径中某一层目录名等于该名字
                        let pat = format!("\\{needle_sp}\\");
                        hay_sp.contains(&pat)
                    }
                }
                None => false,
            },
        }
    }

    /// 属性谓词判定（SoA 列 / 标志位；路径类需现算）。
    fn pred_matches(&self, id: EntryId, p: &Predicate, q: &Query) -> bool {
        let i = id as usize;
        match *p {
            Predicate::IsDir => self.is_dir(id),
            Predicate::IsFile => !self.is_dir(id),
            Predicate::Attrib(mask) => {
                let f = self.flags.get(i).copied().unwrap_or(0);
                let mut want = 0u16;
                if mask & crate::query::win32_attr::READONLY != 0 {
                    want |= flags::READONLY;
                }
                if mask & crate::query::win32_attr::HIDDEN != 0 {
                    want |= flags::HIDDEN;
                }
                if mask & crate::query::win32_attr::SYSTEM != 0 {
                    want |= flags::SYSTEM;
                }
                if mask & crate::query::win32_attr::DIRECTORY != 0 {
                    want |= flags::IS_DIR;
                }
                // 索引未保存其余 Win32 属性位（ARCHIVE / COMPRESSED / ENCRYPTED …），
                // 这些位在 M4 无法判定：只按已保存的位求值（见 DECISIONS）。
                want != 0 && (f & want) != 0
            }
            Predicate::Size(n) => n.test(self.size.get(i).copied().unwrap_or(0)),
            Predicate::Depth(n) => n.test(u64::from(self.depth_of(id))),
            Predicate::Date { field, spec } => {
                let v = match field {
                    DateField::Modified => self.mtime.get(i).copied().unwrap_or(0),
                    DateField::Created => self.ctime.get(i).copied().unwrap_or(0),
                    DateField::Accessed => self.atime.get(i).copied().unwrap_or(0),
                };
                let (start, end) = date_range(spec, q.context);
                v >= start && v < end
            }
        }
    }

    fn is_dir(&self, id: EntryId) -> bool {
        self.flags.get(id as usize).copied().unwrap_or(0) & flags::IS_DIR != 0
    }

    /// 目录深度（根 = 0；沿 parent 上溯，带环保护）。
    pub fn depth_of(&self, id: EntryId) -> u32 {
        let mut d = 0u32;
        let mut cur = self.parent.get(id as usize).copied().unwrap_or(NONE);
        while cur != NONE && d < DEPTH_MAX {
            d += 1;
            cur = self.parent.get(cur as usize).copied().unwrap_or(NONE);
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::super::entry::{attrs, EntryInput, IndexBuilder, VolumeInfo};
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
            mtime: 0,
            ctime: 0,
            atime: 0,
        }
    }

    fn file(frn: u64, parent: u64, name: &str, size: u64, mtime: i64) -> EntryInput {
        EntryInput {
            is_dir: false,
            attrs: 0,
            size,
            mtime,
            ctime: mtime,
            atime: mtime,
            ..dir(frn, parent, name)
        }
    }

    /// 一个小卷：id 0 = 根目录 "."，其余为文件/子目录。
    fn sample() -> VolumeIndex {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(file(100, 5, "README.txt", 100, 1));
        b.push(file(101, 5, "report_2024.pdf", 20 * 1024 * 1024, 2));
        b.push(file(102, 5, "notes.md", 512, 3));
        b.push(dir(200, 5, "Docs"));
        b.push(file(201, 200, "guide.pdf", 1024, 4));
        b.finish()
    }

    fn hits(q: &str) -> Vec<EntryId> {
        let ix = sample();
        let token = CancelToken::new();
        ix.search(&parse(q).unwrap(), &token)
    }

    #[test]
    fn hardlink_alias_is_searchable() {
        // M7 #97：同一 FRN 的两条名字都能被各自的查询命中。
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(dir(100, 5, "Bin"));
        b.push(dir(200, 5, "Sys32"));
        b.push(file(999, 100, "tool.exe", 10, 1)); // 主条目 id=3
        b.push(file(999, 200, "driver.dll", 10, 1)); // 别名 id=4
        let ix = b.finish();
        let token = CancelToken::new();
        assert_eq!(ix.search(&parse("tool").unwrap(), &token), vec![3]);
        assert_eq!(ix.search(&parse("driver").unwrap(), &token), vec![4]);
        // 扩展名过滤与 path: 条件对别名同样生效。
        assert_eq!(ix.search(&parse("*.exe").unwrap(), &token), vec![3]);
        assert_eq!(ix.search(&parse("*.dll").unwrap(), &token), vec![4]);
        assert_eq!(ix.search(&parse("path:sys32").unwrap(), &token), vec![2, 4]);
    }

    #[test]
    fn substring_and_phrase() {
        assert_eq!(hits("read"), vec![1]);
        assert_eq!(hits("pdf"), vec![2, 5]);
        assert_eq!(hits("report 2024"), vec![2]); // AND
    }

    #[test]
    fn or_union() {
        assert_eq!(hits("read|notes"), vec![1, 3]);
    }

    #[test]
    fn not_complement() {
        // 全表 6 条，排除含 "e" 的
        let all: Vec<EntryId> = (0..6).collect();
        let with_e = hits("e");
        let not_e = hits("!e");
        let expect: Vec<EntryId> = all.into_iter().filter(|i| !with_e.contains(i)).collect();
        assert_eq!(not_e, expect);
        assert!(!not_e.is_empty());
    }

    #[test]
    fn not_combines_with_and() {
        // 名字含 "e" 但不是 pdf
        let r = hits("e !pdf");
        assert_eq!(r, vec![1, 3]);
    }

    #[test]
    fn group_precedence() {
        // (read|notes) pdf → 空（两者都不含 pdf）
        assert!(hits("(read|notes) pdf").is_empty());
        // (read|notes)|pdf → 1,2,3,5
        assert_eq!(hits("(read|notes)|pdf"), vec![1, 2, 3, 5]);
    }

    #[test]
    fn glob_wildcard() {
        assert_eq!(hits("*.pdf"), vec![2, 5]);
        assert_eq!(hits("notes.*"), vec![3]);
        assert_eq!(hits("?????.md"), vec![3]); // notes = 5 字符
    }

    #[test]
    fn regex_modifier() {
        assert_eq!(hits("regex:^report"), vec![2]);
        assert_eq!(hits("regex:\\d{4}"), vec![2]);
    }

    #[test]
    fn ext_modifier() {
        assert_eq!(hits("ext:pdf"), vec![2, 5]);
        assert_eq!(hits("ext:pdf;md"), vec![2, 3, 5]);
        assert!(hits("ext:zip").is_empty());
    }

    #[test]
    fn file_folder_modifiers() {
        assert_eq!(hits("folder:"), vec![0, 4]);
        assert_eq!(hits("file:"), vec![1, 2, 3, 5]);
        assert_eq!(hits("folder:Docs"), vec![4]);
    }

    #[test]
    fn size_modifier() {
        assert_eq!(hits("size:>10mb"), vec![2]); // 20MB
                                                 // 100B / 512B / 1024B 三个文件都落在 [100, 1024]
        assert_eq!(hits("size:100..1024"), vec![1, 3, 5]);
        assert_eq!(hits("size:512"), vec![3]);
    }

    #[test]
    fn wholeword_startwith_endwith() {
        assert_eq!(hits("ww:notes"), vec![3]);
        assert!(hits("ww:ote").is_empty());
        assert_eq!(hits("startwith:rep"), vec![2]);
        assert_eq!(hits("endwith:.md"), vec![3]);
    }

    #[test]
    fn parent_and_infolder() {
        // guide.pdf 的父目录是 Docs
        assert_eq!(hits("parent:Docs"), vec![5]);
        assert_eq!(hits("infolder:Docs"), vec![5]);
    }

    #[test]
    fn path_modifier() {
        // 完整路径含 "Docs"：Docs 目录自身（id 4）与其子文件（id 5）都在内
        assert_eq!(hits("path:Docs"), vec![4, 5]);
        // 裸 term 含 \ → 路径匹配
        assert_eq!(hits("Docs\\guide"), vec![5]);
    }

    #[test]
    fn depth_modifier() {
        // 根 = 0，一级 = 1（README.txt 等），Docs\guide.pdf = 2
        assert_eq!(hits("depth:0"), vec![0]);
        assert_eq!(hits("depth:1"), vec![1, 2, 3, 4]);
        assert_eq!(hits("depth:2"), vec![5]);
    }

    #[test]
    fn empty_query_and_cancel() {
        let ix = sample();
        let token = CancelToken::new();
        assert!(ix.search(&parse("").unwrap(), &token).is_empty());
        token.cancel();
        assert!(ix.search(&parse("read").unwrap(), &token).is_empty());
    }

    #[test]
    fn results_are_sorted_and_deduped() {
        let r = hits("pdf|read|pdf");
        assert_eq!(r, vec![1, 2, 5]);
        let sorted = {
            let mut s = r.clone();
            s.sort_unstable();
            s
        };
        assert_eq!(r, sorted);
    }

    #[test]
    fn glob_and_wholeword_helpers() {
        assert!(glob_match(b"hello.txt", b"*.txt"));
        assert!(glob_match(b"hello.txt", b"h?llo.*"));
        assert!(!glob_match(b"hello.txt", b"*.md"));
        assert!(wholeword_match(b"ab cd", b"cd"));
        assert!(!wholeword_match(b"ab cd", b"b"));
    }
}
