import React from 'react';
import { I18N } from '../i18n';

interface Props {
  onBack: () => void;
}

function Toggle({ label, on = true, disabled = false }: { label: string; on?: boolean; disabled?: boolean }) {
  return (
    <div className={'set-row' + (disabled ? ' disabled' : '')}>
      <span>{label}</span>
      <span className={'toggle' + (on ? ' on' : '') + (disabled ? ' dis' : '')} aria-hidden="true">
        <span className="knob" />
      </span>
    </div>
  );
}

const GROUPS: { title: string; rows: React.ReactNode[] }[] = [
  {
    title: '常规',
    rows: [
      <Toggle key="a" label="开机自启" />,
      <Toggle key="b" label="失去焦点自动隐藏" />,
      <Toggle key="c" label="双击 Ctrl 唤出" />,
      <div className="set-row" key="d">
        <span>全局热键</span>
        <span className="hotkey">Ctrl+Alt+Space</span>
      </div>,
    ],
  },
  {
    title: '索引',
    rows: [
      <div className="set-row" key="a">
        <span>索引卷</span>
        <span className="mono">C:\ · D:\ · E:\</span>
      </div>,
      <div className="set-row" key="b">
        <span>排除路径</span>
        <span className="mono">—</span>
      </div>,
      <Toggle key="c" label="显示系统/隐藏文件" on={false} />,
    ],
  },
  {
    title: '匹配',
    rows: [
      <Toggle key="a" label="模糊匹配" />,
      <Toggle key="b" label="拼音匹配" />,
      <div className="set-row" key="c">
        <span>双拼方案</span>
        <span className="radio">关</span>
      </div>,
    ],
  },
  {
    title: '高级（本期占位）',
    rows: [
      <Toggle key="a" label="评分规则编辑器" on={false} disabled />,
      <Toggle key="b" label="HTTP API 开关与 token" on={false} disabled />,
    ],
  },
  {
    title: '关于',
    rows: [
      <div className="set-row" key="a">
        <span>版本</span>
        <span className="mono">Sift 0.1.0</span>
      </div>,
      <div className="set-row" key="b">
        <span>索引位置</span>
        <span className="mono">%LOCALDATA%\sift</span>
      </div>,
    ],
  },
];

export default function Settings({ onBack }: Props) {
  return (
    <div className="settings">
      <div className="settings-head">
        <button className="backbtn" onClick={onBack}>
          {I18N.back}
        </button>
        <span className="settings-title">{I18N.settings}</span>
      </div>
      <div className="settings-body">
        {GROUPS.map((g) => (
          <div className="set-group" key={g.title}>
            <div className="set-group-title">{g.title}</div>
            {g.rows}
          </div>
        ))}
      </div>
    </div>
  );
}
