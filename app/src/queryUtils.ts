// 输入框与 chips 的单一真源同步（UI_SPEC §3.4）。
import { CHIP_MOD } from './i18n';

/** 从输入文本推导出当前高亮的 chip。 */
export function deriveChip(input: string): string {
  const t = input.trim();
  if (/\bfolder:/.test(t)) return '文件夹';
  if (/\bext:/.test(t)) return '文档'; // 任一 ext: 修饰符均归到"文档"高亮（多类共享同修饰符前缀）
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
