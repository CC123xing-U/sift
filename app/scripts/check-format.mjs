// format.ts 纯函数回归断言（P1-8）。
//
// 项目尚无前端测试框架；本脚本零新增依赖：用正式 devDependency `typescript`
// 的 transpileModule 把单个 TS 文件转成 ESM，再由 data: URL 动态导入执行断言。
// 复现：cd app && npm run test:format
//
// 覆盖两类隐患：① FILETIME / NaN / 越界时间不再渲染 `NaN-NaN-NaN`；
// ② 目录行的 null 与异常 size 不再渲染 `0 B` / `NaN B`。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const src = fs.readFileSync(path.join(root, 'src/format.ts'), 'utf8');
const { outputText } = ts.transpileModule(src, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
});
const f = await import(
  'data:text/javascript;base64,' + Buffer.from(outputText).toString('base64')
);

let n = 0;
const eq = (actual, expected, label) => {
  assert.equal(actual, expected, label);
  n += 1;
};

// ── formatSize ──
eq(f.formatSize(null), '', '目录 / 未补齐元数据 → 空白');
eq(f.formatSize(0), '0 B', '零字节文件');
eq(f.formatSize(1023), '1023 B', '<1KB');
eq(f.formatSize(1024), '1 KB', 'KB 边界');
eq(f.formatSize(1024 * 1024), '1.0 MB', 'MB 边界');
eq(f.formatSize(1024 * 1024 * 1024), '1.0 GB', 'GB 边界');
eq(f.formatSize(-1), '', '负数 → 空白');
eq(f.formatSize(Number.NaN), '', 'NaN → 空白');
eq(f.formatSize(Number.POSITIVE_INFINITY), '', 'Infinity → 空白');

// ── 时间格式化：无效输入全部降级 ──
const invalid = [null, Number.NaN, Number.POSITIVE_INFINITY, 8.64e15 + 1, 1.34e17];
for (const v of invalid) {
  eq(f.formatTime(v), '', `formatTime(${String(v)}) → 空白`);
  eq(f.formatTimeFull(v), '', `formatTimeFull(${String(v)}) → 空白`);
  eq(f.formatHHMM(v), '—', `formatHHMM(${String(v)}) → 破折号`);
}

// ── 时间格式化：有效 epoch 毫秒按本地时区渲染 ──
const ms = Date.UTC(2026, 9, 24, 14, 8, 30);
const d = new Date(ms);
const p = (x) => String(x).padStart(2, '0');
const hm = `${p(d.getHours())}:${p(d.getMinutes())}`;
const md = `${d.getMonth() + 1}-${d.getDate()}`;
const year = d.getFullYear();
eq(f.formatHHMM(ms), hm, 'formatHHMM 正常值');
eq(
  f.formatTime(ms),
  year === new Date().getFullYear() ? `${md} ${hm}` : `${year}-${md} ${hm}`,
  'formatTime 同年 / 跨年',
);
eq(
  f.formatTimeFull(ms),
  `${year}-${d.getMonth() + 1}-${d.getDate()} ${hm}:${p(d.getSeconds())}`,
  'formatTimeFull 正常值',
);

// JS epoch 0（1970-01-01）应有效，不能被当成「时间未设置」。
eq(f.formatTime(0).length > 0, true, 'epoch 0 仍可显示');
eq(f.formatHHMM(0), f.formatHHMM(0), 'epoch 0 幂等');

// ── M7 第 5 项：内容搜索状态栏文案（DECISIONS #95）─────────────────────
// i18n.ts 无依赖，同样用 transpileModule 转成 ESM 后直接断言。
const i18nSrc = fs.readFileSync(path.join(root, 'src/i18n.ts'), 'utf8');
const { outputText: i18nOut } = ts.transpileModule(i18nSrc, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
});
const { I18N } = await import(
  'data:text/javascript;base64,' + Buffer.from(i18nOut).toString('base64')
);

eq(
  I18N.contentNote({
    scanned: 12,
    skipped_large: 0,
    skipped_binary: 0,
    unreadable: 0,
    truncated: false,
  }),
  '内容搜索：扫描 12 个文件',
  'contentNote 仅有扫描数',
);
eq(
  I18N.contentNote({
    scanned: 8,
    skipped_large: 2,
    skipped_binary: 1,
    unreadable: 3,
    truncated: false,
  }),
  '内容搜索：扫描 8 个文件 · 跳过 2 个过大、1 个二进制、3 个读取失败',
  'contentNote 三类跳过都列出',
);
eq(
  I18N.contentNote({
    scanned: 0,
    skipped_large: 0,
    skipped_binary: 0,
    unreadable: 0,
    truncated: true,
  }),
  '内容搜索：扫描 0 个文件 · 候选或读取量达上限，结果可能不完整',
  'contentNote 截断提示',
);

console.log(`format.ts: ${n} assertions passed`);
