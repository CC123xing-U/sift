import React from 'react';
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
  const scanning = status.volumes.some((v) => v.state === 'scanning');
  const errored = status.volumes.some((v) => v.state === 'error');
  // P1-3：仍有可搜卷（Ready 或正在刷新旧索引）时，失败卷不当作全局错误。
  const searchable = status.volumes.some(
    (v) => v.state === 'ready' || (v.state === 'scanning' && v.stale),
  );
  const ready = status.volumes.every((v) => v.state === 'ready');

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
        {/* P0-3：快捷键 / 右键路径的可见提示（REVIEW_BRIEF §5 P0-3 建议）。 */}
        {!requested && summary && query.trim() !== '' && summary.total > 0 && (
          <span className="status-hint" aria-hidden="true">
            {I18N.ctxHint}
          </span>
        )}
        <span className="sr-only" aria-live="polite">
          {announce}
        </span>
      </span>
      {toast && (
        <span className="status-toast" role="status">
          {toast}
        </span>
      )}
      <span className="status-right">{right}</span>
    </div>
  );
}
