// 中文文案（UI_SPEC §8，默认资源表管理）。
export const I18N = {
  placeholder: '搜索文件与文件夹…',
  chips: ['全部', '文件夹', '文档', '图片', '视频', '音频', '压缩包'] as const,
  cols: ['名称', '路径', '大小', '修改时间'] as const,
  found: (n: number, ms: number) => `找到 ${n.toLocaleString('zh-CN')} 个结果 · ${ms} ms`,
  // 无障碍播报文案：只播报命中数，不播报耗时（UI_SPEC §3.7）。
  foundA11y: (n: number) => `找到 ${n.toLocaleString('zh-CN')} 个结果`,
  searching: '搜索中…',
  indexReady: (n: number, hhmm: string) =>
    `索引就绪 · ${n.toLocaleString('zh-CN')} 条 · 已保存 ${hhmm}`,
  scanning: (p: number) => `正在建立索引… ${Math.round(p * 100)}%`,
  emptyQuery: '输入关键词开始搜索',
  emptyHint: '支持拼音首字母（zgr → 中国人）与布尔语法（a | b）',
  noResult: (q: string) => `没有找到"${q}"的匹配项`,
  noResultHint: '检查拼写，或尝试更短的关键词、取消筛选',
  clearFilter: '清除筛选',
  // pos 为 0-based（Rust span 起点 / 子串 index），展示时按中文习惯转为 1-based。
  parseError: (pos: number) => `第 ${pos + 1} 位：语法错误，请检查查询`,
  // 引擎带回具体原因时优先展示原因（如"修饰符语法尚未支持"），而不是泛泛的"语法错误"。
  parseErrorWith: (pos: number, message: string) => `第 ${pos + 1} 位：${message}`,
  indexError: (err: string) => `索引出错：${err}`,
  retry: '重试',
  settings: '设置',
  back: '← 返回',
  clear: '清空',
  // 设置页
  recordingHint: '按下组合键…（Esc 取消）',
  recording: '录制中…',
  add: '+ 添加',
  remove: '删除',
  excludePlaceholder: '例：C:\\Windows',
  noExclude: '（无）',
  excludeEmpty: '请输入要排除的路径',
  a11y: {
    search: '搜索',
    clearBtn: '清空',
    settingsBtn: '设置',
    list: '搜索结果列表',
    sorting: '排序',
    colName: '名称',
    colPath: '路径',
    colSize: '大小',
    colModified: '修改时间',
    excludeAdd: '添加排除路径',
    excludeInput: '排除路径',
    hotkeyRecord: (combo: string) => `全局热键 ${combo}，点击录制新热键`,
    removePath: (p: string) => `删除排除路径 ${p}`,
  },
} as const;

// chips → 查询修饰符（UI_SPEC §3.4）
export const CHIP_MOD: Record<string, string> = {
  文件夹: 'folder:',
  文档: 'ext:doc;docx;pdf;md;txt;rtf;xls;xlsx;csv;ppt;pptx',
  图片: 'ext:jpg;jpeg;png;gif;bmp;webp;svg;ico;tif;tiff',
  视频: 'ext:mp4;mkv;avi;mov;wmv;flv;webm',
  音频: 'ext:mp3;wav;flac;aac;ogg;wma;m4a',
  压缩包: 'ext:zip;rar;7z;tar;gz;bz2;xz',
};
