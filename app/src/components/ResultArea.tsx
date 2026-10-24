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
import { formatSize, formatTime, formatTimeFull } from '../format';

export interface ResultAreaHandle {
  moveSelection(delta: number): void;
  page(delta: number): void;
  home(): void;
  end(): void;
  open(action: OpenAction): void;
  hasSelection(): boolean;
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
// 滚动停止后才发 range 请求（UI_SPEC §6.5 / §10 待办 #2）。
// effect 在每次滚动帧都会重跑，用此防抖把实际请求推迟到滚动停止后，
// 滚动中以已加载的旧数据渲染，避免快速滚动时大量在途请求。
const RANGE_DEBOUNCE_MS = 90;
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
  const rangeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
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

  /** roving tabindex：把 DOM 焦点移到指定行。 */
  const focusRow = (i: number) => {
    parentRef.current?.querySelector<HTMLElement>(`#sift-row-${i}`)?.focus();
  };
  const listHasFocus = () =>
    !!parentRef.current && parentRef.current.contains(document.activeElement);

  const move = (next: number) => {
    const clamped = Math.max(0, Math.min(total - 1, next));
    const keepFocusInList = listHasFocus();
    setSelected(clamped);
    virtualizer.scrollToIndex(clamped, { align: 'auto' });
    // 焦点已在列表内时焦点跟随选中行；焦点在输入框时不动，保证还能继续打字。
    if (keepFocusInList) requestAnimationFrame(() => focusRow(clamped));
  };

  useEffect(() => {
    const keepFocusInList = listHasFocus();
    setSelected(0);
    if (keepFocusInList) requestAnimationFrame(() => focusRow(0));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId]);

  // 加载可见窗口内缺失的行（UI_SPEC §6.5：可见 ±1 屏，按 id 丢弃旧回包）。
  // 滚动停止后才真正发 range 请求：effect 在每次滚动帧都会重跑，这里用防抖把实际
  // 请求推迟到滚动停止后（RANGE_DEBOUNCE_MS），滚动中以已加载的旧数据渲染，
  // 避免快速滚动时大量在途请求（§10 待办 #2）。
  useEffect(() => {
    if (rangeTimer.current) {
      clearTimeout(rangeTimer.current);
      rangeTimer.current = null;
    }
    if (!currentId || total === 0 || items.length === 0) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    const missing: number[] = [];
    for (let i = first; i <= last; i++) if (!rowsRef.current[i]) missing.push(i);
    if (missing.length === 0) return;
    const lo = missing[0];
    const hi = missing[missing.length - 1];
    const id = currentId;
    rangeTimer.current = setTimeout(() => {
      rangeTimer.current = null;
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
    }, RANGE_DEBOUNCE_MS);
    return () => {
      if (rangeTimer.current) {
        clearTimeout(rangeTimer.current);
        rangeTimer.current = null;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentId, total, items, engine]);

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
      hasSelection: () => total > 0,
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [total, currentId, engine, virtualizer],
  );

  const scanning = status.volumes.some((v) => v.state === 'scanning');
  const errored = status.volumes.some((v) => v.state === 'error');
  const emptyQuery = query.trim() === '';
  const scanProgress = Math.max(0, ...status.volumes.map((v) => v.progress ?? 0));

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

  // 骨架行只在"新搜索首屏且尚无任何行"时出现（UI_SPEC §4.5），
  // 滚动中缺失的行渲染为空白行，避免快速滚动时满屏闪烁。
  const firstScreenSkeleton = Object.keys(rows).length === 0;

  return (
    <div className="resultarea">
      {requested && <div className="topprogress" aria-hidden="true" />}

      {panel === 'list' ? (
        <>
          <div className="colhead" role="group" aria-label={I18N.a11y.sorting}>
            {COLS.map((c) => {
              const active = sort.key === c.key;
              const dir = sort.descending ? '降序' : '升序';
              return (
                <button
                  key={c.key}
                  className={'col ' + c.cls}
                  onClick={() => onHeaderClick(c.key)}
                  aria-pressed={active}
                  aria-label={active ? `${c.label}，${dir}` : `${c.label}，未排序`}
                >
                  <span>{c.label}</span>
                  {active && (
                    <span className="sortind" aria-hidden="true">
                      {sort.descending ? '▼' : '▲'}
                    </span>
                  )}
                </button>
              );
            })}
          </div>

          <div
            className="list"
            ref={parentRef}
            role="listbox"
            aria-label={I18N.a11y.list}
            aria-busy={requested}
            aria-activedescendant={`sift-row-${selected}`}
          >
            <div
              style={{ height: virtualizer.getTotalSize(), position: 'relative' }}
              role="presentation"
            >
              {items.map((vi) => {
                const i = vi.index;
                const r = rows[i];
                const isSel = i === selected;
                return (
                  <div
                    key={i}
                    id={`sift-row-${i}`}
                    className={'row' + (isSel ? ' sel' : '')}
                    role="option"
                    aria-selected={isSel}
                    aria-posinset={i + 1}
                    aria-setsize={total}
                    tabIndex={isSel ? 0 : -1}
                    style={{ transform: `translateY(${vi.start}px)`, height: 32 }}
                    onClick={() => {
                      setSelected(i);
                      focusRow(i);
                    }}
                    onDoubleClick={() => engine.open(currentId, i, 'open')}
                  >
                    <RowContent row={r} skeleton={firstScreenSkeleton && i < 3} />
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
          progress={scanProgress}
          onClearFilter={onClearFilter}
          onRescan={onRescan}
        />
      )}
    </div>
  );
});

function RowContent({ row, skeleton }: { row: ResultRow | undefined; skeleton: boolean }) {
  if (!row) {
    // 无骨架时渲染空白行（不显示 12px 灰块）。
    if (!skeleton) return null;
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
      <span className="c-name" title={row.name}>
        <span className="ficon">{row.is_dir ? <FolderIcon /> : <FileIcon />}</span>
        <NameHL name={row.name} ranges={row.name_matches} />
      </span>
      <span className="c-path" title={row.dir}>
        {row.dir}
      </span>
      <span className="c-size mono">{formatSize(row.size)}</span>
      <span className="c-mod mono" title={formatTimeFull(row.mtime)}>
        {formatTime(row.mtime)}
      </span>
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
  progress,
  onClearFilter,
  onRescan,
}: {
  panel: Panel;
  query: string;
  progress: number;
  onClearFilter: () => void;
  onRescan: () => void;
}) {
  if (panel === 'scanning')
    return (
      <div className="empty">
        <span className="ring" aria-hidden="true" />
        <div className="empty-main">{I18N.scanning(progress)}</div>
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
