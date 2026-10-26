//! M4 语法测试集（DESIGN §13-M4 验收：语法测试集 ≥ 80 条）。
//!
//! 覆盖：合法语法、错误输入、半成品输入（用户正在打字）、每个修饰符的边界用例、
//! NOT / OR 优先级。表驱动：一行一条用例，便于后续里程碑继续追加。

use ff_core::query::filetime_from_civil;
use ff_core::{attrs, parse, CancelToken, EntryInput, IndexBuilder, VolumeIndex, VolumeInfo};

// ───────────────────────── 测试索引 ─────────────────────────

/// 2024-06-01 00:00 UTC 的 FILETIME（用例里的「新」文件）。
fn t_new() -> i64 {
    filetime_from_civil(2024, 6, 1, 0)
}

/// 2023-06-01 00:00 UTC 的 FILETIME（用例里的「旧」文件）。
fn t_old() -> i64 {
    filetime_from_civil(2023, 6, 1, 0)
}

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
        mtime: t_new(),
        ctime: t_new(),
        atime: t_new(),
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

/// 索引布局（id 顺序即 push 顺序）：
///
/// | id | 名字 | 类型 | 大小 | mtime | 父 |
/// |---|---|---|---|---|---|
/// | 0 | `.` | 目录 | 0 | 2024 | 根 |
/// | 1 | README.txt | 文件 | 100 | 2024 | `.` |
/// | 2 | report_2024.pdf | 文件 | 20MB | 2024 | `.` |
/// | 3 | notes.md | 文件 | 512 | 2024 | `.` |
/// | 4 | Docs | 目录 | 0 | 2024 | `.` |
/// | 5 | guide.pdf | 文件 | 1024 | 2023 | Docs |
/// | 6 | todo.txt | 文件 | 50 | 2023 | Docs |
/// | 7 | 中文文件.txt | 文件 | 1 | 2024 | `.` |
/// | 8 | secret.dat | 文件（隐藏+系统） | 0 | 2024 | `.` |
fn index() -> VolumeIndex {
    let mut b = IndexBuilder::new(info());
    b.push(dir(5, 0, "."));
    b.push(file(100, 5, "README.txt", 100, t_new()));
    b.push(file(101, 5, "report_2024.pdf", 20 * 1024 * 1024, t_new()));
    b.push(file(102, 5, "notes.md", 512, t_new()));
    b.push(dir(200, 5, "Docs"));
    b.push(file(201, 200, "guide.pdf", 1024, t_old()));
    b.push(file(202, 200, "todo.txt", 50, t_old()));
    b.push(file(203, 5, "中文文件.txt", 1, t_new()));
    b.push(EntryInput {
        frn: 204,
        parent_frn: 5,
        name: "secret.dat".to_string(),
        is_dir: false,
        attrs: attrs::FILE_ATTRIBUTE_HIDDEN | attrs::FILE_ATTRIBUTE_SYSTEM,
        size: 0,
        mtime: t_new(),
        ctime: t_new(),
        atime: t_new(),
    });
    b.finish()
}

// ───────────────────────── 用例表 ─────────────────────────

/// 期望结果。
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// 命中且**恰好**是这些 id（升序）。
    Hits(&'static [u32]),
    /// 解析成功但没有命中。
    Empty,
    /// 解析成功但整个查询无贡献（空查询 / 空修饰符值）。
    NoOp,
    /// 解析失败（非法输入；必须有消息与合法 span）。
    Err,
}

use Expect::{Empty, Err as Bad, Hits, NoOp};

/// 全部用例：`(查询串, 期望)`。
#[rustfmt::skip]
const CASES: &[(&str, Expect)] = &[
    // ── 1. 基础：子串 / AND / 短语 / Unicode（12 条）──
    ("read", Hits(&[1])),
    ("READ", Hits(&[1])),                       // 默认不区分大小写
    ("pdf", Hits(&[2, 5])),
    ("report 2024", Hits(&[2])),                // 空格 = AND
    ("report zzz", Empty),                      // AND 不命中
    ("2024 report", Hits(&[2])),                // AND 与顺序无关
    ("\"notes.md\"", Hits(&[3])),               // 短语（含 . 但无空格）
    ("\"report 2024\"", Empty),                 // 短语含空格 → 名字里没有
    ("中文", Hits(&[7])),
    ("文件", Hits(&[7])),
    ("zzz", Empty),
    ("", NoOp),

    // ── 2. OR（6 条）──
    ("read|pdf", Hits(&[1, 2, 5])),
    ("read|zzz", Hits(&[1])),
    ("pdf|md", Hits(&[2, 3, 5])),
    ("read|notes|zzz", Hits(&[1, 3])),
    ("read | pdf", Hits(&[1, 2, 5])),           // OR 两侧允许空格
    ("a|b", Hits(&[1, 8])),                     // README.txt / secret.dat 含 a
    ("zzz|yyy", Empty),

    // ── 3. NOT（7 条）──
    ("!zzz", Hits(&[0, 1, 2, 3, 4, 5, 6, 7, 8])),
    ("!pdf", Hits(&[0, 1, 3, 4, 6, 7, 8])),
    ("read !pdf", Hits(&[1])),
    ("pdf !2024", Hits(&[5])),                  // guide.pdf 不含 2024
    ("!read !pdf", Hits(&[0, 3, 4, 6, 7, 8])),
    ("!", Empty),                               // 单独的 `!` 降级为字面
    ("!中文", Hits(&[0, 1, 2, 3, 4, 5, 6, 8])),

    // ── 4. 分组与优先级（9 条）──
    ("(read|notes)", Hits(&[1, 3])),
    ("<read|notes>", Hits(&[1, 3])),            // <> 等价分组
    ("(read|notes)|pdf", Hits(&[1, 2, 3, 5])),
    ("(read|notes) pdf", Empty),                // AND > OR
    ("pdf (read|notes)", Empty),
    ("!(read|notes)", Hits(&[0, 2, 4, 5, 6, 7, 8])),
    ("(read)", Hits(&[1])),
    ("()", NoOp),                               // 空分组无贡献
    ("(a", Empty),                              // 未闭合 → 降级为字面 "(" + "a"

    // ── 5. 半成品输入（容错，8 条）──
    ("size:", NoOp),
    ("dm:", NoOp),
    ("ext:", NoOp),
    ("path:", NoOp),
    ("regex:", NoOp),
    ("attrib:", NoOp),
    ("folder", Empty),                          // 无冒号 → 字面子串
    ("\"", NoOp),                               // 单独的引号 → 空短语

    // ── 6. 通配符（6 条）──
    ("*.pdf", Hits(&[2, 5])),
    ("*.txt", Hits(&[1, 6, 7])),
    ("notes.*", Hits(&[3])),
    ("?????.md", Hits(&[3])),                   // notes = 5 字符
    ("*2024*", Hits(&[2])),
    ("?otes.md", Hits(&[3])),

    // ── 7. regex:（5 条）──
    ("regex:^notes", Hits(&[3])),
    ("regex:\\d{4}", Hits(&[2])),
    ("regex:pdf$", Hits(&[2, 5])),
    ("regex:(read|notes)", Hits(&[3])),          // 正则默认区分大小写（README 不匹配 read）
    ("regex:(?i)(read|notes)", Hits(&[1, 3])),   // 内联不敏感标志
    ("regex:zzz", Empty),

    // ── 8. ext:（5 条）──
    ("ext:pdf", Hits(&[2, 5])),
    ("ext:pdf;md", Hits(&[2, 3, 5])),
    ("ext:txt", Hits(&[1, 6, 7])),
    ("ext:PDF", Hits(&[2, 5])),                 // 扩展名大小写不敏感
    ("ext:zip", Empty),

    // ── 9. file: / folder:（5 条）──
    ("folder:", Hits(&[0, 4])),
    ("file:", Hits(&[1, 2, 3, 5, 6, 7, 8])),
    ("folder:Docs", Hits(&[4])),
    ("file:notes", Hits(&[3])),
    ("folder:pdf", Empty),

    // ── 10. path: 与裸路径（5 条）──
    ("path:Docs", Hits(&[4, 5, 6])),
    ("Docs\\guide", Hits(&[5])),                // 裸 term 含 \ → 路径匹配
    ("Docs\\", Hits(&[5, 6])),                  // Docs 目录自身路径末尾无反斜杠
    ("path:中文", Hits(&[7])),
    ("path:zzz", Empty),

    // ── 11. size:（8 条）──
    ("size:>10mb", Hits(&[2])),
    ("size:>=1024", Hits(&[2, 5])),
    ("size:<100", Hits(&[0, 4, 6, 7, 8])),      // 目录 size=0 也算
    ("size:100", Hits(&[1])),
    ("size:1kb", Hits(&[5])),
    ("size:1kb..1mb", Hits(&[5])),
    ("size:0..1kb", Hits(&[0, 1, 3, 4, 5, 6, 7, 8])),
    ("size:>1gb", Empty),

    // ── 12. depth:（4 条）──
    ("depth:0", Hits(&[0])),
    ("depth:1", Hits(&[1, 2, 3, 4, 7, 8])),
    ("depth:2", Hits(&[5, 6])),
    ("depth:>1", Hits(&[5, 6])),

    // ── 13. dm: / dc: / da:（7 条）──
    ("dm:2024-01-01..2024-12-31", Hits(&[0, 1, 2, 3, 4, 7, 8])),
    ("dm:2023-01-01..2023-12-31", Hits(&[5, 6])),
    ("dm:2024-06-01", Hits(&[0, 1, 2, 3, 4, 7, 8])),
    ("dc:2024-06-01", Hits(&[0, 1, 2, 3, 4, 7, 8])),
    ("da:2023-06-01", Hits(&[5, 6])),
    ("dm:1900-01-01", Empty),
    ("dm:2024-06-01 zzz", Empty),

    // ── 14. attrib:（5 条）──
    ("attrib:D", Hits(&[0, 4])),
    ("attrib:H", Hits(&[8])),
    ("attrib:S", Hits(&[8])),
    ("attrib:HS", Hits(&[8])),
    ("attrib:R", Empty),                        // 无只读条目

    // ── 15. case: / nocase:（4 条）──
    ("case:READ", Hits(&[1])),
    ("case:read", Empty),                       // 区分大小写后小写不命中
    ("nocase:READ", Hits(&[1])),
    ("case:read|READ", Hits(&[1])),

    // ── 16. parent: / infolder:（4 条）──
    ("parent:Docs", Hits(&[5, 6])),
    ("infolder:Docs", Hits(&[5, 6])),
    ("parent:.", Hits(&[1, 2, 3, 4, 7, 8])),
    ("parent:zzz", Empty),

    // ── 17. 组合（6 条）──
    ("ext:pdf size:>1mb", Hits(&[2])),
    ("*.pdf !2024", Hits(&[5])),
    ("regex:pdf size:<1mb", Hits(&[5])),
    ("path:Docs ext:txt", Hits(&[6])),
    ("dm:2023-01-01..2023-12-31 ext:pdf", Hits(&[5])),
    ("size:>1kb folder:", Empty),

    // ── 18. 错误输入（8 条）──
    ("size:abc", Bad),
    ("size:1x", Bad),
    ("size:1kb..", Bad),
    ("dm:notadate", Bad),
    ("dm:2024-13-01", Bad),                     // 月份 13
    ("attrib:Z", Bad),
    ("regex:(", Bad),
    ("depth:x", Bad),
];

#[test]
fn syntax_suite() {
    let ix = index();
    for (q, expect) in CASES {
        let token = CancelToken::new();
        match parse(q) {
            Err(e) => match expect {
                Bad => {
                    assert!(!e.message.is_empty(), "{q}: 错误消息为空");
                    assert!(e.span.start < e.span.end, "{q}: span 非法 {e:?}");
                    assert!(e.span.end <= q.len(), "{q}: span 越界 {e:?}");
                    assert!(e.to_string().contains("位置"), "{q}: Display 缺位置");
                }
                other => panic!("{q}: 期望 {other:?}，实际报错「{}」", e.message),
            },
            Ok(query) => {
                let hits = ix.search(&query, &token);
                match expect {
                    Hits(ids) => assert_eq!(hits, *ids, "查询 `{q}` 命中不符"),
                    Empty => assert!(hits.is_empty(), "查询 `{q}` 应无命中，实际 {hits:?}"),
                    NoOp => {
                        assert!(query.is_empty(), "查询 `{q}` 应无贡献，实际 {query:?}");
                        assert!(hits.is_empty(), "查询 `{q}` 无贡献却命中 {hits:?}");
                    }
                    Bad => panic!("{q}: 期望解析失败，实际通过"),
                }
            }
        }
    }
}

#[test]
fn suite_meets_size_requirement() {
    // DESIGN §13-M4 验收要求 ≥ 80 条。
    assert!(
        CASES.len() >= 80,
        "语法测试集只有 {} 条，验收要求 ≥ 80",
        CASES.len()
    );
}

#[test]
fn results_always_sorted_and_deduped() {
    let ix = index();
    let token = CancelToken::new();
    for (q, _) in CASES {
        let Ok(query) = parse(q) else { continue };
        let hits = ix.search(&query, &token);
        let mut sorted = hits.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(hits, sorted, "查询 `{q}` 结果未排序/去重");
    }
}

#[test]
fn cancel_yields_no_partial_results() {
    let ix = index();
    let token = CancelToken::new();
    let query = parse("pdf").unwrap();
    assert!(!ix.search(&query, &token).is_empty());
    token.cancel();
    assert!(ix.search(&query, &token).is_empty());
}
