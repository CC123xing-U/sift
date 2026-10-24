import React, { RefObject } from 'react';
import { I18N } from '../i18n';
import { CloseIcon, SearchIcon, SettingsIcon, WarnIcon } from '../icons';

interface Props {
  input: string;
  onInput: (v: string) => void;
  activeChip: string;
  onChip: (chip: string) => void;
  onSettings: () => void;
  onClear: () => void;
  parseError: { message: string; pos: number } | null;
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

      <div className="chips" role="group" aria-label="筛选">
        {I18N.chips.map((c) => (
          <button
            key={c}
            className={'chip' + (c === activeChip ? ' chip-on' : '')}
            aria-pressed={c === activeChip}
            onClick={() => onChip(c)}
          >
            {c}
          </button>
        ))}
      </div>

      {parseError && (
        <div className="errorbar" role="alert">
          <WarnIcon className="erricon" />
          <span>{I18N.parseError(parseError.pos)}</span>
        </div>
      )}
    </div>
  );
}
