// TauriEngine —— EngineClient 的真实实现（Tauri invoke → InProcessEngine）。
// 与 MockEngine 同形：UI 代码（App / ResultArea / StatusBar / Settings）完全不变。
//
// 通过 window.__TAURI__.core.invoke 调用 Rust 命令；依赖 tauri.conf.json 的
// app.withGlobalTauri=true，因此前端**不引入 @tauri-apps/api 包**——
// 保证纯浏览器（vite dev）也能编译运行，进入 Tauri 外壳后自动切换为真实引擎。
import type {
  EngineClient,
  SearchRequest,
  SearchSummary,
  ResultRow,
  EngineStatus,
  OpenAction,
} from '../types';

interface TauriCore {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;
}
interface TauriEvent {
  listen(event: string, handler: (e: unknown) => void): Promise<() => void>;
}
interface TauriGlobal {
  core: TauriCore;
  /** 窗口动效事件（sift:shown / sift:hiding），main.rs 在 show/延迟 hide 前发出。 */
  event?: TauriEvent;
}
declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
    __TAURI_INTERNALS__?: unknown;
  }
}

function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const t = window.__TAURI__;
  if (!t) {
    return Promise.reject(new Error('Tauri 运行时不可用（当前不是 Tauri 环境）'));
  }
  return t.core.invoke<T>(cmd, args);
}

/** 是否运行在 Tauri 外壳内（用于 App 选择真实引擎还是 MockEngine 原型）。 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export class TauriEngine implements EngineClient {
  search(req: SearchRequest): Promise<SearchSummary> {
    return invoke<SearchSummary>('search', { req });
  }
  range(id: number, offset: number, limit: number): Promise<ResultRow[]> {
    return invoke<ResultRow[]>('range', { id, offset, limit });
  }
  cancel(id: number): void {
    // 取消是 fire-and-forget：命令成功与否不影响 UI（UI 只渲染最新 id 的结果）。
    void invoke('cancel', { id });
  }
  async status(): Promise<EngineStatus> {
    return invoke<EngineStatus>('status');
  }
  rescan(volume?: string | null): Promise<void> {
    return invoke<void>('rescan', { volume: volume ?? null });
  }
  open(id: number, index: number, action: OpenAction): Promise<void> {
    return invoke<void>('open', { id, index, action });
  }
}
