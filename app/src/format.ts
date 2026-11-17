// 展示格式化（UI_SPEC §2.5）。

/**
 * 把 epoch 毫秒转成可用的 Date。
 *
 * P1-8：Tauri IPC 层已把 FILETIME 转成 epoch 毫秒（`crates/ff-engine/src/timeconv.rs`，
 * 桌面外壳 `app/src-tauri/src/main.rs` 调它），但这里仍保留防御——MockEngine、
 * 未来的 HTTP 服务或缓存的旧回包都可能送来 null / NaN / 越界值。过去真机因单位
 * 不符显示成 `NaN-NaN-NaN NaN:NaN`，这类无效值统一降级为空白，而不是把 NaN
 * 渲染到界面上。
 */
function toDate(ms: number | null): Date | null {
  if (ms == null || !Number.isFinite(ms)) return null;
  const d = new Date(ms);
  return Number.isNaN(d.getTime()) ? null : d;
}

export function formatSize(size: number | null): string {
  // 目录行由调用方传 null（UI_SPEC §4.2「目录显示空白」）；这里再兜一次底，
  // 避免异常数值被显示成 `NaN B` / `-1 B`。
  if (size == null || !Number.isFinite(size) || size < 0) return '';
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

export function formatTime(ms: number | null): string {
  const d = toDate(ms);
  if (!d) return '';
  const now = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  // UI_SPEC §2.5：同年 `M-d HH:mm`，跨年 `yyyy-M-d HH:mm`（月/日不补零）。
  const hm = `${d.getMonth() + 1}-${d.getDate()} ${p(d.getHours())}:${p(d.getMinutes())}`;
  if (d.getFullYear() === now.getFullYear()) return hm;
  return `${d.getFullYear()}-${hm}`;
}

/** 完整时间，用于修改时间列的 tooltip（UI_SPEC §2.5）。 */
export function formatTimeFull(ms: number | null): string {
  const d = toDate(ms);
  if (!d) return '';
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()} ${p(d.getHours())}:${p(
    d.getMinutes(),
  )}:${p(d.getSeconds())}`;
}

export function formatHHMM(ms: number | null): string {
  const d = toDate(ms);
  if (!d) return '—';
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
}
