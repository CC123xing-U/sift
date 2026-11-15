import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MockEngine } from './engine/MockEngine';
import { TauriEngine, isTauri } from './engine/TauriEngine';
import {
  DEFAULT_OPTS,
  EngineClient,
  EngineStatus,
  ParseError,
  ResultRow,
  SearchOptions,
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

// P2：status 每秒轮询一次，逐字段浅比较；引擎侧已按盘符排序使数组顺序稳定。
function statusShallowEq(a: EngineStatus, b: EngineStatus): boolean {
  if (
    a.total_entries !== b.total_entries ||
    a.searching !== b.searching ||
    a.last_saved !== b.last_saved ||
    a.save_error !== b.save_error ||
    a.volumes.length !== b.volumes.length
  ) {
    return false;
  }
  for (let i = 0; i < a.volumes.length; i++) {
    const x = a.volumes[i];
    const y = b.volumes[i];
    if (
      x.volume !== y.volume ||
      x.state !== y.state ||
      x.entries !== y.entries ||
      x.progress !== y.progress ||
      x.message !== y.message ||
      x.stale !== y.stale
    ) {
      return false;
    }
  }
  return true;
}

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
    save_error: null,
  });
  const [settings, setSettings] = useState<SettingsState>(DEFAULT_SETTINGS);
  // M5：设置页开关直接驱动引擎 SearchOptions（UI_SPEC §4.8：界面不得撒谎）。
  // hide_* 两字段按 §4.8 成对派生；fuzzy / pinyin 直通 M5 的 T2/T3 分层匹配。
  const searchOpts = useMemo<SearchOptions>(
    () => ({
      ...DEFAULT_OPTS,
      fuzzy: settings.fuzzy,
      pinyin: settings.pinyin,
      hide_system: !settings.showHidden,
      hide_hidden: !settings.showHidden,
    }),
    [settings.fuzzy, settings.pinyin, settings.showHidden],
  );
  const [hidden, setHidden] = useState(false);

  const [summary, setSummary] = useState<SearchSummary | null>(null);
  const [total, setTotal] = useState(0);
  const [parseError, setParseError] = useState<ParseError | null>(null);
  const [requested, setRequested] = useState(false);
  const [showSpinner, setShowSpinner] = useState(false);
  const [rows, setRows] = useState<Record<number, ResultRow>>({});
  const rowsRef = useRef<Record<number, ResultRow>>(rows);
  rowsRef.current = rows;

  // P1-4：每次 runSearch 自增的序列号，用于丢弃过期回包（替代 lastAcceptedIdRef）。
  const searchSeqRef = useRef(0);
  // 始终指向最新 runSearch（供 status 轮询回调在索引完成后重跑搜索时调用）。
  const runSearchRef = useRef<(q: string) => void>(() => {});
  // P0-1.4：记录上一轮各卷状态，用于检测「扫描完成」自动重跑当前查询。
  const prevVolStateRef = useRef<Map<string, string>>(new Map());
  // P2：缓存上一次 status，用于浅比较决定是否重渲染。
  const lastStatusRef = useRef<EngineStatus | null>(null);
  // P1-5：中文输入法组词期间不触发搜索。
  const composingRef = useRef(false);
  // P1-9：打开 / 复制失败时的临时提示（3 秒）。
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const spinnerTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showToast = useCallback((msg: string) => {
    setToast(msg);
    if (toastTimer.current) clearTimeout(toastTimer.current);
    toastTimer.current = setTimeout(() => setToast(null), 3000);
  }, []);

  // 状态轮询（1s，UI_SPEC §7.3）。status() 现在是异步（Tauri invoke / Mock 一致），
  // 立即拉一次首帧，之后每秒刷新；卸载时取消未完成的回包写入。
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      const s = await engineRef.current.status();
      if (cancelled) return;
      // P0-1.4：检测「scanning → ready」的卷（索引完成），自动重跑当前查询，
      // 让新索引的卷立即参与结果。仅在确实发生过完成转换且查询非空时触发。
      const prev = prevVolStateRef.current;
      const now = new Map(s.volumes.map((v) => [v.volume, v.state]));
      let completed = false;
      for (const [vol, st] of now) {
        if (prev.get(vol) === 'scanning' && st === 'ready') completed = true;
      }
      prevVolStateRef.current = now;
      // P2：状态每秒轮询一次，与上次「浅相等」时不触发重渲染（避免无谓的整树
      // 重绘；引擎侧已按盘符排序使顺序稳定）。
      if (lastStatusRef.current && statusShallowEq(lastStatusRef.current, s)) {
        if (completed && inputRef.current && inputRef.current.value.trim() !== '') {
          runSearchRef.current(inputRef.current.value);
        }
        return;
      }
      lastStatusRef.current = s;
      setStatus(s);
      if (completed && inputRef.current && inputRef.current.value.trim() !== '') {
        runSearchRef.current(inputRef.current.value);
      }
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

  // P1-5：窗口被热键隐藏再唤出后，输入框重新获得焦点（Tauri 下 hidden 状态不变，
  // 上面的 effect 不会重跑，需用 focus 事件兜底）。
  useEffect(() => {
    const onFocus = () => {
      if (view === 'search') {
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [view]);

  const runSearch = useCallback(
    (q: string) => {
      const seq = ++searchSeqRef.current;
      if (q.trim() === '') {
        // 清空：立即重置并让任何在途旧搜索回包失效（seq 已自增）。
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

      // P1-4.1：先拿 summary，再取首批行，一次性提交 summary + 首屏 rows，
      // 避免「先清空 → 90ms 后才回来」的闪烁；过期回包靠 seq 丢弃。
      engineRef.current
        .search({ query: q, sort, opts: searchOpts })
        .then(async (s) => {
          if (seq !== searchSeqRef.current) {
            engineRef.current.cancel(s.id); // 过期，释放结果集
            return;
          }
          const first = await engineRef.current
            .range(s.id, 0, 40)
            .catch(() => [] as ResultRow[]);
          if (seq !== searchSeqRef.current) {
            engineRef.current.cancel(s.id);
            return;
          }
          if (spinnerTimer.current) clearTimeout(spinnerTimer.current);
          setShowSpinner(false);
          setRequested(false);
          setSummary(s);
          setTotal(s.total);
          setParseError(s.parse_error);
          const init: Record<number, ResultRow> = {};
          first.forEach((r, i) => {
            init[i] = r;
          });
          setRows(init);
        })
        .catch(() => {
          // 搜索命令本身失败（极少见，如引擎 panic）。不闪退，仅收起 spinner。
          if (seq !== searchSeqRef.current) return;
          if (spinnerTimer.current) clearTimeout(spinnerTimer.current);
          setShowSpinner(false);
          setRequested(false);
        });
    },
    [sort, searchOpts],
  );
  runSearchRef.current = runSearch;

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
    // P1-5：组词期间不触发搜索（每个拼音字母都会 onChange，会造成无谓查询与闪烁）。
    if (composingRef.current) return;
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

    // P1-5：输入法组词中的按键（keyCode 229 / isComposing）不要拦截，
    // 否则微软拼音下回车确认候选词会误触「打开」，↑↓ 也会乱跳。
    if (e.nativeEvent.isComposing || (e as unknown as { keyCode?: number }).keyCode === 229)
      return;

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
    // 非搜索视图（设置页）不产生列表导航（Esc 已在上面处理完）。
    if (view !== 'search') return;
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
      // P1-4：若还有未触发的防抖，先立即 flush 出搜索；若搜索仍在进行
      // （requested），忽略本次打开，避免打开「输入框已是新查询、结果还是旧」
      // 的文件。
      if (searchTimer.current) {
        clearTimeout(searchTimer.current);
        searchTimer.current = null;
        runSearch(input);
      }
      if (requested) {
        e.preventDefault();
        return;
      }
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
    if (e.key === 'Home' || e.key === 'End') {
      const goEnd = e.key === 'End';
      if (e.ctrlKey) {
        // Ctrl+Home/End：始终跳列表首尾。
        e.preventDefault();
        if (goEnd) resultRef.current?.end();
        else resultRef.current?.home();
        return;
      }
      // P1-5：输入框聚焦时把 Home/End 交给浏览器移动光标，不拦截。
      if (isSearchInput) return;
      e.preventDefault();
      if (goEnd) resultRef.current?.end();
      else resultRef.current?.home();
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
                onCompositionStart={() => {
                  composingRef.current = true;
                }}
                onCompositionEnd={(v) => {
                  composingRef.current = false;
                  // P1-5：组词结束后立即用最终文本搜索一次。
                  runSearch(v);
                }}
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
                onOpenError={showToast}
              />
              <StatusBar
                summary={summary}
                requested={requested}
                query={input}
                status={status}
                toast={toast}
              />
            </>
          )}
        </div>
      )}
    </>
  );
}
