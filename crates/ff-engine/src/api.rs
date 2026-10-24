//! ff-engine 对外契约（DESIGN §9，契约冻结）。
//!
//! 变更只能由 Orchestrator 修改，并在 `docs/DECISIONS.md` 记录。
//! UI 只依赖本模块的 [`EngineClient`] trait；实现有 `InProcess`（M1–M5，直接调引擎）
//! 与 `Pipe`（M6，经命名管道连服务）两种（DESIGN §2.2-1）。

pub use ff_core::ParseError;

use std::fmt;
use std::ops::Range;

/// 卷标识（定义在 `ff-ntfs`，此处再导出，方便 UI / 引擎侧统一引用）。
pub use ff_ntfs::VolumeId;

/// 搜索请求 id。同一客户端的每次 `search` 分配递增 id；UI 只渲染最新 id 的结果
/// （DESIGN §11.1：连续快速输入不错乱）。
pub type RequestId = u64;

/// 排序键（DESIGN §9 `SortSpec`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// 按评分（DESIGN §8.2）；分数越高越相关。
    Score,
    Name,
    Path,
    Size,
    Modified,
}

/// 排序方式（DESIGN §9 `SortSpec`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    /// true = 降序。Score 默认降序（高分在前）；Name/Path 等可按需切换。
    pub descending: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec {
            key: SortKey::Score,
            descending: true,
        }
    }
}

/// 搜索选项（DESIGN §9 `SearchOptions`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchOptions {
    /// 区分大小写（对应查询语法 `case:`；默认 false，见 DESIGN §7.1）。
    pub case_sensitive: bool,
    /// 查询是否默认按正则解释（`regex:`，M4 细化；当前仅随请求传递）。
    pub regex_default: bool,
    /// 启用模糊匹配 tier T3（默认 true，见 DESIGN §8.1）。
    /// **预留字段，M5 生效**：T3 属 M5（DESIGN §8.1 / §13），`ff-match` 尚无模糊匹配器，
    /// `InProcessEngine::search` **当前不读此字段**——默认值 `true` 不代表模糊匹配已可用。
    pub fuzzy: bool,
    /// 启用拼音匹配 tier T2（默认 true，见 DESIGN §8.1）。
    /// **预留字段，M5 生效**：T2 属 M5（DESIGN §8.1 / §13），`ff-match` 尚无拼音匹配器，
    /// `InProcessEngine::search` **当前不读此字段**——默认值 `true` 不代表拼音匹配已可用。
    pub pinyin: bool,
    /// 隐藏系统文件（DESIGN §5.2：`$MFT` 等元数据默认可在设置中隐藏）。
    /// 只过滤 `flags::SYSTEM`（`FILE_ATTRIBUTE_SYSTEM`）；不过滤隐藏属性。
    pub hide_system: bool,
    /// 隐藏带隐藏属性的条目（`flags::HIDDEN` / `FILE_ATTRIBUTE_HIDDEN`）。
    /// 与 `hide_system` 是两个独立开关：M4 若把 UI 拆成两个 toggle，各映射一个即可，
    /// 引擎不必再改。当前 UI 是单个「显示系统/隐藏文件」toggle，同时驱动这两个字段
    /// （DESIGN §11.3）。
    pub hide_hidden: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            case_sensitive: false,
            regex_default: false,
            fuzzy: true,
            pinyin: true,
            hide_system: true,
            hide_hidden: true,
        }
    }
}

/// 搜索请求（DESIGN §9，契约冻结）。
#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub query: String,
    pub sort: SortSpec,
    /// 评分排序时的 Top-K 窗口（None = 返回全部命中；DESIGN §8.2 默认 10,000）。
    pub max_hits: Option<usize>,
    pub opts: SearchOptions,
}

/// 搜索结果摘要（DESIGN §9）。`search` 必须尽快返回；语法错误通过
/// `parse_error` 返回而不是 `Err`。
#[derive(Clone, Debug)]
pub struct SearchSummary {
    pub id: RequestId,
    pub total: usize,
    pub elapsed_ms: u32,
    pub parse_error: Option<ParseError>,
}

/// 结果行（DESIGN §9）。只包含取数窗口内的行；昂贵信息（高亮等）只在 `range` 中计算。
#[derive(Clone, Debug)]
pub struct ResultRow {
    pub name: String,
    /// 完整父路径（如 `D:\Work\Reports`；根目录条目的 dir 为 `D:\`）。
    pub dir: String,
    pub is_dir: bool,
    /// None = 元数据未补齐（DESIGN §4.3 `STALE_META`，M2 增量条目）。
    pub size: Option<u64>,
    /// FILETIME（100ns，1601 纪元）；None = 未补齐。IPC/UI 边界再转换。
    pub mtime: Option<i64>,
    /// 名字内命中区间（UTF-8 字节区间，相对 `name`），供前端高亮；
    /// 仅为取数窗口内的行计算。转前端索引在 IPC 层（M6）处理。
    pub name_matches: Vec<Range<u32>>,
}

/// 对结果行执行的动作（DESIGN §9 `open`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAction {
    /// 打开（由 UI 进程以当前用户身份执行，DESIGN §12）。
    Open,
    /// 打开所在文件夹并选中。
    OpenFolder,
    /// 复制完整路径。
    CopyPath,
}

/// 引擎错误。
#[derive(Debug)]
pub enum EngineError {
    /// 请求 id 不存在或已过期（会话命中集合已释放，DESIGN §9：60 秒无访问释放）。
    NotFound,
    /// 搜索被取消。
    Cancelled,
    Io(String),
    Internal(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NotFound => write!(f, "请求不存在或已过期"),
            EngineError::Cancelled => write!(f, "搜索已取消"),
            EngineError::Io(m) => write!(f, "IO 错误: {m}"),
            EngineError::Internal(m) => write!(f, "内部错误: {m}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// 本模块的 Result 别名（对应 DESIGN §9 的签名形状）。
pub type Result<T> = std::result::Result<T, EngineError>;

/// 卷状态（DESIGN §9 `EngineStatus`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeState {
    /// 正在扫描 / 建索引。
    Scanning,
    Ready,
    Error,
}

/// 单卷状态。
#[derive(Clone, Debug)]
pub struct VolumeStatus {
    pub volume: VolumeId,
    pub state: VolumeState,
    /// 该卷已索引条目数。
    pub entries: u64,
    /// Scanning 时的进度（0.0–1.0）；其他状态为 None。
    pub progress: Option<f32>,
    /// Error 时的说明；其他状态为 None。
    pub message: Option<String>,
}

/// 引擎状态（DESIGN §9 `status`：各卷状态、条目数、最后保存时间）。
#[derive(Clone, Debug)]
pub struct EngineStatus {
    pub volumes: Vec<VolumeStatus>,
    /// 各卷条目数之和。
    pub total_entries: u64,
    /// 最近一次成功保存索引的时间（FILETIME；从未保存为 None）。
    pub last_saved: Option<i64>,
    /// 当前是否有搜索在进行。
    pub searching: bool,
}

/// 引擎客户端（DESIGN §9，契约冻结）。UI 只依赖本 trait。
pub trait EngineClient: Send + Sync {
    /// 发起搜索；自动取消同一客户端的上一次搜索（DESIGN §2.2-6）。
    ///
    /// 语法错误不返回 `Err`，而是在 [`SearchSummary::parse_error`] 中带回。
    fn search(&self, req: SearchRequest) -> Result<SearchSummary>;

    /// 取结果窗口的行（虚拟滚动按 `offset/limit` 调用，DESIGN §2.2-5）。
    fn range(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>>;

    /// 取消指定请求并释放其命中集合。
    fn cancel(&self, id: RequestId);

    /// 引擎状态（各卷 Scanning(进度) / Ready / Error、条目数、最后保存时间）。
    fn status(&self) -> EngineStatus;

    /// 重建索引（None = 所有卷）。
    fn rescan(&self, volume: Option<VolumeId>) -> Result<()>;

    /// 对结果行执行动作（打开 / 打开所在文件夹 / 复制路径）。
    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()>;
}
