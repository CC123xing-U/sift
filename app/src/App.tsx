import React, { useCallback, useEffect, useRef, useState } from 'react';
import { MockEngine } from './engine/MockEngine';
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
import Settings from './components/Settings';
import { applyChip, deriveChip } from './queryUtils';

export default function App() {
  const engineRef = useRef<EngineClient>(new MockEngine());
  const resultRef = useRef<ResultAreaHandle>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const [input, setInput] = useState('');
  const [sort, setSort] = useState<SortSpec>({ key: 'score', descending: true });
  const [view, setView] = useState<'search' | 'settings'>('search');
  const [status, setStatus] = useState<EngineStatus>(() => engineRef.current.status());

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

  // 状态轮询（1s，UI_SPEC §7.3）。
  useEffect(() => {
    const t = setInterval(() => setStatus(engineRef.current.status()), 1000);
    return () => clearInterval(t);
  }, []);

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
          if (s.id <= lastAcceptedIdRef.current) return; // 旧请求的回包一律丢弃
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

  // 输入 / chip / 排序变化即触发搜索（不防抖，UI_SPEC §3.3）。
  useEffect(() => {
    runSearch(input);
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

    if (e.key === 'Escape') {
      if (input) setInput('');
      else tgt.blur?.();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      inputRef.current?.focus();
      inputRef.current?.select();
      return;
    }
    if (e.ctrlKey && !inInput && e.key.toLowerCase() === 'c') {
      resultRef.current?.open('copy_path');
      e.preventDefault();
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
      if (inInput && input) return; // 输入框有文字时保留光标移动
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
    if (!inInput && e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      inputRef.current?.focus();
    }
  };

  const activeChip = deriveChip(input);
  const currentId = summary?.id ?? 0;

  return (
    <div className="app-window" onKeyDown={onKeyDown}>
      {view === 'settings' ? (
        <Settings onBack={() => setView('search')} />
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
  );
}
