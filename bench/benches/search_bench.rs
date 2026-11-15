//! 搜索与路径基准（DESIGN §14：子串 / AND；§1.3 子串 ≤100ms @200 万）。
//!
//! 在 10 万 / 100 万合成索引上测：
//! - 子串（高频词 `report`，命中约 2% 条目）；
//! - 子串（稀有词：唯一名，命中 1 条）；
//! - 多词 AND（`report summary`，验证稀有词优先路径）；
//! - `write_path`（约 1 万条采样，覆盖全表不同深度）；
//! - M5 的 T2 拼音 / T3 模糊全表 / 评分排序（DESIGN §14 要求的基准面）。
//!
//! **200 万档**（含 ≤100ms 目标验证）由 `memprobe` 跑一次并记录
//! `docs/BENCH.md`；M5 的 200 万档由 `m5probe` 跑一次记录。
//! 复现：`cargo bench -p sift-bench --bench search_bench`。

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use ff_core::{parse, CancelToken, MatchOptions, ScoreConfig};
use sift_bench::{build_index, SynthConfig};

const SIZES: [usize; 2] = [100_000, 1_000_000];

/// 取索引中第一个**文件**的完整名字（含 `_<frn>` 与扩展名）作为稀有词。
/// 完整文件名互不为子串（frn 唯一且 ext 定界），保证命中恰为 1 条；
/// 目录名（无 ext）不行——"report_2" 会是 "report_21.txt" 等的子串。
fn rare_name(ix: &ff_core::VolumeIndex) -> String {
    (1..ix.entry_count() as u32)
        .find_map(|id| {
            ix.name_str(id)
                .filter(|s| s.contains('_') && s.contains('.'))
        })
        .unwrap_or("report")
        .to_string()
}

fn bench_search(c: &mut Criterion) {
    let mut g = c.benchmark_group("search");
    for n in SIZES {
        let ix = build_index(&SynthConfig::with_count(n));
        let token = CancelToken::new();
        let rare = rare_name(&ix);
        g.bench_function(format!("substring_report_highfreq_{n}"), |b| {
            b.iter(|| {
                let q = parse("report").unwrap();
                black_box(ix.search(&q, &token).len())
            })
        });
        g.bench_function(format!("substring_rare_1hit_{n}"), |b| {
            b.iter(|| {
                let q = parse(&rare).unwrap();
                black_box(ix.search(&q, &token).len())
            })
        });
        g.bench_function(format!("and_report_summary_{n}"), |b| {
            b.iter(|| {
                let q = parse("report summary").unwrap();
                black_box(ix.search(&q, &token).len())
            })
        });
    }
    g.finish();
}

fn bench_write_path(c: &mut Criterion) {
    let mut g = c.benchmark_group("write_path");
    let n = 1_000_000;
    let ix = build_index(&SynthConfig::with_count(n));
    // 均匀采样约 1 万条（跳过根 id 0），覆盖全表不同目录深度。
    let ids: Vec<u32> = (1..ix.entry_count() as u32).step_by(101).collect();
    g.bench_function(format!("sample_{}_of_{n}", ids.len()), |b| {
        b.iter(|| {
            let mut buf = Vec::with_capacity(256);
            let mut total = 0usize;
            for &id in &ids {
                assert!(ix.write_path(id, &mut buf), "write_path 失败: id={id}");
                total += buf.len();
            }
            black_box(total)
        })
    });
    g.finish();
}

/// M5：T2 拼音（只遍历 cjk_ids）、T3 模糊（T1+T2 < 100 时全表并行扫描）、
/// 评分排序（T1 命中逐条算分 + 全量排序）。样本数压到 10，避免全表基准拖长 CI。
fn bench_m5(c: &mut Criterion) {
    let mut g = c.benchmark_group("m5");
    g.sample_size(10);
    g.measurement_time(Duration::from_secs(3));
    for n in SIZES {
        let ix = build_index(&SynthConfig::with_count(n));
        let token = CancelToken::new();
        let mopts = MatchOptions {
            fuzzy: true,
            pinyin: true,
        };
        let cfg = ScoreConfig::default();
        g.bench_function(format!("pinyin_zgr_{n}"), |b| {
            b.iter(|| {
                let q = parse("zgr").unwrap();
                black_box(ix.search_ranked(&q, &mopts, &cfg, &token).hits.len())
            })
        });
        g.bench_function(format!("fuzzy_vsc_{n}"), |b| {
            b.iter(|| {
                let q = parse("vsc").unwrap();
                black_box(ix.search_ranked(&q, &mopts, &cfg, &token).hits.len())
            })
        });
        g.bench_function(format!("ranked_report_{n}"), |b| {
            b.iter(|| {
                let q = parse("report").unwrap();
                black_box(ix.search_ranked(&q, &mopts, &cfg, &token).hits.len())
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench_search, bench_write_path, bench_m5);
criterion_main!(benches);
