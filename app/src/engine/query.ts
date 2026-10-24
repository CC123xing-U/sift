// 查询解析与匹配（MockEngine 内部）。支持：
//  - 空格分隔 = AND；子句内 `|` = OR
//  - 修饰符 `ext:doc;pdf`（扩展名过滤）与 `folder:`（仅目录）
//  - 普通词匹配文件名子串（不区分大小写）或拼音首字母子串（如 zgr → 中国人）
import { Entry } from '../data';
import { MatchRange } from '../types';

export interface ParsedQuery {
  exts: string[] | null;
  folderOnly: boolean;
  /** 普通 AND/OR 子句（不含修饰符）。 */
  clauses: string[][];
  error: { pos: number } | null;
}

export function parseQuery(q: string): ParsedQuery {
  const trimmed = q.trim();
  if (trimmed === '') return { exts: null, folderOnly: false, clauses: [], error: null };

  // 语法错误：首尾或连续的 `|`。
  if (trimmed.startsWith('|') || trimmed.endsWith('|') || trimmed.includes('||')) {
    const pos = trimmed.startsWith('|')
      ? 0
      : trimmed.endsWith('|')
        ? trimmed.length - 1
        : trimmed.indexOf('||');
    return { exts: null, folderOnly: false, clauses: [], error: { pos } };
  }

  const tokens = trimmed.split(/\s+/);
  let exts: string[] | null = null;
  let folderOnly = false;
  const plain: string[] = [];

  for (const tok of tokens) {
    if (tok.startsWith('ext:')) {
      const list = tok
        .slice(4)
        .split(';')
        .map((s) => s.trim().toLowerCase())
        .filter(Boolean);
      if (list.length) exts = list;
    } else if (tok === 'folder:') {
      folderOnly = true;
    } else {
      plain.push(tok);
    }
  }

  const clauses = plain.map((t) => t.split('|'));
  return { exts, folderOnly, clauses, error: null };
}

export function matchesEntry(e: Entry, pq: ParsedQuery): boolean {
  if (pq.folderOnly && !e.is_dir) return false;
  if (pq.exts) {
    const lower = e.name.toLowerCase();
    const hit = pq.exts.some((x) => lower.endsWith('.' + x) || lower.endsWith(x));
    if (!hit) return false;
  }
  if (pq.clauses.length === 0) return true;

  const name = e.name.toLowerCase();
  const py = e.py;
  for (const clause of pq.clauses) {
    let clauseOk = false;
    for (const termRaw of clause) {
      const term = termRaw.toLowerCase();
      if (term === '') continue;
      if (name.includes(term) || py.includes(term)) {
        clauseOk = true;
        break;
      }
    }
    if (!clauseOk) return false;
  }
  return true;
}

export function scoreEntry(e: Entry, pq: ParsedQuery): number {
  if (pq.clauses.length === 0) return 0;
  const name = e.name.toLowerCase();
  const py = e.py;
  let score = 0;
  for (const clause of pq.clauses) {
    for (const termRaw of clause) {
      const term = termRaw.toLowerCase();
      if (term === '') continue;
      if (name.includes(term)) score += 2;
      else if (py.includes(term)) score += 1;
    }
  }
  return score;
}

/** 计算名字内命中区间（字符索引），仅对取数窗口内的行调用。 */
export function nameMatchRanges(name: string, pq: ParsedQuery): MatchRange[] {
  const ranges: MatchRange[] = [];
  const lower = name.toLowerCase();
  for (const clause of pq.clauses) {
    for (const termRaw of clause) {
      const term = termRaw.toLowerCase();
      if (!term) continue;
      let from = 0;
      let idx = lower.indexOf(term, from);
      while (idx !== -1) {
        ranges.push([idx, idx + term.length]);
        from = idx + term.length;
        idx = lower.indexOf(term, from);
      }
    }
  }
  ranges.sort((a, b) => a[0] - b[0]);
  return ranges;
}
