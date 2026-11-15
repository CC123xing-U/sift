import React, { useEffect, useRef } from 'react';
import { EngineStatus, SearchSummary, overallScanProgress } from '../types';
import { I18N } from '../i18n';
import { formatHHMM } from '../format';

interface Props {
  summary: SearchSummary | null;
  requested: boolean;
  query: string;
  status: EngineStatus;
  /** P1-9：打开 / 复制失败的临时提示。 */
  toast?: string | null;
}

export default function StatusBar({ summary, requested, query, status, toast }: Props) {
  const detailRef = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const close = (event: MouseEvent) => {
      if (detailRef.current && !detailRef.current.contains(event.target as Node)) {
        detailRef.current.open = false;
      }
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, []);
  const scanning = status.volumes.some((v) => v.state === 'scanning');
  const errored = status.volumes.some((v) => v.state === 'error');
  // P1-3：仍有可搜卷（Ready 或正在刷新旧索引）时，失败卷不当作全局错误。
  const searchable = status.volumes.some(
    (v) => v.state === 'ready' || (v.state === 'scanning' && v.stale),
  );
  const ready = status.volumes.length > 0 && status.volumes.every((v) => v.state === 'ready');

  let left: string;
  if (requested) left = I18N.searching;
  else if (summary && query.trim() !== '')
    left = I18N.found(summary.total, summary.elapsed_ms);
  else left = '';

  let right: string;
  if (status.save_error) {
    right = I18N.saveError(status.save_error); // P0-5
  } else if (errored) {
    const failed = status.volumes.filter((v) => v.state === 'error');
    if (searchable && failed.length > 0) {
      // 仍有可搜卷：只提示失败的那一卷（P1-3）。
      const f = failed[0];
      right = I18N.volumeError(f.volume, f.message ?? '未知');
    } else {
      const m = failed[0];
      right = I18N.indexError(m?.message ?? '未知');
    }
  } else if (scanning) {
    const maxP = overallScanProgress(status.volumes);
    right = I18N.scanning(maxP);
  } else if (ready) {
    right = I18N.indexReady(status.total_entries, formatHHMM(status.last_saved));
  } else {
    right = '';
  }

  // 渲染与播报分离：可见节点被 aria-hidden，播报节点只念"找到 N 个结果"（不含耗时）。
  const announce =
    !requested && summary && query.trim() !== '' ? I18N.foundA11y(summary.total) : '';

  return (
    <div className="statusbar">
      <span className="status-left">
        <span className="mono" aria-hidden="true">
          {left}
        </span>
        <span className="sr-only" aria-live="polite">
          {announce}
        </span>
      </span>
      {toast && (
        <span className="status-toast" role="status">
          {toast}
        </span>
      )}
      <details
        className="index-status"
        ref={detailRef}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            event.stopPropagation();
            if (detailRef.current) {
              detailRef.current.open = false;
              detailRef.current.querySelector('summary')?.focus();
            }
          }
        }}
      >
        <summary title={right}>
          <span className={'status-dot' + (errored || status.save_error ? ' error' : scanning ? ' scanning' : '')} />
          {status.save_error ? '保存失败' : errored ? '索引异常' : scanning ? '索引更新中' : ready ? '索引就绪' : '连接中'}
        </summary>
        <div className="index-popover">
          <strong>索引状态</strong>
          <p>{right || '正在连接引擎'}</p>
          <div className="volume-list">
            {status.volumes.map((v) => (
              <div className="volume-status" key={v.volume}>
                <span title={v.volume}>{v.volume.split(' ')[0]}</span>
                <span>{v.state === 'error' ? '异常' : v.state === 'scanning' ? `${Math.round((v.progress ?? 0) * 100)}%` : v.entries.toLocaleString('zh-CN')}</span>
                {v.message && <p>{v.message}</p>}
              </div>
            ))}
          </div>
        </div>
      </details>
    </div>
  );
}
