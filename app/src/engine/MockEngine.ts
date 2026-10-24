// MockEngine —— EngineClient 的纯浏览器（TS）实现（DESIGN §2.2 / UI_SPEC §7）。
// 前端内存生成数据、模拟延迟与协作式取消；M3 起整体替换为 Tauri invoke，UI 代码不变。
import {
  EngineClient,
  EngineStatus,
  OpenAction,
  ParseError,
  ResultRow,
  SearchRequest,
  SearchSummary,
  VolumeStatus,
} from '../types';
import { Dataset, Entry, FIXED_NOW, generate } from '../data';
import { matchesEntry, nameMatchRanges, parseQuery, scoreEntry } from './query';

const TOTAL = 300_000;
const SEED = 0x5349_4654;
const SCAN_MS = 3000;

interface StoredResult {
  indices: number[];
  clauses: ReturnType<typeof parseQuery>['clauses'];
}

export class MockEngine implements EngineClient {
  private ds: Dataset;
  private nextId = 1;
  private results = new Map<number, StoredResult>();
  private statusState: EngineStatus;
  private scanTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    this.ds = generate(SEED, TOTAL);
    this.statusState = {
      volumes: [
        { volume: 'C:\\', state: 'scanning', entries: 0, progress: 0, message: null },
        { volume: 'D:\\', state: 'scanning', entries: 0, progress: 0, message: null },
        { volume: 'E:\\', state: 'scanning', entries: 0, progress: 0, message: null },
      ],
      total_entries: 0,
      last_saved: null,
      searching: false,
    };
    this.startScan();
  }

  private startScan() {
    const start = Date.now();
    const tick = () => {
      const p = Math.min(1, (Date.now() - start) / SCAN_MS);
      for (const v of this.statusState.volumes) {
        v.state = 'scanning';
        v.progress = p;
        v.entries = Math.floor((TOTAL / 3) * p);
      }
      this.statusState.total_entries = Math.floor(TOTAL * p);
      if (p < 1) {
        this.scanTimer = setTimeout(tick, 100);
      } else {
        for (const v of this.statusState.volumes) {
          v.state = 'ready';
          v.progress = null;
          v.entries = Math.floor(TOTAL / 3);
        }
        this.statusState.total_entries = TOTAL;
        this.statusState.last_saved = FIXED_NOW;
      }
    };
    this.scanTimer = setTimeout(tick, 100);
  }

  async search(req: SearchRequest): Promise<SearchSummary> {
    const id = this.nextId++;
    this.statusState.searching = true;

    const pq = parseQuery(req.query);
    if (pq.error) {
      // 优先透出解析器给出的具体原因（如"修饰符语法尚未支持"），
      // 缺 message 时才回落到通用文案——M20：别把"我们没实现"说成"你写错了"。
      const pe: ParseError = {
        message: pq.error.message || '语法错误，请检查查询',
        span: pq.error.span,
      };
      return new Promise((res) =>
        setTimeout(() => res({ id, total: 0, elapsed_ms: 1, parse_error: pe }), 10),
      );
    }

    const t0 = Date.now();
    const indices: number[] = [];
    const entries: Entry[] = this.ds.entries;
    for (let i = 0; i < entries.length; i++) {
      if (matchesEntry(entries[i], pq)) indices.push(i);
    }

    const { key, descending } = req.sort;
    indices.sort((a, b) => {
      const ea = entries[a];
      const eb = entries[b];
      let cmp = 0;
      switch (key) {
        case 'score': {
          const sa = scoreEntry(ea, pq);
          const sb = scoreEntry(eb, pq);
          cmp = sa - sb;
          if (cmp === 0) cmp = eb.mtime - ea.mtime;
          break;
        }
        case 'name':
          cmp = ea.name.localeCompare(eb.name);
          break;
        case 'path':
          cmp = (ea.dir + ea.name).localeCompare(eb.dir + eb.name);
          break;
        case 'size':
          cmp = (ea.size ?? -1) - (eb.size ?? -1);
          break;
        case 'modified':
          cmp = ea.mtime - eb.mtime;
          break;
      }
      return descending ? -cmp : cmp;
    });

    const elapsed = Date.now() - t0;
    this.results.set(id, { indices, clauses: pq.clauses });

    const delay = 20 + Math.floor(Math.random() * 40); // 20–60ms 模拟 Top-K 计算
    return new Promise((res) =>
      setTimeout(() => {
        this.statusState.searching = false;
        res({ id, total: indices.length, elapsed_ms: elapsed + delay, parse_error: null });
      }, delay),
    );
  }

  async range(id: number, offset: number, limit: number): Promise<ResultRow[]> {
    const rec = this.results.get(id);
    if (!rec) return [];
    const slice = rec.indices.slice(offset, offset + limit);
    const out: ResultRow[] = slice.map((i) => {
      const e = this.ds.entries[i];
      return {
        name: e.name,
        dir: e.dir,
        is_dir: e.is_dir,
        size: e.size,
        mtime: e.mtime,
        name_matches: nameMatchRanges(e.name, { ...EMPTY_PQ(), clauses: rec.clauses }),
      };
    });
    const delay = 5 + Math.floor(Math.random() * 10); // 5–15ms / 窗口
    return new Promise((res) => setTimeout(() => res(out), delay));
  }

  cancel(_id: number): void {
    // 结果与命中集合按 id 保留；UI 侧只渲染最新 id，旧 id 回包一律丢弃。
  }

  status(): EngineStatus {
    // 返回浅拷贝：本对象在扫描过程中被就地修改，若直接返回同一引用，
    // React 的 setState 会因 Object.is 判等而跳过重渲染，索引进度永远不更新。
    return {
      ...this.statusState,
      volumes: this.statusState.volumes.map((v) => ({ ...v })),
    };
  }

  async rescan(_volume?: string | null): Promise<void> {
    if (this.scanTimer) clearTimeout(this.scanTimer);
    for (const v of this.statusState.volumes) {
      v.state = 'scanning';
      v.progress = 0;
      v.entries = 0;
    }
    this.statusState.total_entries = 0;
    this.statusState.last_saved = null;
    this.startScan();
  }

  async open(_id: number, index: number, action: OpenAction): Promise<void> {
    const rec = this.results.get(_id);
    if (!rec) return;
    const e = this.ds.entries[rec.indices[index]];
    if (!e) return;
    if (action === 'copy_path') {
      const full = e.dir + (e.dir.endsWith('\\') ? '' : '\\') + e.name;
      try {
        await navigator.clipboard.writeText(full);
      } catch {
        /* 非安全上下文或拒绝授权时静默忽略 */
      }
    }
    // open / open_folder：浏览器原型无真实文件系统，留待 M3 接 Tauri。
  }
}

// 复用 parseQuery 的空结构，仅取 clauses 用于高亮。
function EMPTY_PQ() {
  return { exts: null as string[] | null, folderOnly: false, clauses: [] as string[][], error: null };
}
