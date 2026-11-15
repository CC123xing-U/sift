//! IPC 边界的时间单位转换（DESIGN §4.3 / §12、`api.rs` 契约注释）。
//!
//! 引擎内部与持久化索引一律使用 **Windows FILETIME**（100 ns 刻度、1601-01-01 纪元，
//! `i64`）：`dm:` 查询求值、`SortKey::Modified` 排序、索引落盘都依赖它，不能改。
//! 前端 `types.ts` 的契约却是 **JS epoch 毫秒**（`new Date(ms)`）。
//!
//! **必须在 Rust 侧转换**：典型 FILETIME ≈ 1.34e17，已超过 IEEE-754 的 53 位安全整数
//! （2^53 ≈ 9.01e15）。若原样经 JSON 送到 JS，数值在反序列化时就已经丢精度，
//! 前端再减常数也补不回来；`new Date(1.34e17)` 更是直接 `Invalid Date`。
//! 反过来，转换后的 epoch 毫秒最大约 9.22e14（`i64::MAX` ticks / 1e4），
//! 始终小于 2^53，JSON number 可无损表示。
//!
//! 本模块只做「单位换算 + 无效值兜底」，不碰引擎契约：`range` / `status`
//! 两个 Tauri 命令在序列化前调用，字段名与类型都不变。

use ff_engine::api::{EngineStatus, ResultRow};

/// 1601-01-01（FILETIME 纪元）到 1970-01-01（Unix 纪元）的毫秒数。
const UNIX_TO_FILETIME_MS: i128 = 11_644_473_600_000;
/// 每个毫秒包含的 100 ns 刻度数。
const TICKS_PER_MS: i128 = 10_000;

/// 可显示日期的下界：1900-01-01T00:00:00Z = -2_208_988_800_000 ms。
///
/// FILETIME 为 0 或极小值（1601 年前后）在 NTFS 上代表「时间未设置 / 元数据损坏」，
/// 不是真实文件日期。直接换算会显示 `1601-1-1 00:00` 这类垃圾值，故视为无效。
/// 1900 之前合法文件极罕见；1900–1970 的真实文件仍可正常显示。
const MIN_VALID_EPOCH_MS: i128 = -2_208_988_800_000;
/// 可显示日期的上界：9999-12-31T23:59:59.999Z = 253_402_300_799_999 ms。
/// 同时覆盖 `i64::MAX` ticks 换算出的荒谬未来日期。
const MAX_VALID_EPOCH_MS: i128 = 253_402_300_799_999;

/// FILETIME → JS epoch 毫秒。
///
/// - `None`：未补齐（`stale_meta`）、0 / 负值、换算后落在不可显示日期区间；
/// - `Some(ms)`：`new Date(ms)` 必然有效，且 `ms` 在 JS 安全整数范围内。
/// - 亚毫秒精度按整除丢弃（UI 只显示到秒，tooltip 亦然）。
pub fn filetime_to_epoch_ms(ft: i64) -> Option<i64> {
    if ft <= 0 {
        return None;
    }
    // 用 i128 做中间运算：即使将来入参换成 u64/i128 也不会在减法前溢出。
    let ms = i128::from(ft) / TICKS_PER_MS - UNIX_TO_FILETIME_MS;
    if !(MIN_VALID_EPOCH_MS..=MAX_VALID_EPOCH_MS).contains(&ms) {
        return None;
    }
    Some(ms as i64)
}

/// 把一批结果行的 `mtime` 从 FILETIME 转为 epoch 毫秒（就地）。
pub fn convert_rows(rows: &mut [ResultRow]) {
    for row in rows {
        row.mtime = row.mtime.and_then(filetime_to_epoch_ms);
    }
}

/// 把 `EngineStatus.last_saved` 从 FILETIME 转为 epoch 毫秒（就地）。
pub fn convert_status(status: &mut EngineStatus) {
    status.last_saved = status.last_saved.and_then(filetime_to_epoch_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    /// Unix 纪元对应的 FILETIME。
    const UNIX_EPOCH_FT: i64 = 116_444_736_000_000_000;
    /// 2026-01-01T00:00:00Z。
    const Y2026_MS: i64 = 1_767_225_600_000;
    const Y2026_FT: i64 = 134_116_992_000_000_000;

    fn row(mtime: Option<i64>) -> ResultRow {
        ResultRow {
            name: "x.txt".into(),
            dir: "C:\\".into(),
            is_dir: false,
            size: Some(1),
            mtime,
            name_matches: Vec::<Range<u32>>::new(),
        }
    }

    fn status(last_saved: Option<i64>) -> EngineStatus {
        EngineStatus {
            volumes: Vec::new(),
            total_entries: 0,
            last_saved,
            searching: false,
            save_error: None,
        }
    }

    #[test]
    fn epoch_and_known_date_roundtrip() {
        assert_eq!(filetime_to_epoch_ms(UNIX_EPOCH_FT), Some(0));
        assert_eq!(filetime_to_epoch_ms(Y2026_FT), Some(Y2026_MS));
        // 逆向：epoch ms 加回纪元差再乘刻度数，应还原 FILETIME。
        let back = (i128::from(Y2026_MS) + UNIX_TO_FILETIME_MS) * TICKS_PER_MS;
        assert_eq!(back, i128::from(Y2026_FT));
    }

    #[test]
    fn sub_millisecond_ticks_are_truncated() {
        // 1..9999 ticks（0.1..0.9999 ms）都归到同一毫秒，不得产生漂移。
        for d in [1_i64, 9_999] {
            assert_eq!(filetime_to_epoch_ms(UNIX_EPOCH_FT + d), Some(0));
        }
        assert_eq!(filetime_to_epoch_ms(UNIX_EPOCH_FT + 10_000), Some(1));
    }

    #[test]
    fn zero_negative_and_damaged_filetimes_are_dropped() {
        assert_eq!(filetime_to_epoch_ms(0), None);
        assert_eq!(filetime_to_epoch_ms(-1), None);
        assert_eq!(filetime_to_epoch_ms(i64::MIN), None);
        // 1601-01-01 + 1s：换算成功但早于 1900，属于「时间未设置」。
        assert_eq!(filetime_to_epoch_ms(10_000_000), None);
        // i64::MAX：换算成公元 ~31000 年，超出可显示上界。
        assert_eq!(filetime_to_epoch_ms(i64::MAX), None);
    }

    #[test]
    fn pre_epoch_dates_are_kept() {
        // 1960-01-01T00:00:00Z = -315_619_200_000 ms，真实老文件应正常显示。
        let ms = -315_619_200_000_i64;
        let ft = (i128::from(ms) + UNIX_TO_FILETIME_MS) * TICKS_PER_MS;
        assert_eq!(filetime_to_epoch_ms(ft as i64), Some(ms));
    }

    #[test]
    fn result_is_always_js_safe_integer() {
        // 2^53 - 1；任何合法返回值都必须 ≤ 它，否则 JSON 反序列化会丢精度。
        const JS_MAX_SAFE: i64 = 9_007_199_254_740_991;
        for ft in [1_i64, UNIX_EPOCH_FT, Y2026_FT, i64::MAX] {
            if let Some(ms) = filetime_to_epoch_ms(ft) {
                assert!(ms.abs() <= JS_MAX_SAFE, "ft={ft} -> ms={ms}");
            }
        }
    }

    #[test]
    fn convert_rows_preserves_none_and_other_fields() {
        let mut rows = vec![row(None), row(Some(Y2026_FT)), row(Some(0))];
        convert_rows(&mut rows);
        assert_eq!(rows[0].mtime, None); // stale_meta 的 None 不得被写成 0
        assert_eq!(rows[1].mtime, Some(Y2026_MS));
        assert_eq!(rows[2].mtime, None);
        assert_eq!(rows[1].name, "x.txt");
        assert_eq!(rows[1].size, Some(1));
    }

    #[test]
    fn convert_status_handles_none_and_valid() {
        let mut a = status(None);
        convert_status(&mut a);
        assert_eq!(a.last_saved, None);

        let mut b = status(Some(Y2026_FT));
        convert_status(&mut b);
        assert_eq!(b.last_saved, Some(Y2026_MS));
        assert_eq!(b.total_entries, 0);
    }
}
