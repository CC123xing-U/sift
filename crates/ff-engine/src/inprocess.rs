//! `EngineClient` 的 InProcess 实现（DESIGN §9，M1）。
//!
//! 直接用 `ff-core` 的 [`VolumeIndex`] 承接 `ff-ntfs` 的 [`RawEntry`]，在进程内
//! 打通 `search` / `range`（含排序与 `ResultRow` 构造）/ `cancel` / `status` /
//! `rescan` / `open`。不依赖任何 Windows 专有 API，保证 `ff-engine` 跨平台可编译。
//!
//! 多卷映射：每条命中记录为 `(VolumeId, EntryId)`。排序与 `range` 时据此回到对应
//! 卷的 `VolumeIndex` 取字段，天然支持多卷并集。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use ff_core::{
    flags, parse, CancelToken, EntryId, EntryInput, IndexBuilder, Query, VolumeIndex, VolumeInfo,
};
use ff_ntfs::{RawEntry, ScanError, VolumeId, VolumeScanner};

use crate::api::{
    EngineClient, EngineError, EngineStatus, OpenAction, RequestId, Result, ResultRow,
    SearchRequest, SearchSummary, SortKey, SortSpec, VolumeState, VolumeStatus,
};

/// 单卷运行时数据。
struct VolumeData {
    info: VolumeInfo,
    index: VolumeIndex,
    state: VolumeState,
}

/// 一次搜索的持久结果：命中列表（全局有序，每条带所属卷）+ 原查询（供高亮）。
struct StoredResult {
    hits: Vec<(VolumeId, EntryId)>,
    query: Query,
}

/// InProcess 引擎：进程内直接调 `ff-core` / `ff-ntfs`，无服务进程。
pub struct InProcessEngine {
    /// 各卷索引（被 trait 的 `&self` 方法并发读写，用 `RwLock` 保护）。
    volumes: RwLock<HashMap<VolumeId, VolumeData>>,
    /// 扫描器（Windows 下由 `NtfsVolumeScanner` 提供；测试 / 合成数据可留空）。
    scanner: Mutex<Option<Box<dyn VolumeScanner + Send + Sync>>>,
    /// 结果集：`search` 分配的新 id 覆盖上一次（单客户端语义），旧 id 自然失效。
    results: Mutex<HashMap<RequestId, Arc<StoredResult>>>,
    /// 每个请求的取消令牌（被 `cancel(id)` 触发）。
    cancels: Mutex<HashMap<RequestId, CancelToken>>,
    /// 请求计数器（从 1 起，0 视为无效）。
    request_counter: AtomicU64,
    /// 是否正在搜索（`status().searching` 来源）。
    searching: AtomicBool,
}

impl InProcessEngine {
    /// 用给定扫描器构造（真实 Windows 场景）。
    pub fn new(scanner: Box<dyn VolumeScanner + Send + Sync>) -> Self {
        Self {
            volumes: RwLock::new(HashMap::new()),
            scanner: Mutex::new(Some(scanner)),
            results: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            request_counter: AtomicU64::new(1),
            searching: AtomicBool::new(false),
        }
    }

    /// 无扫描器构造（测试 / 合成数据：通过 [`Self::add_volume`] 直接注入索引）。
    pub fn empty() -> Self {
        Self {
            volumes: RwLock::new(HashMap::new()),
            scanner: Mutex::new(None),
            results: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            request_counter: AtomicU64::new(1),
            searching: AtomicBool::new(false),
        }
    }

    /// 直接注入一个已建好的索引（测试与合成数据用；标记为 `Ready`）。
    pub fn add_volume(&self, vol: VolumeId, index: VolumeIndex) {
        let info = index.volume.clone();
        let mut vols = self.volumes.write().unwrap();
        vols.insert(
            vol,
            VolumeData {
                info,
                index,
                state: VolumeState::Ready,
            },
        );
    }

    /// 经扫描器全量重建单卷索引（见 trait `rescan`）。
    fn load_volume_with(&self, scanner: &dyn VolumeScanner, vol: &VolumeId) -> Result<()> {
        // 先标记 Scanning
        {
            let mut vols = self.volumes.write().unwrap();
            let vd = vols.entry(vol.clone()).or_insert_with(|| {
                let info = VolumeInfo {
                    serial: vol.serial,
                    drive: vol.drive,
                    ..Default::default()
                };
                VolumeData {
                    info: info.clone(),
                    index: IndexBuilder::new(info).finish(),
                    state: VolumeState::Scanning,
                }
            });
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
        self.results.lock().unwrap().clear();
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

        // 在所有 Ready 卷上求并集命中，记录 (卷, 条目id)
        let mut hits: Vec<(VolumeId, EntryId)> = Vec::new();
        let hide_system = req.opts.hide_system;
        let hide_hidden = req.opts.hide_hidden;
        {
            let vols = self.volumes.read().unwrap();
            for (vid, vd) in vols.iter() {
                if vd.state != VolumeState::Ready {
                    continue;
                }
                for eid in vd.index.search(&query, &token) {
                    // DESIGN §5.2：系统文件"默认可在设置中隐藏"——`hide_system` 在此生效
                    // （此前该选项未被读取，是死字段）。两个开关独立判断，不合并成一个
                    // 掩码常量，便于 M4 按 UI 拆分成两个 toggle（DESIGN §11.3）。
                    let f = vd.index.flags[eid as usize];
                    if hide_system && f & flags::SYSTEM != 0 {
                        continue;
                    }
                    if hide_hidden && f & flags::HIDDEN != 0 {
                        continue;
                    }
                    hits.push((vid.clone(), eid));
                }
            }
            // 排序（Score 在 M1 无评分，按 entry_id 升序近似；见 `sort_hits`）
            sort_hits(&mut hits, &vols, &req.sort);
        }

        // max_hits 截断（排序后）
        if let Some(k) = req.max_hits {
            hits.truncate(k);
        }

        let total = hits.len();
        self.results.lock().unwrap().insert(
            id,
            Arc::new(StoredResult {
                hits,
                query: query.clone(),
            }),
        );
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

        let vols = self.volumes.read().unwrap();
        let mut rows = Vec::with_capacity(window.len());
        for &(ref vid, eid) in &window {
            let vd = match vols.get(vid) {
                Some(v) => v,
                None => continue,
            };
            let index = &vd.index;
            let name = index.name_str(eid).unwrap_or_default().to_string();
            let full = index.path_string(eid).unwrap_or_default();
            let dir = parent_dir(&full);
            let is_dir = (index.flags[eid as usize] & flags::IS_DIR) != 0;
            let size = Some(index.size[eid as usize]);
            let mtime = Some(index.mtime[eid as usize]);
            let name_matches = index.highlight_ranges(eid, &query);
            rows.push(ResultRow {
                name,
                dir,
                is_dir,
                size,
                mtime,
                name_matches,
            });
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
                volume: vid.clone(),
                state: vd.state,
                entries,
                progress: None,
                message: None,
            });
        }
        EngineStatus {
            volumes,
            total_entries,
            last_saved: None,
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
        let vols = self.volumes.read().unwrap();
        let vd = vols.get(&hit.0).ok_or(EngineError::NotFound)?;
        let full = vd.index.path_string(hit.1).ok_or(EngineError::NotFound)?;
        drop(vols);
        match action {
            // 复制到剪贴板由 UI 层处理；InProcess 下暂视为成功
            OpenAction::CopyPath => Ok(()),
            OpenAction::Open => shell_open(&full, false),
            OpenAction::OpenFolder => shell_open(&full, true),
        }
    }
}

impl EngineClient for InProcessEngine {
    fn search(&self, req: SearchRequest) -> Result<SearchSummary> {
        self.search_impl(req)
    }
    fn range(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>> {
        self.range_impl(id, offset, limit)
    }
    fn cancel(&self, id: RequestId) {
        self.cancel_impl(id);
    }
    fn status(&self) -> EngineStatus {
        self.status_impl()
    }
    fn rescan(&self, volume: Option<VolumeId>) -> Result<()> {
        self.rescan_impl(volume)
    }
    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()> {
        self.open_impl(id, index, action)
    }
}

/// 按 `SortSpec` 对命中列表排序。
///
/// - `Score`：M1 无评分，按 `entry_id` 升序近似（注释说明）。
/// - `Name` / `Path`：按名字 / 完整路径字符串字典序。
/// - `Size` / `Modified`：按 `size` / `mtime` 数值序。
fn sort_hits(
    hits: &mut [(VolumeId, EntryId)],
    vols: &HashMap<VolumeId, VolumeData>,
    sort: &SortSpec,
) {
    hits.sort_by(|a, b| {
        let ord = match sort.key {
            SortKey::Score => a.1.cmp(&b.1),
            SortKey::Name => {
                let na = vols
                    .get(&a.0)
                    .and_then(|v| v.index.name_str(a.1))
                    .unwrap_or("");
                let nb = vols
                    .get(&b.0)
                    .and_then(|v| v.index.name_str(b.1))
                    .unwrap_or("");
                na.cmp(nb)
            }
            SortKey::Path => {
                let pa = vols
                    .get(&a.0)
                    .and_then(|v| v.index.path_string(a.1))
                    .unwrap_or_default();
                let pb = vols
                    .get(&b.0)
                    .and_then(|v| v.index.path_string(b.1))
                    .unwrap_or_default();
                pa.cmp(&pb)
            }
            SortKey::Size => {
                let sa = vols
                    .get(&a.0)
                    .map(|v| v.index.size[a.1 as usize])
                    .unwrap_or(0);
                let sb = vols
                    .get(&b.0)
                    .map(|v| v.index.size[b.1 as usize])
                    .unwrap_or(0);
                sa.cmp(&sb)
            }
            SortKey::Modified => {
                let ma = vols
                    .get(&a.0)
                    .map(|v| v.index.mtime[a.1 as usize])
                    .unwrap_or(0);
                let mb = vols
                    .get(&b.0)
                    .map(|v| v.index.mtime[b.1 as usize])
                    .unwrap_or(0);
                ma.cmp(&mb)
            }
        };
        if sort.key != SortKey::Score && sort.descending {
            ord.reverse()
        } else {
            ord
        }
    });
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
    use ff_ntfs::VolumeId;

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
}
