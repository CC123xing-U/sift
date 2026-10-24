// 合成数据集生成（固定种子、可复现）。规模 300,000 条（DESIGN §1.3 目标的约 1/7）。
// 中英文混合文件名（含可被拼音首字母命中的中文名）、4 层目录树、目录:文件 ≈ 1:6、
// 大小对数分布 0B–4GB、mtime 近 8 年。

/** 固定纪元，使 mtime / 保存时间可复现（不随运行时钟漂移）。 */
export const FIXED_NOW = Date.UTC(2026, 9, 24, 14, 8, 0);

/** mulberry32 —— 小体积确定性 PRNG。 */
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// 中文常用字 -> 拼音首字母（仅收录生成器会用到的字，保证拼音匹配可复现）。
const PINYIN: Record<string, string> = {
  中: 'z', 国: 'g', 人: 'r', 北: 'b', 京: 'j', 上: 's', 海: 'h', 广: 'g', 州: 'z',
  深: 's', 圳: 'z', 文: 'w', 件: 'j', 报: 'b', 告: 'g', 图: 't', 片: 'p', 视: 's',
  频: 'p', 音: 'y', 乐: 'l', 歌: 'g', 曲: 'q', 压: 'y', 缩: 's', 财: 'c', 务: 'w',
  会: 'h', 计: 'j', 设: 's', 开: 'k', 发: 'f', 项: 'x', 目: 'm', 公: 'g',
  司: 's', 部: 'b', 门: 'm', 年: 'n', 月: 'y', 日: 'r', 数: 's', 据: 'j', 分: 'f',
  析: 'x', 方: 'f', 案: 'a', 划: 'h', 草: 'c', 稿: 'g', 最: 'z', 终: 'z', 季: 'j',
  度: 'd', 总: 'z', 结: 'j', 议: 'y', 备: 'b', 忘: 'w', 录: 'l', 照: 'z', 影: 'y',
  像: 'x', 份: 'f', 客: 'k', 户: 'h', 订: 'd', 单: 'd', 合: 'h', 同: 't', 导: 'd',
  出: 'c', 版: 'b', 演: 'y', 示: 's', 范: 'f', 例: 'l', 测: 'c', 试: 's', 实: 's',
  验: 'y', 模: 'm', 型: 'x', 技: 'j', 术: 's', 产: 'c', 品: 'p', 工: 'g', 程: 'c',
};
const CN_CHARS = Object.keys(PINYIN);

const EN_WORDS = [
  'project', 'report', 'budget', 'annual', 'summary', 'meeting', 'notes', 'photo',
  'image', 'video', 'clip', 'song', 'music', 'archive', 'backup', 'data', 'figure',
  'analysis', 'plan', 'draft', 'final', 'invoice', 'contract', 'client', 'order',
  'design', 'spec', 'release', 'build', 'test', 'demo', 'sample', 'config', 'readme',
  'index', 'log', 'cache', 'snapshot', 'export', 'import', 'dataset', 'model', 'result',
];
const EN_SUFFIX = ['2021', '2022', '2023', '2024', '2025', 'Q1', 'Q2', 'Q3', 'Q4', 'v2', 'v3', 'final', 'draft'];

const FOLDERS = [
  'Work', 'Personal', 'Media', 'Docs', 'Projects', 'Downloads', 'Pictures', 'Music',
  'Videos', 'Backup', 'Reports', 'Finance', 'Clients', 'Archive', 'Temp', 'Dev',
  '中文资料', '工作文档', '设计稿', '项目文件', '图片素材', '音乐收藏', '视频剪辑', '备份数据',
];

// 中文文件名词库（仅含 PINYIN 表中存在的字，保证拼音首字母匹配可复现）。
const CN_WORDS = [
  '中国', '国人', '北京', '京海', '上海', '海上', '广州', '州深', '深圳', '圳文',
  '文件', '报告', '图片', '视频', '音乐', '歌曲', '压缩', '财务', '会计', '设计',
  '开发', '项目', '公司', '部门', '年月', '日期', '数据', '分析', '方案', '计划',
  '草稿', '最终', '季度', '总结', '会议', '备忘', '记录', '照片', '影像', '客户',
  '订单', '合同', '导出', '版本', '演示', '范例', '测试', '实验', '模型', '技术',
  '产品', '工程', '录音', '中国人', '北京人', '上海人', '深圳人', '报告书', '年度报',
];

const EN_EXT = ['docx', 'pdf', 'md', 'txt', 'xlsx', 'csv', 'pptx', 'zip', 'rar', '7z', 'jpg', 'png', 'gif', 'mp4', 'mkv', 'mp3', 'wav'];
const EXT_CAT: Record<string, string> = {
  docx: 'doc', pdf: 'doc', md: 'doc', txt: 'doc', xlsx: 'doc', csv: 'doc', pptx: 'doc',
  zip: 'zip', rar: 'zip', '7z': 'zip',
  jpg: 'img', png: 'img', gif: 'img',
  mp4: 'vid', mkv: 'vid',
  mp3: 'aud', wav: 'aud',
};

const ROOTS = ['C:\\', 'D:\\', 'E:\\'];

function pinyinOf(s: string): string {
  let out = '';
  for (const ch of s) out += PINYIN[ch] ?? '';
  return out;
}

function pick<T>(rnd: () => number, arr: T[]): T {
  return arr[Math.floor(rnd() * arr.length)];
}

export interface Entry {
  name: string;
  dir: string;
  is_dir: boolean;
  size: number | null;
  mtime: number;
  py: string;
}

export interface Dataset {
  entries: Entry[];
}

export function generate(seed = 0x5349_4654, count = 300_000): Dataset {
  const rnd = mulberry32(seed);
  const entries: Entry[] = [];

  // 先生成目录路径集合（目录:文件 ≈ 1:6 → 约 count/7 个目录）。
  const numDirs = Math.floor(count / 7);
  const dirPaths: string[] = [];
  for (let i = 0; i < numDirs; i++) {
    const root = pick(rnd, ROOTS);
    const depth = 1 + Math.floor(rnd() * 3); // 1–3 层子目录
    const segs: string[] = [];
    for (let d = 0; d < depth; d++) segs.push(pick(rnd, FOLDERS));
    const dir = root + segs.join('\\');
    dirPaths.push(dir);
    entries.push({
      name: segs[segs.length - 1],
      dir: root + segs.slice(0, -1).join('\\'),
      is_dir: true,
      size: null,
      mtime: FIXED_NOW - Math.floor(rnd() * 8 * 365 * 24 * 3600 * 1000),
      py: pinyinOf(segs[segs.length - 1]),
    });
  }

  // 再生成文件，随机挂到某个已有目录。
  const numFiles = count - numDirs;
  for (let i = 0; i < numFiles; i++) {
    const dir = pick(rnd, dirPaths);
    const useCn = rnd() < 0.5;
    let base: string;
    if (useCn) {
      let s = pick(rnd, CN_WORDS);
      if (rnd() < 0.5) s += pick(rnd, CN_WORDS);
      if (rnd() < 0.3) s += ' ' + pick(rnd, EN_SUFFIX);
      base = s;
    } else {
      base = pick(rnd, EN_WORDS) + (rnd() < 0.5 ? ' ' + pick(rnd, EN_SUFFIX) : '');
    }
    const ext = pick(rnd, EN_EXT);
    const name = base + '.' + ext;
    // 大小：对数分布 0B–4GB；中文/英文同分布。
    const size = Math.floor(Math.pow(4e9, rnd()));
    entries.push({
      name,
      dir,
      is_dir: false,
      size,
      mtime: FIXED_NOW - Math.floor(rnd() * 8 * 365 * 24 * 3600 * 1000),
      py: pinyinOf(base),
    });
  }

  return { entries };
}

/** 仅用于调试/统计：按扩展名归类计数（UI 暂未用）。 */
export function categoryOf(ext: string): string {
  return EXT_CAT[ext.toLowerCase()] ?? 'other';
}
