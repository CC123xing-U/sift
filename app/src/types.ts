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
  hide_hidden: boolean;
}

export interface SearchRequest {
  query: string;
  sort: SortSpec;
  max_hits?: number;
  opts: SearchOptions;
}

/**
 * 查询解析错误 —— 逐字段镜像 `ff_core::ParseError`
 * （`crates/ff-core/src/lib.rs:36-41`）。
 *
 * Rust 侧为 `span: Range<usize>`（**字节**区间 `[start, end)`）。此处保持同形，
 * 不做 `pos` 简化：M3 经 IPC 拿到真实引擎错误时不需要再改一次类型，
 * 也避免"UI 侧字段与契约不同名"的隐性漂移。
 */
export interface ParseError {
  message: string;
  span: { start: number; end: number };
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
  status(): Promise<EngineStatus>;
  rescan(volume?: string | null): Promise<void>;
  open(id: number, index: number, action: OpenAction): Promise<void>;
}

// UI 侧复用的常量。
// hide_* 两字段**必须成对派生**——设置页只有一个"显示系统/隐藏文件"开关（UI_SPEC §4.8），
// 接线时写 `hide_system = hide_hidden = !开关`；只派生一个会漏掉纯隐藏文件（.gitignore 这类）。
export const DEFAULT_OPTS: SearchOptions = {
  case_sensitive: false,
  regex_default: false,
  fuzzy: true,
  pinyin: true,
  hide_system: true,
  hide_hidden: true,
};
