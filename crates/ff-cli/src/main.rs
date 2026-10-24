//! ff-cli —— 命令行调试工具（DESIGN §3：scan / search / bench / dump）。
//!
//! 本轮只实现 `scan`（Track A，M0 摸底 + `VolumeScanner::full_scan`）；
//! `search` / `index` / `dump` 留待后续里程碑。参数解析手写
//! （不引入 clap，见 docs/DECISIONS.md）。

use std::process::ExitCode;

const USAGE: &str = r#"ff-cli —— Sift 命令行调试工具

用法:
  ff-cli scan <卷>   全量扫描一个 NTFS 卷（M0）：条目数 / 耗时 / 峰值内存 / 朴素子串搜索耗时
  ff-cli help        显示本帮助

卷的写法: C:  |  \\.\C:  |  \\?\Volume{...}
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
    if args.is_empty() {
        return Err("缺少卷参数，例如: ff-cli scan C:".to_string());
    }
    Err(format!("scan 尚未实现（Track A 本轮交付）: {args:?}"))
}
