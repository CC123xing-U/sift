//! filelist 导入（M7，`docs/01-Everything-功能实现文档.md` §9.2）。
//!
//! 把外部文件列表变成可搜索的卷索引，两条输入路径：
//!
//! 1. **纯路径列表**：每行一个绝对路径（`D:\a\b.txt`）。带 `#` 开头的行当注释。
//! 2. **CSV**（Everything 导出风格）：首行是表头，列名大小写不敏感、列序任意。
//!    必须能找到「路径」列（`Path` / `Full Path` / `Folder` / `路径` / `目录`）；
//!    其余列可选（大小、三个时间、属性）。名字列与路径列同时存在时拼成完整路径。
//!
//! 本模块是**纯逻辑**（`#![forbid(unsafe_code)]`，无平台依赖）：不做任何文件系统 IO，
//! 只解析文本、构造 `VolumeIndex`。同一份列表每次导入得到同一个索引，便于落盘与对账。
//!
//! # 为什么按盘符分卷
//!
//! 路径现算（`path.rs`）的前缀来自 `VolumeInfo.drive`，一个索引只能有一个前缀。
//! 列表里混有 `C:` 与 `D:` 时无法用单卷表达，故按盘符分组、一盘一卷；组内合成
//! 目录树（列表没给出的中间目录也会补齐），使条目路径与真实文件系统一致，可直接打开。

use std::collections::HashMap;

use super::entry::{attrs, EntryInput, IndexBuilder, VolumeIndex, VolumeInfo};

/// 单条路径允许的最大层级（超过按无效行跳过）。与目录遍历的上限同量级。
pub const MAX_DEPTH: usize = 64;

/// 导入卷 `serial` 的标记位：最高位为 1 表示「不是真实卷，是导入的列表」。
///
/// `VolumeId` 以 `serial` 为稳定键，靠这一位把导入卷与真实卷隔开，
/// 避免与某块盘的真实序列号撞车而覆盖它的索引。
pub const IMPORT_SERIAL_FLAG: u32 = 0x8000_0000;

/// 列表格式（决定解析分支与诊断文案）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListFormat {
    /// 尚未判定。
    #[default]
    Unknown,
    /// 每行一个路径。
    Paths,
    /// CSV（首行表头）。
    Csv,
}

/// 一条待导入条目。时间单位是 FILETIME（100ns，1601 纪元），与 `RawEntry` 一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListEntry {
    /// 规范化后的绝对路径（单反斜杠、无尾斜杠、盘符大写）。
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Win32 属性位；0 表示列表未提供（按 `is_dir` 补目录位）。
    pub attrs: u32,
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
}

/// 解析统计（用于给用户看「读了多少行、跳了多少」）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseReport {
    pub format: ListFormat,
    /// 非空、非注释行数。
    pub lines: usize,
    /// 成功解析的行数。
    pub accepted: usize,
    /// 跳过的行数（相对路径 / 缺盘符 / 层级过深 / 时间格式错不算跳过）。
    pub skipped: usize,
}

/// 解析结果。
#[derive(Clone, Debug, Default)]
pub struct ParseOutcome {
    pub entries: Vec<ListEntry>,
    pub report: ParseReport,
    /// 首个导致整份列表不可用的原因（如「CSV 缺少路径列」）；为空表示可继续。
    pub fatal: Option<String>,
    /// 前几条被跳过的原因（最多 5 条，供 CLI 直接打印，避免用户猜）。
    pub skip_reasons: Vec<String>,
}

impl ParseOutcome {
    /// 是否没有任何致命问题且至少有一条条目。
    pub fn is_usable(&self) -> bool {
        self.fatal.is_none() && !self.entries.is_empty()
    }
}

/// 解析一段文本（自动判定纯路径 / CSV）。
pub fn parse_list(text: &str) -> ParseOutcome {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = ParseOutcome::default();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        out.fatal = Some("列表为空".to_string());
        out.report.format = ListFormat::Unknown;
        return out;
    };

    if looks_like_csv_header(first) {
        out.report.format = ListFormat::Csv;
        parse_csv(first, lines, &mut out);
    } else {
        out.report.format = ListFormat::Paths;
        parse_paths(std::iter::once(first).chain(lines), &mut out);
    }
    out
}

/// 首行是否像 CSV 表头：含逗号，且出现任一列名关键词。
fn looks_like_csv_header(line: &str) -> bool {
    if !line.contains(',') {
        return false;
    }
    let lower = line.to_lowercase();
    const KEYS: &[&str] = &[
        "name",
        "filename",
        "path",
        "full path",
        "folder",
        "directory",
        "size",
        "attributes",
        "文件名",
        "名称",
        "路径",
        "目录",
        "大小",
        "属性",
    ];
    KEYS.iter().any(|k| lower.contains(k))
}

fn parse_paths<'a>(lines: impl Iterator<Item = &'a str>, out: &mut ParseOutcome) {
    for line in lines {
        if line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        out.report.lines += 1;
        match normalize_path(line) {
            Ok(path) => {
                out.report.accepted += 1;
                out.entries.push(ListEntry {
                    is_dir: false,
                    attrs: 0,
                    size: 0,
                    mtime: 0,
                    ctime: 0,
                    atime: 0,
                    path,
                });
            }
            Err(reason) => {
                out.report.skipped += 1;
                push_reason(out, &format!("{line}：{reason}"));
            }
        }
    }
    if out.entries.is_empty() && out.fatal.is_none() {
        out.fatal = Some("没有可用的绝对路径（每行需形如 D:\\dir\\file.txt）".to_string());
    }
}

/// CSV 列角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Col {
    Path,
    Name,
    Size,
    Mtime,
    Ctime,
    Atime,
    Attrs,
    Ignored,
}

fn column_role(raw: &str) -> Col {
    let h = raw.trim().trim_matches('"').trim().to_lowercase();
    let h = h.trim_start_matches('\u{feff}');
    match h {
        "path" | "full path" | "fullpath" | "folder" | "directory" | "dir" | "路径" | "目录"
        | "所在文件夹" => Col::Path,
        "name" | "filename" | "file name" | "文件名" | "名称" => Col::Name,
        "size" | "大小" | "size (bytes)" => Col::Size,
        "date modified" | "modified" | "mtime" | "修改时间" | "修改日期" => Col::Mtime,
        "date created" | "created" | "ctime" | "创建时间" | "创建日期" => Col::Ctime,
        "date accessed" | "accessed" | "atime" | "访问时间" | "访问日期" => Col::Atime,
        "attributes" | "attr" | "属性" => Col::Attrs,
        _ => Col::Ignored,
    }
}

fn parse_csv<'a>(header: &str, lines: impl Iterator<Item = &'a str>, out: &mut ParseOutcome) {
    let cols: Vec<Col> = split_csv_line(header)
        .iter()
        .map(|c| column_role(c))
        .collect();
    let path_idx = cols.iter().position(|c| *c == Col::Path);
    let name_idx = cols.iter().position(|c| *c == Col::Name);
    if path_idx.is_none() && name_idx.is_none() {
        out.fatal = Some("CSV 缺少路径列（Path / Full Path / Folder / 路径）".to_string());
        return;
    }

    for line in lines {
        out.report.lines += 1;
        let fields = split_csv_line(line);
        let path_field = path_idx.and_then(|i| fields.get(i)).map(|s| s.as_str());
        let name_field = name_idx.and_then(|i| fields.get(i)).map(|s| s.as_str());

        // 路径列的取值可能已经是完整路径，也可能只是目录；两种都拼一次再规范化。
        let candidate = match (path_field, name_field) {
            (Some(p), Some(n)) if !p.trim().is_empty() && !n.trim().is_empty() => {
                format!("{}\\{}", p.trim().trim_end_matches(['\\', '/']), n.trim())
            }
            (Some(p), _) if !p.trim().is_empty() => p.trim().to_string(),
            (_, Some(n)) if !n.trim().is_empty() => n.trim().to_string(),
            _ => {
                out.report.skipped += 1;
                push_reason(out, &format!("{line}：路径与名字都为空"));
                continue;
            }
        };

        let path = match normalize_path(&candidate) {
            Ok(p) => p,
            Err(reason) => {
                out.report.skipped += 1;
                push_reason(out, &format!("{candidate}：{reason}"));
                continue;
            }
        };

        let attr_bits = cols
            .iter()
            .position(|c| *c == Col::Attrs)
            .and_then(|i| fields.get(i))
            .and_then(|s| parse_attrs(s))
            .unwrap_or(0);
        let is_dir = attr_bits & attrs::FILE_ATTRIBUTE_DIRECTORY != 0;
        let size = cols
            .iter()
            .position(|c| *c == Col::Size)
            .and_then(|i| fields.get(i))
            .and_then(|s| s.trim().replace(',', "").parse::<u64>().ok())
            .unwrap_or(0);
        let ts = |role: Col| {
            cols.iter()
                .position(|c| *c == role)
                .and_then(|i| fields.get(i))
                .and_then(|s| parse_timestamp(s))
                .unwrap_or(0)
        };

        out.report.accepted += 1;
        out.entries.push(ListEntry {
            is_dir,
            attrs: attr_bits,
            size,
            mtime: ts(Col::Mtime),
            ctime: ts(Col::Ctime),
            atime: ts(Col::Atime),
            path,
        });
    }
    if out.entries.is_empty() {
        // 只有文件名列的 CSV（Everything 的部分导出）无法定位文件：给出可操作提示，
        // 而不是让用户对着「0 条」猜。
        out.fatal = Some(
            "CSV 没有可解析的数据行；若文件只有文件名列，请补上 Path / Full Path / 路径 列"
                .to_string(),
        );
    }
}

fn push_reason(out: &mut ParseOutcome, reason: &str) {
    if out.skip_reasons.len() < 5 {
        out.skip_reasons.push(reason.to_string());
    }
}

/// 拆分一行 CSV：支持双引号包裹（内部 `""` 转义）、逗号在引号内不拆。
fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// 规范化绝对路径；返回 `Err(原因)` 表示该行无效。
///
/// - 去掉 `\\?\` 前缀、统一 `/` → `\`、折叠重复分隔符、去掉尾部分隔符；
/// - 盘符统一大写（Windows 路径不区分大小写，统一后同一路径只有一个键）；
/// - 只接受 `<盘符>:\...`；UNC（`\\server\share`）与相对路径按无效处理。
pub fn normalize_path(raw: &str) -> Result<String, &'static str> {
    let mut s = raw.trim().trim_matches('"').trim().to_string();
    if s.is_empty() {
        return Err("空路径");
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }
    if s.starts_with(r"\\") {
        return Err("不支持 UNC 网络路径");
    }
    let mut normalized = String::with_capacity(s.len());
    let mut last_was_sep = false;
    for ch in s.chars() {
        let c = if ch == '/' { '\\' } else { ch };
        if c == '\\' {
            if last_was_sep {
                continue;
            }
            last_was_sep = true;
        } else {
            last_was_sep = false;
        }
        normalized.push(c);
    }

    let bytes = normalized.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return Err("不是绝对路径（需以盘符开头，如 D:\\）");
    }
    // 盘符后面必须是 '\' 或字符串结束（`D:foo` 是相对当前目录的路径，语义不同）。
    if bytes.len() > 2 && bytes[2] != b'\\' {
        return Err("盘符后缺少反斜杠（D:foo 是相对路径）");
    }
    let drive = (bytes[0] as char).to_ascii_uppercase();
    let rest = if normalized.len() > 2 {
        &normalized[3..]
    } else {
        ""
    };
    let rest = rest.trim_end_matches('\\');
    if rest.is_empty() {
        return Err("只有盘符");
    }
    // 层级上限：防止异常输入把索引撑爆（正常路径远小于此）。
    if rest.split('\\').count() > MAX_DEPTH {
        return Err("层级过深");
    }
    Ok(format!("{drive}:\\{rest}"))
}

/// 属性列：接受十进制数字或 `DIR` / `D` / `文件` / `文件夹` 之类的词。
fn parse_attrs(raw: &str) -> Option<u32> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    let lower = s.to_lowercase();
    let mut bits = 0u32;
    if lower.contains("dir") || lower.contains("文件夹") || lower.contains("目录") {
        bits |= attrs::FILE_ATTRIBUTE_DIRECTORY;
    }
    if lower.contains("hidden") || lower.contains("隐藏") {
        bits |= 0x2;
    }
    Some(bits)
}

/// 解析时间列：`2026-11-16 22:03:01` / `2026/11/16 22:03` / `2026-11-16`；
/// 也接受纯数字的 FILETIME（> 1e17 时视为已经是 FILETIME）。
fn parse_timestamp(raw: &str) -> Option<i64> {
    let s = raw.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return if n > 100_000_000_000_000_000 {
            Some(n)
        } else {
            None
        };
    }
    let sep = if s.contains('T') { 'T' } else { ' ' };
    let mut parts = s.split(sep);
    let date = parts.next()?;
    let time = parts.next().unwrap_or("00:00:00");
    let d: Vec<i64> = date
        .split(['-', '/'])
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<i64>().ok())
        .collect::<Option<Vec<i64>>>()?;
    if d.len() != 3 {
        return None;
    }
    let (y, m, day) = (d[0], d[1], d[2]);
    if !(1601..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let t: Vec<i64> = time
        .split(':')
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<i64>()
                .unwrap_or(0)
        })
        .collect();
    let (hh, mm, ss) = (
        t.first().copied().unwrap_or(0),
        t.get(1).copied().unwrap_or(0),
        t.get(2).copied().unwrap_or(0),
    );
    if !(0..24).contains(&hh) || !(0..60).contains(&mm) || !(0..61).contains(&ss) {
        return None;
    }
    let days = days_from_civil(y, m, day);
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    // 1970-01-01 → 1601-01-01 的 100ns 刻度数。
    const UNIX_TO_FILETIME_TICKS: i64 = 116_444_736_000_000_000;
    Some(secs * 10_000_000 + UNIX_TO_FILETIME_TICKS)
}

/// 公历日期 → 距 1970-01-01 的天数（Howard Hinnant 的 `days_from_civil`）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 一个导入卷：`info` 用于 `EngineClient::add_volume` / 落盘，`index` 是可直接搜索的索引。
pub struct ImportedVolume {
    pub info: VolumeInfo,
    pub index: VolumeIndex,
    /// 该卷的条目数（含为路径补齐的中间目录）。
    pub entries: usize,
    /// 为补齐路径而合成的目录数（列表里没显式给出的中间层级）。
    pub synthesized_dirs: usize,
}

/// 把条目按盘符分组、各自合成目录树，得到若干可直接加入引擎的卷。
///
/// `label` 只参与 `serial` 计算（同一份列表 + 同一 label 得到同一 `serial`，
/// 落盘文件名才稳定）；不同 label 的同名路径算两个卷。
pub fn build_indexes(entries: &[ListEntry], label: &str) -> Vec<ImportedVolume> {
    // 盘符 → (路径 → 是否已知为目录)。用 BTreeMap 保证输出顺序稳定（测试可断言）。
    let mut by_drive: std::collections::BTreeMap<char, Vec<&ListEntry>> =
        std::collections::BTreeMap::new();
    for e in entries {
        if let Some(c) = e.path.chars().next() {
            by_drive.entry(c.to_ascii_uppercase()).or_default().push(e);
        }
    }

    let mut out = Vec::with_capacity(by_drive.len());
    for (drive, items) in by_drive {
        let prefix = format!("{drive}:\\");
        let mut info = VolumeInfo {
            drive: Some(drive),
            guid_path: String::new(),
            serial: import_serial(label, drive),
            journal_id: 0,
            next_usn: 0,
        };
        // serial 只是标识；落盘时按它命名，不需要回填。
        info.journal_id = 0;

        let mut builder = IndexBuilder::new(info.clone());
        // 根条目：名字不参与路径拼接（`path.rs` 会跳过根），必须存在，
        // 否则第一个挂到 `parent_frn = 0` 的顶层目录会被当成根、丢掉自己的名字。
        builder.push(EntryInput {
            frn: path_key(&prefix),
            parent_frn: 0,
            name: ".".to_string(),
            is_dir: true,
            attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
            size: 0,
            mtime: 0,
            ctime: 0,
            atime: 0,
        });

        let mut emitted: HashMap<u64, bool> = HashMap::new();
        emitted.insert(path_key(&prefix), true);
        let mut synthesized = 0usize;
        let mut count = 1usize;

        for e in items {
            let rel = &e.path[prefix.len()..];
            let comps: Vec<&str> = rel.split('\\').filter(|c| !c.is_empty()).collect();
            if comps.is_empty() {
                continue;
            }
            let mut parent_key = path_key(&prefix);
            let mut acc = prefix.clone();
            for (i, comp) in comps.iter().enumerate() {
                acc.push_str(comp);
                let is_last = i + 1 == comps.len();
                let key = path_key(&acc);
                let want_dir = if is_last { e.is_dir } else { true };
                match emitted.get(&key) {
                    Some(known_dir) => {
                        // 已存在：若是「先当文件、后来发现是目录」，索引无法改判，
                        // 保持首次写入（Windows 上同名文件与目录不会共存，实际不会发生）。
                        let _ = known_dir;
                    }
                    None => {
                        if !is_last {
                            synthesized += 1;
                        }
                        builder.push(EntryInput {
                            frn: key,
                            parent_frn: if key == parent_key { 0 } else { parent_key },
                            name: (*comp).to_string(),
                            is_dir: want_dir,
                            attrs: if want_dir {
                                attrs::FILE_ATTRIBUTE_DIRECTORY
                            } else {
                                e.attrs
                            },
                            size: if want_dir { 0 } else { e.size },
                            mtime: e.mtime,
                            ctime: e.ctime,
                            atime: e.atime,
                        });
                        emitted.insert(key, want_dir);
                        count += 1;
                    }
                }
                parent_key = key;
                acc.push('\\');
            }
        }

        out.push(ImportedVolume {
            info,
            index: builder.finish(),
            entries: count,
            synthesized_dirs: synthesized,
        });
    }
    out
}

/// 导入卷的 `serial`：`label + 盘符` 的 FNV-1a 低位，再置上导入标记位。
fn import_serial(label: &str, drive: char) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in label.to_lowercase().bytes().chain([drive as u8]) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    ((h as u32) & 0x7fff_ffff) | IMPORT_SERIAL_FLAG
}

/// 路径 → 稳定键（FNV-1a 64，小写折叠）。与 `ff-ntfs::walk::path_key` 同算法：
/// 两个 crate 互不依赖，故各留一份；改算法必须同时改（否则同一路径两种键）。
fn path_key(logical: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in logical.to_lowercase().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    if h == 0 {
        1
    } else {
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(v: &ImportedVolume, path: &str) -> Option<String> {
        let mut buf = Vec::new();
        for id in 0..v.index.entry_count() as u32 {
            if v.index.write_path(id, &mut buf) && buf == path.as_bytes() {
                return v.index.path_string(id);
            }
        }
        None
    }

    #[test]
    fn normalize_basic() {
        assert_eq!(
            normalize_path(r"d:\Music\a.mp3").unwrap(),
            r"D:\Music\a.mp3"
        );
        assert_eq!(
            normalize_path(r"D:/Music//a.mp3").unwrap(),
            r"D:\Music\a.mp3"
        );
        assert_eq!(normalize_path(r"D:\Music\").unwrap(), r"D:\Music");
        assert_eq!(
            normalize_path(r"\\?\D:\a\b.txt").unwrap(),
            r"D:\a\b.txt",
            "\\?\\ 前缀应被剥掉"
        );
        assert_eq!(
            normalize_path("\"D:\\a b\\c.txt\"").unwrap(),
            r"D:\a b\c.txt"
        );
    }

    #[test]
    fn normalize_rejects_bad() {
        assert!(normalize_path("").is_err());
        assert!(normalize_path("relative\\a.txt").is_err());
        assert!(normalize_path(r"D:foo").is_err(), "D:foo 是相对路径");
        assert!(normalize_path("D:").is_err(), "只有盘符");
        assert!(
            normalize_path(r"\\server\share\a.txt").is_err(),
            "UNC 不支持"
        );
        let deep = format!("D:\\{}x.txt", "a\\".repeat(MAX_DEPTH));
        assert!(normalize_path(&deep).is_err(), "层级超限");
    }

    #[test]
    fn parse_plain_paths_with_bom_and_comments() {
        let text = "\u{feff}# 注释\r\nD:\\a\\b.txt\r\n\r\nd:/x/y.txt\r\nbad/relative\r\n";
        let out = parse_list(text);
        assert_eq!(out.report.format, ListFormat::Paths);
        assert_eq!(out.report.accepted, 2);
        assert_eq!(out.report.skipped, 1);
        assert_eq!(out.entries[0].path, r"D:\a\b.txt");
        assert_eq!(out.entries[1].path, r"D:\x\y.txt");
        assert!(out.is_usable());
    }

    #[test]
    fn parse_csv_with_path_and_meta() {
        let csv = "Filename,Path,Size,Date Modified,Attributes\n\
                   b.txt,D:\\a,1024,2026-11-16 22:03:01,32\n\
                   \"we,ird.txt\",D:\\a, 7 ,2026-11-15,0\n";
        let out = parse_list(csv);
        assert_eq!(out.report.format, ListFormat::Csv);
        assert_eq!(out.report.accepted, 2);
        let e = &out.entries[0];
        assert_eq!(e.path, r"D:\a\b.txt");
        assert_eq!(e.size, 1024);
        assert!(!e.is_dir);
        let ft = e.mtime;
        // 2026-11-16 22:03:01 UTC → 校验往返（用 days_from_civil 反算）。
        assert!(ft > 133_000_000_000_000_000 && ft < 135_000_000_000_000_000);
        assert_eq!(out.entries[1].path, r"D:\a\we,ird.txt", "引号内逗号不拆列");
        assert_eq!(out.entries[1].size, 7, "大小两侧空格应被忽略");
    }

    #[test]
    fn csv_column_order_is_free_and_case_insensitive() {
        let csv = "SIZE,PATH,NAME\n10,d:\\m,x.log\n";
        let out = parse_list(csv);
        assert_eq!(out.report.accepted, 1);
        assert_eq!(out.entries[0].path, r"D:\m\x.log");
        assert_eq!(out.entries[0].size, 10);
    }

    #[test]
    fn csv_without_path_column_is_fatal() {
        let csv = "Filename,Size,Date Modified\na.txt,1,2026-11-16\n";
        let out = parse_list(csv);
        assert!(out.fatal.is_some(), "没有路径列无法定位文件，必须报错");
        assert!(!out.is_usable());
    }

    #[test]
    fn csv_directory_attribute() {
        let csv = "Name,Path,Attributes\nsub,D:\\a,16\n";
        let out = parse_list(csv);
        assert!(out.entries[0].is_dir, "16 = FILE_ATTRIBUTE_DIRECTORY");
    }

    #[test]
    fn build_indexes_synthesizes_dirs_and_paths_are_correct() {
        let out = parse_list("D:\\a\\b\\c.txt\nD:\\a\\d.txt\n");
        let vols = build_indexes(&out.entries, "list1");
        assert_eq!(vols.len(), 1);
        let v = &vols[0];
        assert_eq!(v.info.drive, Some('D'));
        assert_ne!(v.info.serial & IMPORT_SERIAL_FLAG, 0, "导入卷要带标记位");
        assert_eq!(find(v, r"D:\a\b\c.txt").as_deref(), Some(r"D:\a\b\c.txt"));
        assert_eq!(find(v, r"D:\a\d.txt").as_deref(), Some(r"D:\a\d.txt"));
        assert_eq!(
            find(v, r"D:\a").as_deref(),
            Some(r"D:\a"),
            "中间目录应被补齐"
        );
        assert_eq!(find(v, r"D:\a\b").as_deref(), Some(r"D:\a\b"));
        assert_eq!(v.synthesized_dirs, 2);
        assert_eq!(v.index.root_id(), Some(0));
    }

    #[test]
    fn build_indexes_splits_by_drive() {
        let out = parse_list("C:\\w\\a.txt\nD:\\x\\b.txt\nC:\\w\\c.txt\n");
        let vols = build_indexes(&out.entries, "mixed");
        assert_eq!(vols.len(), 2);
        assert_eq!(vols[0].info.drive, Some('C'));
        assert_eq!(vols[1].info.drive, Some('D'));
        assert_eq!(
            find(&vols[0], r"C:\w\a.txt").as_deref(),
            Some(r"C:\w\a.txt")
        );
        assert_eq!(
            find(&vols[1], r"D:\x\b.txt").as_deref(),
            Some(r"D:\x\b.txt")
        );
    }

    #[test]
    fn same_label_same_serial_and_differs_across_labels() {
        let out = parse_list("D:\\a.txt\n");
        let a = build_indexes(&out.entries, "one");
        let b = build_indexes(&out.entries, "one");
        let c = build_indexes(&out.entries, "two");
        assert_eq!(a[0].info.serial, b[0].info.serial);
        assert_ne!(a[0].info.serial, c[0].info.serial);
    }

    #[test]
    fn duplicate_paths_collapse_to_one_entry() {
        let out = parse_list("D:\\a\\x.txt\nD:\\a\\x.txt\n");
        let vols = build_indexes(&out.entries, "dup");
        assert_eq!(vols[0].entries, 3, "根 + a + x.txt，重复路径不重复建条目");
        assert_eq!(
            vols[0].index.duplicate_frns(),
            0,
            "重复 push 前已被 emitted 拦下"
        );
    }

    #[test]
    fn timestamp_parsing_roundtrip() {
        // 1970-01-01 00:00:00 = FILETIME 纪元差。
        assert_eq!(
            parse_timestamp("1970-01-01 00:00:00").unwrap(),
            116_444_736_000_000_000
        );
        assert_eq!(
            parse_timestamp("1970/1/1").unwrap(),
            116_444_736_000_000_000
        );
        assert!(parse_timestamp("2026-11-16T22:03:01").unwrap() > 116_444_736_000_000_000);
        assert!(parse_timestamp("").is_none());
        assert!(parse_timestamp("not a date").is_none());
        assert_eq!(
            parse_timestamp("134116992000000000").unwrap(),
            134_116_992_000_000_000,
            "纯数字 FILETIME 原样保留"
        );
    }

    #[test]
    fn empty_input_is_fatal() {
        assert!(parse_list("").fatal.is_some());
        assert!(parse_list("   \n\n  ").fatal.is_some());
    }

    #[test]
    fn relative_only_list_is_fatal() {
        let out = parse_list("a.txt\nb.txt\n");
        assert_eq!(out.report.skipped, 2);
        assert!(out.fatal.is_some(), "没有一条绝对路径时整体不可用");
        assert!(!out.skip_reasons.is_empty(), "要给出跳过原因");
    }
}
