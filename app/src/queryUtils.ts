// 输入框与 chips 的单一真源同步（UI_SPEC §3.4）。
import { CHIP_MOD } from './i18n';

/** 把 `ext:a;b;c` 形式的修饰符解析为规范化（小写、排序）的扩展名集合。 */
function extSet(mod: string): string[] | null {
  if (!mod.startsWith('ext:')) return null;
  return mod
    .slice(4)
    .split(';')
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean)
    .sort();
}

/**
 * 从输入文本推导出当前高亮的 chip。
 * 按 `ext:` 后的**具体扩展名集合**反查 CHIP_MOD，使"图片/视频/音频/压缩包"
 * 各自点亮；只命中前缀但集合不匹配（用户手改过）时按 UI_SPEC §3.4 失效为"全部"。
 */
export function deriveChip(input: string): string {
  const t = input.trim();
  if (/\bfolder:/.test(t)) return '文件夹';
  const m = t.match(/\bext:(\S*)/);
  if (m) {
    const want = extSet('ext:' + m[1]);
    if (want && want.length) {
      for (const chip of Object.keys(CHIP_MOD)) {
        const own = extSet(CHIP_MOD[chip]);
        if (own && own.length === want.length && own.every((x, i) => x === want[i])) {
          return chip;
        }
      }
    }
  }
  return '全部';
}

/** 点击 chip：把对应修饰符写入/替换/清除，返回新的输入文本。 */
export function applyChip(input: string, chip: string): string {
  const stripped = input
    .replace(/\bfolder:\S*/g, '')
    .replace(/\bext:\S*/g, '')
    .replace(/\s+/g, ' ')
    .trim();
  const active = deriveChip(input);
  if (chip === '全部') return stripped;
  if (chip === active) return stripped; // 再次点击当前项 = 取消
  const mod = CHIP_MOD[chip];
  return (stripped + ' ' + mod).trim();
}
