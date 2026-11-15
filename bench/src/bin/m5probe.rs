//! M5 一次性性能实测（DESIGN §14 / docs/BENCH.md）。
//!
//! 200 万条合成索引上测 T1 子串、T2 拼音、T3 模糊全表扫描、评分排序的单次
//! 耗时；每项跑 3 次取最小值，避免首次冷缓存扰动。最后一组是关闭 T2/T3 的
//! 对照，用来证明开关确实生效而不是被全表扫描掩盖。
//!
//! 复现（release，无需管理员）：
//! `cargo run --release -p sift-bench --bin m5probe`

use std::hint::black_box;
use std::time::{Duration, Instant};

use ff_core::{parse, CancelToken, MatchOptions, ScoreConfig};
use sift_bench::{build_index, SynthConfig};

const N: usize = 2_000_000;
const ROUNDS: usize = 3;

/// 跑 `ROUNDS` 轮，返回（命中数，最小耗时）。
fn min_of(mut f: impl FnMut() -> (usize, Duration)) -> (usize, Duration) {
    let mut best = Duration::MAX;
    let mut hits = 0;
    for _ in 0..ROUNDS {
        let (h, d) = f();
        if d < best {
            best = d;
            hits = h;
        }
    }
    (hits, best)
}

fn main() {
    let t0 = Instant::now();
    let ix = build_index(&SynthConfig::with_count(N));
    println!(
        "build_index({N}): {:?}, entries={}, cjk_ids={}",
        t0.elapsed(),
        ix.entry_count(),
        ix.cjk_ids().len()
    );

    let token = CancelToken::new();
    let on = MatchOptions {
        fuzzy: true,
        pinyin: true,
    };
    let off = MatchOptions {
        fuzzy: false,
        pinyin: false,
    };
    let cfg = ScoreConfig::default();

    let (h, d) = min_of(|| {
        let q = parse("report").unwrap();
        let t = Instant::now();
        let n = black_box(ix.search(&q, &token).len());
        (n, t.elapsed())
    });
    println!("T1 substring 'report': {d:?}, hits={h}");

    let (h, d) = min_of(|| {
        let q = parse("zgr").unwrap();
        let t = Instant::now();
        let n = black_box(ix.search_ranked(&q, &on, &cfg, &token).hits.len());
        (n, t.elapsed())
    });
    println!("T2 pinyin 'zgr': {d:?}, hits={h}");

    let (h, d) = min_of(|| {
        let q = parse("vsc").unwrap();
        let t = Instant::now();
        let n = black_box(ix.search_ranked(&q, &on, &cfg, &token).hits.len());
        (n, t.elapsed())
    });
    println!("T3 fuzzy 'vsc' (full scan): {d:?}, hits={h}");

    let (h, d) = min_of(|| {
        let q = parse("report").unwrap();
        let t = Instant::now();
        let n = black_box(ix.search_ranked(&q, &on, &cfg, &token).hits.len());
        (n, t.elapsed())
    });
    println!("ranked 'report' (T1 + score sort): {d:?}, hits={h}");

    let (h, d) = min_of(|| {
        let q = parse("vsc").unwrap();
        let t = Instant::now();
        let n = black_box(ix.search_ranked(&q, &off, &cfg, &token).hits.len());
        (n, t.elapsed())
    });
    println!("fuzzy/pinyin OFF 'vsc': {d:?}, hits={h}");
}
