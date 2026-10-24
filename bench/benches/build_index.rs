//! 建索引基准（DESIGN §14：建索引；§1.3 全量建索引目标 ≤30s @200 万）。
//!
//! 本文件覆盖 1 万 / 10 万 / 100 万三档（criterion 复现：
//! `cargo bench -p sift-bench --bench build_index`）。
//! **200 万档跑一次**，由 `memprobe`（`src/bin/memprobe.rs`）执行并记录
//! `docs/BENCH.md`（criterion 多次采样建 200 万索引过慢，无必要）。
//!
//! 数据：`sift_bench::SynthConfig::with_count(n)`，默认种子
//! `DEFAULT_SEED = 0x5349_4654`（ASCII "SIFT"，见 lib.rs 模块文档）。

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use sift_bench::{build_index, SynthConfig};

fn bench_build_index(c: &mut Criterion) {
    let mut g = c.benchmark_group("build_index");
    for n in [10_000usize, 100_000, 1_000_000] {
        // 大档位减少采样次数：单次建索引即数秒，采样 10 次足够估计均值。
        g.sample_size(if n >= 1_000_000 { 10 } else { 20 });
        g.bench_function(format!("entries_{n}"), |b| {
            b.iter(|| {
                let ix = build_index(&SynthConfig::with_count(black_box(n)));
                // entry_count = n + 1（含推入的虚拟根，见 build_index 文档）。
                black_box(ix.entry_count())
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench_build_index);
criterion_main!(benches);
