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
  /** JS epoch 毫秒；Tauri IPC 层从引擎的 FILETIME 转换（P1-8）。null = 元数据未补齐或时间无效。 */
  mtime: number | null;
  name_matches: MatchRange[];
}

export type OpenAction = 'open' | 'open_folder' | 'copy_path' | 'copy_name' | 'properties';

export type VolumeState = 'scanning' | 'ready' | 'error';

export interface VolumeStatus {
  volume: string;
  state: VolumeState;
  entries: number;
  progress: number | null;
  message: string | null;
  /** P0-1：有旧索引正在后台刷新（true=仍可搜索，UI 淡淡提示「正在更新」；
   *  false 且 state==='scanning' 表示首次建索引，UI 显示「索引中」）。 */
  stale: boolean;
}

export interface EngineStatus {
  volumes: VolumeStatus[];
  total_entries: number;
  /** JS epoch 毫秒；Tauri IPC 层从 FILETIME 转换（P1-8）。null = 从未保存。 */
  last_saved: number | null;
  searching: boolean;
  /** P0-5：最近一次落盘失败原因；成功或未尝试时为 null。 */
  save_error: string | null;
}

/**
 * 全局索引进度（0..1），**按卷的条目数加权**：非扫描中的卷计 1，扫描中的卷取其自身进度。
 *
 * 两处演进（都是实机观感问题）：
 * 1. 旧实现 `Math.max(...progress)`：结束卷 progress 为 null→0，进度条只反映"当前卷"，
 *    每扫完一卷就视觉归零重爬。改为求均值后仍会回跳 —— 因为卷是**逐个顺序扫描**的，
 *    未轮到的卷虽是 `Ready`（索引自磁盘载入）却被算作已完成，于是每开扫一个新卷该卷
 *    归 0、全局进度被拽回低位（"70 多 → 90 多 → 退回 70 多"）。真正的修法在调用侧：
 *    启动前用 `mark_pending` 把本轮要扫的卷**全部**置为待扫，本函数才单调上升。
 * 2. 加权：权重取该卷条目数（未知时退化为 1），使 90 万条目的 C: 与 0 条目的空卷
 *    不再等权影响观感，进度与实际工作量成正比。
 */
export function overallScanProgress(volumes: VolumeStatus[]): number {
  if (volumes.length === 0) return 0;
  let done = 0;
  let weight = 0;
  for (const v of volumes) {
    const w = Math.max(1, v.entries);
    weight += w;
    done += w * (v.state === 'scanning' ? Math.min(1, Math.max(0, v.progress ?? 0)) : 1);
  }
  return weight === 0 ? 0 : done / weight;
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
