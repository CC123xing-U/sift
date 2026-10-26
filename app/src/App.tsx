import React, { useCallback, useEffect, useRef, useState } from 'react';
import { MockEngine } from './engine/MockEngine';
import { TauriEngine, isTauri } from './engine/TauriEngine';
import {
  DEFAULT_OPTS,
  EngineClient,
  EngineStatus,
  ParseError,
  ResultRow,
  SearchSummary,
  SortSpec,
} from './types';
import TopBar from './components/TopBar';
import ResultArea, { ResultAreaHandle } from './components/ResultArea';
import StatusBar from './components/StatusBar';
import Settings, { DEFAULT_SETTINGS, SettingsState } from './components/Settings';
import { applyChip, deriveChip } from './queryUtils';

// 输入防抖窗口：停止输入这么久后才真正发起检索（详见下方 useEffect 注释）。
const SEARCH_DEBOUNCE_MS = 120;

export default function App() {
  // 环境自适应：Tauri 外壳内用真实引擎（invoke → InProcessEngine），
  // 纯浏览器（vite dev / 静态预览）回退到 MockEngine 原型，UI 代码不变。
  const engineRef = useRef<EngineClient>(isTauri() ? new TauriEngine() : new MockEngine());
  const resultRef = useRef<ResultAreaHandle>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const [input, setInput] = useState('');
  const [sort, setSort] = useState<SortSpec>({ key: 'score', descending: true });
  const [view, setView] = useState<'search' | 'settings'>('search');
  // 初始状态：首次轮询会立即填充（见下方 useEffect），此处仅给一个空壳避免首帧 undefined。
  const [status, setStatus] = useState<EngineStatus>({
    volumes: [],
    total_entries: 0,
    last_saved: null,
    searching: false,
  });
  const [settings, setSettings] = useState<SettingsState>(DEFAULT_SETTINGS);
  const [hidden, setHidden] = useState(false);

  const [summary, setSummary] = useState<SearchSummary | null>(null);
  const [total, setTotal] = useState(0);
  const [parseError, setParseError] = useState<ParseError | null>(null);
  const [requested, setRequested] = useState(false);
  const [showSpinner, setShowSpinner] = useState(false);
  const [rows, setRows] = useState<Record<number, ResultRow>>({});
  const rowsRef = useRef<Record<number, ResultRow>>(rows);
  rowsRef.current = rows;

  const lastAcceptedIdRef = useRef(0);
  const spinnerTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 状态轮询（1s，UI_SPEC §7.3）。status() 现在是异步（Tauri invoke / Mock 一致），
  // 立即拉一次首帧，之后每秒刷新；卸载时取消未完成的回包写入。
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      const s = await engineRef.current.status();
      if (!cancelled) setStatus(s);
    };
    void poll();
    const t = setInterval(() => void poll(), 1000);
    return () => {
      cancelled = true;
      clearInterval(t);
    };
  }, []);

  // 全局热键唤出/隐藏（UI_SPEC §5）。原型阶段直接隐藏 DOM；
  // Tauri 下由 Rust 全局快捷键（Ctrl+Alt+Space → toggle_window）管理真实窗口，这里不重复处理。
  useEffect(() => {
    const onDocKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.altKey && !e.shiftKey && !e.metaKey && e.code === 'Space') {
        e.preventDefault();
        if (!isTauri()) setHidden((h) => !h);
      }
    };
    document.addEventListener('keydown', onDocKey);
    return () => document.removeEventListener('keydown', onDocKey);
  }, []);

  // 输入框始终默认聚焦（UI_SPEC §3.3）。
  useEffect(() => {
    if (view === 'search') inputRef.current?.focus();
  }, [view, hidden]);

  const runSearch = useCallback(
    (q: string) => {
      if (q.trim() === '') {
        setRequested(false);
        setShowSpinner(false);
        setSummary(null);
        setTotal(0);
        setParseError(null);
        setRows({});
        return;
      }
      setRequested(true);
      setShowSpinner(false);
      if (spinnerTimer.current) clearTimeout(spinnerTimer.current);
      spinnerTimer.current = setTimeout(() => setShowSpinner(true), 250); // 防抖显示阈值

      engineRef.current
        .search({ query: q, sort, opts: DEFAULT_OPTS })
        .then((s) => {
          if (s.id <= lastAcceptedIdRef.current) {
            // 旧请求的回包一律丢弃；顺手让引擎释放该结果集（否则要等 60s 回收）。
            engineRef.current.cancel(s.id);
            return;
          }
          lastAcceptedIdRef.current = s.id;
          if (spinnerTimer.current) clearTimeout(spinnerTimer.current);
          setShowSpinner(false);
          setRequested(false);
          setSummary(s);
          setTotal(s.total);
          setParseError(s.parse_error);
          setRows({});
        });
    },
    [sort],
  );

  // 输入 / chip / 排序变化触发搜索（UI_SPEC §3.3）。
  //
  // 原实现「每次变化即搜」：每敲一个字都要对全卷（实测约 180 万条目）做一次完整检索，
  // 外加 IPC 往返与虚拟列表重建，连续输入时明显卡顿，停手才恢复（用户实机反馈）。
  // 改为 SEARCH_DEBOUNCE_MS 防抖：输入本身仍是受控 state、立即回显，
  // 只在停止输入后才真正发检索；清空输入立即执行（不必等防抖，避免结果残留）。
  const searchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (searchTimer.current) {
      clearTimeout(searchTimer.current);
      searchTimer.current = null;
    }
    if (input.trim() === '') {
      runSearch(input);
      return;
    }
    searchTimer.current = setTimeout(() => {
      searchTimer.current = null;
      runSearch(input);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      if (searchTimer.current) {
        clearTimeout(searchTimer.current);
        searchTimer.current = null;
      }
    };
  }, [input, runSearch]);

  const onChip = (chip: string) => setInput((prev) => applyChip(prev, chip));
  const onClear = () => setInput('');
  const onClearFilter = () => setInput((prev) => applyChip(prev, '全部'));
  const onRescan = () => {
    engineRef.current.rescan();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const tgt = e.target as HTMLElement;
    const inInput = tgt.tagName === 'INPUT' || tgt.tagName === 'TEXTAREA';
    const isSearchInput = tgt.id === 'sift-input';

    // 设置页里的文本框（排除路径）：除 Esc 外一律交给浏览器原生处理。
    if (inInput && !isSearchInput && e.key !== 'Escape') return;

    if (e.key === 'Escape') {
      e.preventDefault();
      if (view === 'settings') {
        setView('search'); // UI_SPEC §4.8：Esc 返回搜索视图
        return;
      }
      if (input) {
        setInput('');
        return;
      }
      // UI_SPEC §5 + M3：输入已为空 → 隐藏窗口。
      // Tauri 下调用真实窗口命令；原型下隐藏 DOM。
      if (isTauri()) {
        void window.__TAURI__?.core.invoke('hide');
      } else {
        setHidden(true);
      }
      return;
    }
    // 按钮自身的 Enter/Space 激活交还给浏览器：否则列头排序、chips、⚙ 都按不了。
    if (tgt.tagName === 'BUTTON' && (e.key === 'Enter' || e.key === ' ')) return;

    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      inputRef.current?.focus();
      inputRef.current?.select();
      return;
    }
    if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'c') {
      // 输入框内有文本选区时走浏览器原生复制，无选区时复制选中项完整路径。
      const el = tgt as HTMLInputElement;
      if (isSearchInput && el.selectionStart !== el.selectionEnd) return;
      if (resultRef.current?.hasSelection()) {
        resultRef.current.open('copy_path');
        e.preventDefault();
      }
      return;
    }
    if (e.ctrlKey && e.key.toLowerCase() === 'enter') {
      resultRef.current?.open('open_folder');
      e.preventDefault();
      return;
    }
    if (e.key === 'Enter') {
      resultRef.current?.open('open');
      e.preventDefault();
      return;
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      // UI_SPEC §5：↑↓ 在输入框聚焦时也生效（单行输入框无选区/多行冲突）。
      e.preventDefault();
      resultRef.current?.moveSelection(e.key === 'ArrowDown' ? 1 : -1);
      return;
    }
    if (e.key === 'PageDown') {
      e.preventDefault();
      resultRef.current?.page(1);
      return;
    }
    if (e.key === 'PageUp') {
      e.preventDefault();
      resultRef.current?.page(-1);
      return;
    }
    if (e.key === 'Home') {
      e.preventDefault();
      resultRef.current?.home();
      return;
    }
    if (e.key === 'End') {
      e.preventDefault();
      resultRef.current?.end();
      return;
    }
    // 列表聚焦时直接输入 → 焦点转入输入框（保留已输入内容）
    if (!inInput && tgt.tagName !== 'BUTTON' && e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      inputRef.current?.focus();
    }
  };

  const activeChip = deriveChip(input);
  const currentId = summary?.id ?? 0;

  return (
    <>
      {hidden ? (
        <button className="relaunch" onClick={() => setHidden(false)}>
          唤出窗口（模拟 {settings.hotkey}）
        </button>
      ) : (
        <div className="app-window" onKeyDown={onKeyDown}>
          {view === 'settings' ? (
            <Settings
              onBack={() => setView('search')}
              state={settings}
              patch={(p) => setSettings((s) => ({ ...s, ...p }))}
            />
          ) : (
            <>
              <TopBar
                input={input}
                onInput={setInput}
                activeChip={activeChip}
                onChip={onChip}
                onSettings={() => setView('settings')}
                onClear={onClear}
                parseError={parseError}
                spinner={showSpinner}
                inputRef={inputRef}
              />
              <ResultArea
                ref={resultRef}
                engine={engineRef.current}
                currentId={currentId}
                total={total}
                rows={rows}
                setRows={setRows}
                rowsRef={rowsRef}
                requested={requested}
                hasSummary={summary != null}
                parseError={parseError}
                query={input}
                status={status}
                sort={sort}
                onSort={setSort}
                onClearFilter={onClearFilter}
                onRescan={onRescan}
              />
              <StatusBar
                summary={summary}
                requested={requested}
                query={input}
                status={status}
              />
            </>
          )}
        </div>
      )}
    </>
  );
}
