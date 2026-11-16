//! ff-cli —— 命令行调试工具（DESIGN §3：scan / search / bench / dump）。
//!
//! 已实现：`scan`（Track A，M0 摸底 + `VolumeScanner::full_scan`）、
//! `search`（Track F，M1：经 `InProcessEngine` 建索引并执行查询）。
//! `bench` / `dump` 留待后续里程碑。参数解析手写
//! （不引入 clap，见 docs/DECISIONS.md）。

use std::process::ExitCode;

use ff_engine::api::{SortKey, SortSpec};

const USAGE: &str = r#"ff-cli —— Sift 命令行调试工具

用法:
  ff-cli scan <卷> [子串]           全量扫描 NTFS 卷（M0），再计时朴素子串搜索
  ff-cli search <卷> <查询> [选项]  经 InProcess 引擎建索引并搜索（M1）
  ff-cli help                      显示本帮助

search 选项:
  --max <N>        最多输出 N 条（默认全部）
  --sort <键>      排序键: score | name | path | size | modified（默认 score）
  --asc            升序（默认降序）
  --case           区分大小写（默认不区分）

查询语法（M1 子集）: 空格分隔的词取 AND；"..." 为短语（可含空格）；均按名字子串匹配。

卷的写法: C:  |  \\.\C:  |  \\?\Volume{...}
默认子串: ntdll（搜索不区分大小写）
"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 两个子命令统一在这里收尾，避免各自重复错误出口。
    let result = match args.first().map(String::as_str) {
        Some("scan") => run_scan(&args[1..]),
        Some("search") => run_search(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("未知子命令: {other}\n");
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("失败: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Track A 实现：枚举卷 → 先记录 USN Journal → 扫 MFT → 打印统计。
fn run_scan(args: &[String]) -> Result<(), String> {
    if !(1..=2).contains(&args.len()) {
        return Err("用法: ff-cli scan <卷> [子串]，例如: ff-cli scan C: ntdll".to_string());
    }
    if args.get(1).is_some_and(String::is_empty) {
        return Err("搜索子串不能为空".to_string());
    }

    #[cfg(windows)]
    {
        run_scan_windows(args)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        Err("M0 NTFS 扫描仅支持 Windows".to_string())
    }
}

#[cfg(windows)]
fn run_scan_windows(args: &[String]) -> Result<(), String> {
    use std::time::Instant;

    use ff_ntfs::{
        enumerate_volumes, peak_working_set_bytes, working_set_bytes, NtfsVolumeScanner, RawEntry,
        VolumeId, VolumeInfo, VolumeScanner,
    };

    let volumes: Vec<VolumeInfo> = enumerate_volumes()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|v| v.fs_name.eq_ignore_ascii_case("NTFS"))
        .collect();
    if volumes.is_empty() {
        return Err("未枚举到可用的 NTFS 卷".to_string());
    }

    println!("已枚举 NTFS 卷:");
    for volume in &volumes {
        match volume.drive {
            Some(drive) => println!(
                "  {drive}: serial={:#010X} guid={}",
                volume.serial, volume.guid_path
            ),
            None => println!("  {} serial={:#010X}", volume.guid_path, volume.serial),
        }
    }

    let volume = find_volume(&volumes, &args[0])
        .ok_or_else(|| format!("未找到卷 {}；请使用上面列出的盘符或 GUID 路径", args[0]))?;
    let id = VolumeId {
        serial: volume.serial,
        drive: volume.drive,
    };
    println!("\n开始扫描 {id}（先查询 USN Journal，再读取 MFT）");

    // M0 保留 RawEntry 供朴素搜索；因此峰值 WorkingSet 包含本 Vec 与扫描器自身。
    let mut entries: Vec<RawEntry> = Vec::new();
    let meta = NtfsVolumeScanner
        .full_scan(&id, &mut |entry| entries.push(entry))
        .map_err(|e| e.to_string())?;
    debug_assert_eq!(meta.entries, entries.len() as u64);
    let peak = peak_working_set_bytes();
    let current = working_set_bytes();

    println!("条目数: {}", meta.entries);
    println!("全量耗时: {:.3} s", meta.elapsed.as_secs_f64());
    println!("父解析警告: {}", meta.warnings);
    if meta.journal_id == 0 {
        println!("USN Journal: 不可用；本卷退化为仅启动时全量重扫");
    } else {
        println!("USN Journal ID: {}", meta.journal_id);
        println!("扫描前 Next USN: {}", meta.next_usn);
    }
    print_memory("扫描后峰值 WorkingSet", peak);
    print_memory("扫描后当前 WorkingSet", current);

    let query = args.get(1).map_or("ntdll", String::as_str);
    let needle = query.to_lowercase();
    let started = Instant::now();
    let hits = entries
        .iter()
        .filter(|entry| entry.name.to_lowercase().contains(&needle))
        .count();
    let elapsed = started.elapsed();
    println!(
        "朴素子串搜索: query={query:?} 命中={hits} 耗时={:.3} ms",
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

#[cfg(windows)]
fn find_volume<'a>(
    volumes: &'a [ff_ntfs::VolumeInfo],
    input: &str,
) -> Option<&'a ff_ntfs::VolumeInfo> {
    if let Some(drive) = drive_letter(input) {
        return volumes.iter().find(|v| {
            v.drive
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&drive))
        });
    }
    let input = input.trim_end_matches('\\');
    volumes.iter().find(|v| {
        v.guid_path
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(input)
    })
}

#[cfg(windows)]
fn drive_letter(input: &str) -> Option<char> {
    let input = input.to_ascii_uppercase();
    let rest = input
        .strip_prefix(r"\\.\")
        .or_else(|| input.strip_prefix(r"\\?\"))
        .unwrap_or(&input);
    let mut chars = rest.chars();
    let drive = chars.next()?;
    (drive.is_ascii_alphabetic() && chars.next() == Some(':') && chars.next().is_none())
        .then_some(drive)
}

#[cfg(windows)]
fn print_memory(label: &str, bytes: Option<usize>) {
    match bytes {
        Some(bytes) => println!(
            "{label}: {} B ({:.2} MiB)",
            bytes,
            bytes as f64 / 1_048_576.0
        ),
        None => println!("{label}: 获取失败"),
    }
}

// —— Track F：search 子命令（经 InProcess 引擎；M1） ——————————————————

/// `search` 子命令解析后的参数。
///
/// 解析逻辑不依赖平台（便于单测）；实际扫描执行见 [`run_search`]。
#[derive(Debug)]
struct SearchArgs {
    /// 卷指定串（盘符 / GUID 路径）。
    volume: String,
    /// 查询串（多个位置参数以空格连接；空格在查询语法中即 AND）。
    query: String,
    /// 最多输出条数。
    max_hits: Option<usize>,
    /// 排序方式。
    sort: SortSpec,
    /// 是否区分大小写。
    case_sensitive: bool,
}

/// 解析 `search` 参数：`ff-cli search <卷> <查询...> [--max N] [--sort K] [--asc] [--case]`。
fn parse_search_args(args: &[String]) -> Result<SearchArgs, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut max_hits = None;
    let mut key = SortKey::Score;
    let mut ascending = false;
    let mut case_sensitive = false;

    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--max" => {
                let raw = args.get(i + 1).ok_or("--max 需要一个非负整数参数")?;
                max_hits = Some(
                    raw.parse::<usize>()
                        .map_err(|_| format!("--max 需要非负整数，收到 {raw:?}"))?,
                );
                i += 2;
            }
            "--sort" => {
                let raw = args.get(i + 1).ok_or("--sort 需要一个排序键")?;
                key = parse_sort_key(raw)?;
                i += 2;
            }
            "--asc" => {
                ascending = true;
                i += 1;
            }
            "--case" => {
                case_sensitive = true;
                i += 1;
            }
            other if other.starts_with("--") => return Err(format!("未知选项 {other:?}")),
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }

    let mut iter = positional.into_iter();
    let volume = iter.next().ok_or("缺少 <卷> 参数")?;
    let terms: Vec<String> = iter.collect();
    if terms.is_empty() {
        return Err("缺少 <查询> 参数".to_string());
    }
    Ok(SearchArgs {
        volume,
        query: terms.join(" "),
        max_hits,
        sort: SortSpec {
            key,
            descending: !ascending,
        },
        case_sensitive,
    })
}

/// 排序键字面量 → [`SortKey`]。
fn parse_sort_key(raw: &str) -> Result<SortKey, String> {
    match raw.to_ascii_lowercase().as_str() {
        "score" => Ok(SortKey::Score),
        "name" => Ok(SortKey::Name),
        "path" => Ok(SortKey::Path),
        "size" => Ok(SortKey::Size),
        "modified" | "mtime" => Ok(SortKey::Modified),
        other => Err(format!(
            "未知排序键 {other:?}；可用: score | name | path | size | modified"
        )),
    }
}

/// `search` 子命令入口：仅在 Windows 上可用（依赖 NTFS 卷读取）。
fn run_search(args: &[String]) -> Result<(), String> {
    let parsed = parse_search_args(args)?;
    #[cfg(windows)]
    {
        run_search_windows(&parsed)
    }
    #[cfg(not(windows))]
    {
        let _ = parsed;
        Err("search 需要 Windows（依赖 NTFS 卷读取）".to_string())
    }
}

#[cfg(windows)]
fn run_search_windows(args: &SearchArgs) -> Result<(), String> {
    use std::time::Instant;

    use ff_engine::api::{EngineClient, SearchOptions, SearchRequest};
    use ff_engine::inprocess::InProcessEngine;
    use ff_ntfs::{enumerate_volumes, AutoVolumeScanner, VolumeId};

    // M7（DESIGN §5.4）：不再预过滤 NTFS——非 NTFS 卷（exFAT / FAT32）由
    // `AutoVolumeScanner` 自动改走目录遍历回退，CLI 因此也能查 U 盘上的文件。
    let volumes: Vec<ff_ntfs::VolumeInfo> = enumerate_volumes().map_err(|e| e.to_string())?;
    if volumes.is_empty() {
        return Err("未枚举到可用的卷".to_string());
    }
    let info = find_volume(&volumes, &args.volume)
        .ok_or_else(|| format!("未找到卷 {}；请使用上面列出的盘符或 GUID 路径", args.volume))?;
    let id = VolumeId {
        serial: info.serial,
        drive: info.drive,
    };
    println!(
        "目标卷 {} 文件系统 {}（{}）",
        info.drive
            .map(|c| format!("{c}:"))
            .unwrap_or_else(|| info.guid_path.clone()),
        info.fs_name,
        if info.fs_name.eq_ignore_ascii_case("NTFS") {
            "MFT + USN 增量"
        } else {
            "目录遍历回退（无增量）"
        }
    );

    // 建索引：`rescan(Some(卷))` 经扫描器全量建索引（NTFS 走 MFT，其它走目录遍历）。
    println!("正在扫描并建立索引: {id}");
    let build_start = Instant::now();
    let engine = InProcessEngine::new(Box::new(AutoVolumeScanner));
    engine.rescan(Some(id.clone())).map_err(|e| e.to_string())?;
    let status = engine.status();
    println!(
        "索引就绪: {} 卷 / {} 条 / 建索引 {:.3} s",
        status.volumes.len(),
        status.total_entries,
        build_start.elapsed().as_secs_f64()
    );

    let summary = engine
        .search(SearchRequest {
            query: args.query.clone(),
            sort: args.sort,
            max_hits: args.max_hits,
            opts: SearchOptions {
                case_sensitive: args.case_sensitive,
                ..Default::default()
            },
        })
        .map_err(|e| e.to_string())?;
    // 语法错误经 SearchSummary 带回，不是 Err；CLI 上仍算失败退出。
    if let Some(e) = summary.parse_error {
        return Err(format!("查询语法错误: {e}"));
    }
    println!(
        "查询 {:?}: 命中 {} 条 / 搜索耗时 {} ms",
        args.query, summary.total, summary.elapsed_ms
    );
    if summary.total == 0 {
        return Ok(());
    }

    let limit = args.max_hits.unwrap_or(summary.total).min(summary.total);
    let rows = engine
        .range(summary.id, 0, limit)
        .map_err(|e| e.to_string())?;
    for (n, row) in rows.iter().enumerate() {
        println!(
            "{:>6}  [{}]  {:>10}  {}\t{}",
            n,
            if row.is_dir { "D" } else { "F" },
            row.size.map_or_else(|| "-".to_string(), human_size),
            row.dir,
            row.name
        );
    }
    Ok(())
}

/// 字节数 → 人类可读字符串（1024 进制）。
#[cfg(windows)]
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// —— Track F 单测：参数解析不依赖平台，可在本 crate 的 CI（含非 Windows）上跑 ———
#[cfg(test)]
mod search_tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_to_score_descending_case_insensitive() {
        let parsed = parse_search_args(&argv(&["C:", "readme"])).unwrap();
        assert_eq!(parsed.volume, "C:");
        assert_eq!(parsed.query, "readme");
        assert_eq!(parsed.max_hits, None);
        assert_eq!(parsed.sort.key, SortKey::Score);
        assert!(parsed.sort.descending);
        assert!(!parsed.case_sensitive);
    }

    #[test]
    fn multiword_query_joined_by_space_is_and() {
        // 查询语法里空格即 AND：多个位置参数应拼成单个查询串
        let parsed = parse_search_args(&argv(&["C:", "my", "docs"])).unwrap();
        assert_eq!(parsed.query, "my docs");
    }

    #[test]
    fn max_and_sort_options() {
        let parsed =
            parse_search_args(&argv(&["C:", "x", "--max", "25", "--sort", "SIZE"])).unwrap();
        assert_eq!(parsed.max_hits, Some(25));
        assert_eq!(parsed.sort.key, SortKey::Size);
        assert!(parsed.sort.descending); // 未给 --asc
    }

    #[test]
    fn all_sort_keys_accepted() {
        for (raw, expected) in [
            ("score", SortKey::Score),
            ("name", SortKey::Name),
            ("path", SortKey::Path),
            ("size", SortKey::Size),
            ("modified", SortKey::Modified),
            ("mtime", SortKey::Modified),
        ] {
            let parsed = parse_search_args(&argv(&["C:", "x", "--sort", raw])).unwrap();
            assert_eq!(parsed.sort.key, expected, "{raw} 应解析为 {expected:?}");
        }
    }

    #[test]
    fn asc_and_case_flags() {
        let parsed = parse_search_args(&argv(&["C:", "x", "--asc", "--case"])).unwrap();
        assert!(!parsed.sort.descending);
        assert!(parsed.case_sensitive);
    }

    #[test]
    fn missing_arguments_error() {
        assert!(parse_search_args(&argv(&[])).is_err());
        let err = parse_search_args(&argv(&["C:"])).unwrap_err();
        assert!(err.contains("查询"), "{err}");
    }

    #[test]
    fn rejects_unknown_flags_and_bad_values() {
        assert!(parse_search_args(&argv(&["C:", "x", "--nope"])).is_err());
        assert!(parse_search_args(&argv(&["C:", "x", "--max", "abc"])).is_err());
        assert!(parse_search_args(&argv(&["C:", "x", "--sort", "zzz"])).is_err());
        // 缺参数的开关应收干净的错误，不能 panic / 越界
        assert!(parse_search_args(&argv(&["C:", "x", "--max"])).is_err());
        assert!(parse_search_args(&argv(&["C:", "x", "--sort"])).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn human_size_formats_binaries() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_drive_forms() {
        assert_eq!(drive_letter("C:"), Some('C'));
        assert_eq!(drive_letter(r"\\.\c:"), Some('C'));
        assert_eq!(drive_letter(r"\\?\C:"), Some('C'));
        assert_eq!(drive_letter("C"), None);
        assert_eq!(drive_letter(r"\\.\C:\"), None);
        assert_eq!(drive_letter(r"\\?\Volume{ABC}\"), None);
    }

    #[test]
    fn finds_volume_by_guid_without_trailing_slash() {
        let volumes = [ff_ntfs::VolumeInfo {
            serial: 7,
            fs_name: "NTFS".to_string(),
            guid_path: r"\\?\Volume{ABC}\".to_string(),
            drive: None,
        }];
        let found = find_volume(&volumes, r"\\?\volume{abc}").expect("GUID 应匹配");
        assert_eq!(found.serial, 7);
    }
}
