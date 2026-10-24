import React from 'react';
import { EngineStatus, SearchSummary } from '../types';
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
    const maxP = Math.max(...status.volumes.map((v) => v.progress ?? 0));
    right = I18N.scanning(maxP);
  } else if (ready) {
    right = I18N.indexReady(status.total_entries, formatHHMM(status.last_saved));
  } else {
    right = '';
  }

  return (
    <div className="statusbar">
      <span className="status-left" aria-live="polite">
        {left}
      </span>
      <span className="status-right">{right}</span>
    </div>
  );
}
