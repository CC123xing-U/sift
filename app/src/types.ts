// Sift UI — TypeScript 镜像的 EngineClient 契约（DESIGN §9，接口同形、实现可换）。
// 浏览器原型由 MockEngine（TS）实现；M3 起底层切到 Tauri invoke，UI 代码不动。

export type SortKey = 'score' | 'name' | 'path' | 'size' | 'modified';

export interface SortSpec {
  key: SortKey;
  descending: boolean;
}

export interface SearchOptions {
  case_sensitive: boolean;
  regex_default: boolean;
  fuzzy: boolean;
  pinyin: boolean;
  hide_system: boolean;
}

export interface SearchRequest {
  query: string;
  sort: SortSpec;
  max_hits?: number;
  opts: SearchOptions;
}

export interface ParseError {
  message: string;
  pos: number;
}

export interface SearchSummary {
  id: number;
  total: number;
  elapsed_ms: number;
  parse_error: ParseError | null;
}

/** 名字内命中区间：[start, end) 字符索引，供前端 <mark> 高亮。 */
export type MatchRange = [number, number];

export interface ResultRow {
  name: string;
  /** 完整父路径，如 `D:\Work\Reports`；根目录条目为 `D:\`。 */
  dir: string;
  is_dir: boolean;
  size: number | null;
  /** JS epoch 毫秒（M3 IPC 边界再转 FILETIME）。 */
  mtime: number | null;
  name_matches: MatchRange[];
}

export type OpenAction = 'open' | 'open_folder' | 'copy_path';

export type VolumeState = 'scanning' | 'ready' | 'error';

export interface VolumeStatus {
  volume: string;
  state: VolumeState;
  entries: number;
  progress: number | null;
  message: string | null;
}

export interface EngineStatus {
  volumes: VolumeStatus[];
  total_entries: number;
  last_saved: number | null;
  searching: boolean;
}

export interface EngineClient {
  search(req: SearchRequest): Promise<SearchSummary>;
  range(id: number, offset: number, limit: number): Promise<ResultRow[]>;
  cancel(id: number): void;
  status(): EngineStatus;
  rescan(volume?: string | null): Promise<void>;
  open(id: number, index: number, action: OpenAction): Promise<void>;
}

// UI 侧复用的常量
export const DEFAULT_OPTS: SearchOptions = {
  case_sensitive: false,
  regex_default: false,
  fuzzy: true,
  pinyin: true,
  hide_system: true,
};
