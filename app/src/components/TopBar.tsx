import React, { RefObject, useRef, useState } from 'react';
import { I18N } from '../i18n';
import { CloseIcon, SearchIcon, SettingsIcon, WarnIcon } from '../icons';

interface Props {
  input: string;
  onInput: (v: string) => void;
  activeChip: string;
  onChip: (chip: string) => void;
  onSettings: () => void;
  onClear: () => void;
  parseError: { message: string; span: { start: number; end: number } } | null;
  spinner: boolean;
  inputRef: RefObject<HTMLInputElement>;
}

export default function TopBar({
  input,
  onInput,
  activeChip,
  onChip,
  onSettings,
  onClear,
  parseError,
  spinner,
  inputRef,
}: Props) {
  // chips 用 toolbar + roving tabindex：整组只占 1 个 Tab stop（UI_SPEC §5）。
  const chipRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [focusIdx, setFocusIdx] = useState(0);

  const focusChip = (i: number) => {
    const n = I18N.chips.length;
    const idx = ((i % n) + n) % n;
    setFocusIdx(idx);
    chipRefs.current[idx]?.focus();
  };

  // 焦点不在工具栏内时，Tab 落点回到当前选中的 chip（roving tabindex 惯例）。
  const activeIdx = I18N.chips.findIndex((c) => c === activeChip);
  const tabbable = activeIdx >= 0 ? activeIdx : focusIdx;

  const onChipKeyDown = (e: React.KeyboardEvent) => {
    // Home/End 与主窗口的"跳到首行/末行"冲突，处理掉之后不再向上冒泡。
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      e.stopPropagation();
      focusChip(focusIdx + 1);
    } else if (e.key === 'ArrowLeft') {
      e.preventDefault();
      e.stopPropagation();
      focusChip(focusIdx - 1);
    } else if (e.key === 'Home') {
      e.preventDefault();
      e.stopPropagation();
      focusChip(0);
    } else if (e.key === 'End') {
      e.preventDefault();
      e.stopPropagation();
      focusChip(I18N.chips.length - 1);
    }
  };

  return (
    <div className="topbar">
      <div className="searchrow">
        <span className="searchicon" aria-hidden="true">
          <SearchIcon />
        </span>
        <input
          ref={inputRef}
          id="sift-input"
          className="searchinput"
          type="text"
          value={input}
          placeholder={I18N.placeholder}
          aria-label={I18N.a11y.search}
          autoFocus
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => onInput(e.target.value)}
        />
        {spinner && <span className="spinner" aria-hidden="true" />}
        {!spinner && input && (
          <button
            className="iconbtn"
            aria-label={I18N.a11y.clearBtn}
            title={I18N.clear}
            onClick={onClear}
          >
            <CloseIcon />
          </button>
        )}
        <button
          className="iconbtn"
          aria-label={I18N.a11y.settingsBtn}
          title={I18N.settings}
          onClick={onSettings}
        >
          <SettingsIcon />
        </button>
      </div>

      <div
        className="chips"
        role="toolbar"
        aria-label="筛选"
        aria-orientation="horizontal"
        onKeyDown={onChipKeyDown}
      >
        {I18N.chips.map((c, i) => (
          <button
            key={c}
            ref={(el) => {
              chipRefs.current[i] = el;
            }}
            className={'chip' + (c === activeChip ? ' chip-on' : '')}
            aria-pressed={c === activeChip}
            tabIndex={i === tabbable ? 0 : -1}
            onFocus={() => setFocusIdx(i)}
            onClick={() => onChip(c)}
          >
            {c}
          </button>
        ))}
      </div>

      {parseError && (
        <div className="errorbar" role="alert">
          <WarnIcon className="erricon" />
          <span>
            {parseError.message
              ? I18N.parseErrorWith(parseError.span.start, parseError.message)
              : I18N.parseError(parseError.span.start)}
          </span>
        </div>
      )}
    </div>
  );
}
