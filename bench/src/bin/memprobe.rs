//! 200 万条内存与性能实测（DESIGN §1.3：内存 ≤300MB / 建索引 ≤30s / 子串 ≤100ms）。
//!
//! 运行（release，长任务建议后台）：
//! `cargo run --release -p sift-bench --bin memprobe -- [条目数]`
//!
//! 输出：
//! - 条目数（含推入的虚拟根，= 条目数 + 1）；
//! - 建索引耗时（单次实测）；
//! - WorkingSet / PeakWorkingSet（`GetProcessMemoryInfo` 实测，非外推）；
//! - 子串（高频 `report` / 稀有唯一名）与 AND（`report summary`）耗时
//!   （7 次取中位数）；
//! - `write_path` 全量一次耗时。
//!
//! 条目数可用参数覆盖（如 50 万小档 smoke）；默认 200 万。

use std::time::Instant;

use ff_core::{parse, CancelToken};
use sift_bench::{build_index, meminfo, SynthConfig};

fn median_ms(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    let cfg = SynthConfig::with_count(count);
    println!("memprobe 配置: {cfg}");

    // 1) 建索引（单次实测；流式生成，不驻留全量 Vec）。
    let t0 = Instant::now();
    let ix = build_index(&cfg);
    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // 2) 内存实测（建索引后）。
    let ws = meminfo::working_set_bytes().unwrap_or(0);
    let peak = meminfo::peak_working_set_bytes().unwrap_or(0);

    println!("条目数: {} （含虚拟根）", ix.entry_count());
    println!("unresolved_parents: {}", ix.unresolved_parents());
    println!("建索引耗时: {build_ms:.1} ms");
    println!(
        "WorkingSet: {:.1} MB (Peak {:.1} MB) —— GetProcessMemoryInfo 实测",
        ws as f64 / 1048576.0,
        peak as f64 / 1048576.0
    );

    // 3) 搜索耗时（7 次取中位数）。
    let token = CancelToken::new();
    // 稀有词：取**文件**的完整名字（stem_frn.ext）。完整文件名互不为子串
    // （frn 唯一且 ext 定界），保证命中恰为 1 条；目录名（无 ext）不行——
    // "report_2" 会是 "report_21.txt" 等的子串。
    let rare = (1..ix.entry_count() as u32)
        .find_map(|id| {
            ix.name_str(id)
                .filter(|s| s.contains('_') && s.contains('.'))
        })
        .unwrap_or("report")
        .to_string();

    let queries: Vec<(&str, String)> = vec![
        ("substring_report(高频)", "report".to_string()),
        ("substring_rare(唯一名)", rare),
        ("and_report_summary", "report summary".to_string()),
    ];
    for (label, text) in &queries {
        let q = parse(text).expect("parse 失败");
        let mut samples = Vec::new();
        let mut hits = 0usize;
        for _ in 0..7 {
            let t = Instant::now();
            hits = ix.search(&q, &token).len();
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "{label}: 中位 {:.1} ms / 命中 {hits} 条",
            median_ms(samples)
        );
    }

    // 4) write_path 全量一次（含路径长度校验和，防优化掉）。
    let t = Instant::now();
    let mut buf = Vec::with_capacity(256);
    let mut total_len = 0usize;
    for id in 0..ix.entry_count() as u32 {
        assert!(ix.write_path(id, &mut buf), "write_path 失败: id={id}");
        total_len += buf.len();
    }
    println!(
        "write_path 全量 {} 条: {:.1} ms （路径字节和 {total_len}）",
        ix.entry_count(),
        t.elapsed().as_secs_f64() * 1000.0
    );

    // 目标对照（DESIGN §1.3）。
    println!("—— 目标对照（DESIGN §1.3，条目 {count}）——");
    println!(
        "内存 ≤ 300 MB: {} ({:.1} MB)",
        if ws <= 300 * 1048576 { "PASS" } else { "FAIL" },
        ws as f64 / 1048576.0
    );
    println!(
        "建索引 ≤ 30000 ms: {} ({build_ms:.1} ms)",
        if build_ms <= 30_000.0 { "PASS" } else { "FAIL" }
    );
}
