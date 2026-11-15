import React, { useRef, useState } from 'react';
import { I18N } from '../i18n';

/** 设置项状态。当前仅存内存（跨视图切换保留）；匹配/隐藏三项已接引擎，落盘待 M6。 */
export interface SettingsState {
  autostart: boolean;
  hideOnBlur: boolean;
  doubleCtrl: boolean;
  hotkey: string;
  /** 参与索引的卷，如 `C:\`。 */
  volumes: string[];
  excludes: string[];
  showHidden: boolean;
  fuzzy: boolean;
  pinyin: boolean;
  shuangpin: string;
  lang: 'zh' | 'en';
}

export const DEFAULT_SETTINGS: SettingsState = {
  autostart: false,
  hideOnBlur: true,
  doubleCtrl: false,
  hotkey: 'Ctrl+Alt+Space',
  volumes: ['C:\\', 'D:\\', 'E:\\'],
  excludes: [],
  showHidden: false,
  fuzzy: true,
  pinyin: true,
  shuangpin: 'off',
  lang: 'zh',
};

const ALL_VOLUMES = ['C:\\', 'D:\\', 'E:\\'];

const SHUANGPIN = [
  { value: 'off', label: '关' },
  { value: 'ziranma', label: '自然码' },
  { value: 'xiaohe', label: '小鹤双拼' },
];

const LANGS = [
  { value: 'zh' as const, label: '中文' },
  { value: 'en' as const, label: 'English' },
];

const SOFTWARE_LICENSE = 'MIT';

interface Props {
  onBack: () => void;
  state: SettingsState;
  patch: (p: Partial<SettingsState>) => void;
}

/** 把键盘事件拼成 `Ctrl+Alt+Space` 形式的组合键字符串。 */
function comboFromEvent(e: React.KeyboardEvent): string | null {
  const k = e.key;
  if (k === 'Control' || k === 'Alt' || k === 'Shift' || k === 'Meta') return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  if (e.metaKey) parts.push('Win');
  parts.push(k.length === 1 ? k.toUpperCase() : k === ' ' ? 'Space' : k);
  return parts.join('+');
}

// 未接线项在原型里点了只改界面状态、不产生任何行为。
// 必须**在可见 UI 上标出来**（UI_SPEC §4.8）——只写进文档或 title 悬停不够：
// 用户点开关后界面有反馈、结果却不变，界面本身就在撒谎。
// 每个未接线项必须**在可见 UI 上**写明它真实的生效里程碑（UI_SPEC §4.8.1）：
// 不得统一套用同一个标签——否则在对应里程碑交付时，旧标签会变成新的谎言。

function Toggle({
  label,
  on,
  disabled = false,
  pendingNote,
  onToggle,
}: {
  label: string;
  on: boolean;
  disabled?: boolean;
  pendingNote?: string;
  onToggle: (next: boolean) => void;
}) {
  return (
    <div className={'set-row' + (disabled ? ' disabled' : '')}>
      <span>
        {label}
        {pendingNote && <span className="pending-tag">{pendingNote}</span>}
      </span>
      <button
        type="button"
        role="switch"
        aria-checked={on}
        // 无障碍名里也带上阶段说明：与 M5/M6 占位项（"（M5 提供）"）口径一致，
        // 且可见文字是完整无障碍名的前缀，满足 WCAG 2.5.3 Label in Name。
        aria-label={pendingNote ? `${label}，${pendingNote}` : label}
        disabled={disabled}
        className={'toggle' + (on ? ' on' : '')}
        onClick={() => onToggle(!on)}
      >
        <span className="knob" />
      </button>
    </div>
  );
}

function HotkeyRow({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [recording, setRecording] = useState(false);

  // 录制中截获按键：阻止冒泡，避免被主窗口的全局快捷键处理掉。
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') {
      setRecording(false);
      return;
    }
    const combo = comboFromEvent(e);
    if (combo) {
      onChange(combo);
      setRecording(false);
    }
  };

  return (
    <div className="set-row">
      <span>
        全局热键 <span className="pending-tag">自定义未接线</span>
      </span>
      <button
        type="button"
        className={'hotkey hotkey-btn' + (recording ? ' recording' : '')}
        aria-label={`${I18N.a11y.hotkeyRecord(value)}，自定义未接线`}
        aria-pressed={recording}
        onClick={() => setRecording((r) => !r)}
        onKeyDown={onKeyDown}
      >
        {recording ? I18N.recording : value}
      </button>
      <span className="sr-only" aria-live="polite">
        {recording ? I18N.recordingHint : ''}
      </span>
    </div>
  );
}

function ExcludePaths({
  paths,
  onChange,
}: {
  paths: string[];
  onChange: (next: string[]) => void;
}) {
  const [draft, setDraft] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  const add = () => {
    const v = draft.trim();
    if (!v) {
      inputRef.current?.focus();
      return;
    }
    if (!paths.includes(v)) onChange([...paths, v]);
    setDraft('');
  };

  return (
    <div className="set-block">
      <span className="set-block-label">
        排除路径 <span className="pending-tag">未接线</span>
      </span>
      {paths.length === 0 ? (
        <div className="exclude-empty">{I18N.noExclude}</div>
      ) : (
        <ul className="exclude-list">
          {paths.map((p) => (
            <li key={p} className="exclude-row">
              <span className="mono">{p}</span>
              <button
                type="button"
                className="linkbtn"
                aria-label={I18N.a11y.removePath(p)}
                onClick={() => onChange(paths.filter((x) => x !== p))}
              >
                {I18N.remove}
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="exclude-add">
        <input
          ref={inputRef}
          className="pathinput"
          type="text"
          value={draft}
          placeholder={I18N.excludePlaceholder}
          aria-label={I18N.a11y.excludeInput}
          spellCheck={false}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              e.stopPropagation();
              add();
            }
          }}
        />
        <button type="button" className="linkbtn" aria-label={I18N.a11y.excludeAdd} onClick={add}>
          {I18N.add}
        </button>
      </div>
    </div>
  );
}

export default function Settings({ onBack, state, patch }: Props) {
  const setFlag =
    (key: 'autostart' | 'hideOnBlur' | 'doubleCtrl' | 'showHidden' | 'fuzzy' | 'pinyin') =>
    (next: boolean) =>
      patch({ [key]: next });

  return (
    <div className="settings">
      <div className="settings-head">
        <button className="backbtn" onClick={onBack}>
          {I18N.back}
        </button>
        <h1 className="settings-title">{I18N.settings}</h1>
      </div>
      <div className="settings-body">
        <section className="set-group">
          <h2 className="set-group-title">常规</h2>
          {/* UI_SPEC §4.8.1：未接线控件必须按各自真实阶段作可见标注，不统一套用。 */}
          <Toggle
            label="开机自启"
            on={state.autostart}
            onToggle={setFlag('autostart')}
            pendingNote="M6 提供"
          />
          <Toggle
            label="失去焦点自动隐藏"
            on={state.hideOnBlur}
            onToggle={setFlag('hideOnBlur')}
            pendingNote="开关未接线"
          />
          <Toggle
            label="双击 Ctrl 唤出"
            on={state.doubleCtrl}
            onToggle={setFlag('doubleCtrl')}
            pendingNote="未实现"
          />
          <HotkeyRow value={state.hotkey} onChange={(hotkey) => patch({ hotkey })} />
        </section>

        <section className="set-group">
          <h2 className="set-group-title">索引</h2>
          <fieldset className="set-fieldset">
            <legend className="set-block-label">
              索引卷 <span className="pending-tag">未接线</span>
            </legend>
            {ALL_VOLUMES.map((v) => (
              <label key={v} className="checkbox-row">
                <input
                  type="checkbox"
                  checked={state.volumes.includes(v)}
                  onChange={(e) =>
                    patch({
                      volumes: e.target.checked
                        ? [...state.volumes, v]
                        : state.volumes.filter((x) => x !== v),
                    })
                  }
                />
                <span className="mono">{v}</span>
              </label>
            ))}
          </fieldset>
          <ExcludePaths paths={state.excludes} onChange={(excludes) => patch({ excludes })} />
          {/* M5 已接线：showHidden 成对驱动 hide_system / hide_hidden（App.tsx::searchOpts）。 */}
          <Toggle label="显示系统/隐藏文件" on={state.showHidden} onToggle={setFlag('showHidden')} />
        </section>

        <section className="set-group">
          <h2 className="set-group-title">匹配</h2>
          {/* M5 已接线：两项直通 SearchOptions.fuzzy / .pinyin，不再标「M5 生效」。 */}
          <Toggle label="模糊匹配" on={state.fuzzy} onToggle={setFlag('fuzzy')} />
          <Toggle label="拼音匹配" on={state.pinyin} onToggle={setFlag('pinyin')} />
          <fieldset className="set-fieldset">
            <legend className="set-block-label">
              双拼方案 <span className="pending-tag">暂未支持</span>
            </legend>
            {SHUANGPIN.map((s) => (
              <label key={s.value} className="radio-row">
                <input
                  type="radio"
                  name="shuangpin"
                  value={s.value}
                  checked={state.shuangpin === s.value}
                  onChange={() => patch({ shuangpin: s.value })}
                />
                <span>{s.label}</span>
              </label>
            ))}
          </fieldset>
        </section>

        <section className="set-group">
          <h2 className="set-group-title">高级（本期占位）</h2>
          <div className="set-row disabled">
            <span>
              评分规则编辑器 <span className="pending-tag">M5 预留，未实现</span>
            </span>
            <button
              type="button"
              role="switch"
              aria-checked={false}
              aria-label="评分规则编辑器（M5 预留，未实现）"
              className="toggle"
              disabled
            >
              <span className="knob" />
            </button>
          </div>
          <div className="set-row disabled">
            <span>HTTP API 开关与 token</span>
            <button
              type="button"
              role="switch"
              aria-checked={false}
              aria-label="HTTP API 开关与 token（M6 提供）"
              className="toggle"
              disabled
            >
              <span className="knob" />
            </button>
          </div>
        </section>

        <section className="set-group">
          <h2 className="set-group-title">语言</h2>
          <fieldset className="set-fieldset">
            <legend className="set-block-label">
              界面语言 <span className="pending-tag">仅中文</span>
            </legend>
            {LANGS.map((l) => (
              <label key={l.value} className="radio-row">
                <input
                  type="radio"
                  name="lang"
                  value={l.value}
                  checked={state.lang === l.value}
                  onChange={() => patch({ lang: l.value })}
                />
                <span>{l.label}</span>
              </label>
            ))}
          </fieldset>
        </section>

        <section className="set-group">
          <h2 className="set-group-title">关于</h2>
          <div className="set-row">
            <span>版本</span>
            <span className="mono">Sift 0.1.0</span>
          </div>
          <div className="set-row">
            <span>索引位置</span>
            <span className="mono">%LOCALAPPDATA%\Sift\</span>
          </div>
          <div className="set-row">
            <span>开源许可</span>
            <span className="mono">{SOFTWARE_LICENSE}</span>
          </div>
        </section>
      </div>
    </div>
  );
}
