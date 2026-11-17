//! 索引持久化（DESIGN §6，M2-①）。
//!
//! 零依赖、纯逻辑、跨平台可单测（不引入 `serde` / `zstd` / `crc` 等外部 crate，
//! CRC32 自实现；压缩作为后续优化，见 `docs/DECISIONS.md` #58）。
//!
//! 磁盘格式（全部小端）：
//! ```text
//!   [0..4]    magic = b"FFIX"
//!   [4..8]    format_version: u32  (= 1；任何格式变更都递增，不兼容即重建)
//!   [8..?]    volume 块：
//!                drive: u32            (0 = None；否则 char 的 u32 码点)
//!                guid_path: u32 长度 + UTF-8 字节
//!                serial: u32
//!                journal_id: u64
//!                next_usn: i64
//!   [?..]     列块序列（固定顺序，元素数 = `name_off.len()` = 条目数）：
//!                0 names      : u64 字节数 + 原始字节（arena，含 0x00 分隔）
//!                1 names_lower: u64 字节数 + 原始字节
//!                2 name_off   : u64 字节数 + 每元素 u32
//!                3 parent     : u64 字节数 + 每元素 u32
//!                4 flags      : u64 字节数 + 每元素 u16
//!                5 frn        : u64 字节数 + 每元素 u64
//!                6 size       : u64 字节数 + 每元素 u64
//!                7 mtime      : u64 字节数 + 每元素 i64
//!                8 ctime      : u64 字节数 + 每元素 i64
//!                9 atime      : u64 字节数 + 每元素 i64
//!   [末尾]    crc32: u32   （覆盖从 magic 到最后一列末尾，不含本字段；IEEE 802.3）
//! ```
//!
//! 语义（DESIGN §6）：`load` 对任何损坏 / 版本不符 / 解析越界**一律返回 `None`**
//! （调用方据此视为"无索引"并触发全量重建），**绝不 panic**。

use std::fs;
use std::io;
use std::path::Path;

use crate::entry::{VolumeIndex, VolumeInfo};

/// 文件魔数。
pub const MAGIC: [u8; 4] = *b"FFIX";
/// 当前格式版本。任何布局变更都必须递增；旧版本 `load` 直接返回 `None`。
pub const FORMAT_VERSION: u32 = 1;

// ───────────────────────── CRC32（IEEE 802.3，表驱动，自实现） ─────────────────────────

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0usize;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC_TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

// ───────────────────────── 二进制写入器 ─────────────────────────

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    /// 不透明字节块（名字 arena 等）：先写 u64 长度再写原字节。
    fn blob(&mut self, data: &[u8]) {
        self.u64(data.len() as u64);
        self.buf.extend_from_slice(data);
    }
    fn str(&mut self, v: &str) {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v.as_bytes());
    }
    /// 定长元素序列：先写 u64 总字节数（元素数 × `N`），再逐个写 `f(e)`（`[u8; N]`）。
    fn slice<T, const N: usize, F>(&mut self, data: &[T], mut f: F)
    where
        F: FnMut(&T) -> [u8; N],
    {
        self.u64((data.len() * N) as u64);
        for e in data {
            self.buf.extend_from_slice(&f(e));
        }
    }
}

// ───────────────────────── 二进制读取器（越界即 None） ─────────────────────────

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        if end > self.data.len() {
            return None;
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    fn i64(&mut self) -> Option<i64> {
        self.u64().map(|v| v as i64)
    }
    fn str(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let b = self.take(len)?;
        String::from_utf8(b.to_vec()).ok()
    }
    /// 读取不透明字节块（对应 [`Writer::blob`]）。
    fn blob(&mut self) -> Option<Vec<u8>> {
        let len = self.u64()? as usize;
        let b = self.take(len)?;
        Some(b.to_vec())
    }
    /// 读取一列：u64 总字节数 → 校验可被 `elem` 整除 → 按 `elem` 切分并 `f` 映射。
    fn slice<T, F>(&mut self, elem: usize, mut f: F) -> Option<Vec<T>>
    where
        F: FnMut(&[u8]) -> T,
    {
        let byte_len = self.u64()? as usize;
        if !byte_len.is_multiple_of(elem) {
            return None;
        }
        let raw = self.take(byte_len)?;
        let mut out = Vec::with_capacity(byte_len / elem);
        let mut i = 0;
        while i < byte_len {
            out.push(f(&raw[i..i + elem]));
            i += elem;
        }
        Some(out)
    }
}

// ───────────────────────── volume 序列化 ─────────────────────────

fn write_volume(w: &mut Writer, v: &VolumeInfo) {
    w.u32(v.drive.map(|c| c as u32).unwrap_or(0));
    w.str(&v.guid_path);
    w.u32(v.serial);
    w.u64(v.journal_id);
    w.i64(v.next_usn);
}

fn read_volume(r: &mut Reader) -> Option<VolumeInfo> {
    let drive_code = r.u32()?;
    let drive = if drive_code == 0 {
        None
    } else {
        char::from_u32(drive_code)
    };
    let guid_path = r.str()?;
    let serial = r.u32()?;
    let journal_id = r.u64()?;
    let next_usn = r.i64()?;
    Some(VolumeInfo {
        drive,
        guid_path,
        serial,
        journal_id,
        next_usn,
    })
}

// ───────────────────────── 落盘 / 载入 ─────────────────────────

impl VolumeIndex {
    /// 原子保存索引到 `path`（DESIGN §6：写 `*.tmp` → `rename` 覆盖，崩溃不留半个文件）。
    ///
    /// 由调用方传入完整路径（开发期 `%LOCALAPPDATA%\Sift\index-<serial>.bin`，
    /// 服务期 `%ProgramData%\Sift\...`）；本函数不触碰环境变量，保持纯逻辑可单测。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.save_with_volume(path, &self.volume)
    }

    /// 用外部卷元数据落盘，条目列仍取自本索引。
    ///
    /// 引擎运行时的 `VolumeInfo.next_usn` 会随每次 USN 增量推进，而
    /// `index.volume` 只是建索引那一刻的快照。若落盘坚持用 `self.volume`，
    /// 重启后就会从旧的续读起点（全扫时甚至为 0）回放，轻则重复处理大批变更，
    /// 重则被判 `JournalUnavailable` 触发全量重建——磁盘缓存就此失效。
    /// 调用方应传入权威的运行时 `VolumeInfo`。
    pub fn save_with_volume(&self, path: &Path, volume: &VolumeInfo) -> io::Result<()> {
        let mut w = Writer::new();
        w.bytes(&MAGIC);
        w.u32(FORMAT_VERSION);
        write_volume(&mut w, volume);
        w.blob(self.names.as_slice());
        w.blob(self.names_lower.as_slice());
        w.slice(self.name_off.as_slice(), |e| e.to_le_bytes());
        w.slice(self.parent.as_slice(), |e| e.to_le_bytes());
        w.slice(self.flags.as_slice(), |e| e.to_le_bytes());
        w.slice(self.frn.as_slice(), |e| e.to_le_bytes());
        w.slice(self.size.as_slice(), |e| e.to_le_bytes());
        w.slice(self.mtime.as_slice(), |e| e.to_le_bytes());
        w.slice(self.ctime.as_slice(), |e| e.to_le_bytes());
        w.slice(self.atime.as_slice(), |e| e.to_le_bytes());
        let crc = crc32(&w.buf);
        w.u32(crc);

        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &w.buf)?;
        // Windows 上 std 的 rename 会替换已存在的目标（DESIGN §6）。
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 从 `path` 载入索引。任何损坏 / 版本不符 / 解析错误均返回 `None`
    /// （调用方据此视为无索引并触发全量重建，**不 panic**）。文件不存在亦返回 `None`。
    pub fn load(path: &Path) -> Option<VolumeIndex> {
        let data = fs::read(path).ok()?;
        let mut r = Reader::new(&data);

        let magic = r.take(4)?;
        if magic != MAGIC {
            return None;
        }
        let version = r.u32()?;
        if version != FORMAT_VERSION {
            return None;
        }
        let volume = read_volume(&mut r)?;

        // 列顺序与 `save` 严格对应：名字 arena 为不透明字节块；`name_off` 长度即条目数 `n`。
        let names = r.blob()?;
        let names_lower = r.blob()?;
        if names_lower.len() != names.len() {
            return None;
        }
        let name_off = r.slice(4, u32_from)?;
        let n = name_off.len();
        let parent = r.slice(4, u32_from)?;
        if parent.len() != n {
            return None;
        }
        let flags = r.slice(2, u16_from)?;
        if flags.len() != n {
            return None;
        }
        let frn = r.slice(8, u64_from)?;
        if frn.len() != n {
            return None;
        }
        let size = r.slice(8, u64_from)?;
        if size.len() != n {
            return None;
        }
        let mtime = r.slice(8, i64_from)?;
        if mtime.len() != n {
            return None;
        }
        let ctime = r.slice(8, i64_from)?;
        if ctime.len() != n {
            return None;
        }
        let atime = r.slice(8, i64_from)?;
        if atime.len() != n {
            return None;
        }

        // 末尾 CRC32 覆盖"魔法到最后一列"，不含这 4 字节。
        let expected = r.u32()?;
        if r.pos != data.len() || crc32(&data[..data.len() - 4]) != expected {
            return None;
        }

        // name_off 是名字 arena 的切片偏移，name_in/highlight 会直接拿它索引 names。
        // CRC 只保证字节未被意外改动（算法公开可重算），不保证语义合法：一个 CRC 正确
        // 但 name_off 越界/非单调的索引会在首次取名字时 panic。这里把不变量在数据入口
        // 一次性校验：首项 0、单调不减、每项的界不超过 arena 长度。
        // 非法则返回 None → 调用方触发全量重建，符合"load 绝不 panic"契约。
        if !name_off_valid(&name_off, names.len()) {
            return None;
        }

        Some(VolumeIndex::from_raw(
            volume,
            names,
            names_lower,
            name_off,
            parent,
            flags,
            frn,
            size,
            mtime,
            ctime,
            atime,
        ))
    }
}

/// 校验名字偏移表的语义不变量：条目非空时首项必须为 0、序列单调不减、
/// 每项都落在名字 arena 范围内。防止 CRC 合法但内容畸形的索引在取名字时越界 panic。
fn name_off_valid(off: &[u32], arena_len: usize) -> bool {
    if off.is_empty() {
        return true;
    }
    let limit = arena_len as u64;
    if off[0] != 0 {
        return false;
    }
    off.windows(2).all(|w| w[0] <= w[1])
        && off.iter().all(|&o| (o as u64) <= limit)
}

fn u16_from(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn u32_from(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn u64_from(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}
fn i64_from(b: &[u8]) -> i64 {
    u64_from(b) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{attrs, EntryInput, IndexBuilder};
    use std::env;

    fn info() -> VolumeInfo {
        VolumeInfo {
            drive: Some('C'),
            serial: 0xDEAD_BEEF,
            journal_id: 0x1122_3344_5566_7788,
            next_usn: 0x7FFF_FFFF_FFFF,
            ..Default::default()
        }
    }
    fn dir(frn: u64, parent: u64, name: &str) -> EntryInput {
        EntryInput {
            frn,
            parent_frn: parent,
            name: name.to_string(),
            is_dir: true,
            attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
            size: 0,
            mtime: 1,
            ctime: 1,
            atime: 1,
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
    fn sample() -> VolumeIndex {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, "."));
        b.push(dir(100, 5, "Windows"));
        b.push(file(101, 100, "readme.txt", 1024));
        b.push(dir(200, 5, "用户"));
        b.push(file(201, 200, "文档.txt", 2048));
        b.finish()
    }

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        env::temp_dir().join(format!("sift_persist_{}_{}.bin", std::process::id(), tag))
    }
    fn cleanup(p: &Path) {
        let _ = fs::remove_file(p);
        let _ = fs::remove_file(p.with_extension("tmp"));
    }

    #[test]
    fn round_trip_preserves_structure() {
        let ix = sample();
        let p = tmp_path("rt");
        ix.save(&p).unwrap();
        let loaded = VolumeIndex::load(&p).expect("load must succeed");
        assert_eq!(loaded.entry_count(), 5);
        assert_eq!(loaded.name_str(4), Some("文档.txt"));
        assert_eq!(loaded.path_string(4).as_deref(), Some("C:\\用户\\文档.txt"));
        assert_eq!(loaded.flags, ix.flags);
        assert_eq!(loaded.names, ix.names);
        assert_eq!(loaded.names_lower, ix.names_lower);
        assert_eq!(loaded.frn_to_id(201), Some(4));
        assert_eq!(loaded.volume.serial, 0xDEAD_BEEF);
        assert_eq!(loaded.volume.journal_id, 0x1122_3344_5566_7788);
        assert_eq!(loaded.volume.next_usn, 0x7FFF_FFFF_FFFF);
        assert_eq!(loaded.volume.drive, Some('C'));
        cleanup(&p);
    }

    #[test]
    fn empty_index_round_trips() {
        let ix = IndexBuilder::new(info()).finish();
        let p = tmp_path("empty");
        ix.save(&p).unwrap();
        let loaded = VolumeIndex::load(&p).expect("empty load");
        assert_eq!(loaded.entry_count(), 0);
        assert_eq!(loaded.name(0), &[] as &[u8]);
        cleanup(&p);
    }

    #[test]
    fn corruption_returns_none() {
        let ix = sample();
        let p = tmp_path("corrupt");
        ix.save(&p).unwrap();
        let mut bytes = fs::read(&p).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF; // 翻转中部字节 → CRC 必失配
        fs::write(&p, &bytes).unwrap();
        assert!(VolumeIndex::load(&p).is_none(), "损坏文件必须返回 None");
        cleanup(&p);
    }

    #[test]
    fn version_mismatch_returns_none() {
        let mut bad = Vec::new();
        bad.extend_from_slice(&MAGIC);
        bad.extend_from_slice(&99u32.to_le_bytes());
        bad.extend_from_slice(&[0u8; 32]);
        let p = tmp_path("ver");
        fs::write(&p, &bad).unwrap();
        assert!(VolumeIndex::load(&p).is_none());
        cleanup(&p);
    }

    #[test]
    fn missing_file_returns_none() {
        assert!(VolumeIndex::load(Path::new("C:\\nonexistent\\sift\\x.bin")).is_none());
    }

    #[test]
    fn guid_path_volume_round_trips() {
        let mut vol = info();
        vol.drive = None;
        vol.guid_path = "\\\\?\\Volume{abc-def}".to_string();
        let mut b = IndexBuilder::new(vol);
        b.push(dir(5, 0, "."));
        b.push(file(101, 5, "data.bin", 7));
        let ix = b.finish();
        let p = tmp_path("guid");
        ix.save(&p).unwrap();
        let loaded = VolumeIndex::load(&p).expect("guid load");
        assert_eq!(loaded.volume.drive, None);
        assert_eq!(loaded.volume.guid_path, "\\\\?\\Volume{abc-def}");
        assert_eq!(
            loaded.path_string(1).as_deref(),
            Some("\\\\?\\Volume{abc-def}\\data.bin")
        );
        cleanup(&p);
    }

    #[test]
    fn truncation_returns_none() {
        let ix = sample();
        let p = tmp_path("trunc");
        ix.save(&p).unwrap();
        let bytes = fs::read(&p).unwrap();
        // 砍掉末尾 8 字节（含 CRC）→ 长度 / CRC 校验失败
        fs::write(&p, &bytes[..bytes.len() - 8]).unwrap();
        assert!(VolumeIndex::load(&p).is_none());
        cleanup(&p);
    }

    #[test]
    fn name_off_valid_rejects_malformed_offsets() {
        // 合法：首项 0、单调、不越界
        assert!(name_off_valid(&[0, 3, 7], 8));
        assert!(name_off_valid(&[0], 0));
        assert!(name_off_valid(&[], 100));
        // 首项非 0
        assert!(!name_off_valid(&[1, 3], 8));
        // 非单调
        assert!(!name_off_valid(&[0, 5, 3], 8));
        // 越界（含 u32::MAX）
        assert!(!name_off_valid(&[0, u32::MAX], 8));
        assert!(!name_off_valid(&[0, 9], 8));
    }
}
