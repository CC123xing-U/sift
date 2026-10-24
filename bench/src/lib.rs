//! sift-bench —— 合成数据生成器 + criterion 基准（DESIGN §14，Track D 所有）。
//!
//! # 生成器契约（DESIGN §14）
//! 固定随机种子、可调**条目数 / 目录深度分布 / CJK 名字占比 / 重名率**；
//! 同一种子与配置的输出**逐字节一致**（不依赖系统时间、线程调度或哈希遍历顺序），
//! 用于复现性能数字。确定性由单元测试验证（同一配置生成两次逐字段比对）。
//!
//! # 固定种子
//! `DEFAULT_SEED = 0x5349_4654`（ASCII `"SIFT"`）为基准默认种子，
//! 所有记录到 `docs/BENCH.md` 的数字均使用该种子（除非另有注明）。
//!
//! # 基准查询词约定（criterion 基准与集成测试共用）
//! 生成器保证下列词在名字中的大致频次，使基准可复现：
//! - `report`：高频（约 2% 条目，重名池 + 复合名首词）——子串基准词；
//! - `summary`：中频（复合名次词）——与 `report` 组成 AND 基准 `report summary`；
//! - `backup`：中高频（重名池 + 复合名首词）；
//! - 唯一名后缀 `_<frn>`：每条唯一条目可精确命中——稀有词基准词。
//!
//! # 与 ff-core 的关系
//! 产出 [`SynthEntry`]，字段与 `ff_core::EntryInput` / `ff_ntfs::RawEntry`
//! （DESIGN §4.1 / §5.1）一一对应。待 Track B 提交 `EntryInput` 后，
//! 转换点集中在 [`SynthEntry::into_entry_input`]（TODO(b-core)）。
//! 时间戳单位与 ff-ntfs 契约一致：Windows FILETIME（i64，100ns，1601 纪元）。

use std::fmt;

#[cfg(windows)]
pub mod meminfo;

/// 基准默认随机种子（ASCII "SIFT"）。所有基准数字以此种子复现。
pub const DEFAULT_SEED: u64 = 0x5349_4654;

/// 合成数据的"当前时刻"基准：2026-01-01T00:00:00Z 的 FILETIME
/// （1601 纪元 117 年 + 1970 纪元 56 年的 100ns 计数）。合成数据非真实时间。
const BASE_FILETIME: i64 = 134_117_092_000_000_000;

/// 普通英文词干（唯一条目的名字主体）。
const ASCII_STEMS: &[&str] = &[
    "report", "document", "image", "backup", "notes", "project", "data", "log", "config", "cache",
    "temp", "test", "demo", "sample", "draft", "release", "build", "source", "archive", "media",
    "photo", "video", "music", "download", "upload", "export", "import", "summary", "detail",
    "index",
];

/// 中文词干（唯一条目的名字主体；CJK 占比由此表与 [`SynthConfig::cjk_ratio`] 共同决定）。
const CJK_STEMS: &[&str] = &[
    "报告", "文档", "图片", "备份", "笔记", "项目", "数据", "日志", "配置", "缓存", "测试", "演示",
    "样例", "草稿", "发布", "构建", "源码", "归档", "媒体", "照片", "视频", "音乐", "下载", "上传",
    "导出", "导入", "汇总", "详情", "索引", "说明",
];

/// 重名池：完全同名的预制名（不同目录下同名，模拟真实卷的重名文件）。
/// `report` 在此池中出现一次，是子串基准词的主要来源之一。
const DUP_FILE_NAMES: &[&str] = &[
    "report.txt",
    "notes.md",
    "data.csv",
    "temp.tmp",
    "backup.zip",
    "index.html",
    "报告.docx",
    "数据.xlsx",
    "日志.log",
];

/// 重名目录池。
const DUP_DIR_NAMES: &[&str] = &["backup", "cache", "temp", "old", "备份", "新建文件夹"];

/// 复合名首词（高频，含基准词 `report` / `backup`）：与次词以空格相连，
/// 使 AND 基准（如 `report summary`）有稳定命中量。
const COMPOUND_A: &[&str] = &[
    "report", "backup", "notes", "data", "报告", "备份", "数据", "日志",
];

/// 复合名次词（中频，含基准词 `summary`）。
const COMPOUND_B: &[&str] = &[
    "summary",
    "detail",
    "final",
    "draft",
    "old",
    "new",
    "汇总",
    "详情",
    "最终版",
    "草稿",
];

/// 文件扩展名池（不含点）。
const EXTS: &[&str] = &[
    "txt", "log", "png", "jpg", "pdf", "docx", "xlsx", "rs", "md", "zip", "mp3", "mp4", "csv",
    "json", "html", "dll", "exe", "ini", "bak", "7z",
];

// Win32 文件属性位（FILE_ATTRIBUTE_*，见 ff-ntfs RawEntry.attrs 契约）。
const ATTR_ARCHIVE: u32 = 0x20;
const ATTR_HIDDEN: u32 = 0x02;
const ATTR_READONLY: u32 = 0x01;
const ATTR_SYSTEM: u32 = 0x04;
const ATTR_DIRECTORY: u32 = 0x10;

/// 单条合成条目。字段与 `ff_core::EntryInput` / `ff_ntfs::RawEntry` 对齐
/// （frn / parent_frn / name / is_dir / attrs / size / mtime / ctime / atime）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthEntry {
    /// 文件引用号（合成：从 2 起连续分配；1 保留给虚拟根）。
    pub frn: u64,
    /// 父目录 FRN；虚拟根的父为 0。
    pub parent_frn: u64,
    pub name: String,
    pub is_dir: bool,
    /// Win32 文件属性位。
    pub attrs: u32,
    pub size: u64,
    /// FILETIME（100ns，1601 纪元）。
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
}

/// 合成数据配置（DESIGN §14 的四项可调参数 + 目录占比）。
#[derive(Clone, Debug, PartialEq)]
pub struct SynthConfig {
    /// 总条目数（目录 + 文件）。
    pub count: usize,
    /// 随机种子（固定即可复现）。
    pub seed: u64,
    /// 目录树最大深度（根为深度 0）。
    pub max_depth: u32,
    /// 深度衰减因子：第 d 层目录数权重 = `depth_decay^(d-1)`（越小越偏浅层）。
    pub depth_decay: f64,
    /// 目录占比（其余为文件）。
    pub dir_ratio: f64,
    /// CJK 名字占比（唯一条目词干选中文表的概率）。
    pub cjk_ratio: f64,
    /// 重名率：名字从重名池完全重复选取的条目占比。
    pub dup_ratio: f64,
    /// 复合名占比（唯一名中 `"{a} {b}_{frn}"` 形式的比例）。
    pub compound_ratio: f64,
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self {
            count: 100_000,
            seed: DEFAULT_SEED,
            max_depth: 8,
            depth_decay: 0.55,
            dir_ratio: 0.15,
            cjk_ratio: 0.25,
            dup_ratio: 0.12,
            compound_ratio: 0.10,
        }
    }
}

impl SynthConfig {
    /// 以指定条目数构造，其余取默认值（默认种子 [`DEFAULT_SEED`]）。
    pub fn with_count(count: usize) -> Self {
        Self {
            count,
            ..Self::default()
        }
    }

    fn sanitized(&self) -> Self {
        let mut c = self.clone();
        c.max_depth = c.max_depth.max(1);
        c.depth_decay = c.depth_decay.clamp(0.05, 0.95);
        c.dir_ratio = c.dir_ratio.clamp(0.0, 0.9);
        c.cjk_ratio = c.cjk_ratio.clamp(0.0, 1.0);
        c.dup_ratio = c.dup_ratio.clamp(0.0, 1.0);
        c.compound_ratio = c.compound_ratio.clamp(0.0, 1.0);
        c
    }
}

impl fmt::Display for SynthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "count={} seed={:#x} max_depth={} depth_decay={:.2} dir_ratio={:.2} cjk_ratio={:.2} dup_ratio={:.2} compound_ratio={:.2}",
            self.count, self.seed, self.max_depth, self.depth_decay, self.dir_ratio, self.cjk_ratio,
            self.dup_ratio, self.compound_ratio,
        )
    }
}

/// splitmix64：小、快、确定性充分的 PRNG（无外部依赖，避免引入 `rand`）。
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `[0, n)` 均匀（Lemire 无偏差法，128 位乘法取高 64 位）。
    fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n > 0);
        (((self.next_u64() as u128) * (n as u128)) >> 64) as u64
    }

    /// `[0, 1)` 的 53 位定点小数。
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

/// 目录树中的虚拟节点（索引 0 为根，不产出条目）。
struct DirNode {
    frn: u64,
    parent_frn: u64,
}

/// 确定性合成数据迭代器：先目录（frn 连续）后文件，与真实 MFT 扫描的
/// "目录聚集"形态近似。逐条产出，200 万条模式下不驻留全量 `Vec`。
pub struct SynthIter {
    cfg: SynthConfig,
    rng: Rng,
    dirs: Vec<DirNode>,
    /// 目录阶段光标（0 为根，跳过）；目录阶段结束后进入文件阶段。
    dir_cursor: usize,
    files_left: u64,
    next_frn: u64,
    produced: u64,
}

impl SynthIter {
    /// 按配置创建迭代器（内部先构建目录树，再逐条产出）。
    pub fn new(cfg: &SynthConfig) -> Self {
        let cfg = cfg.sanitized();
        let mut rng = Rng::new(cfg.seed);
        let dirs = build_dirs(&cfg, &mut rng);
        let dir_count = dirs.len() - 1; // 除根
        let count = cfg.count as u64;
        let files_left = count.saturating_sub(dir_count as u64);
        Self {
            cfg,
            rng,
            dirs,
            dir_cursor: 1, // 跳过根
            files_left,
            // 目录已占用 frn 2..=dir_count+1，文件紧随其后。
            next_frn: 2 + dir_count as u64,
            produced: 0,
        }
    }

    fn next_dir(&mut self) -> Option<SynthEntry> {
        if self.dir_cursor >= self.dirs.len() {
            return None;
        }
        let (frn, parent_frn) = {
            let node = &self.dirs[self.dir_cursor];
            (node.frn, node.parent_frn)
        };
        let name = self.make_name(frn, true);
        let (c, m, a) = synth_times(&mut self.rng);
        self.dir_cursor += 1;
        Some(SynthEntry {
            frn,
            parent_frn,
            name,
            is_dir: true,
            attrs: dir_attrs(&mut self.rng),
            size: 0,
            mtime: m,
            ctime: c,
            atime: a,
        })
    }

    fn next_file(&mut self) -> Option<SynthEntry> {
        if self.files_left == 0 {
            return None;
        }
        self.files_left -= 1;
        let frn = self.next_frn;
        self.next_frn += 1;
        // 父目录：全部目录（含根）均匀随机。
        let parent_idx = self.rng.below(self.dirs.len() as u64) as usize;
        let parent_frn = self.dirs[parent_idx].frn;
        let name = self.make_name(frn, false);
        let (c, m, a) = synth_times(&mut self.rng);
        // 文件大小：立方偏置（偏向小文件），上限 4 GiB。
        let size = (self.rng.unit().powi(3) * 4.0e9) as u64;
        Some(SynthEntry {
            frn,
            parent_frn,
            name,
            is_dir: false,
            attrs: file_attrs(&mut self.rng),
            size,
            mtime: m,
            ctime: c,
            atime: a,
        })
    }

    /// 名字生成：重名池 / 唯一简单名 / 唯一复合名三条路径（见模块级基准词约定）。
    fn make_name(&mut self, frn: u64, is_dir: bool) -> String {
        let mut name = String::with_capacity(32);
        if self.rng.unit() < self.cfg.dup_ratio {
            // 重名：从预制池整名选取（完全重复）。
            if is_dir {
                let i = self.rng.below(DUP_DIR_NAMES.len() as u64) as usize;
                name.push_str(DUP_DIR_NAMES[i]);
            } else {
                let i = self.rng.below(DUP_FILE_NAMES.len() as u64) as usize;
                name.push_str(DUP_FILE_NAMES[i]);
            }
            return name;
        }
        let cjk = self.rng.unit() < self.cfg.cjk_ratio;
        let stems: &[&str] = if cjk { CJK_STEMS } else { ASCII_STEMS };
        if self.rng.unit() < self.cfg.compound_ratio {
            let a = stems[self.rng.below(stems.len() as u64) as usize];
            // 首词偏向高频表以支撑 AND 基准（report/backup 等）。
            let a = pick_compound_a(&mut self.rng, a);
            let b_idx = self.rng.below(COMPOUND_B.len() as u64) as usize;
            name.push_str(a);
            name.push(' ');
            name.push_str(COMPOUND_B[b_idx]);
        } else {
            let i = self.rng.below(stems.len() as u64) as usize;
            name.push_str(stems[i]);
        }
        // 唯一后缀：frn 十进制，保证全局唯一。
        name.push('_');
        name.push_str(&frn.to_string());
        if !is_dir {
            name.push('.');
            let i = self.rng.below(EXTS.len() as u64) as usize;
            name.push_str(EXTS[i]);
        }
        name
    }
}

/// 复合名首词：70% 走高频表 [`COMPOUND_A`]（保证基准词频次），30% 保留普通词干。
fn pick_compound_a<'a>(rng: &mut Rng, fallback: &'a str) -> &'a str {
    if rng.unit() < 0.70 {
        let i = rng.below(COMPOUND_A.len() as u64) as usize;
        COMPOUND_A[i]
    } else {
        fallback
    }
}

impl Iterator for SynthIter {
    type Item = SynthEntry;

    fn next(&mut self) -> Option<SynthEntry> {
        if self.produced >= self.cfg.count as u64 {
            return None;
        }
        self.produced += 1;
        if let Some(e) = self.next_dir() {
            return Some(e);
        }
        self.next_file()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.cfg.count as u64 - self.produced;
        (remaining as usize, Some(remaining as usize))
    }
}

impl ExactSizeIterator for SynthIter {}

/// 构建目录树：按层配额（深度权重 `decay^(d-1)`，整数分配，余数并入最深可用层），
/// 父目录在上一层中均匀随机选取。索引 0 为虚拟根（frn=1, parent=0, depth=0）。
fn build_dirs(cfg: &SynthConfig, rng: &mut Rng) -> Vec<DirNode> {
    let count = cfg.count;
    let mut dir_total = ((count as f64) * cfg.dir_ratio).round() as usize;
    dir_total = dir_total.clamp(1, count.max(1));
    let mut dirs = vec![DirNode {
        frn: 1,
        parent_frn: 0,
    }];
    let inner = dir_total - 1; // 除根的目录数
    if inner == 0 {
        return dirs;
    }
    // 每层配额（quota[0] 不用）。
    let max_depth = cfg.max_depth as usize;
    let usable_depth = max_depth.min(inner);
    let sum_w: f64 = (1..=usable_depth)
        .map(|d| cfg.depth_decay.powi(d as i32 - 1))
        .sum();
    let mut quota = vec![0u64; usable_depth + 1];
    let mut assigned = 0u64;
    for (d, q) in quota.iter_mut().enumerate().skip(1).take(usable_depth) {
        let w = cfg.depth_decay.powi(d as i32 - 1);
        *q = ((inner as f64) * w / sum_w).floor() as u64;
        assigned += *q;
    }
    // 余数并入最深层（usable_depth >= 1，必然存在）。
    quota[usable_depth] += inner as u64 - assigned;
    // 逐层生成：frn 连续分配（目录先于文件）。
    let mut next_frn = 2u64;
    let mut layer_start = 0usize; // depth 1 的父层 = dirs[0..1]（根）
    for q in quota.iter().skip(1).take(usable_depth).copied() {
        if q == 0 {
            continue;
        }
        let layer_end = dirs.len();
        // 父层 = dirs[layer_start..layer_end]（depth d-1）。
        for _ in 0..q {
            let pi = layer_start + rng.below((layer_end - layer_start) as u64) as usize;
            let parent_frn = dirs[pi].frn;
            dirs.push(DirNode {
                frn: next_frn,
                parent_frn,
            });
            next_frn += 1;
        }
        layer_start = layer_end;
    }
    dirs
}

/// 合成 (ctime, mtime, atime)：基准时刻前 0..3 年 / 0..1 年 / 0..30 天。
fn synth_times(rng: &mut Rng) -> (i64, i64, i64) {
    const THREE_YEARS: u64 = 3 * 365 * 24 * 3600;
    const ONE_YEAR: u64 = 365 * 24 * 3600;
    const THIRTY_DAYS: u64 = 30 * 24 * 3600;
    let ctime = BASE_FILETIME - (rng.below(THREE_YEARS) * 10_000_000) as i64;
    let mtime = ctime + (rng.below(ONE_YEAR) * 10_000_000) as i64;
    let atime = mtime - (rng.below(THIRTY_DAYS) * 10_000_000) as i64;
    (ctime, mtime, atime)
}

fn dir_attrs(rng: &mut Rng) -> u32 {
    let a = ATTR_DIRECTORY;
    match rng.below(100) {
        0..=1 => a | ATTR_HIDDEN,
        2..=3 => a | ATTR_SYSTEM,
        _ => a,
    }
}

fn file_attrs(rng: &mut Rng) -> u32 {
    let mut a = ATTR_ARCHIVE;
    if rng.below(100) < 5 {
        a |= ATTR_READONLY;
    }
    if rng.below(100) < 2 {
        a |= ATTR_HIDDEN;
    }
    if rng.below(1000) < 5 {
        a |= ATTR_SYSTEM;
    }
    a
}

/// 一次性生成全部条目（小规模测试用；200 万条请用迭代器流式喂给索引构建器）。
pub fn generate(cfg: &SynthConfig) -> Vec<SynthEntry> {
    SynthIter::new(cfg).collect()
}

/// 流式生成：边产出边消费（建索引基准 / memprobe 使用，避免全量驻留）。
pub fn synth_iter(cfg: &SynthConfig) -> SynthIter {
    SynthIter::new(cfg)
}

/// 用合成数据构建 `VolumeIndex`（criterion 基准 / memprobe / 集成测试共用）。
///
/// 先推入**虚拟根**（`frn=1, parent_frn=0, name="."`）：
/// - 与 ff-core 单测样本形态一致，`root_id` 存在、parent 链完整；
/// - 因此 `entry_count() == cfg.count + 1`（含根本身）。
///   不推根时，挂在根下的约 `1/dir_ratio` 个文件会 unresolved（路径仍正确，
///   但 `unresolved_parents()` 计数偏高，基准日志会失真）。
pub fn build_index(cfg: &SynthConfig) -> ff_core::VolumeIndex {
    use ff_core::attrs;
    use ff_core::{EntryInput, IndexBuilder, VolumeInfo};
    let vol = VolumeInfo {
        drive: Some('D'),
        ..Default::default()
    };
    let mut b = IndexBuilder::new(vol);
    b.push(EntryInput {
        frn: 1,
        parent_frn: 0,
        name: ".".to_string(),
        is_dir: true,
        attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
        size: 0,
        mtime: 0,
        ctime: 0,
        atime: 0,
    });
    for e in SynthIter::new(cfg) {
        b.push(EntryInput {
            frn: e.frn,
            parent_frn: e.parent_frn,
            name: e.name,
            is_dir: e.is_dir,
            attrs: e.attrs,
            size: e.size,
            mtime: e.mtime,
            ctime: e.ctime,
            atime: e.atime,
        });
    }
    b.finish()
}

/// 名字是否含 CJK 统一表意文字（用于测试断言，与 `HAS_CJK` 语义一致）。
pub fn name_has_cjk(name: &str) -> bool {
    name.chars().any(|c| {
        matches!(
            c,
            '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn deterministic_same_seed_byte_identical() {
        // 同种子同配置：两次生成逐字段一致（DESIGN §14 确定性要求）。
        let cfg = SynthConfig::with_count(2_000);
        let a = generate(&cfg);
        let b = generate(&cfg);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x, y, "同种子输出必须逐字段一致");
        }
    }

    #[test]
    fn deterministic_across_iterator_and_collect() {
        // 迭代器逐条产出与 collect 结果一致（size_hint 准确）。
        let cfg = SynthConfig::with_count(1_500);
        let it = synth_iter(&cfg);
        assert_eq!(it.len(), 1_500);
        let collected: Vec<SynthEntry> = it.collect();
        assert_eq!(collected, generate(&cfg));
    }

    #[test]
    fn different_seed_differs() {
        let a = SynthConfig::with_count(1_000);
        let mut b = a.clone();
        b.seed = a.seed ^ 0xDEAD_BEEF;
        assert_ne!(generate(&a), generate(&b));
    }

    #[test]
    fn entry_count_frn_unique_parents_resolvable() {
        let cfg = SynthConfig::with_count(5_000);
        let entries = generate(&cfg);
        assert_eq!(entries.len(), 5_000);
        let mut frns: HashSet<u64> = HashSet::new();
        let mut by_frn: Vec<(u64, u64)> = Vec::with_capacity(entries.len() + 1);
        by_frn.push((1, 0)); // 虚拟根
        for e in &entries {
            assert!(frns.insert(e.frn), "frn 必须唯一: {}", e.frn);
            by_frn.push((e.frn, e.parent_frn));
        }
        assert_eq!(frns.len(), entries.len());
        let frn_set: HashSet<u64> = by_frn.iter().map(|(f, _)| *f).collect();
        for (f, p) in &by_frn {
            if *f == 1 {
                continue; // 虚拟根本身的父为 0（约定）
            }
            assert!(frn_set.contains(p), "parent_frn 必须存在: {p}");
        }
        // 目录先于文件（frn 连续分配）。
        let first_file = entries.iter().position(|e| !e.is_dir);
        if let Some(i) = first_file {
            assert!(entries[..i].iter().all(|e| e.is_dir));
            assert!(entries[i..].iter().all(|e| !e.is_dir));
        }
    }

    #[test]
    fn depth_within_limit_and_chain_reaches_root() {
        let cfg = SynthConfig::with_count(3_000);
        let entries = generate(&cfg);
        let mut parent_of: HashMap<u64, u64> =
            entries.iter().map(|e| (e.frn, e.parent_frn)).collect();
        parent_of.insert(1, 0); // 根
        for e in &entries {
            let mut depth = 0u32;
            let mut cur = e.frn;
            while cur != 1 {
                let p = parent_of[&cur];
                cur = p;
                depth += 1;
                assert!(depth <= cfg.max_depth + 1, "超过最大深度");
                assert!(depth < 10_000, "疑似环");
            }
        }
    }

    #[test]
    fn dir_ratio_and_cjk_ratio_approx() {
        let cfg = SynthConfig::with_count(20_000);
        let entries = generate(&cfg);
        let dirs = entries.iter().filter(|e| e.is_dir).count();
        let dir_ratio = dirs as f64 / entries.len() as f64;
        assert!(
            (dir_ratio - cfg.dir_ratio).abs() < 0.03,
            "目录占比 {dir_ratio} 偏离 {:.2}",
            cfg.dir_ratio
        );
        let cjk = entries.iter().filter(|e| name_has_cjk(&e.name)).count();
        let cjk_ratio = cjk as f64 / entries.len() as f64;
        // 唯一条目按 cjk_ratio 选中文词干；重名池含 3/9 中文项，整体略高于 cjk_ratio。
        assert!(
            (cjk_ratio - cfg.cjk_ratio).abs() < 0.08,
            "CJK 占比 {cjk_ratio} 偏离 {:.2}",
            cfg.cjk_ratio
        );
    }

    #[test]
    fn dup_rate_approx_and_unique_names_unique() {
        let cfg = SynthConfig::with_count(20_000);
        let entries = generate(&cfg);
        let dup_pool: HashSet<&str> = DUP_FILE_NAMES
            .iter()
            .copied()
            .chain(DUP_DIR_NAMES.iter().copied())
            .collect();
        let dups = entries
            .iter()
            .filter(|e| dup_pool.contains(e.name.as_str()))
            .count();
        let dup_ratio = dups as f64 / entries.len() as f64;
        assert!(
            (dup_ratio - cfg.dup_ratio).abs() < 0.03,
            "重名率 {dup_ratio} 偏离 {:.2}",
            cfg.dup_ratio
        );
        // 非重名条目名字必须全局唯一（frn 后缀保证）。
        let mut seen: HashSet<&str> = HashSet::new();
        for e in entries
            .iter()
            .filter(|e| !dup_pool.contains(e.name.as_str()))
        {
            assert!(seen.insert(e.name.as_str()), "唯一名重复: {}", e.name);
        }
    }

    #[test]
    fn bench_words_present() {
        // 基准查询词约定：report/summary/backup 必须出现且频次进入预期量级。
        let cfg = SynthConfig::with_count(100_000);
        let entries = generate(&cfg);
        let count = |needle: &str| entries.iter().filter(|e| e.name.contains(needle)).count();
        let n = entries.len();
        let report = count("report");
        let summary = count("summary");
        let backup = count("backup");
        assert!(report > n / 50, "report 命中 {report} 过少（n={n}）");
        assert!(summary > n / 200, "summary 命中 {summary} 过少");
        assert!(backup > n / 100, "backup 命中 {backup} 过少");
    }

    #[test]
    fn attrs_and_sizes_sane() {
        let cfg = SynthConfig::with_count(3_000);
        let entries = generate(&cfg);
        for e in &entries {
            if e.is_dir {
                assert_eq!(e.attrs & ATTR_DIRECTORY, ATTR_DIRECTORY);
                assert_eq!(e.size, 0);
            } else {
                assert_eq!(e.attrs & ATTR_DIRECTORY, 0);
                assert!(e.size <= 4_000_000_000);
            }
            assert!(e.ctime <= BASE_FILETIME);
            assert!(e.mtime >= e.ctime);
            assert!(e.atime <= e.mtime);
            assert!(!e.name.is_empty());
        }
    }

    #[test]
    fn zero_and_tiny_counts_do_not_panic() {
        for n in [0usize, 1, 2, 3] {
            let cfg = SynthConfig::with_count(n);
            let entries = generate(&cfg);
            assert_eq!(entries.len(), n);
        }
        // dir_ratio 经 sanitize 钳制上限 0.9：该配置下目录占比应接近 0.9。
        let mut cfg = SynthConfig::with_count(50);
        cfg.dir_ratio = 0.9;
        let entries = generate(&cfg);
        let dir_share = entries.iter().filter(|e| e.is_dir).count() as f64 / entries.len() as f64;
        assert!(dir_share >= 0.85, "目录占比 {dir_share} 应接近 0.9");
    }

    #[test]
    fn max_depth_one_is_flat() {
        let mut cfg = SynthConfig::with_count(200);
        cfg.max_depth = 1;
        let entries = generate(&cfg);
        // max_depth=1：所有目录的父必须是根（目录树只有一层）；文件挂在根或一层目录下。
        for e in entries.iter().filter(|e| e.is_dir) {
            assert_eq!(e.parent_frn, 1, "max_depth=1 时目录父必须是根");
        }
    }
}
