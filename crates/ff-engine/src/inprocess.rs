//! `EngineClient` 的 InProcess 实现（DESIGN §9，M1–M2）。
//!
//! 直接用 `ff-core` 的 [`VolumeIndex`] 承接 `ff-ntfs` 的 [`RawEntry`]，在进程内
//! 打通 `search` / `range`（含排序与 `ResultRow` 构造）/ `cancel` / `status` /
//! `rescan` / `open`。Windows 专有的 `ShellExecuteW` 与剪贴板 FFI 由
//! `#[cfg(windows)]` 隔离，非 Windows 分支保留可编译的显式回退，
//! 保证 `ff-engine` 跨平台可编译。
//!
//! 多卷映射：每条命中记录为 `(VolumeId, Hit)`。排序与 `range` 时据此回到对应
//! 卷的 `VolumeIndex` 取字段，天然支持多卷并集。
//!
//! **M2（②/③/④）**：每个卷在 Base（`VolumeIndex`）+ Overlay（增量层）之上工作；
//! 后台线程轮询 `ff-ntfs::VolumeScanner::read_changes` 把变更写入 Overlay，
//! `search` 时合并 Base+Overlay；`read_changes` 返回 `JournalUnavailable` 即触发
//! 全量重建（③）；结果集 60 秒无访问释放（④）；周期性落盘（`VolumeIndex::save`）
//! 并通过 `status().last_saved` 暴露（②）。

use std::collections::HashMap;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ff_core::{
    attrs, eval::EntryFacts, flags, highlight_name, match_overlay_name, parse, CancelToken,
    EntryId, EntryInput, IndexBuilder, MatchOptions, Query, ScoreConfig, Tier, VolumeIndex,
    VolumeInfo, NONE,
};
use ff_ntfs::{RawEntry, ScanError, VolumeId, VolumeScanner};

use crate::api::{
    EngineClient, EngineError, EngineStatus, OpenAction, RequestId, Result, ResultRow,
    SearchRequest, SearchSummary, SortKey, SortSpec, VolumeState, VolumeStatus,
};
use crate::overlay::{Overlay, OverlayEntry};

/// 锁中毒容错（#10）：持锁线程一旦 panic，`std` 会把锁标记为中毒，后续所有
/// `.lock_or_recover()` 全部连锁 panic——一次局部故障被放大成整个引擎不可用。
///
/// 引擎持锁区内的临界区都很短（取快照 / 换入结果），panic 后内部值仍是**上一次
/// 完整写入**的状态，取回它继续服务比整进程崩溃安全。因此统一用下面三个方法。
trait LockRecover<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `RwLock` 的中毒容错版本（读 / 写各一个）。
trait RwLockRecover<T> {
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T>;
    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> RwLockRecover<T> for RwLock<T> {
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(|e| e.into_inner())
    }
    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// Overlay 合并阈值（DESIGN §4.3：增量条目超过此数即落入 Base）。
const MERGE_THRESHOLD: usize = 50_000;
/// 结果集无访问释放时限（DESIGN §9：60 秒）。
const RESULT_TTL: Duration = Duration::from_secs(60);
/// 无增量通道的卷（目录遍历回退，DESIGN §5.4）的全量重走间隔。
///
/// 这类卷没有 USN，`read_changes` 只会返回 `Unsupported`；若仍按 100ms 轮询，
/// 每轮都是白读一次错误。改为每 `30s` 全量重走一遍：U 盘 / 存储卡上的增删
/// 在 30 秒内可见，且不会像「每次轮询都重扫」那样把 scanner 锁占死。
const WALK_REBUILD_INTERVAL: Duration = Duration::from_secs(30);

/// 无增量卷是否到了该重走的时刻（单独提出来便于边界单测，不必真等 30 秒）。
fn walk_due(since_full: Duration) -> bool {
    since_full >= WALK_REBUILD_INTERVAL
}

/// 单卷运行时数据（M2：新增 `overlay` 增量层；P1：新增 `message` / `scan_progress`）。
struct VolumeData {
    info: VolumeInfo,
    index: VolumeIndex,
    overlay: Overlay,
    state: VolumeState,
    /// Error 时的说明（P1-3：打通错误信息链路，status_impl 不再硬编码 None）。
    message: Option<String>,
    /// 扫描进度（千分比 0–1000；`u64::MAX` = 非扫描态/不确定）。
    /// 用 Arc<AtomicU64> 使 sink 闭包（无 volumes 锁）能原子更新，status_impl 无锁读取。
    scan_progress: Arc<AtomicU64>,
    /// 本卷是否有 USN 增量通道（扫描成功时由 `scanner.supports_incremental` 决定）。
    /// `false` 的卷（非 NTFS 目录遍历）走 [`WALK_REBUILD_INTERVAL`] 定期重走。
    incremental: bool,
    /// 最近一次全量扫描**完成**的时刻（定期重走的计时起点）。
    last_full: Instant,
    /// 索引代次：每次 Base 索引被整体替换（全量扫描 / Overlay 合并 / 退出合并）
    /// 自增一次。搜索结果集里存的是搜索时的代次快照，`range` / `open` 据此判断
    /// 「这一批 Hit 的 EntryId 是否仍指向同一个文件」——换代后旧 eid 会错位甚至
    /// 越界，必须让旧结果集失效而不是拿新索引去解读（#21）。
    generation: u64,
}

/// 一次搜索的持久结果：命中列表（全局有序，每条带所属卷 + 命中来源）+ 原查询 +
/// 本次搜索的 M5 开关（供 `range` 高亮时复用同一套 T2/T3 判定）。
struct StoredResult {
    hits: Vec<(VolumeId, Hit)>,
    /// 搜索时各卷的索引代次快照。`range` / `open` 用它与当前 `VolumeData::generation`
    /// 比对：不一致说明索引已换代，`Hit::Base(eid)` 指向的条目可能已错位（#21）。
    generations: HashMap<VolumeId, u64>,
    query: Query,
    mopts: MatchOptions,
}

#[derive(Default)]
struct SessionState {
    results: HashMap<RequestId, Arc<StoredResult>>,
    result_access: HashMap<RequestId, Instant>,
    cancels: HashMap<RequestId, CancelToken>,
}

/// 命中来源（Base 条目或 Overlay 增量条目）。
#[derive(Clone, Copy)]
enum Hit {
    /// Base（`VolumeIndex`）中的条目，存其 `EntryId`。
    Base(EntryId),
    /// Overlay 增量条目，存其 `frn`。
    Overlay(u64),
}

/// 引擎内部共享状态（被 `Arc` 包裹，供后台线程与应用线程并发访问）。
struct EngineInner {
    /// 各卷索引 + 增量层（被 trait 的 `&self` 方法并发读写，用 `RwLock` 保护）。
    volumes: RwLock<HashMap<VolumeId, VolumeData>>,
    /// 扫描器（Windows 下由 `NtfsVolumeScanner` 提供；测试 / 合成数据可留空）。
    scanner: Mutex<Option<Box<dyn VolumeScanner + Send + Sync>>>,
    /// 每个客户端会话独立持有结果、TTL 和取消令牌；索引仍由 `volumes` 共享。
    sessions: Mutex<HashMap<u64, SessionState>>,
    session_counter: AtomicU64,
    /// 请求计数器（从 1 起，0 视为无效）。
    request_counter: AtomicU64,
    /// 是否正在搜索（`status().searching` 来源）。
    searching: AtomicBool,
    /// 最近一次成功保存索引的时间（FILETIME；从未保存为 `None`）。
    last_saved: Mutex<Option<i64>>,
    /// 落盘目录（`None` 表示不持久化）。
    save_dir: Mutex<Option<PathBuf>>,
    /// 后台线程退出标志。
    bg_running: AtomicBool,
    /// 索引是否需要落盘（P0-5：只在 Base 变化时落盘，而非每 10s 无条件重写）。
    dirty: AtomicBool,
    /// 最近一次落盘失败原因（透传给 UI；None = 未失败 / 未尝试）。
    save_error: Mutex<Option<String>>,
}

impl EngineInner {
    fn new(scanner: Option<Box<dyn VolumeScanner + Send + Sync>>) -> Self {
        EngineInner {
            volumes: RwLock::new(HashMap::new()),
            scanner: Mutex::new(scanner),
            sessions: Mutex::new(HashMap::from([(1, SessionState::default())])),
            session_counter: AtomicU64::new(2),
            request_counter: AtomicU64::new(1),
            searching: AtomicBool::new(false),
            last_saved: Mutex::new(None),
            save_dir: Mutex::new(None),
            bg_running: AtomicBool::new(true),
            dirty: AtomicBool::new(false),
            save_error: Mutex::new(None),
        }
    }

    /// 直接注入一个已建好的索引（测试与合成数据用；标记为 `Ready`）。
    fn add_volume(&self, vol: VolumeId, index: VolumeIndex) {
        let info = index.volume.clone();
        let mut vols = self.volumes.write_or_recover();
        // P0-5：直接注入的索引视为「已就绪、可能需落盘」（测试持久化 / 合成数据）。
        self.dirty.store(true, Ordering::SeqCst);
        vols.insert(
            vol,
            VolumeData {
                info,
                index,
                overlay: Overlay::default(),
                state: VolumeState::Ready,
                message: None,
                scan_progress: Arc::new(AtomicU64::new(u64::MAX)),
                // 直接注入的索引（测试 / 合成数据）默认按「有增量」处理，
                // 与真实 NTFS 卷一致；测试若要覆盖无增量分支可用 `set_incremental`。
                incremental: true,
                last_full: Instant::now(),
                generation: 0,
            },
        );
    }

    /// 经扫描器全量重建单卷索引（见 trait `rescan`，M2-③ 重建也走这里）。
    ///
    /// P1 修复：① 错误信息写入 `vd.message`（status_impl 不再硬编码 None）；
    /// ② 扫描进度经 `scan_progress` 原子量节流上报（100ms 级，不每文件推送）。
    fn load_volume_with(&self, scanner: &dyn VolumeScanner, vol: &VolumeId) -> Result<()> {
        // 先标记 Scanning，取出 progress atomic 和上一轮条目数作为进度分母。
        let (progress_arc, total_estimate) = {
            let mut vols = self.volumes.write_or_recover();
            let vd = vols.entry(vol.clone()).or_insert_with(|| blank_volume(vol));
            vd.state = VolumeState::Scanning;
            vd.message = None;
            vd.scan_progress.store(0, Ordering::Relaxed);
            let total = vd.index.entry_count() as u64;
            (vd.scan_progress.clone(), total)
        };

        let info = VolumeInfo {
            serial: vol.serial,
            drive: vol.drive,
            ..Default::default()
        };
        let mut builder = IndexBuilder::new(info.clone());
        let mut count: u64 = 0;
        let mut last_report = Instant::now();

        // 流式喂入 RawEntry → EntryInput；节流更新进度原子量。
        let res = scanner.full_scan(vol, &mut |e: RawEntry| {
            builder.push(EntryInput {
                frn: e.frn,
                parent_frn: e.parent_frn,
                name: e.name,
                is_dir: e.is_dir,
                attrs: e.attrs,
                size: e.size,
                mtime: e.mtime,
                ctime: e.ctime,
                atime: e.atime,
            });
            count += 1;
            // 节流：每 100ms 更新一次（IPC 轮询 1s 一次，100ms 粒度足够平滑）。
            if total_estimate > 0 && last_report.elapsed() >= Duration::from_millis(100) {
                last_report = Instant::now();
                let permille = (count * 1000 / total_estimate).min(999);
                progress_arc.store(permille, Ordering::Relaxed);
            }
        });

        match res {
            Ok(meta) => {
                progress_arc.store(u64::MAX, Ordering::Relaxed);
                let index = builder.finish();
                // 本卷有没有 USN 增量由扫描器说了算（目录遍历回退 = false）。
                // 在拿 `volumes` 写锁之前问：`AutoVolumeScanner` 的判定要走一次
                // `GetVolumeInformationW`，不能持锁做 IO（DESIGN §0.6 / 锁序）。
                let incremental = scanner.supports_incremental(vol);
                let mut vols = self.volumes.write_or_recover();
                let vd = vols
                    .get_mut(vol)
                    .expect("load_volume_with: 卷在 Scanning 阶段已插入");
                // Bug #6 修复：必须接住 `full_scan` 返回的 `ScanMeta`。
                // 旧代码丢弃 meta 只写 `info`（journal_id=0 / next_usn=0），于是下一轮
                // `poll_volume` 以 `from_usn = 0` 调 `read_changes`；真实卷 `first_usn > 0`，
                // `scanner.rs` 判定「记录已回收」恒返回 `JournalUnavailable` → `rebuild_volume`
                // → 再全量扫描 → 无限循环重建（M2 的 USN 增量在真实运行中从未生效）。
                //
                // DESIGN §5.2：full_scan 在扫 MFT **之前**记录的 next_usn 即续读起点，
                // 扫描期间发生的变化由后续从该 USN 续读补齐，故这里直接采用 meta 值。
                // `guid_path` 不在 `VolumeId` 里，从旧 info 继承（否则无盘符卷的路径前缀会丢）。
                let guid_path = std::mem::take(&mut vd.info.guid_path);
                vd.info = VolumeInfo {
                    serial: vol.serial,
                    drive: vol.drive,
                    guid_path,
                    journal_id: meta.journal_id,
                    next_usn: meta.next_usn,
                };
                // 把权威续读起点同步进 index.volume，再赋给 vd.index：
                // 任何直接 index.save() 的路径（或旧版本代码）都不会把
                // next_usn 写回成 0。vd.info 与 index.volume 保持同一份值。
                let mut index = index;
                index.volume = vd.info.clone();
                vd.index = index;
                vd.overlay = Overlay::default();
                // #21：Base 整体替换 → 代次自增，旧结果集在 range/open 时判失效。
                vd.generation = vd.generation.wrapping_add(1);
                vd.state = VolumeState::Ready;
                vd.message = None;
                vd.incremental = incremental;
                vd.last_full = Instant::now();
                // P0-5：索引已变，标记需落盘。
                self.dirty.store(true, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                progress_arc.store(u64::MAX, Ordering::Relaxed);
                let msg = e.to_string();
                let mut vols = self.volumes.write_or_recover();
                if let Some(vd) = vols.get_mut(vol) {
                    vd.state = VolumeState::Error;
                    vd.message = Some(msg);
                }
                Err(map_scan_err(e))
            }
        }
    }

    fn search_impl(&self, session: u64, req: SearchRequest) -> Result<SearchSummary> {
        let start = Instant::now();
        let id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        self.searching.store(true, Ordering::SeqCst);
        // 新搜索覆盖上一次（单客户端语义）
        {
            let mut sessions = self.sessions.lock_or_recover();
            let state = sessions.entry(session).or_default();
            state.results.clear();
            state.result_access.clear();
        }
        let token = CancelToken::new();
        {
            let mut sessions = self.sessions.lock_or_recover();
            let cancels = &mut sessions.entry(session).or_default().cancels;
            cancels.insert(id, token.clone());
            // P2：清理过期令牌（id 单调递增，只保留最新一个）。
            cancels.retain(|k, _| *k >= id);
        }

        // 解析查询；语法错误不返回 Err，写入 parse_error
        let parsed = match parse(&req.query) {
            Ok(q) => q,
            Err(e) => {
                self.searching.store(false, Ordering::SeqCst);
                return Ok(SearchSummary {
                    id,
                    total: 0,
                    elapsed_ms: start.elapsed().as_millis() as u32,
                    parse_error: Some(e),
                    content: None,
                });
            }
        };
        let mut query = parsed;
        query.case_sensitive = req.opts.case_sensitive;
        // 日期按**本地时区**解释（DESIGN §7.2）：`ff-core` 拿不到时区偏移，
        // 这里把 Win32 取到的真实值注入求值上下文（非 Windows 保持 0 = UTC）。
        #[cfg(windows)]
        {
            query.context.utc_offset_minutes = ff_ntfs::win32::local_utc_offset_minutes();
        }

        // M5：匹配开关由 UI 的 fuzzy / pinyin 驱动，评分权重集中在 `ScoreConfig`。
        let mopts = MatchOptions {
            fuzzy: req.opts.fuzzy,
            pinyin: req.opts.pinyin,
        };
        let score_cfg = ScoreConfig::default();

        // 在所有 Ready 卷上求并集命中：Base + Overlay 合并
        let hide_system = req.opts.hide_system;
        let hide_hidden = req.opts.hide_hidden;
        // 三元组：所属卷 + 命中来源 + M5 评分（SortKey::Score 的排序键）。
        let mut hits: Vec<(VolumeId, Hit, f64)> = Vec::new();
        // #21：与命中同批采集各卷索引代次，随结果集保存，供 range/open 校验。
        let mut generations: HashMap<VolumeId, u64> = HashMap::new();
        // M7：`content:` 的统计（无内容条件时为 None）。
        let mut content_report: Option<ff_core::ContentReport> = None;
        {
            let vols = self.volumes.read_or_recover();
            for (vid, vd) in vols.iter() {
                // P0-1：旧索引正在后台刷新（`Scanning` 但已有条目）的卷仍应参与搜索；
                // 只有「首次建索引、毫无可用索引」的卷（`Scanning` 且 `entries==0`）
                // 才跳过。这样启动 / 重扫期间旧索引立即可搜，不会误报「没有找到」。
                let searchable = vd.state == VolumeState::Ready
                    || (vd.state == VolumeState::Scanning && vd.index.entry_count() > 0);
                if !searchable {
                    continue;
                }
                generations.insert(vid.clone(), vd.generation);
                // —— Base 命中：M5 分层搜索（T1 子串 / T2 拼音 / T3 模糊）+ 评分 ——
                let ranked = vd.index.search_ranked(&query, &mopts, &score_cfg, &token);
                // T3 稀疏触发条件（DESIGN §8.1）：Base 上 T1+T2 命中 < 100 时，
                // 才让 Overlay 增量条目也参与模糊匹配。
                let allow_fuzzy = ranked.needle.is_some()
                    && ranked.hits.iter().filter(|h| h.tier != Tier::Fuzzy).count() < 100;
                for rh in &ranked.hits {
                    let eid = rh.id;
                    // DESIGN §5.2：系统文件"默认可在设置中隐藏"——`hide_system` 在此生效。
                    // 两个开关独立判断，不合并成一个掩码常量，便于 M4 按 UI 拆分成两个 toggle。
                    let f = vd.index.flags[eid as usize];
                    if hide_system && f & flags::SYSTEM != 0 {
                        continue;
                    }
                    if hide_hidden && f & flags::HIDDEN != 0 {
                        continue;
                    }
                    let frn = vd.index.frn[eid as usize];
                    // Overlay 已删除 → 从结果剔除
                    if vd.overlay.deleted.contains(&frn) {
                        continue;
                    }
                    // Overlay 已覆盖（改名 / 移动）→ Base 版本失效，改由 Overlay 命中呈现
                    if vd.overlay.added_or_changed.contains_key(&frn) {
                        continue;
                    }
                    hits.push((vid.clone(), Hit::Base(eid), rh.score));
                }
                // —— Overlay 增量命中 ——
                // P1-2：仅当查询用到 path:/parent:/infolder:/depth: 时才为每条
                // 增量现算父名 / 完整路径 / 深度（这些分配在大 Overlay 上成本很高）；
                // 普通名字子串查询传 None，省掉这部分开销。
                let need_path = query.needs_path_context();
                for (frn, oe) in vd.overlay.added_or_changed.iter() {
                    let f = oe.flags();
                    if hide_system && f & flags::SYSTEM != 0 {
                        continue;
                    }
                    if hide_hidden && f & flags::HIDDEN != 0 {
                        continue;
                    }
                    // 路径上下文（若需要）提升到外层作用域，确保 `facts` 借用时仍存活。
                    let (parent_name, path, depth) = if need_path {
                        overlay_context(&vd.index, oe)
                    } else {
                        (None, None, 0u32)
                    };
                    let facts = EntryFacts {
                        name: &oe.name,
                        is_dir: oe.is_dir,
                        attrs: oe.attrs,
                        size: oe.size,
                        mtime: oe.mtime,
                        ctime: oe.ctime,
                        atime: oe.atime,
                        parent_name: parent_name.as_deref(),
                        path: path.as_deref(),
                        depth,
                    };
                    // T1 由完整 AST 判定；未命中时按 M5 分层追加 T2 / T3，
                    // 与 Base 共用 `match_overlay_name`，避免增量条目漏匹配。
                    let t1_hit = overlay_matches(&query, &facts);
                    if let Some((_tier, score)) = match_overlay_name(
                        oe.name.as_bytes(),
                        &query,
                        &mopts,
                        &score_cfg,
                        depth,
                        t1_hit,
                        allow_fuzzy,
                    ) {
                        hits.push((vid.clone(), Hit::Overlay(*frn), score));
                    }
                }
            }
            // M7 内容搜索（DECISIONS #95）：`content:` 不进 AST，名字条件先出
            // 候选集，这里在候选上串行读文件过滤。放在排序前、截断前：
            // 先过滤再 sort，保序且不会让 max_hits 把本该命中的行截掉。
            if !query.content.is_empty() {
                let idx: Vec<EntryId> = (0..hits.len() as EntryId).collect();
                let filter = ff_core::ContentFilter::new(&query.content, &FsContentSource);
                let (keep, rep) = filter.filter(
                    &idx,
                    |i| content_path(&vols, &hits, i),
                    |i| content_size(&vols, &hits, i),
                    &token,
                );
                let mut kept = Vec::with_capacity(keep.len());
                for i in keep {
                    if let Some(h) = hits.get(i as usize) {
                        kept.push(h.clone());
                    }
                }
                hits = kept;
                content_report = Some(rep);
            }
            // 排序（M5：Score 用真实评分，其他列用真值；见 `sort_hits`）
            sort_hits(&mut hits, &vols, &req.sort);
        }

        // max_hits 截断（排序后）；评分仅用于排序，存入结果集时丢弃。
        if let Some(k) = req.max_hits {
            hits.truncate(k);
        }
        let hits: Vec<(VolumeId, Hit)> = hits.into_iter().map(|(v, h, _)| (v, h)).collect();

        let total = hits.len();
        {
            let mut sessions = self.sessions.lock_or_recover();
            let state = sessions.entry(session).or_default();
            state.results.insert(
                id,
                Arc::new(StoredResult {
                    hits,
                    generations,
                    query: query.clone(),
                    mopts,
                }),
            );
            state.result_access.insert(id, Instant::now());
        }
        self.searching.store(false, Ordering::SeqCst);
        Ok(SearchSummary {
            id,
            total,
            elapsed_ms: start.elapsed().as_millis() as u32,
            parse_error: None,
            content: content_report,
        })
    }

    fn range_impl(
        &self,
        session: u64,
        id: RequestId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ResultRow>> {
        // 先取窗口命中 + 查询（避免同时持有两个锁导致加锁顺序反转死锁）
        let (window, generations, query, mopts) = {
            let sessions = self.sessions.lock_or_recover();
            let stored = sessions
                .get(&session)
                .and_then(|s| s.results.get(&id))
                .ok_or(EngineError::NotFound)?;
            let len = stored.hits.len();
            if offset >= len {
                return Ok(Vec::new());
            }
            let end = (offset + limit).min(len);
            (
                stored.hits[offset..end].to_vec(),
                stored.generations.clone(),
                stored.query.clone(),
                stored.mopts,
            )
        };
        // 更新访问时间（60 秒无访问释放，DESIGN §9）
        if let Some(state) = self.sessions.lock_or_recover().get_mut(&session) {
            state.result_access.insert(id, Instant::now());
        }

        let vols = self.volumes.read_or_recover();
        let mut rows = Vec::with_capacity(window.len());
        for (vid, hit) in &window {
            // #21：索引换代后旧 `Hit::Base(eid)` 指向的条目已错位（甚至越界）。
            // 代次不符整批作废 → 前端收到 NotFound 后重搜，避免显示出错的行。
            let vd = vols.get(vid).ok_or(EngineError::NotFound)?;
            if generations.get(vid) != Some(&vd.generation) {
                return Err(EngineError::NotFound);
            }
            rows.push(resolve_row(&vols, vid, hit, &query, mopts)?);
        }
        Ok(rows)
    }

    fn cancel_impl(&self, session: u64, id: RequestId) {
        if let Some(tok) = self
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .and_then(|s| s.cancels.get(&id))
        {
            tok.cancel();
        }
    }

    fn status_impl(&self) -> EngineStatus {
        let vols = self.volumes.read_or_recover();
        let mut volumes = Vec::new();
        let mut total_entries = 0u64;
        for (vid, vd) in vols.iter() {
            let entries = vd.index.entry_count() as u64;
            total_entries += entries;
            // P1-4：Scanning 时读原子进度；其他状态为 None。
            let progress = if vd.state == VolumeState::Scanning {
                let p = vd.scan_progress.load(Ordering::Relaxed);
                if p == u64::MAX {
                    None
                } else {
                    Some(p as f32 / 1000.0)
                }
            } else {
                None
            };
            volumes.push(VolumeStatus {
                volume: vid.to_string(),
                state: vd.state,
                entries,
                progress,
                message: vd.message.clone(), // P1-3：不再硬编码 None
                // P0-1：`Scanning` 但有旧索引 → 仍可搜，UI 只提示「正在更新」。
                stale: vd.state == VolumeState::Scanning && entries > 0,
            });
        }
        // P2：卷来自 HashMap 迭代，顺序每次不同会让状态栏每秒闪动、也让前端的
        // 浅比较失效。按盘符（显示名）稳定排序后再返回。
        volumes.sort_by(|a, b| a.volume.cmp(&b.volume));
        EngineStatus {
            volumes,
            total_entries,
            last_saved: *self.last_saved.lock_or_recover(),
            searching: self.searching.load(Ordering::SeqCst),
            save_error: self.save_error.lock_or_recover().clone(),
        }
    }

    fn rescan_impl(&self, volume: Option<VolumeId>) -> Result<()> {
        let guard = self.scanner.lock_or_recover();
        let scanner = match guard.as_ref() {
            Some(s) => s,
            None => return Ok(()), // 无扫描器（测试 / 合成数据）：跳过
        };
        match volume {
            Some(v) => self.load_volume_with(&**scanner, &v),
            None => {
                let ids: Vec<VolumeId> = self.volumes.read_or_recover().keys().cloned().collect();
                for id in ids {
                    // P2：单个卷扫描失败不应中断其余卷（一个 U 盘拔出不影响 C: 重扫）。
                    let _ = self.load_volume_with(&**scanner, &id);
                }
                Ok(())
            }
        }
    }

    fn open_impl(
        &self,
        session: u64,
        id: RequestId,
        index: usize,
        action: OpenAction,
    ) -> Result<()> {
        let (hit, gen) = {
            let sessions = self.sessions.lock_or_recover();
            let stored = sessions
                .get(&session)
                .and_then(|s| s.results.get(&id))
                .ok_or(EngineError::NotFound)?;
            let hit = stored.hits.get(index).ok_or(EngineError::NotFound)?.clone();
            let gen = stored.generations.get(&hit.0).copied();
            (hit, gen)
        };
        if let Some(state) = self.sessions.lock_or_recover().get_mut(&session) {
            state.result_access.insert(id, Instant::now());
        }
        let vols = self.volumes.read_or_recover();
        let vd = vols.get(&hit.0).ok_or(EngineError::NotFound)?;
        // #21：代次不符 → 该 eid 已不是用户看到的那一条（索引换代），拒绝操作。
        if gen != Some(vd.generation) {
            return Err(EngineError::NotFound);
        }
        // #36：两后端统一口径——都按结果行的 name 字段取名，并同样拒绝空名行。
        // 旧实现这里用 `file_name_from_full(&full)` 从完整路径反推，管道侧却直接用
        // `row.name`，空名 / 特殊字符场景两边会给出不同结果。
        let (full, name) = match &hit.1 {
            Hit::Base(eid) => (
                vd.index.path_string(*eid).ok_or(EngineError::NotFound)?,
                vd.index
                    .name_str(*eid)
                    .unwrap_or_default()
                    .to_string(),
            ),
            Hit::Overlay(frn) => match vd.overlay.added_or_changed.get(frn) {
                Some(oe) => (
                    overlay_full_path(&vd.index, &vd.overlay, oe),
                    oe.name.clone(),
                ),
                None => return Err(EngineError::NotFound),
            },
        };
        drop(vols);
        if name.is_empty() {
            // 与 `actions::execute_open_checked` 一致：空名行没有可打开的东西。
            return Err(EngineError::NotFound);
        }
        match action {
            // P0-2：真实写入系统剪贴板。InProcess 引擎运行在 UI 进程内，
            // 由 UI 进程以当前用户身份执行符合 DESIGN §12；不再「静默成功」。
            OpenAction::CopyPath => clipboard_set_text(&full),
            OpenAction::CopyName => clipboard_set_text(&name),
            OpenAction::Open => shell_open(&full, false),
            OpenAction::OpenFolder => shell_open(&full, true),
            OpenAction::Properties => shell_properties(&full),
        }
    }

    // ───────────────────────── 后台轮询与合并（M2-②/③） ─────────────────────────

    /// 轮询所有 Ready 卷的增量变更（后台线程与测试 `poll_once` 共用）。
    fn poll_all(&self) {
        let vols: Vec<VolumeId> = self.volumes.read_or_recover().keys().cloned().collect();
        for vid in vols {
            self.poll_volume(&vid);
        }
    }

    /// 单卷增量轮询：读 `read_changes` → 应用 Overlay；`JournalUnavailable` 触发重建（③）。
    ///
    /// **锁序修复（P0 死锁根因）**：旧代码在持 `volumes.read()` 的同时抢 `scanner` 锁，
    /// 而 `rescan_impl` 持 `scanner` 锁又要 `volumes.write()` → AB-BA 死锁。
    /// 修复：三阶段分离，任何时刻至多持一把锁：
    ///   1. `volumes.read()` 快照 → 释放
    ///   2. `scanner` 锁 → `read_changes` → 释放
    ///   3. `volumes.write()` 应用结果
    fn poll_volume(&self, vid: &VolumeId) {
        // Phase 0：快照轮询参数与「是否有增量通道」，释放 volumes 读锁。
        let (from_usn, journal_id, incremental, since_full) = {
            let vols = self.volumes.read_or_recover();
            let vd = match vols.get(vid) {
                Some(v) => v,
                None => return,
            };
            if vd.state != VolumeState::Ready {
                return;
            }
            (
                vd.info.next_usn,
                vd.info.journal_id,
                vd.incremental,
                vd.last_full.elapsed(),
            )
        }; // volumes.read() 已释放

        // 无增量通道的卷（目录遍历回退，DESIGN §5.4）：没有 USN 可读，
        // 按 WALK_REBUILD_INTERVAL 全量重走一遍来反映增删，别每 100ms 白读一次错误。
        if !incremental {
            if walk_due(since_full) {
                self.rebuild_volume(vid);
            }
            return;
        }

        // Phase 2: 取 scanner 锁执行 IO，释放 scanner。
        let res = {
            let guard = self.scanner.lock_or_recover();
            match guard.as_ref() {
                Some(sc) => sc.read_changes(vid, from_usn, journal_id),
                None => return,
            }
        }; // scanner 已释放

        // Phase 3: 应用结果。
        match res {
            Ok((changes, new_usn)) => {
                // P2：无变更时不持写锁（否则每 1s 轮询都短暂阻塞搜索 / 取行）。
                // 空变更也意味着 USN 位置未推进，下次从同一起点续读即可，无害。
                if changes.is_empty() {
                    return;
                }
                // 先判断本次是否会触发「增量过大 → 重建 Base」合并。合并要重建整份
                // 索引（数十万条、逐条 to_string），绝不能持写锁做（P0-4：否则
                // search/range/status 全被卡住，窗口冻结）。因此合并走「读锁快照 →
                // 锁外重建 → 短暂写锁换入」三阶段。
                let need_merge = {
                    let vols = self.volumes.read_or_recover();
                    match vols.get(vid) {
                        Some(vd) => vd.overlay.len() + changes.len() > MERGE_THRESHOLD,
                        None => return,
                    }
                };
                if !need_merge {
                    // 常见路径：增量未达阈值，短暂持写锁应用即可（廉价，不重建索引）。
                    let mut vols = self.volumes.write_or_recover();
                    if let Some(vd) = vols.get_mut(vid) {
                        vd.overlay.apply(&changes, &vd.index);
                        vd.info.next_usn = new_usn;
                    }
                    return;
                }
                // 合并路径（昂贵）：读锁取快照（index + overlay + 代次 usn）→ 释放
                // 读锁 → 锁外重建 merged → 短暂写锁换入并校验代次。
                let (snap_index, snap_overlay, snap_usn) = {
                    let vols = self.volumes.read_or_recover();
                    let vd = match vols.get(vid) {
                        Some(v) => v,
                        None => return,
                    };
                    // 读锁下克隆：RwLock 允许多读者，此克隆不阻塞 search/range/status；
                    // 仅阻塞其他写者（rescan 等），可接受。
                    (vd.index.clone(), vd.overlay.clone(), vd.info.next_usn)
                }; // 读锁释放
                let mut overlay = snap_overlay;
                overlay.apply(&changes, &snap_index);
                let merged = merge_overlay_into_base(&snap_index, &overlay);
                let mut vols = self.volumes.write_or_recover();
                if let Some(vd) = vols.get_mut(vid) {
                    // 代次校验：若期间被 rescan 替换（index 已变、usn 已推进），
                    // 丢弃本次合并结果，交给 rescan 产生的新索引。
                    if vd.state == VolumeState::Ready && vd.info.next_usn == snap_usn {
                        vd.index = merged;
                        vd.overlay = Overlay::default();
                        // #21：合并即换代，旧结果集的 eid 不再可信。
                        vd.generation = vd.generation.wrapping_add(1);
                        vd.info.next_usn = new_usn;
                        // P0-5：Base 已变，标记需落盘。
                        self.dirty.store(true, Ordering::SeqCst);
                    }
                }
            }
            // M2-③：日志被重建 / 记录被回收 → 全量重建
            Err(ScanError::JournalUnavailable(_)) => {
                self.rebuild_volume(vid);
            }
            Err(_) => { /* 其他错误忽略（真实场景记入诊断） */ }
        }
    }

    /// 触发单卷全量重建（不持锁，内部自行加锁）。
    fn rebuild_volume(&self, vid: &VolumeId) {
        let guard = self.scanner.lock_or_recover();
        if let Some(sc) = guard.as_ref() {
            let _ = self.load_volume_with(&**sc, vid);
        }
    }

    // ───────────────────────── 落盘与回收（M2-②/④） ─────────────────────────

    /// 落盘（P0-5：只在 Base 变化时落盘，而非每 10s 无条件重写字节完全相同的索引）。
    ///
    /// 仅在 `dirty` 为 true 时执行；执行后清零 `dirty`。写盘失败写入 `save_error`
    /// 透传给 UI（旧代码 `let _ =` 吞掉，C 盘满时连「写入失败」都看不到）。
    fn maybe_save(&self) {
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let dir = self.save_dir.lock_or_recover().clone();
        let dir = match dir {
            Some(d) => d,
            None => return,
        };
        let ids: Vec<VolumeId> = self.volumes.read_or_recover().keys().cloned().collect();
        let mut err: Option<String> = None;
        for id in ids {
            if let Err(e) = self.save_volume(&id, &dir) {
                err = Some(e.to_string());
            }
        }
        *self.save_error.lock_or_recover() = err;
    }

    /// 结果集 60 秒无访问即释放（DESIGN §9 / #54）。
    fn reap_stale(&self) {
        let now = Instant::now();
        let mut sessions = self.sessions.lock_or_recover();
        for state in sessions.values_mut() {
            let stale: Vec<_> = state
                .result_access
                .iter()
                .filter_map(|(id, t)| (now.duration_since(*t) > RESULT_TTL).then_some(*id))
                .collect();
            for id in stale {
                state.results.remove(&id);
                state.result_access.remove(&id);
            }
        }
    }

    fn save_volume(&self, vid: &VolumeId, dir: &Path) -> io::Result<()> {
        let vols = self.volumes.read_or_recover();
        let vd = match vols.get(vid) {
            Some(v) => v,
            None => return Ok(()),
        };
        // Bug #7 修复：只落盘 `Ready` 卷。Scanning / Error 态的索引是空壳（120 B），
        // 若让它覆盖磁盘上的好索引，进程在重扫完成前死亡就会永久丢失索引。
        if vd.state != VolumeState::Ready {
            return Ok(());
        }
        let path = dir.join(format!("{}.ffix", vd.info.serial));
        // 用运行时权威 info 落盘：它携带推进后的 journal_id / next_usn。
        // vd.index.volume 只是全扫那一刻的快照（next_usn 常为 0），若直接
        // 用 index.save() 会让磁盘索引的续读起点永远停在旧值，重启后回放 /
        // 全量重建，磁盘缓存形同虚设（见 I 组索引复用缺陷）。
        vd.index.save_with_volume(&path, &vd.info)?;
        *self.last_saved.lock_or_recover() = Some(now_filetime());
        Ok(())
    }

    fn save_all(&self) -> io::Result<()> {
        let dir = self.save_dir.lock_or_recover().clone();
        let dir = match dir {
            Some(d) => d,
            None => return Ok(()),
        };
        let ids: Vec<VolumeId> = self.volumes.read_or_recover().keys().cloned().collect();
        for id in ids {
            self.save_volume(&id, &dir)?;
        }
        Ok(())
    }

    /// 退出前落盘（P0-5）：先把 Overlay 合并进 Base，再整盘落盘。
    ///
    /// 避免「退出时只存了 Base、Overlay 增量丢失」——下次启动这些增量要靠 USN
    /// 回放补齐，若 USN 起点未随索引落盘（P1-7）就会被迫全量重扫。
    fn flush(&self) {
        {
            let mut vols = self.volumes.write_or_recover();
            for vd in vols.values_mut() {
                if vd.state == VolumeState::Ready && !vd.overlay.is_empty() {
                    let merged = merge_overlay_into_base(&vd.index, &vd.overlay);
                    vd.index = merged;
                    vd.overlay = Overlay::default();
                    // #21：退出前合并同样换代（进程即将结束，此处只是保持口径一致）。
                    vd.generation = vd.generation.wrapping_add(1);
                }
            }
        }
        if let Err(e) = self.save_all() {
            *self.save_error.lock_or_recover() = Some(e.to_string());
        }
    }

    /// 把一批卷置为「待扫描」（见 [`InProcessEngine::mark_pending`]）。
    fn mark_pending(&self, vols: &[VolumeId]) {
        let mut map = self.volumes.write_or_recover();
        for vol in vols {
            let vd = map.entry(vol.clone()).or_insert_with(|| blank_volume(vol));
            vd.state = VolumeState::Scanning;
            vd.message = None;
            vd.scan_progress.store(0, Ordering::Relaxed);
        }
    }

    fn load_all(&self, dir: &Path) -> io::Result<()> {
        // 三阶段，避免持 volumes 写锁时做 IO（GetVolumeInformationW）/ 与 scanner
        // 锁形成 AB-BA：① 读盘收集；② 锁外查增量通道；③ 一次性插入。
        let mut loaded: Vec<(VolumeId, VolumeIndex)> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ffix") {
                continue;
            }
            if let Some(index) = VolumeIndex::load(&path) {
                let vid = VolumeId {
                    serial: index.volume.serial,
                    drive: index.volume.drive,
                };
                loaded.push((vid, index));
            }
        }

        // 增量通道在锁外一次性查好。冷启动不再强制全量 rescan，所以这里必须把
        // load_all 原先硬编码的 `incremental: true` 覆盖成真实值：非 NTFS 卷若
        // 误判为有 USN，后台会一直空读 Unsupported，永不走定期重走而停在旧索引。
        let incremental_flags: HashMap<VolumeId, bool> = {
            let guard = self.scanner.lock_or_recover();
            match guard.as_ref() {
                Some(sc) => loaded
                    .iter()
                    .map(|(vid, _)| (vid.clone(), sc.supports_incremental(vid)))
                    .collect(),
                None => HashMap::new(),
            }
        };

        let loaded_count = loaded.len();
        {
            let mut vols = self.volumes.write_or_recover();
            for (vid, index) in loaded {
                let incremental = incremental_flags.get(&vid).copied().unwrap_or(true);
                vols.insert(
                    vid,
                    VolumeData {
                        info: index.volume.clone(),
                        index,
                        overlay: Overlay::default(),
                        state: VolumeState::Ready,
                        message: None,
                        scan_progress: Arc::new(AtomicU64::new(u64::MAX)),
                        incremental,
                        last_full: Instant::now(),
                        generation: 0,
                    },
                );
            }
        }
        // E5：只在真的载入了索引时才标记"已保存"；全新安装 0 个 .ffix 时保持 None，
        // 避免状态栏显示误导性的"已保存 HH:MM"。
        if loaded_count > 0 {
            *self.last_saved.lock_or_recover() = Some(now_filetime());
        }
        Ok(())
    }

    /// 启动对账：返回磁盘上无可复用索引、必须全量重建的卷。
    ///
    /// `load_all` 已把可续读的磁盘索引置为 `Ready`；这些卷保持原样，由后台
    /// `poll_volume` 从 `next_usn` 续读关机期间的变更（journal 失效会自动重建，
    /// 见 `poll_volume` 的 `JournalUnavailable` 分支）。只有新卷 / 空卷 /
    /// 载入失败卷才需要全量扫描，交给调用方 `mark_pending` + `rescan`。
    fn startup_reconcile(&self, available: &[VolumeId]) -> Vec<VolumeId> {
        let vols = self.volumes.read_or_recover();
        available
            .iter()
            .filter(|vid| match vols.get(*vid) {
                Some(vd) => vd.state != VolumeState::Ready || vd.index.entry_count() == 0,
                None => true,
            })
            .cloned()
            .collect()
    }
}

/// `InProcess` 引擎：进程内直接调 `ff-core` / `ff-ntfs`，无服务进程。
///
/// 内部状态经 `Arc` 共享；[`InProcessEngine::new`] 会额外启动一个后台线程轮询
/// 增量变更并节流落盘。测试用 [`InProcessEngine::empty`] 不启动线程，配合
/// [`InProcessEngine::set_scanner`] + [`InProcessEngine::poll_once`] 精确驱动。
pub struct InProcessEngine {
    inner: Arc<EngineInner>,
    /// 后台线程句柄（`empty()` 为 `None`）。
    _bg: Option<JoinHandle<()>>,
}

impl InProcessEngine {
    /// 用给定扫描器构造（真实 Windows 场景），并启动后台增量轮询线程。
    pub fn new(scanner: Box<dyn VolumeScanner + Send + Sync>) -> Self {
        let inner = Arc::new(EngineInner::new(Some(scanner)));
        let bg = {
            let i = inner.clone();
            thread::spawn(move || background_loop(i))
        };
        Self {
            inner,
            _bg: Some(bg),
        }
    }

    /// 无扫描器构造（测试 / 合成数据：通过 [`Self::add_volume`] 直接注入索引）。
    /// 不启动后台线程，便于测试精确驱动 `poll_once`。
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(EngineInner::new(None)),
            _bg: None,
        }
    }

    /// 设置扫描器（测试用，不启动后台线程）。
    pub fn set_scanner(&self, scanner: Box<dyn VolumeScanner + Send + Sync>) {
        *self.inner.scanner.lock_or_recover() = Some(scanner);
    }

    /// 手动触发一次全卷增量轮询（测试用；真实场景由后台线程自动调用）。
    pub fn poll_once(&self) {
        self.inner.poll_all();
    }

    /// 把一批卷标记为「待扫描」（`Scanning` + progress 0），不实际启动扫描。
    ///
    /// 用途：启动时先把本轮要扫的**全部**卷置为待扫，再逐个 `rescan`。
    /// 不这样做的话，还没轮到的卷仍是 `Ready`（索引是从磁盘载入的），前端
    /// `overallScanProgress` 会把它们计入「已完成」，于是每开扫一个新卷该卷
    /// 进度归 0、全局进度被拽回低位 —— 实机表现为「70 多 → 90 多 → 退回 70 多」
    /// 反复回跳。先全部置为待扫后，全局进度在本轮内单调上升到 100%。
    ///
    /// 注意：调用方只应传入**确实会去 rescan** 的卷；否则该卷会停在
    /// `Scanning` 态不恢复。
    pub fn mark_pending(&self, vols: &[VolumeId]) {
        self.inner.mark_pending(vols);
    }

    /// 直接注入一个已建好的索引（测试与合成数据用；标记为 `Ready`）。
    pub fn add_volume(&self, vol: VolumeId, index: VolumeIndex) {
        self.inner.add_volume(vol, index);
    }

    /// 设置落盘目录（持久化开关）。
    pub fn set_save_dir(&self, dir: &Path) {
        *self.inner.save_dir.lock_or_recover() = Some(dir.to_path_buf());
    }

    /// 为一个独立调用方创建会话。会话只隔离请求结果与取消状态，共享同一份卷索引。
    pub fn create_session(&self) -> EngineSession {
        let id = self.inner.session_counter.fetch_add(1, Ordering::SeqCst);
        self.inner.sessions.lock_or_recover().entry(id).or_default();
        EngineSession {
            inner: self.inner.clone(),
            id,
        }
    }

    /// 立即落盘所有卷（忽略节流）。
    pub fn save_all(&self) -> io::Result<()> {
        self.inner.save_all()
    }

    /// 从落盘目录载入所有卷（按 `<serial>.ffix` 文件名）。
    pub fn load_all(&self, dir: &Path) -> io::Result<()> {
        self.inner.load_all(dir)
    }

    /// 启动对账：载入磁盘索引后，返回仍需全量重建的卷（新卷 / 空索引 / 失效卷）。
    ///
    /// 配套用法：`load_all(dir)` → `startup_reconcile(&vids)` → 仅对返回列表
    /// `mark_pending` + `rescan`。已复用的卷保持 `Ready`，由后台续读增量。
    pub fn startup_reconcile(&self, available: &[VolumeId]) -> Vec<VolumeId> {
        self.inner.startup_reconcile(available)
    }

    /// 当前已载入的卷 ID 列表。
    ///
    /// 上层（桌面外壳的 `rescan("C")`）需要把显示用的盘符映射回带 `serial` 的
    /// [`VolumeId`]——`status()` 只给人类可读字符串，不足以构造引擎入参。
    pub fn volume_ids(&self) -> Vec<VolumeId> {
        self.inner
            .volumes
            .read_or_recover()
            .keys()
            .cloned()
            .collect()
    }
}

impl Drop for InProcessEngine {
    fn drop(&mut self) {
        self.inner.bg_running.store(false, Ordering::SeqCst);
        if let Some(h) = self._bg.take() {
            let _ = h.join();
        }
        // P0-5：退出前合并 Overlay 并进 Base 落盘（避免增量丢失 / 下次被迫全量重扫）。
        self.inner.flush();
    }
}

impl EngineClient for InProcessEngine {
    fn search(&self, req: SearchRequest) -> Result<SearchSummary> {
        self.inner.search_impl(1, req)
    }
    fn range(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>> {
        self.inner.range_impl(1, id, offset, limit)
    }
    fn cancel(&self, id: RequestId) {
        self.inner.cancel_impl(1, id);
    }
    fn status(&self) -> EngineStatus {
        self.inner.status_impl()
    }
    fn rescan(&self, volume: Option<VolumeId>) -> Result<()> {
        self.inner.rescan_impl(volume)
    }
    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()> {
        self.inner.open_impl(1, id, index, action)
    }
}

/// 共享索引的独立客户端会话。
pub struct EngineSession {
    inner: Arc<EngineInner>,
    id: u64,
}

impl EngineClient for EngineSession {
    fn search(&self, req: SearchRequest) -> Result<SearchSummary> {
        self.inner.search_impl(self.id, req)
    }
    fn range(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>> {
        self.inner.range_impl(self.id, id, offset, limit)
    }
    fn cancel(&self, id: RequestId) {
        self.inner.cancel_impl(self.id, id);
    }
    fn status(&self) -> EngineStatus {
        self.inner.status_impl()
    }
    fn rescan(&self, volume: Option<VolumeId>) -> Result<()> {
        self.inner.rescan_impl(volume)
    }
    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()> {
        self.inner.open_impl(self.id, id, index, action)
    }
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        self.inner.sessions.lock_or_recover().remove(&self.id);
    }
}

/// 后台循环：轮询增量 → 节流落盘 → 回收过期结果，按 100ms 切片可快速退出。
fn background_loop(inner: Arc<EngineInner>) {
    while inner.bg_running.load(Ordering::SeqCst) {
        inner.poll_all();
        inner.maybe_save();
        inner.reap_stale();
        for _ in 0..10 {
            if !inner.bg_running.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// 空卷（Scanning 占位，供 `or_insert_with`）。
fn blank_volume(vol: &VolumeId) -> VolumeData {
    let info = VolumeInfo {
        serial: vol.serial,
        drive: vol.drive,
        ..Default::default()
    };
    VolumeData {
        info: info.clone(),
        index: IndexBuilder::new(info).finish(),
        overlay: Overlay::default(),
        state: VolumeState::Scanning,
        message: None,
        scan_progress: Arc::new(AtomicU64::new(0)),
        incremental: true,
        last_full: Instant::now(),
        generation: 0,
    }
}

/// 合并 Overlay 增量回 Base（DESIGN §4.3）：Base 跳过已删除 / 已被覆盖条目，
/// 再追加 Overlay 的 upsert，重建出一个全新的 `VolumeIndex`。
fn merge_overlay_into_base(index: &VolumeIndex, overlay: &Overlay) -> VolumeIndex {
    let mut b = IndexBuilder::new(index.volume.clone());
    for eid in 0..index.entry_count() as EntryId {
        let frn = index.frn[eid as usize];
        if overlay.deleted.contains(&frn) {
            continue;
        }
        if overlay.added_or_changed.contains_key(&frn) {
            continue;
        }
        let parent_eid = index.parent[eid as usize];
        let parent_frn = if parent_eid == NONE {
            0
        } else {
            index.frn[parent_eid as usize]
        };
        b.push(EntryInput {
            frn,
            parent_frn,
            name: index.name_str(eid).unwrap_or_default().to_string(),
            is_dir: (index.flags[eid as usize] & flags::IS_DIR) != 0,
            attrs: flags_to_attrs(index.flags[eid as usize]),
            size: index.size[eid as usize],
            mtime: index.mtime[eid as usize],
            ctime: index.ctime[eid as usize],
            atime: index.atime[eid as usize],
        });
    }
    for oe in overlay.added_or_changed.values() {
        b.push(EntryInput {
            frn: oe.frn,
            parent_frn: oe.parent_frn,
            name: oe.name.clone(),
            is_dir: oe.is_dir,
            attrs: oe.attrs,
            size: oe.size,
            mtime: oe.mtime,
            ctime: oe.ctime,
            atime: oe.atime,
        });
    }
    b.finish()
}

/// `flags` 位回推 `Win32` 属性位（合并重建时用；CJK 位不映射回 attrs）。
fn flags_to_attrs(f: u16) -> u32 {
    let mut a = 0u32;
    if f & flags::IS_DIR != 0 {
        a |= attrs::FILE_ATTRIBUTE_DIRECTORY;
    }
    if f & flags::HIDDEN != 0 {
        a |= attrs::FILE_ATTRIBUTE_HIDDEN;
    }
    if f & flags::SYSTEM != 0 {
        a |= attrs::FILE_ATTRIBUTE_SYSTEM;
    }
    if f & flags::READONLY != 0 {
        a |= attrs::FILE_ATTRIBUTE_READONLY;
    }
    a
}

/// `ScanError` → `EngineError`（IO 保留原信息，其余归为 `Internal`）。
fn map_scan_err(e: ScanError) -> EngineError {
    match e {
        ScanError::Io(io) => EngineError::Io(io.to_string()),
        ScanError::VolumeNotFound(v) => EngineError::Internal(format!("卷不存在: {v}")),
        ScanError::NotNtfs { volume, fs_name } => {
            EngineError::Internal(format!("非 NTFS 卷 {volume}（{fs_name}）"))
        }
        ScanError::AccessDenied(m) => EngineError::Internal(format!("权限不足: {m}")),
        ScanError::JournalUnavailable(m) => EngineError::Internal(m),
        ScanError::Unsupported(m) => EngineError::Internal(m),
        ScanError::Other(m) => EngineError::Internal(m),
    }
}

/// 当前 FILETIME（100ns，1601 纪元），用于 `last_saved`。
fn now_filetime() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64 + 11_644_473_600;
    let nsec100 = (dur.subsec_nanos() as i64) / 100;
    secs * 10_000_000 + nsec100
}

/// 按 `SortSpec` 对命中列表排序（Base 与 Overlay 统一处理）。
///
/// M5：`Score` 用 `search_ranked` / `match_overlay_name` 给出的真实评分排序
/// （DESIGN §8.2，高分在前），同分按命中来源 + id 稳定 tie-break；
/// 按列排序仍取真值，语义与 M4 一致。
fn sort_hits(
    hits: &mut [(VolumeId, Hit, f64)],
    vols: &HashMap<VolumeId, VolumeData>,
    sort: &SortSpec,
) {
    hits.sort_by(|a, b| match sort.key {
        SortKey::Score => {
            let ord =
                b.2.partial_cmp(&a.2)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        hit_order_key(vols, &a.0, &a.1).cmp(&hit_order_key(vols, &b.0, &b.1))
                    });
            // ord 本身是「评分降序」；descending=true（默认）即高分在前。
            if sort.descending {
                ord
            } else {
                ord.reverse()
            }
        }
        SortKey::Name => {
            let ord = hit_name(vols, &a.0, &a.1).cmp(&hit_name(vols, &b.0, &b.1));
            if sort.descending {
                ord.reverse()
            } else {
                ord
            }
        }
        SortKey::Path => {
            let ord = hit_path(vols, &a.0, &a.1).cmp(&hit_path(vols, &b.0, &b.1));
            if sort.descending {
                ord.reverse()
            } else {
                ord
            }
        }
        SortKey::Size => {
            let ord = hit_size(vols, &a.0, &a.1).cmp(&hit_size(vols, &b.0, &b.1));
            if sort.descending {
                ord.reverse()
            } else {
                ord
            }
        }
        SortKey::Modified => {
            let ord = hit_mtime(vols, &a.0, &a.1).cmp(&hit_mtime(vols, &b.0, &b.1));
            if sort.descending {
                ord.reverse()
            } else {
                ord
            }
        }
    });
}

/// Score 同分时的稳定 tie-break：（来源序 Base 先于 Overlay，0<1）+ 原 id（eid 或 frn）升序。
/// `vols` / `vid` 保留以备未来扩展（如跨卷优先级），当前实现仅用 `hit`。
#[allow(unused_variables)]
fn hit_order_key(_vols: &HashMap<VolumeId, VolumeData>, _vid: &VolumeId, hit: &Hit) -> (u8, u64) {
    match hit {
        Hit::Base(eid) => (0, *eid as u64),
        Hit::Overlay(frn) => (1, *frn),
    }
}

fn hit_name(vols: &HashMap<VolumeId, VolumeData>, vid: &VolumeId, hit: &Hit) -> String {
    let vd = vols.get(vid).expect("hit_name: 卷必存在");
    match hit {
        Hit::Base(eid) => vd.index.name_str(*eid).unwrap_or_default().to_string(),
        Hit::Overlay(frn) => vd
            .overlay
            .added_or_changed
            .get(frn)
            .map(|oe| oe.name.clone())
            .unwrap_or_default(),
    }
}

fn hit_path(vols: &HashMap<VolumeId, VolumeData>, vid: &VolumeId, hit: &Hit) -> String {
    let vd = vols.get(vid).expect("hit_path: 卷必存在");
    match hit {
        Hit::Base(eid) => vd.index.path_string(*eid).unwrap_or_default(),
        Hit::Overlay(frn) => match vd.overlay.added_or_changed.get(frn) {
            Some(oe) => overlay_full_path(&vd.index, &vd.overlay, oe),
            None => String::new(),
        },
    }
}

fn hit_size(vols: &HashMap<VolumeId, VolumeData>, vid: &VolumeId, hit: &Hit) -> u64 {
    let vd = vols.get(vid).expect("hit_size: 卷必存在");
    match hit {
        Hit::Base(eid) => vd.index.size[*eid as usize],
        Hit::Overlay(frn) => vd
            .overlay
            .added_or_changed
            .get(frn)
            .map(|oe| oe.size)
            .unwrap_or(0),
    }
}

fn hit_mtime(vols: &HashMap<VolumeId, VolumeData>, vid: &VolumeId, hit: &Hit) -> i64 {
    let vd = vols.get(vid).expect("hit_mtime: 卷必存在");
    match hit {
        Hit::Base(eid) => vd.index.mtime[*eid as usize],
        Hit::Overlay(frn) => vd
            .overlay
            .added_or_changed
            .get(frn)
            .map(|oe| oe.mtime)
            .unwrap_or(0),
    }
}

/// 字节偏移 → UTF-16 偏移（P0-3：中文 / emoji 高亮错位修复）。
///
/// 引擎侧存的 `name_matches` 是相对名字的 **字节**偏移（与 names arena 布局一致）。
/// 但前端是 JavaScript：JS 字符串下标是 **UTF-16 代码单元**，对 CJK（3 字节/字、1 单元）
/// 与 emoji（4 字节/字、2 单元）而言字节偏移 ≠ 字符串下标，直接 `slice(s,e)` 会错位，
/// 多词重叠命中还会重复显示字符（报告复现：`report` 搜 `re ep` 显示成 `reeport`）。
///
/// 这里把每个字节偏移经「字节位置 → UTF-16 位置」映射表换成 JS 可用的下标。
fn to_utf16_ranges(name: &str, ranges: &[Range<u32>]) -> Vec<Range<u32>> {
    // map[b] = 字节位置 b 对应的 UTF-16 位置；末位填入名字总长（UTF-16 单元数）。
    let mut map = vec![0u32; name.len() + 1];
    let mut pos = 0u32;
    for (b, ch) in name.char_indices() {
        map[b] = pos;
        pos += ch.len_utf16() as u32;
    }
    map[name.len()] = pos;
    // E6：钳制到 [0, name.len()]。即便上游匹配器回归给出越界字节端点，也不 panic；
    // 非法区间（end <= start）直接丢弃，保持 UTF-16 高亮地基的健壮性。
    let cap = name.len();
    ranges
        .iter()
        .filter_map(|r| {
            let start = (r.start as usize).min(cap);
            let end = (r.end as usize).min(cap);
            (start < end).then_some(map[start]..map[end])
        })
        .collect()
}

/// 由 `(vid, hit)` 构造单条 `ResultRow`（Base 走索引原路径，Overlay 走增量路径解析）。
///
/// 任何取不到的情况都返回 `NotFound` 而不是 panic：结果集与索引之间本就存在
/// 「后台换代 / Overlay 条目被删」的竞态，`range` 拿到错误会让前端重搜一次，
/// 比整进程崩掉或静默显示错行更好（#21）。
fn resolve_row(
    vols: &HashMap<VolumeId, VolumeData>,
    vid: &VolumeId,
    hit: &Hit,
    query: &Query,
    mopts: MatchOptions,
) -> Result<ResultRow> {
    let vd = vols.get(vid).ok_or(EngineError::NotFound)?;
    match hit {
        Hit::Base(eid) => {
            let index = &vd.index;
            let i = *eid as usize;
            let flags_v = *index.flags.get(i).ok_or(EngineError::NotFound)?;
            let size_v = *index.size.get(i).ok_or(EngineError::NotFound)?;
            let mtime_v = *index.mtime.get(i).ok_or(EngineError::NotFound)?;
            let name = index
                .name_str(*eid)
                .ok_or(EngineError::NotFound)?
                .to_string();
            let full = index.path_string(*eid).ok_or(EngineError::NotFound)?;
            let dir = parent_dir(&full);
            let is_dir = (flags_v & flags::IS_DIR) != 0;
            let name_matches = to_utf16_ranges(
                &name,
                &index.highlight_ranges(*eid, query, mopts.fuzzy, mopts.pinyin),
            );
            Ok(ResultRow {
                name,
                dir,
                is_dir,
                size: Some(size_v),
                mtime: Some(mtime_v),
                name_matches,
            })
        }
        Hit::Overlay(frn) => {
            let oe = vd
                .overlay
                .added_or_changed
                .get(frn)
                .ok_or(EngineError::NotFound)?;
            let name = oe.name.clone();
            let full = overlay_full_path(&vd.index, &vd.overlay, oe);
            let dir = parent_dir(&full);
            let is_dir = (oe.flags() & flags::IS_DIR) != 0;
            let size = if oe.stale_meta { None } else { Some(oe.size) };
            let mtime = if oe.stale_meta { None } else { Some(oe.mtime) };
            let name_matches = to_utf16_ranges(&name, &overlay_highlight(query, &name, mopts));
            Ok(ResultRow {
                name,
                dir,
                is_dir,
                size,
                mtime,
                name_matches,
            })
        }
    }
}

/// Overlay 条目完整路径（先查 Overlay 再查 Base 解析父链；到根停止）。
fn overlay_full_path(index: &VolumeIndex, overlay: &Overlay, oe: &OverlayEntry) -> String {
    let mut names: Vec<String> = Vec::new();
    let mut cur_frn = oe.parent_frn;
    let mut cur_name = oe.name.clone();
    for _ in 0..10_000 {
        names.push(cur_name);
        if cur_frn == 0 {
            break;
        }
        // 增量父链优先查 Overlay
        if let Some(p) = overlay.added_or_changed.get(&cur_frn) {
            cur_name = p.name.clone();
            cur_frn = p.parent_frn;
            continue;
        }
        // 回退查 Base
        if let Some(eid) = index.frn_to_id(cur_frn) {
            if Some(eid) == index.root_id() {
                break; // 根无名字分量
            }
            cur_name = index.name_str(eid).unwrap_or_default().to_string();
            let peid = index.parent[eid as usize];
            if peid == NONE {
                break;
            }
            cur_frn = index.frn[peid as usize];
        } else {
            break; // 父未知，停止
        }
    }
    names.reverse();
    let prefix = drive_prefix(index);
    format!("{}{}", prefix, names.join("\\"))
}

/// `std::fs` 版内容源（M7 / DECISIONS #95）：打开 → 分块读 → 折叠比对。
///
/// 无状态，可被任意线程复用；失败一律降级为 [`ff_core::ContentScan`] 里的
/// `Unreadable`（权限 / 占用 / 路径失效），不向调用方抛错——内容搜索里
/// 「某个文件读不了」是常态，不该让整次搜索失败。
struct FsContentSource;

impl ff_core::ContentSource for FsContentSource {
    fn scan(&self, path: &str, spec: &ff_core::ContentSpec) -> ff_core::ContentScan {
        let mut f = match std::fs::File::open(io_path(path)) {
            Ok(f) => f,
            Err(_) => return ff_core::ContentScan::Unreadable,
        };
        // 先按真实大小判限（索引里的 size 可能过期，import 的条目干脆是 0）。
        // 一致地「超限一律跳过」而不是「读到 32 MiB 为止」：否则 needle 恰好在
        // 开头的大文件会命中、在尾部的不会命中，同一类文件两种结论。
        if let Ok(md) = f.metadata() {
            if md.len() > ff_core::MAX_FILE_BYTES {
                return ff_core::ContentScan::TooLarge;
            }
        }
        ff_core::content::scan_stream(&mut f, spec, ff_core::MAX_FILE_BYTES)
    }
}

/// 给 IO 路径加 `\\?\` 前缀，绕开 `MAX_PATH`(260)（与 `ff-ntfs::walk` 同口径）。
fn io_path(logical: &str) -> String {
    if logical.starts_with(r"\\?\") {
        logical.to_string()
    } else if let Some(rest) = logical.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{rest}")
    } else {
        format!(r"\\?\{logical}")
    }
}

/// 命中条目的完整路径（目录 / 无路径 → `None`，内容搜索只对文件生效）。
fn content_path(
    vols: &HashMap<VolumeId, VolumeData>,
    hits: &[(VolumeId, Hit, f64)],
    i: EntryId,
) -> Option<String> {
    let (vid, hit, _) = hits.get(i as usize)?;
    let vd = vols.get(vid)?;
    match hit {
        Hit::Base(eid) => {
            let f = vd.index.flags.get(*eid as usize).copied().unwrap_or(0);
            if f & flags::IS_DIR != 0 {
                None
            } else {
                vd.index.path_string(*eid)
            }
        }
        Hit::Overlay(frn) => {
            let oe = vd.overlay.added_or_changed.get(frn)?;
            if oe.is_dir {
                None
            } else {
                Some(overlay_full_path(&vd.index, &vd.overlay, oe))
            }
        }
    }
}

/// 命中条目的字节数（用于读文件前的大小预判）。
fn content_size(
    vols: &HashMap<VolumeId, VolumeData>,
    hits: &[(VolumeId, Hit, f64)],
    i: EntryId,
) -> Option<u64> {
    let (vid, hit, _) = hits.get(i as usize)?;
    let vd = vols.get(vid)?;
    match hit {
        Hit::Base(eid) => vd.index.size.get(*eid as usize).copied(),
        Hit::Overlay(frn) => vd.overlay.added_or_changed.get(frn).map(|oe| oe.size),
    }
}

/// 卷路径前缀（`C:\` / GUID 路径 / 退化 `\`）。
fn drive_prefix(index: &VolumeIndex) -> String {
    if let Some(c) = index.volume.drive {
        format!("{c}:\\")
    } else if !index.volume.guid_path.is_empty() {
        let mut s = index.volume.guid_path.clone();
        if !s.ends_with('\\') {
            s.push('\\');
        }
        s
    } else {
        "\\".to_string()
    }
}

/// Overlay 名字是否命中查询（与 Base 一致的子串 AND + 大小写策略）。
/// Overlay 条目不在索引里，父名 / 完整路径 / 深度只能借 Base 索引推导。
///
/// Overlay 存的是「新增或改名的条目」，其父目录通常在索引中，因此
/// `parent:` / `infolder:` / `path:` / `depth:` 仍可判定；父不在索引时
/// 相应条件判定为 false（记录在 DECISIONS）。
fn overlay_context(
    index: &VolumeIndex,
    oe: &OverlayEntry,
) -> (Option<String>, Option<String>, u32) {
    match index.frn_to_id(oe.parent_frn) {
        Some(pid) => {
            let parent_name = index.name_str(pid).map(str::to_string);
            let path = index.path_string(pid).map(|mut p| {
                if !p.ends_with('\\') {
                    p.push('\\');
                }
                p.push_str(&oe.name);
                p
            });
            (parent_name, path, index.depth_of(pid) + 1)
        }
        None => (None, None, 0),
    }
}

/// Overlay 条目是否命中：走与 Base 相同的 AST（M4 的 OR / NOT / 修饰符
/// 在增量条目上才不会漏匹配——旧实现只按 `terms` 做 AND）。
fn overlay_matches(query: &Query, f: &EntryFacts<'_>) -> bool {
    query.matches_facts(f)
}

/// Overlay 名字内的命中区间（供高亮；与 Base 共用 ff-core 的 M5 分层判定）。
fn overlay_highlight(query: &Query, name: &str, mopts: MatchOptions) -> Vec<Range<u32>> {
    highlight_name(name.as_bytes(), query, mopts.fuzzy, mopts.pinyin)
}

/// 从完整路径提取父目录（含末尾 `\`）。根条目 `C:\` 的父目录为 `C:\`。
fn parent_dir(full: &str) -> String {
    let bs = full.matches('\\').count();
    if bs <= 1 {
        // 根 `C:\` 或顶层目录 `C:\Windows`：父目录即前缀（含末尾 `\`）。
        // 仅含一个 `\`（前缀那个），剥离组件后只剩盘符，故保留前缀。
        return match full.find('\\') {
            Some(i) => full[..i + 1].to_string(),
            None => full.to_string(),
        };
    }
    // 否则去掉最后一个路径组件（`\` 前的部分即父目录）。
    match full.rfind('\\') {
        Some(last_bs) => full[..last_bs].to_string(),
        None => full.to_string(),
    }
}

/// 构造 `explorer /select,"<path>"` 的参数串（P0-2：纯函数，可测）。
///
/// 直接在 `ShellExecuteW` 里把这个参数作为宽字符串传给 `explorer.exe`，不经
/// `cmd`，因此路径里的 `& % ^ ( )` 与空格都不会被 shell 当作命令 / 变量展开
/// （旧实现走 `cmd /c start` 时 `D:\dl\x&calc.exe&.txt` 会以管理员权限执行 `calc`）。
#[cfg(windows)]
fn explorer_select_arg(path: &str) -> String {
    format!("/select,\"{path}\"")
}

/// 跨平台打开 / 选中文件（真实进程行为由 OS 完成）。
///
/// P0-2（Windows）：改用 `ShellExecuteW`，**不再**经过 `cmd /c start`：
/// - 路径含 `&` / `%` / 空格不再被解释为命令或变量（防命令注入）；
/// - 不闪控制台黑窗（`CREATE_NO_WINDOW` 的等价效果：不经控制台子系统进程）；
/// - 不以 `explorer` 退出码误判失败（`cmd` 链路里 explorer 常返回 1 被当成失败）；
/// - 不再继承管理员权限去 `start` 任意程序（至少不再新起一个 admin 控制台）。
#[cfg(windows)]
pub(crate) fn shell_open(path: &str, folder: bool) -> Result<()> {
    use std::ptr::{null, null_mut};

    let to_wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0u16))
            .collect::<Vec<u16>>()
    };

    // ShellExecuteW 在 shell32.dll；返回值 > 32 为成功（HINSTANCE 语义）。
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut std::ffi::c_void,
            lpOperation: *const u16,
            lpFile: *const u16,
            lpParameters: *const u16,
            lpDirectory: *const u16,
            nShowCmd: i32,
        ) -> isize;
    }

    let open_w = to_wide("open");
    // SAFETY：宽串指针在调用期间有效；ShellExecuteW 不取所有权、不长期保存指针。
    let ret = if folder {
        let explorer_w = to_wide("explorer.exe");
        let params_w = to_wide(&explorer_select_arg(path));
        unsafe {
            ShellExecuteW(
                null_mut(),
                open_w.as_ptr(),
                explorer_w.as_ptr(),
                params_w.as_ptr(),
                null::<u16>(),
                1, // SW_SHOWNORMAL
            )
        }
    } else {
        let file_w = to_wide(path);
        unsafe {
            ShellExecuteW(
                null_mut(),
                open_w.as_ptr(),
                file_w.as_ptr(),
                null::<u16>(),
                null::<u16>(),
                1,
            )
        }
    };
    if ret as isize > 32 {
        Ok(())
    } else {
        Err(EngineError::Internal(format!(
            "打开失败（ShellExecuteW 返回 {ret}）：{path}"
        )))
    }
}

/// `SHELLEXECUTEINFOW`（Win32，`repr(C)` 字段顺序/对齐与 SDK 一致）。
/// 只需声明到 `hProcess`；`hIcon`/`hMonitor` 是 union，两者都是指针大小。
#[cfg(windows)]
#[repr(C)]
struct ShellExecuteInfoW {
    cb_size: u32,
    f_mask: u32,
    hwnd: *mut std::ffi::c_void,
    lp_verb: *const u16,
    lp_file: *const u16,
    lp_parameters: *const u16,
    lp_directory: *const u16,
    n_show: i32,
    h_inst_app: *mut std::ffi::c_void,
    lp_id_list: *mut std::ffi::c_void,
    lp_class: *const u16,
    h_key_class: *mut std::ffi::c_void,
    dw_hot_key: u32,
    h_icon_or_monitor: *mut std::ffi::c_void,
    h_process: *mut std::ffi::c_void,
}

/// 打开 Windows 系统文件属性对话框（M7，DESIGN §11.1 `Alt+Enter`）。
///
/// 用 `ShellExecuteExW` + `SEE_MASK_INVOKEIDLIST`（微软文档推荐的唯一可靠方式）：
/// - 简单版 `ShellExecuteW("properties", ...)` 在本机实测返回 31（`SE_ERR_NOASSOC`
///   量级）被 `>32` 判定误报失败，故不用；`INVOKEIDLIST` 让 shell 从 PIDL 调用
///   `properties` verb，对话框才会真正弹出；
/// - 不经 `cmd`，路径含 `&` / `%` / 空格不被解释（防注入）；
/// - 由 UI 进程以当前用户身份执行（DESIGN §12），服务模式走 `ResolvedAction` 回传后
///   由 `ff_engine::execute_open_checked` 调用本函数。
#[cfg(windows)]
pub(crate) fn shell_properties(path: &str) -> Result<()> {
    use std::ptr::{null, null_mut};

    const SEE_MASK_INVOKEIDLIST: u32 = 0x0000_000c;
    const SW_SHOWNORMAL: i32 = 1;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
    }

    let file_w = path
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .collect::<Vec<u16>>();
    let verb_w = "properties"
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .collect::<Vec<u16>>();
    let mut info = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_INVOKEIDLIST,
        hwnd: null_mut(),
        lp_verb: verb_w.as_ptr(),
        lp_file: file_w.as_ptr(),
        lp_parameters: null::<u16>(),
        lp_directory: null::<u16>(),
        n_show: SW_SHOWNORMAL,
        h_inst_app: null_mut(),
        lp_id_list: null_mut(),
        lp_class: null::<u16>(),
        h_key_class: null_mut(),
        dw_hot_key: 0,
        h_icon_or_monitor: null_mut(),
        h_process: null_mut(),
    };
    // SAFETY：info 与宽串指针在调用期间有效；ShellExecuteExW 同步读取 info 并弹出
    // 属性对话框（由 shell 进程持有），不读取调用方栈上宽串的长期所有权。
    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok != 0 {
        Ok(())
    } else {
        Err(EngineError::Internal(format!(
            "打开属性失败（ShellExecuteExW 失败）：{path}"
        )))
    }
}

/// 非 Windows：属性对话框由 UI 层处理；保持跨平台可编译，不实际启动进程。
#[cfg(not(windows))]
pub(crate) fn shell_properties(_path: &str) -> Result<()> {
    Ok(())
}

/// 非 Windows：真实打开由 UI 层处理；本实现保持跨平台可编译，不实际启动进程。
#[cfg(not(windows))]
pub(crate) fn shell_open(_path: &str, _folder: bool) -> Result<()> {
    Ok(())
}

/// 字符串转 UTF-16 + 结尾 NUL（Win32 宽串约定）。纯函数，便于单测。
fn wide_with_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0u16)).collect()
}

/// 从完整路径取出最后一段名字。
///
/// 末尾分隔符（盘符根 `C:\`）先裁掉；无分隔符时返回整串（`C:`）。
/// 不用 `std::path::Path::file_name`，避免非 Windows 构建把 `\` 当普通字符。
///
/// 仅供测试保留：#36 之后 `open(CopyName)` 与管道侧统一改用结果行的 `name`
/// 字段，运行时不再需要从路径反推（保留此工具与用例是因为它能覆盖分隔符边界）。
#[cfg(test)]
fn file_name_from_full(full: &str) -> &str {
    let trimmed = full.trim_end_matches(['\\', '/']);
    trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed)
}

/// 把文本写入系统剪贴板（P0-2，Windows）。
///
/// 与 `shell_open` 同风格：直接 Win32 FFI，不引入 `arboard` 等新依赖
/// （DESIGN §2.1 依赖准入）。要点：
/// - `CF_UNICODETEXT` 要求 `GMEM_MOVEABLE` 的全局内存，且文本以 UTF-16 NUL 结尾；
/// - `SetClipboardData` **成功后内存所有权移交系统**，绝不能再 `GlobalFree`；
///   失败才由调用方释放；
/// - `OpenClipboard` 可能被别的进程短暂占用，有限重试 5×10 ms，不长期阻塞 UI；
/// - 失败返回 `Err`，经 `TauriEngine.open` → `onOpenError` 在状态栏显示（P1-9），
///   不再像旧实现那样静默 `Ok(())` 让界面撒谎。
#[cfg(windows)]
pub(crate) fn clipboard_set_text(text: &str) -> Result<()> {
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::thread::sleep;
    use std::time::Duration;

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
        fn CloseClipboard() -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalFree(mem: *mut c_void) -> *mut c_void;
    }

    let wide = wide_with_nul(text);
    let bytes = wide.len() * std::mem::size_of::<u16>();

    // SAFETY：所有句柄均由本函数分配、生命周期限于本函数；
    // SetClipboardData 成功后不再触碰 mem（所有权已移交系统）。
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem.is_null() {
            return Err(EngineError::Io("剪贴板内存分配失败".into()));
        }
        let dst = GlobalLock(mem);
        if dst.is_null() {
            GlobalFree(mem);
            return Err(EngineError::Io("剪贴板内存锁定失败".into()));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst.cast::<u16>(), wide.len());
        GlobalUnlock(mem);

        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(null_mut()) != 0 {
                opened = true;
                break;
            }
            sleep(Duration::from_millis(10));
        }
        if !opened {
            GlobalFree(mem);
            return Err(EngineError::Io("剪贴板被其他程序占用".into()));
        }

        EmptyClipboard();
        let ok = !SetClipboardData(CF_UNICODETEXT, mem).is_null();
        CloseClipboard();
        if ok {
            Ok(())
        } else {
            GlobalFree(mem);
            Err(EngineError::Io("写入剪贴板失败".into()))
        }
    }
}

/// 非 Windows：显式报错而不是假成功——前端会如实提示「剪贴板不可用」。
#[cfg(not(windows))]
pub(crate) fn clipboard_set_text(_text: &str) -> Result<()> {
    Err(EngineError::Io("当前平台不支持系统剪贴板".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{OpenAction, SearchOptions, SearchRequest, SortKey, SortSpec};
    use ff_core::{attrs, flags, EntryInput, IndexBuilder, VolumeInfo};
    use ff_ntfs::{Change, RawEntry, ScanError, ScanMeta, VolumeId, VolumeScanner};
    use std::time::Duration;

    const DIR: u32 = attrs::FILE_ATTRIBUTE_DIRECTORY;

    fn dir(frn: u64, parent: u64, name: &str, size: u64, mtime: i64) -> EntryInput {
        EntryInput {
            frn,
            parent_frn: parent,
            name: name.to_string(),
            is_dir: true,
            attrs: DIR,
            size,
            mtime,
            ctime: mtime,
            atime: mtime,
        }
    }

    fn file(frn: u64, parent: u64, name: &str, size: u64, mtime: i64) -> EntryInput {
        EntryInput {
            is_dir: false,
            attrs: 0,
            ..dir(frn, parent, name, size, mtime)
        }
    }

    /// C:\ (root, frn 5)
    ///  ├─ Windows\        (frn 100, size 4096, mtime 100)
    ///  │   └─ readme.txt  (frn 101, size 1024, mtime 200)
    ///  └─ 用户\            (frn 200, size 0,    mtime 50)
    ///      └─ 文档.txt     (frn 201, size 2048, mtime 300)
    fn sample_index() -> VolumeIndex {
        let info = VolumeInfo {
            drive: Some('C'),
            ..Default::default()
        };
        let mut b = IndexBuilder::new(info);
        b.push(dir(5, 0, ".", 0, 1));
        b.push(dir(100, 5, "Windows", 4096, 100));
        b.push(file(101, 100, "readme.txt", 1024, 200));
        b.push(dir(200, 5, "用户", 0, 50));
        b.push(file(201, 200, "文档.txt", 2048, 300));
        b.finish()
    }

    fn engine() -> InProcessEngine {
        let e = InProcessEngine::empty();
        e.add_volume(
            VolumeId {
                serial: 1,
                drive: Some('C'),
            },
            sample_index(),
        );
        e
    }

    fn req(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            sort: SortSpec::default(),
            max_hits: None,
            opts: SearchOptions::default(),
        }
    }

    #[test]
    fn search_substring_hit() {
        let e = engine();
        let sum = e.search(req("readme")).unwrap();
        assert!(sum.parse_error.is_none());
        assert_eq!(sum.total, 1);
        let rows = e.range(sum.id, 0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "readme.txt");
        assert_eq!(rows[0].dir, "C:\\Windows");
        assert!(!rows[0].is_dir);
    }

    // ───────────────── M5：T2 拼音 / T3 模糊 / 评分排序（引擎接线） ─────────────────

    /// 中文名、模糊名、精确名与前缀名共存的索引。
    fn m5_index() -> VolumeIndex {
        let info = VolumeInfo {
            drive: Some('C'),
            ..Default::default()
        };
        let mut b = IndexBuilder::new(info);
        b.push(dir(5, 0, ".", 0, 1));
        b.push(file(101, 5, "中国人.txt", 100, 200));
        b.push(file(102, 5, "Visual Studio Code", 100, 200));
        b.push(file(103, 5, "report.txt", 100, 200));
        b.push(file(104, 5, "report-final.txt", 100, 200));
        b.push(file(105, 5, "拼音搜索Everything.txt", 100, 200));
        b.finish()
    }

    fn m5_engine() -> InProcessEngine {
        let e = InProcessEngine::empty();
        e.add_volume(
            VolumeId {
                serial: 1,
                drive: Some('C'),
            },
            m5_index(),
        );
        e
    }

    #[test]
    fn m5_pinyin_hits_chinese_name() {
        let e = m5_engine();
        let sum = e.search(req("zgr")).unwrap();
        assert!(sum.parse_error.is_none());
        let rows = e.range(sum.id, 0, 20).unwrap();
        let hit = rows
            .iter()
            .find(|r| r.name == "中国人.txt")
            .expect("zgr 应经 T2 拼音命中 中国人.txt");
        // 拼音命中也要有高亮区间（引擎侧已转成 UTF-16 下标）。
        assert_eq!(hit.name_matches.len(), 3);
    }

    #[test]
    fn m5_fuzzy_hits_visual_studio_code() {
        let e = m5_engine();
        let sum = e.search(req("vsc")).unwrap();
        let rows = e.range(sum.id, 0, 20).unwrap();
        assert!(
            rows.iter().any(|r| r.name == "Visual Studio Code"),
            "vsc 应经 T3 模糊命中"
        );
    }

    #[test]
    fn m5_score_sort_exact_first() {
        let e = m5_engine();
        let sum = e.search(req("report")).unwrap();
        let rows = e.range(sum.id, 0, 20).unwrap();
        assert_eq!(rows.len(), 2);
        // DESIGN §8.2 金标准：更短 / 更精确的名字排在长前缀名之前。
        assert_eq!(rows[0].name, "report.txt");
    }

    #[test]
    fn m5_switches_disable_t2_t3() {
        let e = m5_engine();
        let mut no_fuzzy = req("vsc");
        no_fuzzy.opts.fuzzy = false;
        no_fuzzy.opts.pinyin = false;
        assert_eq!(e.search(no_fuzzy).unwrap().total, 0);

        let mut no_pinyin = req("zgr");
        no_pinyin.opts.fuzzy = false;
        no_pinyin.opts.pinyin = false;
        assert_eq!(e.search(no_pinyin).unwrap().total, 0);
    }

    #[test]
    fn m5_column_sort_still_uses_real_values() {
        let e = m5_engine();
        let mut r = req("report");
        r.sort = SortSpec {
            key: SortKey::Name,
            descending: false,
        };
        let sum = e.search(r).unwrap();
        let rows = e.range(sum.id, 0, 20).unwrap();
        assert_eq!(rows[0].name, "report-final.txt");
    }

    // ───────────────── M7：内容搜索 content:（DECISIONS #95） ─────────────────

    /// 临时目录（Drop 时删；只用于内容搜索的真文件验收）。
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "sift-content-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).expect("建临时目录");
        TempDir(dir)
    }

    /// 以临时目录为「卷根」的引擎：`guid_path` 直接指绝对路径，
    /// `path_string` 拼出来的路径就是真实可打开的文件路径。
    fn content_engine(root: &Path) -> InProcessEngine {
        let info = VolumeInfo {
            drive: None,
            guid_path: root.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let mut b = IndexBuilder::new(info);
        b.push(dir(5, 0, ".", 0, 1));
        b.push(dir(100, 5, "docs", 0, 1));
        b.push(file(101, 100, "alpha.txt", 26, 200));
        b.push(file(102, 100, "beta.txt", 12, 200));
        b.push(file(103, 100, "blob.bin", 14, 200));
        // 索引里有、磁盘上没有 → Unreadable
        b.push(file(104, 100, "ghost.txt", 10, 200));
        let e = InProcessEngine::empty();
        e.add_volume(
            VolumeId {
                serial: 7,
                drive: None,
            },
            b.finish(),
        );
        e
    }

    fn write_content_fixtures(tmp: &TempDir) {
        let docs = tmp.0.join("docs");
        std::fs::write(docs.join("alpha.txt"), "2024 逾期未付款项 SECRET-TOKEN").unwrap();
        std::fs::write(docs.join("beta.txt"), "无关内容").unwrap();
        std::fs::write(docs.join("blob.bin"), b"\x00\x01SECRET-TOKEN").unwrap();
    }

    #[test]
    fn content_search_filters_by_file_body() {
        let tmp = temp_dir("body");
        write_content_fixtures(&tmp);
        let e = content_engine(&tmp.0);

        let sum = e.search(req("*.txt content:SECRET-TOKEN")).unwrap();
        assert!(sum.parse_error.is_none());
        assert_eq!(sum.total, 1);
        let rows = e.range(sum.id, 0, 10).unwrap();
        assert_eq!(rows[0].name, "alpha.txt");
        let rep = sum.content.expect("内容条件应有统计");
        assert_eq!(rep.terms, 1);
        assert_eq!(rep.candidates, 3, "三份 .txt 进候选");
        assert_eq!(rep.scanned, 2, "alpha / beta 被真读");
        assert_eq!(rep.matched, 1);
        assert_eq!(rep.unreadable, 1, "ghost.txt 打不开");
        assert!(!rep.truncated);
    }

    #[test]
    fn content_search_skips_binary_and_dirs() {
        let tmp = temp_dir("bin");
        write_content_fixtures(&tmp);
        let e = content_engine(&tmp.0);
        // 通配所有条目：目录被跳过、含 NUL 的文件判为二进制
        let sum = e.search(req("* content:SECRET-TOKEN")).unwrap();
        assert_eq!(sum.total, 1);
        let rep = sum.content.expect("内容条件应有统计");
        assert!(rep.skipped_binary >= 1, "blob.bin 应判二进制");
        assert!(rep.skipped_dir >= 1, "卷根 / docs 应跳过");
        assert!(rep.incomplete());
    }

    #[test]
    fn content_search_absent_without_content_term() {
        let tmp = temp_dir("none");
        write_content_fixtures(&tmp);
        let e = content_engine(&tmp.0);
        let sum = e.search(req("*.txt")).unwrap();
        assert!(sum.content.is_none(), "无 content 条件不应有统计");
        assert_eq!(sum.total, 3);
    }

    #[test]
    fn content_search_case_sensitivity_and_phrase() {
        let tmp = temp_dir("case");
        std::fs::write(tmp.0.join("docs").join("alpha.txt"), "Hello World").unwrap();
        std::fs::write(tmp.0.join("docs").join("beta.txt"), "hello world").unwrap();
        let e = content_engine(&tmp.0);
        // 默认不区分大小写 → alpha / beta 都命中（ghost.txt 打不开，不计命中）
        assert_eq!(e.search(req("*.txt content:hello")).unwrap().total, 2);
        // case: 前置 → 只命中全小写的 beta
        let sum = e.search(req("case: *.txt content:hello")).unwrap();
        assert_eq!(sum.total, 1);
        let rows = e.range(sum.id, 0, 10).unwrap();
        assert_eq!(rows[0].name, "beta.txt");
        // 短语形式（含空格）
        assert_eq!(
            e.search(req(r#"*.txt content:"Hello World""#))
                .unwrap()
                .total,
            2
        );
    }

    #[test]
    fn range_builds_row_and_highlight() {
        let e = engine();
        let sum = e.search(req("txt")).unwrap();
        assert_eq!(sum.total, 2);
        let rows = e.range(sum.id, 0, 10).unwrap();
        // 默认 Score 排序 = entry_id 升序：readme.txt(2) 先于 文档.txt(4)
        assert_eq!(rows[0].name, "readme.txt");
        assert_eq!(rows[0].dir, "C:\\Windows");
        assert!(!rows[0].is_dir);
        assert_eq!(rows[0].size, Some(1024));
        assert_eq!(rows[0].mtime, Some(200));
        // "readme.txt" 中 "txt" 位于字节 7..10
        assert_eq!(rows[0].name_matches, vec![7..10]);

        assert_eq!(rows[1].name, "文档.txt");
        assert_eq!(rows[1].dir, "C:\\用户");
        assert_eq!(rows[1].size, Some(2048));
        assert_eq!(rows[1].mtime, Some(300));
        // P0-3 修复：`name_matches` 现是 **UTF-16 代码单元**偏移（JS 字符串下标）。
        // "文档.txt" = 文(1单元) 档(1) .(1) t(1) x(1) t(1) = 6 单元；"txt" 位于 3..6。
        assert_eq!(rows[1].name_matches, vec![3..6]);
    }

    #[test]
    fn range_dir_of_root_is_prefix() {
        let e = engine();
        // 搜索 "Windows"（目录）：dir 应为根前缀 C:\
        let sum = e.search(req("Windows")).unwrap();
        let rows = e.range(sum.id, 0, 10).unwrap();
        assert_eq!(rows[0].name, "Windows");
        assert!(rows[0].is_dir);
        assert_eq!(rows[0].dir, "C:\\");
    }

    #[test]
    fn sort_by_size_descending() {
        let e = engine();
        let mut r = req("txt");
        r.sort = SortSpec {
            key: SortKey::Size,
            descending: true,
        };
        let sum = e.search(r).unwrap();
        let rows = e.range(sum.id, 0, 10).unwrap();
        // 文档.txt(2048) 在 readme.txt(1024) 之前
        assert_eq!(rows[0].name, "文档.txt");
        assert_eq!(rows[1].name, "readme.txt");
    }

    #[test]
    fn sort_by_modified_descending() {
        let e = engine();
        let mut r = req("txt");
        r.sort = SortSpec {
            key: SortKey::Modified,
            descending: true,
        };
        let sum = e.search(r).unwrap();
        let rows = e.range(sum.id, 0, 10).unwrap();
        // mtime: 文档.txt=300 > readme.txt=200
        assert_eq!(rows[0].name, "文档.txt");
        assert_eq!(rows[1].name, "readme.txt");
    }

    #[test]
    fn hide_system_filters_system_entries() {
        // DESIGN §5.2：「默认可在设置中隐藏」——`SearchOptions::hide_system` 此前是死字段，
        // 现在在 search 的命中汇总里生效。
        let mut b = IndexBuilder::new(VolumeInfo {
            drive: Some('C'),
            ..Default::default()
        });
        b.push(dir(5, 0, ".", 0, 1));
        b.push(file(101, 5, "readme.txt", 1024, 200));
        b.push(EntryInput {
            frn: 102,
            parent_frn: 5,
            name: "pagefile.sys".to_string(),
            is_dir: false,
            attrs: attrs::FILE_ATTRIBUTE_SYSTEM,
            size: 4096,
            mtime: 300,
            ctime: 300,
            atime: 300,
        });
        let e = InProcessEngine::empty();
        e.add_volume(
            VolumeId {
                serial: 1,
                drive: Some('C'),
            },
            b.finish(),
        );

        // 默认 hide_system = true（SearchOptions::default）：系统文件被过滤
        let mut hide = req("e"); // "e" 同时命中 readme.txt 与 pagefile.sys
        hide.opts.hide_system = true;
        let sum = e.search(hide).unwrap();
        assert_eq!(sum.total, 1);
        assert_eq!(e.range(sum.id, 0, 10).unwrap()[0].name, "readme.txt");

        // 关掉 → 两条都出现
        let mut show = req("e");
        show.opts.hide_system = false;
        let sum = e.search(show).unwrap();
        assert_eq!(sum.total, 2);
    }

    /// 三条属性向量（名字都含 "e"，便于用同一条查询串覆盖）：
    ///   - `readme.txt`（无属性）
    ///   - `pagefile.sys`（只 SYSTEM，`flags::SYSTEM` = 0x4）
    ///   - `hidefile.txt`（只 HIDDEN，`flags::HIDDEN` = 0x2）
    ///   - `syshidefile.dat`（SYSTEM | HIDDEN）
    fn attr_index() -> VolumeIndex {
        let mut b = IndexBuilder::new(VolumeInfo {
            drive: Some('C'),
            ..Default::default()
        });
        b.push(dir(5, 0, ".", 0, 1));
        b.push(file(101, 5, "readme.txt", 1024, 200));
        b.push(EntryInput {
            frn: 102,
            parent_frn: 5,
            name: "pagefile.sys".to_string(),
            is_dir: false,
            attrs: attrs::FILE_ATTRIBUTE_SYSTEM,
            size: 4096,
            mtime: 300,
            ctime: 300,
            atime: 300,
        });
        b.push(EntryInput {
            frn: 103,
            parent_frn: 5,
            name: "hidefile.txt".to_string(),
            is_dir: false,
            attrs: attrs::FILE_ATTRIBUTE_HIDDEN,
            size: 512,
            mtime: 400,
            ctime: 400,
            atime: 400,
        });
        b.push(EntryInput {
            frn: 104,
            parent_frn: 5,
            name: "syshidefile.dat".to_string(),
            is_dir: false,
            attrs: attrs::FILE_ATTRIBUTE_SYSTEM | attrs::FILE_ATTRIBUTE_HIDDEN,
            size: 8192,
            mtime: 500,
            ctime: 500,
            atime: 500,
        });
        b.finish()
    }

    fn attr_engine() -> InProcessEngine {
        let e = InProcessEngine::empty();
        e.add_volume(
            VolumeId {
                serial: 1,
                drive: Some('C'),
            },
            attr_index(),
        );
        e
    }

    /// 断言用 `flags` 的实际位值（见 `ff_core/src/entry.rs:22-26`）：HIDDEN = 1<<1 = 0x2，
    /// SYSTEM = 1<<2 = 0x4。
    #[test]
    fn flags_bits_are_0x2_and_0x4() {
        assert_eq!(flags::HIDDEN, 0x2);
        assert_eq!(flags::SYSTEM, 0x4);
        let ix = attr_index();
        // entry_id 即 push 顺序：0 = "."、1 = readme.txt、2 = pagefile.sys、
        // 3 = hidefile.txt、4 = syshidefile.dat
        let (plain, sys_only, hidden_only, both) =
            (ix.flags[1], ix.flags[2], ix.flags[3], ix.flags[4]);
        assert_eq!(plain & (flags::SYSTEM | flags::HIDDEN), 0);
        assert_eq!(sys_only & flags::SYSTEM, 0x4);
        assert_eq!(sys_only & flags::HIDDEN, 0);
        assert_eq!(hidden_only & flags::HIDDEN, 0x2);
        assert_eq!(hidden_only & flags::SYSTEM, 0);
        assert_eq!(both & flags::SYSTEM, 0x4);
        assert_eq!(both & flags::HIDDEN, 0x2);
    }

    /// `hide_system` 与 `hide_hidden` 独立生效：4 种开关组合 × 3 条属性向量。
    #[test]
    fn hide_system_and_hide_hidden_are_independent() {
        let e = attr_engine();
        let names = |hide_system: bool, hide_hidden: bool| {
            let mut r = req("e"); // 四条都含 "e"
            r.opts.hide_system = hide_system;
            r.opts.hide_hidden = hide_hidden;
            let sum = e.search(r).unwrap();
            e.range(sum.id, 0, 10)
                .unwrap()
                .iter()
                .map(|row| row.name.clone())
                .collect::<Vec<_>>()
        };

        // 都开（默认）：只剩无属性的 readme.txt
        assert_eq!(names(true, true), vec!["readme.txt".to_string()]);
        // 只关 hide_hidden：只带 HIDDEN 的回来
        assert_eq!(
            names(true, false),
            vec!["readme.txt".to_string(), "hidefile.txt".to_string()]
        );
        // 只关 hide_system：只带 SYSTEM 的回来
        assert_eq!(
            names(false, true),
            vec!["readme.txt".to_string(), "pagefile.sys".to_string()]
        );
        // 都关：四条全在
        assert_eq!(
            names(false, false),
            vec![
                "readme.txt".to_string(),
                "pagefile.sys".to_string(),
                "hidefile.txt".to_string(),
                "syshidefile.dat".to_string(),
            ]
        );
    }

    #[test]
    fn parse_error_written_not_err() {
        let e = engine();
        // `a|b` 在 M1 是「OR 未支持」的错误，M4 起合法；改用一个真正非法的
        // 修饰符值来验证「语法错误写进 summary 而不是返回 Err」这一契约。
        let sum = e.search(req("size:abc")).unwrap();
        assert!(sum.parse_error.is_some());
        assert_eq!(sum.total, 0);
    }

    #[test]
    fn empty_query_empty_result() {
        let e = engine();
        let sum = e.search(req("")).unwrap();
        assert!(sum.parse_error.is_none());
        assert_eq!(sum.total, 0);
    }

    #[test]
    fn status_reports_volume() {
        let e = engine();
        let st = e.status();
        assert_eq!(st.volumes.len(), 1);
        assert_eq!(st.total_entries, 5);
        assert!(!st.searching);
        assert_eq!(st.volumes[0].state, VolumeState::Ready);
    }

    #[test]
    fn range_out_of_range_empty() {
        let e = engine();
        let sum = e.search(req("txt")).unwrap();
        assert!(e.range(sum.id, 100, 10).unwrap().is_empty());
    }

    #[test]
    fn open_unknown_id_not_found() {
        let e = engine();
        // 未发起搜索即取结果 → NotFound
        assert!(matches!(
            e.open(999, 0, OpenAction::Open),
            Err(EngineError::NotFound)
        ));
        // 已发起搜索但越界 index → NotFound
        let sum = e.search(req("txt")).unwrap();
        assert!(matches!(
            e.open(sum.id, 99, OpenAction::Open),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn cancel_is_noop_safe() {
        let e = engine();
        let sum = e.search(req("txt")).unwrap();
        e.cancel(sum.id); // 不应 panic
    }

    #[test]
    fn previous_result_invalidated_by_new_search() {
        let e = engine();
        let first = e.search(req("txt")).unwrap();
        let _second = e.search(req("readme")).unwrap();
        // 新搜索覆盖旧结果集（单客户端语义）
        assert!(matches!(
            e.range(first.id, 0, 10),
            Err(EngineError::NotFound)
        ));
    }

    #[test]
    fn client_sessions_keep_results_and_cancellation_isolated() {
        let engine = InProcessEngine::empty();
        engine.add_volume(
            VolumeId {
                serial: 1,
                drive: Some('C'),
            },
            sample_index(),
        );
        let a = engine.create_session();
        let b = engine.create_session();
        let first = a.search(req("txt")).unwrap();
        let second = b.search(req("readme")).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(second.total, 1);
        assert_eq!(a.range(first.id, 0, 1).unwrap().len(), 1);
        assert_eq!(b.range(second.id, 0, 1).unwrap().len(), 1);
        assert!(
            a.range(second.id, 0, 1).is_err(),
            "会话 A 不应读取会话 B 的结果"
        );
        let newer = a.search(req("readme")).unwrap();
        assert!(
            a.range(first.id, 0, 1).is_err(),
            "同一会话的新搜索仍应替换旧结果"
        );
        assert_eq!(
            b.range(second.id, 0, 1).unwrap().len(),
            1,
            "会话 B 的结果不应被 A 作废"
        );
        a.cancel(newer.id);
        assert!(b.range(second.id, 0, 1).is_ok(), "取消 A 不应影响 B");
    }

    // ───────────────────── M2（②/③/④）新增：Overlay / 重建 / 持久化 ─────────────────────

    /// 内存假扫描器：`full_scan` 产出 `full`，`read_changes` 返回 `changes`；
    /// `unavailable=true` 时 `read_changes` 返回 `JournalUnavailable`（触发重建）。
    struct FakeScanner {
        changes: Vec<Change>,
        next_usn: i64,
        full: Vec<RawEntry>,
        unavailable: bool,
    }

    impl VolumeScanner for FakeScanner {
        fn full_scan(
            &self,
            _vol: &VolumeId,
            sink: &mut dyn FnMut(RawEntry),
        ) -> ff_ntfs::Result<ScanMeta> {
            for e in &self.full {
                sink(e.clone());
            }
            Ok(ScanMeta {
                journal_id: 1,
                next_usn: self.next_usn,
                entries: self.full.len() as u64,
                elapsed: Duration::ZERO,
                warnings: 0,
            })
        }
        fn read_changes(
            &self,
            _vol: &VolumeId,
            _from: i64,
            _jid: u64,
        ) -> ff_ntfs::Result<(Vec<Change>, i64)> {
            if self.unavailable {
                return Err(ScanError::JournalUnavailable("test journal gone".into()));
            }
            Ok((self.changes.clone(), self.next_usn))
        }
    }

    #[test]
    fn overlay_upsert_reflected_in_search() {
        let e = engine(); // C: 含 sample_index（readme.txt 在 Windows/frn100 下）
        e.set_scanner(Box::new(FakeScanner {
            changes: vec![Change::Upsert(RawEntry {
                frn: 9001,
                parent_frn: 100,
                name: "fresh.txt".to_string(),
                is_dir: false,
                attrs: 0,
                size: 77,
                mtime: 12345,
                ctime: 12345,
                atime: 12345,
            })],
            next_usn: 2,
            full: vec![],
            unavailable: false,
        }));
        e.poll_once();
        let sum = e.search(req("fresh")).unwrap();
        assert_eq!(sum.total, 1, "Overlay 新增条目应反映到搜索结果");
        let rows = e.range(sum.id, 0, 10).unwrap();
        assert_eq!(rows[0].name, "fresh.txt");
        assert_eq!(rows[0].dir, "C:\\Windows");
        assert_eq!(rows[0].size, Some(77));
    }

    /// S1/S2 回归：无名字的 USN 增量（内容/元数据变化）不得让 Base 条目从搜索中
    /// 消失，也不得显示成空名 / "0 B"。应按 frn 从 Base 查回原名并标 STALE_META。
    #[test]
    fn overlay_nameless_upsert_preserves_base_name_and_marks_stale() {
        let e = engine(); // Base 中 readme.txt(frn101, size1024)
        e.set_scanner(Box::new(FakeScanner {
            changes: vec![Change::Upsert(RawEntry {
                frn: 101,
                parent_frn: 100,
                name: String::new(), // 写数据事件：USN 不带名字
                is_dir: false,
                attrs: 0,
                size: 0, // USN 不含大小
                mtime: 999,
                ctime: 999,
                atime: 999,
            })],
            next_usn: 2,
            full: vec![],
            unavailable: false,
        }));
        e.poll_once();
        let sum = e.search(req("readme")).unwrap();
        assert_eq!(sum.total, 1, "空名增量不得让已存在文件从搜索结果消失");
        let rows = e.range(sum.id, 0, 10).unwrap();
        assert_eq!(rows[0].name, "readme.txt", "应保留 Base 原名，而非空串");
        assert_eq!(
            rows[0].size, None,
            "USN 无 size，应标 stale 显示占位而非 0 B"
        );
    }

    #[test]
    fn overlay_remove_excluded_from_search() {
        let e = engine();
        e.set_scanner(Box::new(FakeScanner {
            changes: vec![Change::Remove { frn: 101 }], // 删除 readme.txt(frn101)
            next_usn: 2,
            full: vec![],
            unavailable: false,
        }));
        e.poll_once();
        let sum = e.search(req("readme")).unwrap();
        assert_eq!(sum.total, 0, "被删除的 base 条目不应再出现");
    }

    /// #22 回归：同一批里「FILE_CREATE（带名）→ 写入（空名）」时，空名记录必须
    /// 沿用本 Overlay 已有的名字。Base 里没有这个新文件，旧实现只查 Base 会拿到
    /// 空名并把带名条目覆盖掉，文件直接从搜索结果消失。
    #[test]
    fn overlay_nameless_upsert_keeps_same_batch_new_name() {
        let e = engine();
        e.set_scanner(Box::new(FakeScanner {
            changes: vec![
                Change::Upsert(RawEntry {
                    frn: 9100,
                    parent_frn: 100,
                    name: "newborn.txt".to_string(),
                    is_dir: false,
                    attrs: 0,
                    size: 0,
                    mtime: 1,
                    ctime: 1,
                    atime: 1,
                }),
                // 同批后续写入：USN 不带名字。
                Change::Upsert(RawEntry {
                    frn: 9100,
                    parent_frn: 100,
                    name: String::new(),
                    is_dir: false,
                    attrs: 0,
                    size: 0,
                    mtime: 2,
                    ctime: 2,
                    atime: 2,
                }),
            ],
            next_usn: 3,
            full: vec![],
            unavailable: false,
        }));
        e.poll_once();
        let sum = e.search(req("newborn")).unwrap();
        assert_eq!(sum.total, 1, "空名记录不得把同批新建条目覆盖成空名");
        let rows = e.range(sum.id, 0, 10).unwrap();
        assert_eq!(rows[0].name, "newborn.txt");
    }

    /// #21 回归：索引换代（后台合并 / 重建）后旧结果集的 `EntryId` 已错位，
    /// `range` / `open` 必须返回 NotFound 让上层重搜，而不是越界 panic 或串行到别的文件。
    #[test]
    fn range_and_open_reject_stale_generation() {
        let e = engine();
        let sum = e.search(req("txt")).unwrap();
        assert_eq!(sum.total, 2);
        assert_eq!(e.range(sum.id, 0, 10).unwrap().len(), 2);

        // 模拟后台换代（合并 Overlay / 全量重建都会 bump）。
        {
            let vid = VolumeId {
                serial: 1,
                drive: Some('C'),
            };
            let mut vols = e.inner.volumes.write_or_recover();
            vols.get_mut(&vid).unwrap().generation += 1;
        }
        assert!(
            matches!(e.range(sum.id, 0, 10), Err(EngineError::NotFound)),
            "换代后 range 应报 NotFound（前端据此重搜）"
        );
        assert!(
            matches!(
                e.open(sum.id, 0, OpenAction::CopyName),
                Err(EngineError::NotFound)
            ),
            "换代后 open 不应拿新索引去解读旧结果"
        );
    }

    #[test]
    fn journal_unavailable_triggers_rescan() {
        let e = engine();
        // 注入：full_scan 产出 rescanned.txt；read_changes 返回 JournalUnavailable
        e.set_scanner(Box::new(FakeScanner {
            changes: vec![],
            next_usn: 0,
            full: vec![
                RawEntry {
                    frn: 5,
                    parent_frn: 0,
                    name: ".".to_string(),
                    is_dir: true,
                    attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
                    size: 0,
                    mtime: 1,
                    ctime: 1,
                    atime: 1,
                },
                RawEntry {
                    frn: 777,
                    parent_frn: 5,
                    name: "rescanned.txt".to_string(),
                    is_dir: false,
                    attrs: 0,
                    size: 999,
                    mtime: 2000,
                    ctime: 2000,
                    atime: 2000,
                },
            ],
            unavailable: true,
        }));
        e.poll_once();
        // 触发重建：base 现在应是 full_scan 的结果
        let st = e.status();
        assert_eq!(st.volumes[0].state, VolumeState::Ready);
        let sum = e.search(req("rescanned")).unwrap();
        assert_eq!(sum.total, 1, "重建后应能搜到 full_scan 产出的条目");
        // 旧的 readme.txt 已被重建覆盖
        let old = e.search(req("readme")).unwrap();
        assert_eq!(old.total, 0);
    }

    /// 回归测试：Bug #6 —— 索引完成后无限循环重建。
    ///
    /// 真实 Win32 语义（`scanner.rs::read_changes`）：`from_usn` 落后于日志首条
    /// `first_usn` 时返回 `JournalUnavailable`（记录已被回收）。
    /// 旧代码 `load_volume_with` 丢弃 `full_scan` 的 `ScanMeta`，把 `next_usn` 写成 0，
    /// 于是每轮轮询都以 `from_usn = 0` 续读 → 恒不可用 → `rebuild_volume` → 再全量
    /// 扫描 —— 每卷「扫描耗时 + 1s」无限重建，M2 的 USN 增量在真实运行中从未生效。
    ///
    /// 断言：`rescan` 后连续三轮 `poll_once`，`full_scan` 只被调用 **1** 次。
    #[test]
    fn scan_meta_is_kept_so_poll_does_not_rebuild_forever() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        const FIRST_USN: i64 = 500; // 日志首条；from_usn < 此值即判定"已回收"
        const SCANNED_USN: i64 = 1000; // full_scan 在扫 MFT 前记录的续读起点

        /// 模拟真实 USN 语义，并统计 `full_scan` 调用次数。
        struct UsnScanner {
            full_scans: std::sync::Arc<AtomicUsize>,
        }
        impl VolumeScanner for UsnScanner {
            fn full_scan(
                &self,
                _vol: &VolumeId,
                sink: &mut dyn FnMut(RawEntry),
            ) -> ff_ntfs::Result<ScanMeta> {
                self.full_scans.fetch_add(1, AtomicOrdering::SeqCst);
                sink(RawEntry {
                    frn: 5,
                    parent_frn: 0,
                    name: ".".to_string(),
                    is_dir: true,
                    attrs: attrs::FILE_ATTRIBUTE_DIRECTORY,
                    size: 0,
                    mtime: 1,
                    ctime: 1,
                    atime: 1,
                });
                Ok(ScanMeta {
                    journal_id: 1,
                    next_usn: SCANNED_USN,
                    entries: 1,
                    elapsed: Duration::ZERO,
                    warnings: 0,
                })
            }
            fn read_changes(
                &self,
                _vol: &VolumeId,
                from: i64,
                _jid: u64,
            ) -> ff_ntfs::Result<(Vec<Change>, i64)> {
                // 与真实实现同构：落后于日志首条即不可用（否则返回空增量，起点不变）。
                if from < FIRST_USN {
                    return Err(ScanError::JournalUnavailable(format!(
                        "from_usn {from} 早于日志首条 {FIRST_USN}，记录已回收"
                    )));
                }
                Ok((vec![], from))
            }
        }

        let e = engine();
        let scans = std::sync::Arc::new(AtomicUsize::new(0));
        e.set_scanner(Box::new(UsnScanner {
            full_scans: scans.clone(),
        }));
        let vid = VolumeId {
            serial: 1,
            drive: Some('C'),
        };

        e.rescan(Some(vid)).unwrap();
        assert_eq!(
            scans.load(AtomicOrdering::SeqCst),
            1,
            "rescan 应恰好做一次全量扫描"
        );

        // 修复前：next_usn 为 0 → 每轮轮询都触发一次重建（此处会断言失败）。
        for round in 1..=3 {
            e.poll_once();
            assert_eq!(
                scans.load(AtomicOrdering::SeqCst),
                1,
                "第 {round} 轮轮询不应触发重建：\
                 续读起点应取自 ScanMeta（{SCANNED_USN}）而非 0"
            );
        }
        assert_eq!(e.status().volumes[0].state, VolumeState::Ready);
    }

    /// 回归测试：复现 AB-BA 死锁（scanner Mutex vs volumes RwLock）。
    ///
    /// 旧代码路径：
    /// - 扫描线程：`rescan_impl` 持 scanner 锁 → `load_volume_with` → full_scan 完成后要 volumes.write()
    /// - 后台线程：`poll_volume` 持 volumes.read() → 要 scanner 锁
    ///
    /// 当卷 A 已 Ready、卷 B 正在扫描时，后台轮询 A 持 read 等 scanner，扫描线程持 scanner 等 write → 死锁。
    ///
    /// 修复后：poll_volume 先释放 volumes.read() 再取 scanner 锁，打破环路。
    #[test]
    fn background_poll_does_not_deadlock_with_rescan() {
        use std::time::{Duration, Instant};

        struct SlowScanner;
        impl VolumeScanner for SlowScanner {
            fn full_scan(
                &self,
                vol: &VolumeId,
                sink: &mut dyn FnMut(RawEntry),
            ) -> ff_ntfs::Result<ScanMeta> {
                // 卷 B 扫描耗时 2.5s（> 后台 1s 轮询周期），确保后台在扫描期间轮询已 Ready 的卷 A。
                if vol.drive == Some('B') {
                    std::thread::sleep(Duration::from_millis(2500));
                }
                sink(RawEntry {
                    frn: 5,
                    parent_frn: 0,
                    name: ".".to_string(),
                    is_dir: true,
                    attrs: ff_core::attrs::FILE_ATTRIBUTE_DIRECTORY,
                    size: 0,
                    mtime: 0,
                    ctime: 0,
                    atime: 0,
                });
                Ok(ScanMeta {
                    journal_id: 1,
                    next_usn: 0,
                    entries: 1,
                    elapsed: Duration::ZERO,
                    warnings: 0,
                })
            }
            fn read_changes(
                &self,
                _v: &VolumeId,
                _from: i64,
                _jid: u64,
            ) -> ff_ntfs::Result<(Vec<Change>, i64)> {
                Ok((vec![], 0))
            }
        }

        // 用 new() 构造（启动 background_loop），预置卷 A 为 Ready。
        let e = Arc::new(InProcessEngine::new(Box::new(SlowScanner)));
        let info_a = VolumeInfo {
            drive: Some('A'),
            serial: 1,
            ..Default::default()
        };
        let mut b = IndexBuilder::new(info_a);
        b.push(EntryInput {
            frn: 5,
            parent_frn: 0,
            name: ".".to_string(),
            is_dir: true,
            attrs: ff_core::attrs::FILE_ATTRIBUTE_DIRECTORY,
            size: 0,
            mtime: 0,
            ctime: 0,
            atime: 0,
        });
        e.add_volume(
            VolumeId {
                serial: 1,
                drive: Some('A'),
            },
            b.finish(),
        );

        // 等后台线程进入轮询节奏（100ms × 10 = 1s 周期）。
        std::thread::sleep(Duration::from_millis(300));

        // 在另一线程 rescan 卷 B（SlowScanner 会 sleep 2.5s 模拟慢扫描）。
        let e2 = e.clone();
        let h = std::thread::spawn(move || {
            let _ = e2.rescan(Some(VolumeId {
                serial: 2,
                drive: Some('B'),
            }));
        });

        // 8s 内必须完成；否则判定死锁。
        let start = Instant::now();
        while !h.is_finished() {
            if start.elapsed() > Duration::from_secs(8) {
                panic!(
                    "死锁：rescan(B) 在 8s 内未完成 — scanner Mutex 与 volumes RwLock 的 AB-BA 死锁"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        h.join().unwrap();

        // 验证卷 B 确实完成了扫描（state = Ready）。
        let st = e.status();
        let b_vol = st.volumes.iter().find(|v| v.volume.contains("B:"));
        assert!(
            b_vol.is_some_and(|v| v.state == VolumeState::Ready),
            "卷 B 应为 Ready，实际: {:?}",
            b_vol
        );
    }

    #[test]
    fn engine_persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("sift_ffix_{}", now_filetime()));
        std::fs::create_dir_all(&dir).unwrap();
        let vid = VolumeId {
            serial: 1,
            drive: Some('C'),
        };
        let e1 = InProcessEngine::empty();
        e1.add_volume(vid.clone(), sample_index());
        e1.set_save_dir(&dir);
        e1.save_all().unwrap();

        let e2 = InProcessEngine::empty();
        e2.set_save_dir(&dir);
        e2.load_all(&dir).unwrap();
        let st = e2.status();
        assert_eq!(st.total_entries, 5, "载入后条目数应与原索引一致");
        let sum = e2.search(req("txt")).unwrap();
        assert_eq!(sum.total, 2, "载入后可正常搜索");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ───────────────────── P0-3 / P0-2 新增单测 ─────────────────────

    /// `to_utf16_ranges`：字节偏移 → JS 可用的 UTF-16 单元偏移。
    #[test]
    fn to_utf16_ranges_chinese_and_emoji_and_overlap() {
        // 单区间收集成 Vec，避免 clippy 的 single_range_in_vec_init 误读 `[7..10]`。
        let one =
            |start: u32, end: u32| -> Vec<Range<u32>> { std::iter::once(start..end).collect() };
        // 纯 ASCII：字节 == UTF-16 单元。
        assert_eq!(to_utf16_ranges("readme.txt", &one(7, 10)), one(7, 10));
        // 中文「文档」= 2 字 × 3 字节，但 UTF-16 各 1 单元 → "txt" 的字节 7..10 映射到单元 3..6。
        // （"文档.txt" 共 10 字节：文0..3 档3..6 .6 t7 x8 t9）
        assert_eq!(to_utf16_ranges("文档.txt", &one(7, 10)), one(3, 6));
        // emoji「😀」= 4 字节、2 个 UTF-16 单元（代理对）。字符串 "a😀b"，字节：
        // a=0, 😀=1..5, b=5。"😀" 的字节 1..5 应映射为单元 1..3。
        assert_eq!(to_utf16_ranges("a😀b", &one(1, 5)), one(1, 3));
        // 空区间 / 空名。
        assert!(to_utf16_ranges("x", &[]).is_empty());
        // E6：越界端点被钳制，非法区间被丢弃，均不 panic。
        assert_eq!(to_utf16_ranges("abc", &one(1, 999)), one(1, 3));
        assert!(to_utf16_ranges("abc", &one(5, 9)).is_empty()); // 全越界
        assert!(to_utf16_ranges("abc", &one(2, 2)).is_empty()); // end == start
        assert!(to_utf16_ranges("abc", &one(3, 1)).is_empty()); // end < start
    }

    /// P0-1：Scanning 但有旧索引（entry_count>0）的卷仍可搜索；纯 Scanning（无索引）跳过。
    #[test]
    fn scanning_volume_with_old_index_is_searchable() {
        let e = InProcessEngine::empty();
        let vid = VolumeId {
            serial: 1,
            drive: Some('C'),
        };
        e.add_volume(vid.clone(), sample_index());
        // 标记为「正在刷新」（有旧索引）。
        {
            let inner = &e.inner;
            let mut vols = inner.volumes.write_or_recover();
            vols.get_mut(&vid).unwrap().state = VolumeState::Scanning;
        }
        let sum = e.search(req("readme")).unwrap();
        assert_eq!(sum.total, 1, "刷新中的旧索引仍应可搜");
        let st = e.status();
        assert!(st.volumes[0].stale, "Scanning 且有索引应标记 stale");

        // 撤销索引（清空为无条目）再标 Scanning → 不应参与搜索。
        {
            let inner = &e.inner;
            let mut vols = inner.volumes.write_or_recover();
            let vd = vols.get_mut(&vid).unwrap();
            vd.index = IndexBuilder::new(VolumeInfo {
                drive: Some('C'),
                ..Default::default()
            })
            .finish();
            vd.state = VolumeState::Scanning;
        }
        let sum2 = e.search(req("readme")).unwrap();
        assert_eq!(sum2.total, 0, "首次建索引（无可用索引）的卷应跳过");
    }

    /// P1-2：`needs_path_context` 控制 Overlay 路径上下文的按需计算。
    #[test]
    fn needs_path_context_only_when_required() {
        assert!(!parse("report").unwrap().needs_path_context());
        assert!(!parse("ext:pdf").unwrap().needs_path_context());
        assert!(parse("path:Docs").unwrap().needs_path_context());
        assert!(parse("infolder:Docs").unwrap().needs_path_context());
        assert!(parse("parent:Docs").unwrap().needs_path_context());
        assert!(parse("depth:2").unwrap().needs_path_context());
        assert!(parse("report path:Docs").unwrap().needs_path_context());
    }

    /// P0-5：索引变化后置脏，落盘只发生在脏时；写盘失败透传。
    #[test]
    fn dirty_only_saves_on_change() {
        let dir = std::env::temp_dir().join(format!("sift_dirty_{}", now_filetime()));
        std::fs::create_dir_all(&dir).unwrap();
        let vid = VolumeId {
            serial: 1,
            drive: Some('C'),
        };
        let e = InProcessEngine::empty();
        e.add_volume(vid.clone(), sample_index());
        e.set_save_dir(&dir);
        // 注入：此时 dirty 应为 true（add_volume 置脏）。maybe_save 落盘后清零。
        assert!(e.inner.dirty.load(Ordering::SeqCst));
        e.inner.maybe_save();
        assert!(!e.inner.dirty.load(Ordering::SeqCst), "落盘后 dirty 应清零");
        // 再次 maybe_save 不应重写（dirty 为 false 直接返回，且文件 mtime 不变）。
        // 注意：sample_index() 的 VolumeInfo.serial 默认 0，落盘文件名为 `0.ffix`。
        let p = dir.join("0.ffix");
        let m1 = std::fs::metadata(&p).unwrap().modified().unwrap();
        e.inner.maybe_save();
        let m2 = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(m1, m2, "非脏时不应重写磁盘");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P0-2：参数构造函数对 `& % ^ ( )` 与空格 / 中文正确加引号（Windows 单测）。
    #[test]
    #[cfg(windows)]
    fn explorer_select_arg_quotes_special_chars() {
        assert_eq!(
            explorer_select_arg("D:\\a b\\x&y.exe"),
            "/select,\"D:\\a b\\x&y.exe\""
        );
        assert_eq!(
            explorer_select_arg("C:\\中文 (1).txt"),
            "/select,\"C:\\中文 (1).txt\""
        );
    }

    #[test]
    fn wide_with_nul_handles_unicode_and_empty() {
        assert_eq!(wide_with_nul(""), vec![0u16]);
        assert_eq!(wide_with_nul("ab"), vec![b'a' as u16, b'b' as u16, 0]);
        // emoji 是代理对（2 个 UTF-16 单元），中文 1 个；结尾恒有 NUL。
        let w = wide_with_nul("中😀");
        assert_eq!(w, vec![0x4E2D, 0xD83D, 0xDE00, 0]);
    }

    #[test]
    fn file_name_from_full_strips_dirs_and_trailing_sep() {
        assert_eq!(file_name_from_full("C:\\Work\\report.txt"), "report.txt");
        assert_eq!(file_name_from_full("D:/a/b/中文 名.md"), "中文 名.md");
        assert_eq!(file_name_from_full("C:\\Windows\\"), "Windows");
        assert_eq!(file_name_from_full("C:\\"), "C:");
        assert_eq!(file_name_from_full("readme.txt"), "readme.txt");
    }

    /// P0-2：真实剪贴板往返。先备份原文本，测试结束恢复，避免污染用户剪贴板；
    /// 原剪贴板不是纯文本时直接跳过本次验证（不破坏图片等格式）。
    #[test]
    #[cfg(windows)]
    fn clipboard_roundtrip_writes_unicode_and_restores_previous() {
        use std::ffi::c_void;
        use std::ptr::null_mut;

        const CF_UNICODETEXT: u32 = 13;
        #[link(name = "user32")]
        extern "system" {
            fn OpenClipboard(hwnd: *mut c_void) -> i32;
            fn IsClipboardFormatAvailable(format: u32) -> i32;
            fn GetClipboardData(format: u32) -> *mut c_void;
            fn CloseClipboard() -> i32;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalLock(mem: *mut c_void) -> *mut c_void;
            fn GlobalUnlock(mem: *mut c_void) -> i32;
        }

        /// 读取当前剪贴板文本；None = 本次验证应跳过
        /// （无文本格式 / 剪贴板被占用），避免覆盖图片等非文本内容。
        fn read_clipboard_text() -> Option<String> {
            unsafe {
                if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                    return None;
                }
                if OpenClipboard(null_mut()) == 0 {
                    return None;
                }
                let h = GetClipboardData(CF_UNICODETEXT);
                if h.is_null() {
                    CloseClipboard();
                    return Some(String::new());
                }
                let p = GlobalLock(h).cast::<u16>();
                let mut s = String::new();
                if !p.is_null() {
                    let mut i = 0usize;
                    while *p.add(i) != 0 {
                        i += 1;
                    }
                    s = String::from_utf16_lossy(std::slice::from_raw_parts(p, i));
                    GlobalUnlock(h);
                }
                CloseClipboard();
                Some(s)
            }
        }

        let previous = match read_clipboard_text() {
            Some(s) => s,
            None => {
                eprintln!("clipboard roundtrip SKIPPED: 无文本格式或剪贴板被占用");
                return; // 不动用户剪贴板
            }
        };

        let probe = "Sift 剪贴板自测 😀 C:\\中文 目录\\文件.txt";
        assert!(clipboard_set_text(probe).is_ok(), "写剪贴板应成功");
        assert_eq!(read_clipboard_text().as_deref(), Some(probe));

        // 恢复原内容（原为空时写空串，等价于清空）。
        assert!(clipboard_set_text(&previous).is_ok());
        assert_eq!(read_clipboard_text().as_deref(), Some(previous.as_str()));
        eprintln!("clipboard roundtrip OK: 写入并回读 {} 字节", probe.len());
    }
}

#[cfg(test)]
mod non_ntfs_volume_tests {
    //! M7（DESIGN §5.4）：无增量通道的卷（目录遍历回退）在引擎里的行为。

    use super::*;
    use ff_ntfs::{Change, ScanMeta};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 搜索请求（与 `tests` 模块里的同名辅助一致，保持断言口径统一）。
    fn req(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            sort: SortSpec::default(),
            max_hits: None,
            opts: crate::api::SearchOptions::default(),
        }
    }

    /// 「遍历型」假扫描器：`full_scan` 计数，`read_changes` 计数且声明无增量。
    struct WalkFakeScanner {
        full: Vec<RawEntry>,
        full_calls: Arc<AtomicU64>,
        change_calls: Arc<AtomicU64>,
    }

    impl VolumeScanner for WalkFakeScanner {
        fn full_scan(
            &self,
            _vol: &VolumeId,
            sink: &mut dyn FnMut(RawEntry),
        ) -> ff_ntfs::Result<ScanMeta> {
            self.full_calls.fetch_add(1, Ordering::SeqCst);
            for e in &self.full {
                sink(e.clone());
            }
            Ok(ScanMeta {
                // 遍历卷没有 journal：两个字段就是 0，引擎不该拿它们去读日志。
                journal_id: 0,
                next_usn: 0,
                entries: self.full.len() as u64,
                elapsed: Duration::ZERO,
                warnings: 0,
            })
        }

        fn read_changes(
            &self,
            _vol: &VolumeId,
            _from: i64,
            _jid: u64,
        ) -> ff_ntfs::Result<(Vec<Change>, i64)> {
            self.change_calls.fetch_add(1, Ordering::SeqCst);
            Err(ScanError::Unsupported("no usn on walked volume".into()))
        }

        fn supports_incremental(&self, _vol: &VolumeId) -> bool {
            false
        }
    }

    fn raw(frn: u64, parent: u64, name: &str, is_dir: bool) -> RawEntry {
        RawEntry {
            frn,
            parent_frn: parent,
            name: name.to_string(),
            is_dir,
            attrs: 0,
            size: if is_dir { 0 } else { 42 },
            mtime: 1,
            ctime: 1,
            atime: 1,
        }
    }

    /// 走一遍「遍历卷建索引 → 多轮轮询」：索引可搜，且不会被当成 USN 卷空转。
    #[test]
    fn walked_volume_is_searchable_and_not_polled_for_usn() {
        let e = InProcessEngine::empty();
        let full_calls = Arc::new(AtomicU64::new(0));
        let change_calls = Arc::new(AtomicU64::new(0));
        let vol = VolumeId {
            serial: 0xE7A1,
            drive: Some('F'),
        };
        e.set_scanner(Box::new(WalkFakeScanner {
            // 根条目（parent 0，名字不参与路径）+ Windows 目录 + 一个文件，
            // 结构照搬真实遍历输出，用来验证路径拼接。
            full: vec![
                raw(11, 0, ".", true),
                raw(12, 11, "Windows", true),
                raw(13, 12, "walked_here.txt", false),
            ],
            full_calls: full_calls.clone(),
            change_calls: change_calls.clone(),
        }));

        e.rescan(Some(vol.clone())).expect("遍历卷应能建索引");
        assert_eq!(full_calls.load(Ordering::SeqCst), 1);

        // 5 轮轮询：无增量卷一次都不该去读 USN，也不该在 30s 内重复全量重走。
        for _ in 0..5 {
            e.poll_once();
        }
        assert_eq!(
            change_calls.load(Ordering::SeqCst),
            0,
            "无增量卷不应每轮空读 read_changes"
        );
        assert_eq!(
            full_calls.load(Ordering::SeqCst),
            1,
            "未到间隔不应重复全量重走"
        );

        let sum = e.search(req("walked_here")).unwrap();
        assert_eq!(sum.total, 1, "遍历卷的条目必须能被搜到");
        let rows = e.range(sum.id, 0, 1).unwrap();
        assert_eq!(rows[0].name, "walked_here.txt");
        assert_eq!(
            rows[0].dir, "F:\\Windows",
            "路径按 parent 链拼出，根名不重复"
        );
        assert_eq!(rows[0].size, Some(42));
    }

    /// 间隔边界：30s 前不重走，到点重走（只测判定，不真等）。
    #[test]
    fn walk_rebuild_interval_boundary() {
        assert!(!walk_due(WALK_REBUILD_INTERVAL - Duration::from_millis(1)));
        assert!(!walk_due(Duration::ZERO));
        assert!(walk_due(WALK_REBUILD_INTERVAL));
        assert!(walk_due(WALK_REBUILD_INTERVAL + Duration::from_secs(5)));
    }

    // ───────────── 索引复用回归：落盘续读起点 + 启动对账 ─────────────

    /// 落盘必须携带权威 journal 起点（next_usn / journal_id），否则重启后
    /// 从 0 续读 → JournalUnavailable → 全量重建，磁盘缓存形同虚设。
    #[test]
    fn save_persists_runtime_usn_position_not_index_snapshot() {
        let dir = std::env::temp_dir().join(format!("sift_usn_pos_{}", now_filetime()));
        std::fs::create_dir_all(&dir).unwrap();
        let vid = VolumeId {
            serial: 7,
            drive: Some('D'),
        };

        // 索引快照里的 volume 是默认值（next_usn=0）；运行时的 vd.info 带真实起点。
        let entry = |frn, parent, name: &str, is_dir| EntryInput {
            frn,
            parent_frn: parent,
            name: name.to_string(),
            is_dir,
            attrs: 0,
            size: 42,
            mtime: 1,
            ctime: 1,
            atime: 1,
        };
        let mut b = IndexBuilder::new(VolumeInfo {
            drive: Some('D'),
            serial: 7,
            ..Default::default()
        });
        b.push(entry(5, 0, ".", true));
        b.push(entry(100, 5, "reuse_me.txt", false));
        let snap = b.finish();
        let e = InProcessEngine::empty();
        e.add_volume(vid.clone(), snap);

        // 通过增量轮询推进运行时 next_usn（Overlay 变更路径），index 快照不会跟着变。
        {
            let mut vols = e.inner.volumes.write_or_recover();
            let vd = vols.get_mut(&vid).unwrap();
            vd.info.journal_id = 0xABCD;
            vd.info.next_usn = 987_654;
        }
        e.set_save_dir(&dir);
        e.save_all().unwrap();

        // 重新载入：索引元数据必须带推进后的起点，而不是快照的 0。
        let e2 = InProcessEngine::empty();
        e2.set_save_dir(&dir);
        e2.load_all(&dir).unwrap();
        {
            let vols = e2.inner.volumes.read_or_recover();
            let vd = vols.get(&vid).expect("卷应从磁盘载入");
            assert_eq!(
                vd.info.journal_id, 0xABCD,
                "落盘应携带运行时 journal_id"
            );
            assert_eq!(
                vd.info.next_usn, 987_654,
                "落盘应携带推进后的 next_usn（而非索引快照的 0）"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// startup_reconcile：磁盘有可续读索引的卷不应再被全扫；新卷/空索引才需重建。
    #[test]
    fn startup_reconcile_only_returns_missing_or_empty_volumes() {
        let e = InProcessEngine::empty();
        let reused = VolumeId {
            serial: 10,
            drive: Some('C'),
        };
        let brand_new = VolumeId {
            serial: 11,
            drive: Some('D'),
        };
        // 非空 Ready 索引 → 可复用
        let entry = |frn, parent, name: &str, is_dir| EntryInput {
            frn,
            parent_frn: parent,
            name: name.to_string(),
            is_dir,
            attrs: 0,
            size: 42,
            mtime: 1,
            ctime: 1,
            atime: 1,
        };
        let mut b = IndexBuilder::new(VolumeInfo {
            drive: Some('C'),
            ..Default::default()
        });
        b.push(entry(5, 0, ".", true));
        b.push(entry(100, 5, "reuse_me.txt", false));
        e.add_volume(reused.clone(), b.finish());
        // brand_new 完全不在 vols 里 → 需要全扫

        let need = e.startup_reconcile(&[reused.clone(), brand_new.clone()]);
        assert_eq!(need, vec![brand_new], "只有缺失索引的卷需要全量重建");
        assert!(!need.contains(&reused), "已有磁盘索引的卷必须复用，不得重扫");
    }
}
