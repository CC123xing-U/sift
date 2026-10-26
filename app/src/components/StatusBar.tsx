import React from 'react';
import { EngineStatus, SearchSummary, overallScanProgress } from '../types';
import { I18N } from '../i18n';
import { formatHHMM } from '../format';

interface Props {
  summary: SearchSummary | null;
  requested: boolean;
  query: string;
  status: EngineStatus;
}

export default function StatusBar({ summary, requested, query, status }: Props) {
  const scanning = status.volumes.some((v) => v.state === 'scanning');
  const errored = status.volumes.some((v) => v.state === 'error');
  const ready = status.volumes.every((v) => v.state === 'ready');

  let left: string;
  if (requested) left = I18N.searching;
  else if (summary && query.trim() !== '')
    left = I18N.found(summary.total, summary.elapsed_ms);
  else left = '';

  let right: string;
  if (errored) {
    const m = status.volumes.find((v) => v.state === 'error');
    right = I18N.indexError(m?.message ?? '未知');
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
      <span className="status-right">{right}</span>
    </div>
  );
}
