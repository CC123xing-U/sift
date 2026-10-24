//! 路径现算（DESIGN §4.2、§2.2-3）：路径不存储，沿 parent 链向上、反向拼接。

use super::entry::{EntryId, VolumeIndex, NONE};

/// parent 链长度上限（链上条目数 ≤ 512；超出按损坏数据处理，DESIGN §4.2）。
const MAX_CHAIN: usize = 512;

impl VolumeIndex {
    /// 把条目 `id` 的完整路径写入 `buf`（先清空 `buf`）。
    ///
    /// - 根前缀取 [`VolumeInfo`] 盘符（如 `C:\`）；
    /// - 沿 parent 链向上、反向拼接（目录重命名 / 移动后子树路径自动正确）；
    /// - 复用调用方缓冲区，本函数自身**零分配**；
    /// - 卷根条目自身的路径即前缀（`C:\`）；
    /// - 返回 `false` = 失败（无效 id / 链超长 / 环）：此时 `buf` 为空，
    ///   调用方应计警告并跳过该条目（DESIGN §4.2）。
    pub fn write_path(&self, id: EntryId, buf: &mut Vec<u8>) -> bool {
        buf.clear();
        let n = self.entry_count();
        if id == NONE || id as usize >= n {
            return false;
        }
        // 1) 收集祖先链（含自身）；环 / 超深 → false（不返回半条路径）
        let mut chain = [0 as EntryId; MAX_CHAIN];
        let mut len = 0usize;
        let mut cur = id;
        loop {
            if len >= MAX_CHAIN {
                return false;
            }
            chain[len] = cur;
            len += 1;
            let p = self.parent.get(cur as usize).copied().unwrap_or(NONE);
            if p == NONE {
                break;
            }
            cur = p;
        }
        // 2) 组件 = 链上除“根条目自身”外的名字；先算总长再一次性写入
        let top_is_root = self.root_id() == Some(chain[len - 1]);
        let mut comp_count = 0usize;
        let mut total = self.path_prefix_bytes().len();
        for (i, &e) in chain[..len].iter().enumerate() {
            if top_is_root && i == len - 1 {
                continue;
            }
            total += self.name(e).len();
            comp_count += 1;
        }
        total += comp_count.saturating_sub(1); // 组件间的 '\'
                                               // 3) 反向写入
        buf.resize(total, 0);
        let mut pos = total;
        let mut written = 0usize;
        for (i, &e) in chain[..len].iter().enumerate() {
            if top_is_root && i == len - 1 {
                continue;
            }
            if written > 0 {
                pos -= 1;
                buf[pos] = b'\\';
            }
            let name = self.name(e);
            pos -= name.len();
            buf[pos..pos + name.len()].copy_from_slice(name);
            written += 1;
        }
        let plen = self.path_prefix_bytes().len();
        buf[..plen].copy_from_slice(self.path_prefix_bytes());
        debug_assert_eq!(pos, plen);
        true
    }

    /// 便捷包装：路径字符串（无效 id / 环 / 非法 UTF-8 返回 `None`）。
    pub fn path_string(&self, id: EntryId) -> Option<String> {
        let mut buf = Vec::new();
        if !self.write_path(id, &mut buf) {
            return None;
        }
        String::from_utf8(buf).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::super::entry::attrs;
    use super::super::entry::{EntryInput, IndexBuilder, VolumeInfo, NONE};
    use super::*;

    fn info() -> VolumeInfo {
        VolumeInfo {
            drive: Some('C'),
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

    fn file(frn: u64, parent: u64, name: &str) -> EntryInput {
        EntryInput {
            is_dir: false,
            attrs: 0,
            ..dir(frn, parent, name)
        }
    }

    fn tree() -> VolumeIndex {
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, ".")); // 0 root
        b.push(dir(100, 5, "Windows")); // 1
        b.push(file(101, 100, "readme.txt")); // 2
        b.push(dir(200, 5, "用户")); // 3
        b.push(file(201, 200, "文档.txt")); // 4
        b.finish()
    }

    #[test]
    fn basic_paths() {
        let ix = tree();
        let mut buf = Vec::new();
        assert!(ix.write_path(0, &mut buf));
        assert_eq!(buf, b"C:\\");
        assert!(ix.write_path(1, &mut buf));
        assert_eq!(buf, b"C:\\Windows");
        assert!(ix.write_path(2, &mut buf));
        assert_eq!(buf, b"C:\\Windows\\readme.txt");
        assert!(ix.write_path(4, &mut buf));
        assert_eq!(buf, "C:\\用户\\文档.txt".as_bytes());
        assert_eq!(ix.path_string(4).unwrap(), "C:\\用户\\文档.txt");
    }

    #[test]
    fn write_path_reuses_buffer() {
        let ix = tree();
        let mut buf = Vec::with_capacity(64);
        assert!(ix.write_path(2, &mut buf));
        let cap = buf.capacity();
        assert!(ix.write_path(0, &mut buf));
        assert_eq!(buf, b"C:\\");
        assert_eq!(buf.capacity(), cap); // 复用同一缓冲区，不重新分配
    }

    #[test]
    fn invalid_id_fails() {
        let ix = tree();
        let mut buf = Vec::new();
        assert!(!ix.write_path(NONE, &mut buf));
        assert!(!ix.write_path(999, &mut buf));
        assert!(buf.is_empty());
        assert_eq!(ix.path_string(NONE), None);
    }

    #[test]
    fn deep_chain_within_limit() {
        // 499 层目录 + root + 1 file = 501 层链（≤512）
        let mut b = IndexBuilder::new(info());
        b.push(dir(1, 0, ".")); // root
        for i in 2..=500u64 {
            b.push(dir(i, i - 1, &format!("d{i}")));
        }
        b.push(file(501, 500, "deep.txt"));
        let ix = b.finish();
        assert_eq!(ix.entry_count(), 501);
        let mut buf = Vec::new();
        assert!(ix.write_path(500, &mut buf));
        let s = ix.path_string(500).unwrap();
        assert!(s.starts_with("C:\\d2\\d3\\"));
        assert!(s.ends_with("\\deep.txt"));
        assert_eq!(s.matches('\\').count(), 500); // 前缀 1 个 + 499 个分隔
    }

    #[test]
    fn chain_over_limit_fails_without_hang() {
        // 600 层链 > 512 → 失败（防损坏数据死循环）
        let mut b = IndexBuilder::new(info());
        b.push(dir(1, 0, "."));
        for i in 2..=601u64 {
            b.push(dir(i, i - 1, "d"));
        }
        let ix = b.finish();
        let mut buf = Vec::new();
        assert!(!ix.write_path(600, &mut buf));
        assert!(buf.is_empty());
    }

    #[test]
    fn cycle_detected() {
        // a ↔ b 成环
        let mut b = IndexBuilder::new(info());
        b.push(dir(10, 20, "a"));
        b.push(dir(20, 10, "b"));
        let ix = b.finish();
        let mut buf = Vec::new();
        assert!(!ix.write_path(0, &mut buf));
        assert!(!ix.write_path(1, &mut buf));
        assert!(buf.is_empty());
    }

    #[test]
    fn self_cycle_detected() {
        let mut b = IndexBuilder::new(info());
        b.push(dir(10, 10, "self"));
        let ix = b.finish();
        let mut buf = Vec::new();
        assert!(!ix.write_path(0, &mut buf));
    }

    #[test]
    fn cycle_into_deep_tail() {
        // 良性链尾 + 环：仍能安全失败
        let mut b = IndexBuilder::new(info());
        b.push(dir(5, 0, ".")); // root
        b.push(dir(6, 5, "ok")); // 1
        b.push(dir(7, 8, "x")); // 2
        b.push(dir(8, 7, "y")); // 3  ← 2↔3 成环
        let ix = b.finish();
        assert!(ix.write_path(1, &mut Vec::new())); // 良性链正常
        assert!(!ix.write_path(2, &mut Vec::new()));
        assert!(!ix.write_path(3, &mut Vec::new()));
    }

    #[test]
    fn orphan_attached_to_root() {
        // 无根条目时 orphan 的 parent 为 NONE，路径 = 前缀 + 自身名字
        let mut b = IndexBuilder::new(info());
        b.push(file(999, 12345, "orphan.txt"));
        let ix = b.finish();
        assert_eq!(ix.path_string(0).as_deref(), Some("C:\\orphan.txt"));
    }

    #[test]
    fn mount_point_guid_prefix() {
        let vol = VolumeInfo {
            drive: None,
            guid_path: "\\\\?\\Volume{1234}".to_string(),
            ..Default::default()
        };
        let mut b = IndexBuilder::new(vol);
        b.push(dir(5, 0, "."));
        b.push(dir(6, 5, "data"));
        let ix = b.finish();
        assert_eq!(ix.path_string(1).unwrap(), "\\\\?\\Volume{1234}\\data");
    }
}
