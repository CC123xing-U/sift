// 展示格式化（UI_SPEC §2.5）。
export function formatSize(size: number | null): string {
  if (size == null) return '';
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

export function formatTime(ms: number | null): string {
  if (ms == null) return '';
  const d = new Date(ms);
  const now = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  // UI_SPEC §2.5：同年 `M-d HH:mm`，跨年 `yyyy-M-d HH:mm`（月/日不补零）。
  const hm = `${d.getMonth() + 1}-${d.getDate()} ${p(d.getHours())}:${p(d.getMinutes())}`;
  if (d.getFullYear() === now.getFullYear()) return hm;
  return `${d.getFullYear()}-${hm}`;
}

/** 完整时间，用于修改时间列的 tooltip（UI_SPEC §2.5）。 */
export function formatTimeFull(ms: number | null): string {
  if (ms == null) return '';
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()} ${p(d.getHours())}:${p(
    d.getMinutes(),
  )}:${p(d.getSeconds())}`;
}

export function formatHHMM(ms: number | null): string {
  if (ms == null) return '—';
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
}
