//! `EngineClient` 的 InProcess 实现（DESIGN §9，M1–M2）。
//!
//! 直接用 `ff-core` 的 [`VolumeIndex`] 承接 `ff-ntfs` 的 [`RawEntry`]，在进程内
//! 打通 `search` / `range`（含排序与 `ResultRow` 构造）/ `cancel` / `status` /
//! `rescan` / `open`。不依赖任何 Windows 专有 API，保证 `ff-engine` 跨平台可编译。
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
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ff_core::{
    attrs, flags, parse, simple_lowercase, CancelToken, EntryId, EntryInput, IndexBuilder, Query,
    VolumeIndex, VolumeInfo, NONE,
};
use ff_match::{Matcher, SubstringMatcher};
use ff_ntfs::{RawEntry, ScanError, VolumeId, VolumeScanner};

use crate::api::{
    EngineClient, EngineError, EngineStatus, OpenAction, RequestId, Result, ResultRow,
    SearchRequest, SearchSummary, SortKey, SortSpec, VolumeState, VolumeStatus,
};
use crate::overlay::{Overlay, OverlayEntry};

/// Overlay 合并阈值（DESIGN §4.3：增量条目超过此数即落入 Base）。
const MERGE_THRESHOLD: usize = 50_000;
/// 落盘节流间隔（后台线程最慢每 `SAVE_INTERVAL` 落盘一次）。
const SAVE_INTERVAL: Duration = Duration::from_secs(10);
/// 结果集无访问释放时限（DESIGN §9：60 秒）。
const RESULT_TTL: Duration = Duration::from_secs(60);

/// 单卷运行时数据（M2：新增 `overlay` 增量层）。
struct VolumeData {
    info: VolumeInfo,
    index: VolumeIndex,
    overlay: Overlay,
    state: VolumeState,
}

/// 一次搜索的持久结果：命中列表（全局有序，每条带所属卷 + 命中来源）+ 原查询（供高亮）。
struct StoredResult {
    hits: Vec<(VolumeId, Hit)>,
    query: Query,
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
    /// 结果集：`search` 分配的新 id 覆盖上一次（单客户端语义），旧 id 自然失效。
    results: Mutex<HashMap<RequestId, Arc<StoredResult>>>,
    /// 每个结果集的最后访问时间（供 60 秒无访问释放判断，DESIGN §9）。
    result_access: Mutex<HashMap<RequestId, Instant>>,
    /// 每个请求的取消令牌（被 `cancel(id)` 触发）。
    cancels: Mutex<HashMap<RequestId, CancelToken>>,
    /// 请求计数器（从 1 起，0 视为无效）。
    request_counter: AtomicU64,
    /// 是否正在搜索（`status().searching` 来源）。
    searching: AtomicBool,
    /// 最近一次成功保存索引的时间（FILETIME；从未保存为 `None`）。
    last_saved: Mutex<Option<i64>>,
    /// 落盘目录（`None` 表示不持久化）。
    save_dir: Mutex<Option<PathBuf>>,
    /// 上次尝试落盘的时间（节流用）。
    last_save_attempt: Mutex<Option<Instant>>,
    /// 后台线程退出标志。
    bg_running: AtomicBool,
}

impl EngineInner {
    fn new(scanner: Option<Box<dyn VolumeScanner + Send + Sync>>) -> Self {
        EngineInner {
            volumes: RwLock::new(HashMap::new()),
            scanner: Mutex::new(scanner),
            results: Mutex::new(HashMap::new()),
            result_access: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            request_counter: AtomicU64::new(1),
            searching: AtomicBool::new(false),
            last_saved: Mutex::new(None),
            save_dir: Mutex::new(None),
            last_save_attempt: Mutex::new(None),
            bg_running: AtomicBool::new(true),
        }
    }

    /// 直接注入一个已建好的索引（测试与合成数据用；标记为 `Ready`）。
    fn add_volume(&self, vol: VolumeId, index: VolumeIndex) {
        let info = index.volume.clone();
        let mut vols = self.volumes.write().unwrap();
        vols.insert(
            vol,
            VolumeData {
                info,
                index,
                overlay: Overlay::default(),
                state: VolumeState::Ready,
            },
        );
    }

    /// 经扫描器全量重建单卷索引（见 trait `rescan`，M2-③ 重建也走这里）。
    fn load_volume_with(&self, scanner: &dyn VolumeScanner, vol: &VolumeId) -> Result<()> {
        // 先标记 Scanning
        {
            let mut vols = self.volumes.write().unwrap();
            let vd = vols.entry(vol.clone()).or_insert_with(|| blank_volume(vol));
            vd.state = VolumeState::Scanning;
        }

        let info = VolumeInfo {
            serial: vol.serial,
            drive: vol.drive,
            ..Default::default()
        };
        let mut builder = IndexBuilder::new(info.clone());
        // 流式喂入 RawEntry → EntryInput（字段一一对应，可直接映射）
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
        });

        match res {
            Ok(_meta) => {
                let index = builder.finish();
                let mut vols = self.volumes.write().unwrap();
                let vd = vols
                    .get_mut(vol)
                    .expect("load_volume_with: 卷在 Scanning 阶段已插入");
                vd.info = info;
                vd.index = index;
                vd.overlay = Overlay::default();
                vd.state = VolumeState::Ready;
                Ok(())
            }
            Err(e) => {
                let mut vols = self.volumes.write().unwrap();
                if let Some(vd) = vols.get_mut(vol) {
                    vd.state = VolumeState::Error;
                }
                Err(map_scan_err(e))
            }
        }
    }

    fn search_impl(&self, req: SearchRequest) -> Result<SearchSummary> {
        let start = Instant::now();
        let id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        self.searching.store(true, Ordering::SeqCst);
        // 新搜索覆盖上一次（单客户端语义）
        {
            let mut results = self.results.lock().unwrap();
            results.clear();
            let mut access = self.result_access.lock().unwrap();
            access.clear();
        }
        let token = CancelToken::new();
        self.cancels.lock().unwrap().insert(id, token.clone());

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
                });
            }
        };
        let mut query = parsed;
        query.case_sensitive = req.opts.case_sensitive;

        // 在所有 Ready 卷上求并集命中：Base + Overlay 合并
        let hide_system = req.opts.hide_system;
        let hide_hidden = req.opts.hide_hidden;
        let mut hits: Vec<(VolumeId, Hit)> = Vec::new();
        {
            let vols = self.volumes.read().unwrap();
            for (vid, vd) in vols.iter() {
                if vd.state != VolumeState::Ready {
                    continue;
                }
                // —— Base 命中 ——
                for eid in vd.index.search(&query, &token) {
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
                    hits.push((vid.clone(), Hit::Base(eid)));
                }
                // —— Overlay 增量命中 ——
                for (frn, oe) in vd.overlay.added_or_changed.iter() {
                    let f = oe.flags();
                    if hide_system && f & flags::SYSTEM != 0 {
                        continue;
                    }
                    if hide_hidden && f & flags::HIDDEN != 0 {
                        continue;
                    }
                    if !overlay_matches(&query, &oe.name) {
                        continue;
                    }
                    hits.push((vid.clone(), Hit::Overlay(*frn)));
                }
            }
            // 排序（Score 在 M1 无评分，按来源+id 升序近似；见 `sort_hits`）
            sort_hits(&mut hits, &vols, &req.sort);
        }

        // max_hits 截断（排序后）
        if let Some(k) = req.max_hits {
            hits.truncate(k);
        }

        let total = hits.len();
        {
            let mut results = self.results.lock().unwrap();
            results.insert(
                id,
                Arc::new(StoredResult {
                    hits,
                    query: query.clone(),
                }),
            );
            let mut access = self.result_access.lock().unwrap();
            access.insert(id, Instant::now());
        }
        self.searching.store(false, Ordering::SeqCst);
        Ok(SearchSummary {
            id,
            total,
            elapsed_ms: start.elapsed().as_millis() as u32,
            parse_error: None,
        })
    }

    fn range_impl(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>> {
        // 先取窗口命中 + 查询（避免同时持有两个锁导致加锁顺序反转死锁）
        let (window, query) = {
            let results = self.results.lock().unwrap();
            let stored = results.get(&id).ok_or(EngineError::NotFound)?;
            let len = stored.hits.len();
            if offset >= len {
                return Ok(Vec::new());
            }
            let end = (offset + limit).min(len);
            (stored.hits[offset..end].to_vec(), stored.query.clone())
        };
        // 更新访问时间（60 秒无访问释放，DESIGN §9）
        self.result_access
            .lock()
            .unwrap()
            .insert(id, Instant::now());

        let vols = self.volumes.read().unwrap();
        let mut rows = Vec::with_capacity(window.len());
        for (vid, hit) in &window {
            rows.push(resolve_row(&vols, vid, hit, &query));
        }
        Ok(rows)
    }

    fn cancel_impl(&self, id: RequestId) {
        if let Some(tok) = self.cancels.lock().unwrap().get(&id) {
            tok.cancel();
        }
    }

    fn status_impl(&self) -> EngineStatus {
        let vols = self.volumes.read().unwrap();
        let mut volumes = Vec::new();
        let mut total_entries = 0u64;
        for (vid, vd) in vols.iter() {
            let entries = vd.index.entry_count() as u64;
            total_entries += entries;
            volumes.push(VolumeStatus {
                volume: vid.to_string(),
                state: vd.state,
                entries,
                progress: None,
                message: None,
            });
        }
        EngineStatus {
            volumes,
            total_entries,
            last_saved: *self.last_saved.lock().unwrap(),
            searching: self.searching.load(Ordering::SeqCst),
        }
    }

    fn rescan_impl(&self, volume: Option<VolumeId>) -> Result<()> {
        let guard = self.scanner.lock().unwrap();
        let scanner = match guard.as_ref() {
            Some(s) => s,
            None => return Ok(()), // 无扫描器（测试 / 合成数据）：跳过
        };
        match volume {
            Some(v) => self.load_volume_with(&**scanner, &v),
            None => {
                let ids: Vec<VolumeId> = self.volumes.read().unwrap().keys().cloned().collect();
                for id in ids {
                    self.load_volume_with(&**scanner, &id)?;
                }
                Ok(())
            }
        }
    }

    fn open_impl(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()> {
        let hit = {
            let results = self.results.lock().unwrap();
            let stored = results.get(&id).ok_or(EngineError::NotFound)?;
            stored.hits.get(index).ok_or(EngineError::NotFound)?.clone()
        };
        self.result_access
            .lock()
            .unwrap()
            .insert(id, Instant::now());
        let vols = self.volumes.read().unwrap();
        let vd = vols.get(&hit.0).ok_or(EngineError::NotFound)?;
        let full = match &hit.1 {
            Hit::Base(eid) => vd.index.path_string(*eid).ok_or(EngineError::NotFound)?,
            Hit::Overlay(frn) => match vd.overlay.added_or_changed.get(frn) {
                Some(oe) => overlay_full_path(&vd.index, &vd.overlay, oe),
                None => return Err(EngineError::NotFound),
            },
        };
        drop(vols);
        match action {
            // 复制到剪贴板由 UI 层处理；InProcess 下暂视为成功
            OpenAction::CopyPath => Ok(()),
            OpenAction::Open => shell_open(&full, false),
            OpenAction::OpenFolder => shell_open(&full, true),
        }
    }

    // ───────────────────────── 后台轮询与合并（M2-②/③） ─────────────────────────

    /// 轮询所有 Ready 卷的增量变更（后台线程与测试 `poll_once` 共用）。
    fn poll_all(&self) {
        let vols: Vec<VolumeId> = self.volumes.read().unwrap().keys().cloned().collect();
        for vid in vols {
            self.poll_volume(&vid);
        }
    }

    /// 单卷增量轮询：读 `read_changes` → 应用 Overlay；`JournalUnavailable` 触发重建（③）。
    fn poll_volume(&self, vid: &VolumeId) {
        let res = {
            let vols = self.volumes.read().unwrap();
            let vd = match vols.get(vid) {
                Some(v) => v,
                None => return,
            };
            if vd.state != VolumeState::Ready {
                return;
            }
            let from_usn = vd.info.next_usn;
            let journal_id = vd.info.journal_id;
            let guard = self.scanner.lock().unwrap();
            match guard.as_ref() {
                Some(sc) => sc.read_changes(vid, from_usn, journal_id),
                None => return,
            }
        };
        match res {
            Ok((changes, new_usn)) => {
                let mut vols = self.volumes.write().unwrap();
                if let Some(vd) = vols.get_mut(vid) {
                    vd.overlay.apply(&changes);
                    vd.info.next_usn = new_usn;
                    // 合并触发：增量过大即落入 Base（DESIGN §4.3）
                    if vd.overlay.len() > MERGE_THRESHOLD {
                        let merged = merge_overlay_into_base(&vd.index, &vd.overlay);
                        vd.index = merged;
                        vd.overlay = Overlay::default();
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
        let guard = self.scanner.lock().unwrap();
        if let Some(sc) = guard.as_ref() {
            let _ = self.load_volume_with(&**sc, vid);
        }
    }

    // ───────────────────────── 落盘与回收（M2-②/④） ─────────────────────────

    /// 节流落盘：仅在设定了目录且距上次尝试超过 `SAVE_INTERVAL` 时执行。
    fn maybe_save(&self) {
        let last = *self.last_save_attempt.lock().unwrap();
        if let Some(t) = last {
            if t.elapsed() < SAVE_INTERVAL {
                return;
            }
        }
        let dir = self.save_dir.lock().unwrap().clone();
        let dir = match dir {
            Some(d) => d,
            None => return,
        };
        let ids: Vec<VolumeId> = self.volumes.read().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.save_volume(&id, &dir);
        }
        *self.last_save_attempt.lock().unwrap() = Some(Instant::now());
    }

    /// 结果集 60 秒无访问即释放（DESIGN §9 / #54）。
    fn reap_stale(&self) {
        let now = Instant::now();
        let access = self.result_access.lock().unwrap();
        let mut to_remove = Vec::new();
        for (id, t) in access.iter() {
            if now.duration_since(*t) > RESULT_TTL {
                to_remove.push(*id);
            }
        }
        drop(access);
        if !to_remove.is_empty() {
            let mut results = self.results.lock().unwrap();
            let mut access = self.result_access.lock().unwrap();
            for id in to_remove {
                results.remove(&id);
                access.remove(&id);
            }
        }
    }

    fn save_volume(&self, vid: &VolumeId, dir: &Path) -> io::Result<()> {
        let vols = self.volumes.read().unwrap();
        let vd = match vols.get(vid) {
            Some(v) => v,
            None => return Ok(()),
        };
        let path = dir.join(format!("{}.ffix", vd.info.serial));
        vd.index.save(&path)?;
        *self.last_saved.lock().unwrap() = Some(now_filetime());
        Ok(())
    }

    fn save_all(&self) -> io::Result<()> {
        let dir = self.save_dir.lock().unwrap().clone();
        let dir = match dir {
            Some(d) => d,
            None => return Ok(()),
        };
        let ids: Vec<VolumeId> = self.volumes.read().unwrap().keys().cloned().collect();
        for id in ids {
            self.save_volume(&id, &dir)?;
        }
        Ok(())
    }

    fn load_all(&self, dir: &Path) -> io::Result<()> {
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
                let mut vols = self.volumes.write().unwrap();
                vols.insert(
                    vid,
                    VolumeData {
                        info: index.volume.clone(),
                        index,
                        overlay: Overlay::default(),
                        state: VolumeState::Ready,
                    },
                );
            }
        }
        *self.last_saved.lock().unwrap() = Some(now_filetime());
        Ok(())
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
        *self.inner.scanner.lock().unwrap() = Some(scanner);
    }

    /// 手动触发一次全卷增量轮询（测试用；真实场景由后台线程自动调用）。
    pub fn poll_once(&self) {
        self.inner.poll_all();
    }

    /// 直接注入一个已建好的索引（测试与合成数据用；标记为 `Ready`）。
    pub fn add_volume(&self, vol: VolumeId, index: VolumeIndex) {
        self.inner.add_volume(vol, index);
    }

    /// 设置落盘目录（持久化开关）。
    pub fn set_save_dir(&self, dir: &Path) {
        *self.inner.save_dir.lock().unwrap() = Some(dir.to_path_buf());
    }

    /// 立即落盘所有卷（忽略节流）。
    pub fn save_all(&self) -> io::Result<()> {
        self.inner.save_all()
    }

    /// 从落盘目录载入所有卷（按 `<serial>.ffix` 文件名）。
    pub fn load_all(&self, dir: &Path) -> io::Result<()> {
        self.inner.load_all(dir)
    }
}

impl Drop for InProcessEngine {
    fn drop(&mut self) {
        self.inner.bg_running.store(false, Ordering::SeqCst);
        if let Some(h) = self._bg.take() {
            let _ = h.join();
        }
    }
}

impl EngineClient for InProcessEngine {
    fn search(&self, req: SearchRequest) -> Result<SearchSummary> {
        self.inner.search_impl(req)
    }
    fn range(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>> {
        self.inner.range_impl(id, offset, limit)
    }
    fn cancel(&self, id: RequestId) {
        self.inner.cancel_impl(id);
    }
    fn status(&self) -> EngineStatus {
        self.inner.status_impl()
    }
    fn rescan(&self, volume: Option<VolumeId>) -> Result<()> {
        self.inner.rescan_impl(volume)
    }
    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()> {
        self.inner.open_impl(id, index, action)
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
fn sort_hits(hits: &mut [(VolumeId, Hit)], vols: &HashMap<VolumeId, VolumeData>, sort: &SortSpec) {
    hits.sort_by(|a, b| {
        let ord = match sort.key {
            SortKey::Score => hit_order_key(vols, &a.0, &a.1).cmp(&hit_order_key(vols, &b.0, &b.1)),
            SortKey::Name => hit_name(vols, &a.0, &a.1).cmp(&hit_name(vols, &b.0, &b.1)),
            SortKey::Path => hit_path(vols, &a.0, &a.1).cmp(&hit_path(vols, &b.0, &b.1)),
            SortKey::Size => hit_size(vols, &a.0, &a.1).cmp(&hit_size(vols, &b.0, &b.1)),
            SortKey::Modified => hit_mtime(vols, &a.0, &a.1).cmp(&hit_mtime(vols, &b.0, &b.1)),
        };
        if sort.key != SortKey::Score && sort.descending {
            ord.reverse()
        } else {
            ord
        }
    });
}

/// Score 排序键：（来源序 Base 先于 Overlay，0<1）+ 原 id（eid 或 frn）升序。
/// `vols` / `vid` 保留以备未来扩展（如稳定 tie-break），当前实现仅用 `hit`。
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

/// 由 `(vid, hit)` 构造单条 `ResultRow`（Base 走索引原路径，Overlay 走增量路径解析）。
fn resolve_row(
    vols: &HashMap<VolumeId, VolumeData>,
    vid: &VolumeId,
    hit: &Hit,
    query: &Query,
) -> ResultRow {
    let vd = vols.get(vid).expect("resolve_row: 卷必存在");
    match hit {
        Hit::Base(eid) => {
            let index = &vd.index;
            let name = index.name_str(*eid).unwrap_or_default().to_string();
            let full = index.path_string(*eid).unwrap_or_default();
            let dir = parent_dir(&full);
            let is_dir = (index.flags[*eid as usize] & flags::IS_DIR) != 0;
            let size = Some(index.size[*eid as usize]);
            let mtime = Some(index.mtime[*eid as usize]);
            let name_matches = index.highlight_ranges(*eid, query);
            ResultRow {
                name,
                dir,
                is_dir,
                size,
                mtime,
                name_matches,
            }
        }
        Hit::Overlay(frn) => {
            let oe = vd
                .overlay
                .added_or_changed
                .get(frn)
                .expect("resolve_row: overlay 命中必存在");
            let name = oe.name.clone();
            let full = overlay_full_path(&vd.index, &vd.overlay, oe);
            let dir = parent_dir(&full);
            let is_dir = (oe.flags() & flags::IS_DIR) != 0;
            let size = if oe.stale_meta { None } else { Some(oe.size) };
            let mtime = if oe.stale_meta { None } else { Some(oe.mtime) };
            let name_matches = overlay_highlight(query, &name);
            ResultRow {
                name,
                dir,
                is_dir,
                size,
                mtime,
                name_matches,
            }
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
fn overlay_matches(query: &Query, name: &str) -> bool {
    if query.terms.is_empty() {
        return false; // 空查询 → 空结果（与 Base 一致）
    }
    let hay: Vec<u8> = if query.case_sensitive {
        name.as_bytes().to_vec()
    } else {
        simple_lowercase(name.as_bytes())
    };
    for t in &query.terms {
        let needle: Vec<u8> = if query.case_sensitive {
            t.text.clone().into_bytes()
        } else {
            simple_lowercase(t.text.as_bytes())
        };
        if !SubstringMatcher::new(&needle).matches(&hay) {
            return false;
        }
    }
    true
}

/// Overlay 名字内的命中区间（供高亮；与 Base `highlight_ranges` 语义一致）。
fn overlay_highlight(query: &Query, name: &str) -> Vec<Range<u32>> {
    if query.terms.is_empty() {
        return Vec::new();
    }
    // simple_lowercase 逐字节等长（ASCII/CJK 字节数不变），偏移相对原名一致
    let folded: String = if query.case_sensitive {
        name.to_string()
    } else {
        String::from_utf8_lossy(&simple_lowercase(name.as_bytes())).into_owned()
    };
    let mut ranges: Vec<Range<u32>> = Vec::new();
    for t in &query.terms {
        let needle: Vec<u8> = if query.case_sensitive {
            t.text.clone().into_bytes()
        } else {
            simple_lowercase(t.text.as_bytes())
        };
        ranges.extend(SubstringMatcher::new(&needle).find_ranges(folded.as_bytes()));
    }
    ranges.sort_by_key(|r| r.start);
    ranges
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

/// 跨平台打开 / 选中文件（真实进程行为由 OS 完成）。
#[cfg(windows)]
fn shell_open(path: &str, folder: bool) -> Result<()> {
    let (prog, args): (String, Vec<String>) = if folder {
        ("explorer".to_string(), vec![format!("/select,\"{path}\"")])
    } else {
        (
            "cmd".to_string(),
            vec![
                "/c".to_string(),
                "start".to_string(),
                "".to_string(),
                path.to_string(),
            ],
        )
    };
    let status = std::process::Command::new(prog)
        .args(args)
        .status()
        .map_err(|e| EngineError::Io(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(EngineError::Internal(format!("打开失败，退出码 {status}")))
    }
}

/// 非 Windows：真实打开由 UI 层处理；本实现保持跨平台可编译，不实际启动进程。
#[cfg(not(windows))]
fn shell_open(_path: &str, _folder: bool) -> Result<()> {
    Ok(())
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
        // "文档.txt" = 2 个中文(6 字节) + "." + "txt" → "txt" 位于字节 7..10
        assert_eq!(rows[1].name_matches, vec![7..10]);
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
        let sum = e.search(req("a|b")).unwrap();
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
}
