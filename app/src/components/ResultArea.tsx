import React, {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import {
  EngineClient,
  EngineStatus,
  MatchRange,
  OpenAction,
  ParseError,
  ResultRow,
  SortKey,
  SortSpec,
} from '../types';
import { I18N } from '../i18n';
import { EmptyIcon, FileIcon, FolderIcon, RetryIcon, WarnIcon } from '../icons';
import { formatSize, formatTime } from '../format';

export interface ResultAreaHandle {
  moveSelection(delta: number): void;
  page(delta: number): void;
  home(): void;
  end(): void;
  open(action: OpenAction): void;
}

interface Props {
  engine: EngineClient;
  currentId: number;
  total: number;
  rows: Record<number, ResultRow>;
  setRows: React.Dispatch<React.SetStateAction<Record<number, ResultRow>>>;
  rowsRef: React.MutableRefObject<Record<number, ResultRow>>;
  requested: boolean;
  hasSummary: boolean;
  parseError: ParseError | null;
  query: string;
  status: EngineStatus;
  sort: SortSpec;
  onSort: (s: SortSpec) => void;
  onClearFilter: () => void;
  onRescan: () => void;
}

type Panel = 'list' | 'scanning' | 'emptyQuery' | 'noResult' | 'searchingEmpty' | 'error';

const PAGE = 15;
const COLS: { key: SortKey; label: string; cls: string }[] = [
  { key: 'name', label: I18N.cols[0], cls: 'c-name' },
  { key: 'path', label: I18N.cols[1], cls: 'c-path' },
  { key: 'size', label: I18N.cols[2], cls: 'c-size' },
  { key: 'modified', label: I18N.cols[3], cls: 'c-mod' },
];

const ResultArea = forwardRef<ResultAreaHandle, Props>((props, ref) => {
  const {
    engine,
    currentId,
    total,
    rows,
    setRows,
    rowsRef,
    requested,
    hasSummary,
    query,
    status,
    sort,
    onSort,
    onClearFilter,
    onRescan,
  } = props;

  const parentRef = useRef<HTMLDivElement>(null);
  const [selected, setSelected] = useState(0);
  const selectedRef = useRef(0);
  selectedRef.current = selected;

  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 32,
    overscan: 12,
  });
  const items = virtualizer.getVirtualItems();

  useEffect(() => {
    setSelected(0);
  }, [currentId]);

  // 加载可见窗口内缺失的行（UI_SPEC §6.5：可见 ±1 屏，按 id 丢弃旧回包）。
  useEffect(() => {
    if (!currentId || total === 0 || items.length === 0) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    const missing: number[] = [];
    for (let i = first; i <= last; i++) if (!rowsRef.current[i]) missing.push(i);
    if (missing.length === 0) return;
    const lo = missing[0];
    const hi = missing[missing.length - 1];
    const id = currentId;
    engine.range(id, lo, hi - lo + 1).then((batch) => {
      if (id !== currentId) return;
      setRows((prev) => {
        const next = { ...prev };
        batch.forEach((r, k) => {
          next[lo + k] = r;
        });
        return next;
      });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId, total, items, engine]);

  const move = (next: number) => {
    const clamped = Math.max(0, Math.min(total - 1, next));
    setSelected(clamped);
    virtualizer.scrollToIndex(clamped, { align: 'auto' });
  };

  useImperativeHandle(
    ref,
    () => ({
      moveSelection: (d) => move(selectedRef.current + d),
      page: (d) => move(selectedRef.current + d * PAGE),
      home: () => move(0),
      end: () => move(total - 1),
      open: (action) => {
        const i = selectedRef.current;
        if (i >= total) return;
        engine.open(currentId, i, action);
      },
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [total, currentId, engine, virtualizer],
  );

  const scanning = status.volumes.some((v) => v.state === 'scanning');
  const errored = status.volumes.some((v) => v.state === 'error');
  const emptyQuery = query.trim() === '';

  let panel: Panel = 'list';
  if (errored && !scanning) panel = 'error';
  else if (emptyQuery && scanning) panel = 'scanning';
  else if (emptyQuery) panel = 'emptyQuery';
  else if (hasSummary && total === 0) panel = 'noResult';
  else if (!emptyQuery && !hasSummary && total === 0) panel = 'searchingEmpty';

  const onHeaderClick = (key: SortKey) => {
    if (sort.key === key) onSort({ key, descending: !sort.descending });
    else onSort({ key, descending: true });
  };

  return (
    <div className="resultarea">
      {requested && <div className="topprogress" aria-hidden="true" />}

      {panel === 'list' ? (
        <>
          <div className="colhead" role="row">
            {COLS.map((c) => {
              const active = sort.key === c.key;
              const aria = active
                ? sort.descending
                  ? 'descending'
                  : 'ascending'
                : 'none';
              return (
                <button
                  key={c.key}
                  className={'col ' + c.cls}
                  onClick={() => onHeaderClick(c.key)}
                  aria-sort={aria}
                >
                  <span>{c.label}</span>
                  {active && <span className="sortind">{sort.descending ? '▼' : '▲'}</span>}
                </button>
              );
            })}
          </div>

          <div
            className="list"
            ref={parentRef}
            role="listbox"
            aria-label={I18N.a11y.list}
            tabIndex={0}
          >
            <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
              {items.map((vi) => {
                const i = vi.index;
                const r = rows[i];
                const isSel = i === selected;
                return (
                  <div
                    key={i}
                    className={'row' + (isSel ? ' sel' : '')}
                    role="option"
                    aria-selected={isSel}
                    style={{ transform: `translateY(${vi.start}px)`, height: 32 }}
                    onClick={() => setSelected(i)}
                    onDoubleClick={() => engine.open(currentId, i, 'open')}
                  >
                    <RowContent row={r} />
                  </div>
                );
              })}
            </div>
          </div>
        </>
      ) : (
        <EmptyState
          panel={panel}
          query={query}
          onClearFilter={onClearFilter}
          onRescan={onRescan}
        />
      )}
    </div>
  );
});

function RowContent({ row }: { row: ResultRow | undefined }) {
  if (!row) {
    return (
      <>
        <span className="c-name skel" />
        <span className="c-path skel" />
        <span className="c-size skel" />
        <span className="c-mod skel" />
      </>
    );
  }
  return (
    <>
      <span className="c-name">
        <span className="ficon">{row.is_dir ? <FolderIcon /> : <FileIcon />}</span>
        <NameHL name={row.name} ranges={row.name_matches} />
        {row.is_dir && <span className="dirbadge">文件夹</span>}
      </span>
      <span className="c-path" title={row.dir}>
        {row.dir}
      </span>
      <span className="c-size mono">{formatSize(row.size)}</span>
      <span className="c-mod mono">{formatTime(row.mtime)}</span>
    </>
  );
}

function NameHL({ name, ranges }: { name: string; ranges: MatchRange[] }) {
  if (!ranges.length) return <>{name}</>;
  const nodes: React.ReactNode[] = [];
  let cur = 0;
  ranges.forEach(([s, e], idx) => {
    if (s > cur) nodes.push(name.slice(cur, s));
    nodes.push(<mark key={idx}>{name.slice(s, e)}</mark>);
    cur = e;
  });
  if (cur < name.length) nodes.push(name.slice(cur));
  return <>{nodes}</>;
}

function EmptyState({
  panel,
  query,
  onClearFilter,
  onRescan,
}: {
  panel: Panel;
  query: string;
  onClearFilter: () => void;
  onRescan: () => void;
}) {
  if (panel === 'scanning')
    return (
      <div className="empty">
        <span className="ring" aria-hidden="true" />
        <div className="empty-main">正在建立索引…</div>
        <div className="empty-sub">索引就绪后自动搜索，可先输入关键词</div>
      </div>
    );
  if (panel === 'emptyQuery')
    return (
      <div className="empty">
        <EmptyIcon />
        <div className="empty-main">{I18N.emptyQuery}</div>
        <div className="empty-sub">{I18N.emptyHint}</div>
      </div>
    );
  if (panel === 'searchingEmpty')
    return (
      <div className="empty">
        <span className="spinner big" aria-hidden="true" />
        <div className="empty-main">{I18N.searching}</div>
      </div>
    );
  if (panel === 'noResult')
    return (
      <div className="empty">
        <EmptyIcon />
        <div className="empty-main">{I18N.noResult(query)}</div>
        <div className="empty-sub">
          {I18N.noResultHint}
          <button className="linkbtn" onClick={onClearFilter}>
            {I18N.clearFilter}
          </button>
        </div>
      </div>
    );
  if (panel === 'error')
    return (
      <div className="empty">
        <WarnIcon className="errbig" />
        <div className="empty-main">{I18N.indexError('索引服务不可用')}</div>
        <button className="linkbtn" onClick={onRescan}>
          <RetryIcon /> {I18N.retry}
        </button>
      </div>
    );
  return null;
}

export default ResultArea;
