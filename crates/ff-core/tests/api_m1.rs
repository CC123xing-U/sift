//! ff-core 公开 API 冒烟测试（模拟 Track D 基准 / 集成测试的用法）。
//!
//! 只依赖 `ff_core` 的公开面：`EntryId`/`NONE` · `EntryInput` ·
//! `IndexBuilder` · `VolumeIndex { entry_count / write_path / search }` ·
//! `Query` · `parse` · `CancelToken` · `highlight_ranges`。

use ff_core::{
    attrs, flags, parse, CancelToken, EntryId, EntryInput, IndexBuilder, Query, VolumeIndex,
    VolumeInfo, NONE,
};
use std::ops::Range;

fn dir(frn: u64, parent: u64, name: &str) -> EntryInput {
    EntryInput {
        frn,
        parent_frn: parent,
        name: name.to_string(),
        is_dir: true,
        attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
        size: 0,
        mtime: 100,
        ctime: 100,
        atime: 100,
    }
}

fn file(frn: u64, parent: u64, name: &str, size: u64) -> EntryInput {
    EntryInput {
        is_dir: false,
        attrs: 0,
        size,
        ..dir(frn, parent, name)
    }
}

/// C:\                       (0, root)
///  ├─ Windows\              (1)
///  │   ├─ System32\         (2)
///  │   │   └─ cmd.exe       (3)
///  │   └─ readme.txt        (4)
///  ├─ Users\                (5)
///  │   └─ 张三\              (6)
///  │       └─ 简历.docx      (7)
///  └─ data.bin              (8)
fn build() -> VolumeIndex {
    let vol = VolumeInfo {
        drive: Some('C'),
        guid_path: "\\\\?\\Volume{c0ffee}".to_string(),
        serial: 0x1234_5678,
        journal_id: 42,
        next_usn: 1000,
    };
    let mut b = IndexBuilder::new(vol);
    b.push(dir(5, 0, "."));
    b.push(dir(100, 5, "Windows"));
    b.push(dir(101, 100, "System32"));
    b.push(file(102, 101, "cmd.exe", 289_792));
    b.push(file(103, 100, "readme.txt", 1024));
    b.push(dir(200, 5, "Users"));
    b.push(dir(201, 200, "张三"));
    b.push(file(202, 201, "简历.docx", 20_480));
    b.push(file(300, 5, "data.bin", 1 << 20));
    b.finish()
}

#[test]
fn model_surface() {
    let ix = build();
    assert_eq!(ix.entry_count(), 9);
    // SoA 列可直接读
    assert_eq!(ix.names.len(), ix.names_lower.len());
    assert_eq!(ix.parent[3], 2);
    assert!(ix.flags[0] & flags::IS_DIR != 0);
    assert_eq!(ix.size[3], 289_792);
    assert_eq!(ix.mtime[3], 100);
    assert_eq!(ix.frn[3], 102);
    // 派生数据访问器
    assert_eq!(ix.root_id(), Some(0));
    assert_eq!(ix.unresolved_parents(), 0);
    assert_eq!(ix.duplicate_frns(), 0);
    assert_eq!(ix.cjk_ids(), &[6, 7]); // 张三 / 简历.docx
    assert_eq!(ix.frn_to_id(102), Some(3));
    assert_eq!(ix.frn_to_id(999), None);
    assert_eq!(ix.volume.serial, 0x1234_5678);
    assert_eq!(ix.volume.next_usn, 1000);
}

#[test]
fn path_surface() {
    let ix = build();
    let mut buf = Vec::new();
    assert!(ix.write_path(3, &mut buf));
    assert_eq!(buf, b"C:\\Windows\\System32\\cmd.exe");
    assert!(ix.write_path(0, &mut buf));
    assert_eq!(buf, b"C:\\");
    assert!(!ix.write_path(NONE, &mut buf));
    assert!(!ix.write_path(99, &mut buf));
    assert_eq!(ix.path_string(7).unwrap(), "C:\\Users\\张三\\简历.docx");
    assert_eq!(ix.path_string(NONE), None);
}

#[test]
fn search_surface() {
    let ix = build();
    let token = CancelToken::new();
    // 子串
    let hits = ix.search(&parse("cmd").unwrap(), &token);
    assert_eq!(hits, vec![3]);
    // AND（默认不区分大小写）
    let hits = ix.search(&parse("read txt").unwrap(), &token);
    assert_eq!(hits, vec![4]);
    // Unicode
    let hits = ix.search(&parse("简历").unwrap(), &token);
    assert_eq!(hits, vec![7]);
    // 空查询
    assert!(ix.search(&Query::default(), &token).is_empty());
    assert!(ix.search(&parse("").unwrap(), &token).is_empty());
    // 取消
    token.cancel();
    assert!(ix.search(&parse("cmd").unwrap(), &token).is_empty());
}

#[test]
fn parse_errors_surface() {
    for bad in ["a|b", "!x", "(x)", "*.txt", "size:>1mb", "a\\b", "a/b"] {
        let e = parse(bad).unwrap_err();
        assert!(!e.message.is_empty());
        assert!(e.span.start < e.span.end);
        assert!(e.span.end <= bad.len());
        assert!(e.to_string().contains("位置"));
    }
    // 合法
    assert_eq!(parse("a b").unwrap().terms.len(), 2);
    assert_eq!(parse("\"a b\"").unwrap().terms.len(), 1);
}

#[test]
fn highlight_surface() {
    let ix = build();
    let q = parse("read").unwrap();
    let r: Vec<Range<u32>> = ix.highlight_ranges(4, &q);
    assert_eq!(r, vec![0..4]);
    // 中文按字节偏移
    let r = ix.highlight_ranges(7, &parse("简历").unwrap());
    assert_eq!(r, vec![0..6]);
    // 空 / 无效
    assert!(ix.highlight_ranges(4, &Query::default()).is_empty());
    assert!(ix.highlight_ranges(NONE, &q).is_empty());
}

#[test]
fn entry_id_type_is_u32() {
    let id: EntryId = 7;
    let _: u32 = id;
    assert_eq!(NONE, u32::MAX);
}

/// 性能冒烟（默认 `#[ignore]`，手动 `cargo test -p ff-core -- --ignored perf` 运行）。
/// 真实基准由 Track D（criterion）负责；此处仅本地 sanity check。
#[test]
#[ignore]
fn perf_smoke_500k() {
    // 确定性伪随机（LCG），不引 rand
    let n: u32 = 500_000;
    let mut b = IndexBuilder::new(VolumeInfo {
        drive: Some('D'),
        ..Default::default()
    });
    b.push(EntryInput {
        frn: 5,
        parent_frn: 0,
        name: ".".into(),
        is_dir: true,
        attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
        size: 0,
        mtime: 0,
        ctime: 0,
        atime: 0,
    });
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    let words = [
        "alpha", "beta", "gamma", "delta", "文档", "项目", "report", "data", "temp", "backup",
    ];
    let t0 = std::time::Instant::now();
    let mut arena_bytes = 0usize;
    for i in 0..n {
        let w1 = words[(next() % words.len() as u32) as usize];
        let w2 = words[(next() % words.len() as u32) as usize];
        let name = format!("{w1}-{w2}-{i:06}.txt");
        arena_bytes += name.len() + 1;
        b.push(EntryInput {
            frn: 100 + i as u64,
            parent_frn: 5,
            name,
            is_dir: false,
            attrs: 0,
            size: next() as u64 % 1_000_000,
            mtime: 130_000_000_000_000_000 + next() as i64,
            ctime: 130_000_000_000_000_000,
            atime: 130_000_000_000_000_000,
        });
    }
    let ix = b.finish();
    let build_ms = t0.elapsed().as_millis();

    let token = CancelToken::new();
    // 预热（rayon 线程池 + 页错误）
    let _ = ix.search(&parse("alpha").unwrap(), &token);
    let t1 = std::time::Instant::now();
    let iters = 20;
    let mut last = 0usize;
    for _ in 0..iters {
        last = ix.search(&parse("alpha").unwrap(), &token).len();
    }
    let sub_ms = t1.elapsed().as_millis() as f64 / f64::from(iters);
    let t2 = std::time::Instant::now();
    let mut last_and = 0usize;
    for _ in 0..iters {
        last_and = ix.search(&parse("alpha 文档").unwrap(), &token).len();
    }
    let and_ms = t2.elapsed().as_millis() as f64 / f64::from(iters);

    eprintln!(
        "[perf_smoke_500k] entries={} arena={}MB build={build_ms}ms \
         substring({last} hits)={sub_ms:.2}ms and({last_and} hits)={and_ms:.2}ms",
        ix.entry_count(),
        arena_bytes / (1024 * 1024),
    );
    assert!(last > 0 && last_and > 0);
}
