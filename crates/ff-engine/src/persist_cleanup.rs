//! 索引落盘目录的清理（桌面 InProcess 与服务模式共用）。
//!
//! `.ffix` 按卷序列号命名，卷消失 / 换盘 / 重装后旧文件就成了永不使用的孤儿；
//! `save_with_volume` 采用「写 `<serial>.tmp` → rename」的原子落盘，崩溃恰在两者
//! 之间会残留半截 `.tmp`。两者都需要在启动时收敛，否则索引目录随使用时间堆积。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// 清理索引目录中不再有效的文件，返回被删文件数。
///
/// - 删除 `serial` 不在 `valid_serials` 中的 `*.ffix`（幽灵卷 / 旧盘索引）。
/// - 删除所有 `*.tmp`（原子落盘的崩溃残留；启动时调用，此刻无并发写入）。
/// - **空集守卫**：`valid_serials` 为空时什么都不删。卷枚举「成功但为空」或失败时
///   若照常清理，会把全部合法索引误判为幽灵卷删光，毁掉秒级冷启动。
///
/// 无法解析为 u32 的 `*.ffix`（如手工放置、其它版本格式）一律保留，避免误删。
pub fn prune_index_dir(dir: &Path, valid_serials: &HashSet<u32>) -> usize {
    if valid_serials.is_empty() {
        return 0;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        match path.extension().and_then(|x| x.to_str()) {
            Some("ffix") => {
                let Some(serial) = path
                    .file_stem()
                    .and_then(|x| x.to_str())
                    .and_then(|x| x.parse::<u32>().ok())
                else {
                    continue;
                };
                if !valid_serials.contains(&serial) && fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
            Some("tmp") if fs::remove_file(&path).is_ok() => {
                removed += 1;
            }
            Some("tmp") => {}
            _ => {}
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"x").unwrap();
    }

    #[test]
    fn removes_stale_ffix_and_tmp_keeps_valid() {
        let dir = std::env::temp_dir().join(format!("sift_prune_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        touch(&dir, "1.ffix");
        touch(&dir, "2.ffix");
        touch(&dir, "3.tmp");
        touch(&dir, "notes.txt");

        let valid: HashSet<u32> = [1u32].into_iter().collect();
        let removed = prune_index_dir(&dir, &valid);
        assert_eq!(removed, 2, "stale 2.ffix + 3.tmp removed");
        assert!(dir.join("1.ffix").exists());
        assert!(!dir.join("2.ffix").exists());
        assert!(!dir.join("3.tmp").exists());
        assert!(dir.join("notes.txt").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_valid_set_deletes_nothing() {
        let dir = std::env::temp_dir().join(format!("sift_prune_empty_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        touch(&dir, "1.ffix");
        touch(&dir, "2.ffix");
        touch(&dir, "9.tmp");

        let removed = prune_index_dir(&dir, &HashSet::new());
        assert_eq!(removed, 0);
        assert!(dir.join("1.ffix").exists());
        assert!(dir.join("2.ffix").exists());
        assert!(dir.join("9.tmp").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unparsable_ffix_is_preserved() {
        let dir = std::env::temp_dir().join(format!("sift_prune_weird_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        touch(&dir, "abc.ffix");
        touch(&dir, "1.ffix");

        let valid: HashSet<u32> = [1u32].into_iter().collect();
        prune_index_dir(&dir, &valid);
        assert!(dir.join("abc.ffix").exists(), "unknown file preserved");
        assert!(dir.join("1.ffix").exists());
        fs::remove_dir_all(&dir).ok();
    }
}
