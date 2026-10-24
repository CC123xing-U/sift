//! ff-cli —— 命令行调试工具（DESIGN §3：scan / search / bench / dump）。
//!
//! 本轮只实现 `scan`（Track A，M0 摸底 + `VolumeScanner::full_scan`）；
//! `search` / `index` / `dump` 留待后续里程碑。参数解析手写
//! （不引入 clap，见 docs/DECISIONS.md）。

use std::process::ExitCode;

const USAGE: &str = r#"ff-cli —— Sift 命令行调试工具

用法:
  ff-cli scan <卷> [子串]   全量扫描一个 NTFS 卷（M0），再计时朴素子串搜索
  ff-cli help              显示本帮助

卷的写法: C:  |  \\.\C:  |  \\?\Volume{...}
默认子串: ntdll（搜索不区分大小写）
"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("scan") => match run_scan(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("scan 失败: {e}");
                ExitCode::FAILURE
            }
        },
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("未知子命令: {other}\n");
            eprint!("{USAGE}");
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
