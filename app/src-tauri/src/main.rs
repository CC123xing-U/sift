// Sift 桌面外壳（Tauri v2）：承载进程内引擎，暴露 search/range/cancel/status/rescan/open
// 六个引擎命令 + show/hide/toggle_window 三个窗口命令；全局热键 Ctrl+Alt+Space 唤出/隐藏；
// 托盘菜单；失焦自动隐藏（DESIGN §11.2）。
//
// 平台：M0–M5 以管理员运行（DESIGN §12），引擎进程内直接调 NtfsVolumeScanner 读真实 NTFS 卷。
// 注意：无条件 windows 子系统——debug 构建也不弹控制台（GUI 应用不需要；
// 若日后要调试日志，用 dbgview 或改为 cfg_attr 条件编译）。
#![windows_subsystem = "windows"]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ff_engine::timeconv;

use ff_engine::api::{
    EngineClient, EngineError, EngineStatus, OpenAction, RequestId, ResultRow, SearchRequest,
    SearchSummary, VolumeId,
};
use ff_engine::InProcessEngine;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};

/// 引擎后端：M6 普通权限模式优先走命名管道连接 `ff-service`；服务不可用时回退
/// 进程内引擎（M0–M5 管理员行为），保证未安装服务的机器仍可正常使用。
///
/// 两种实现都通过 [`EngineClient`] 暴露相同六方法；差异仅在 `open`：
/// - `InProcess`：引擎内部以 UI 进程身份执行打开/剪贴板动作（见 inprocess::open_impl）；
/// - `Pipe`：服务只解析出 `ResolvedAction`，由 UI 进程调用 `ff_engine::execute_open_checked`
///   以当前用户身份执行（DESIGN §12）。
enum Backend {
    InProcess(Arc<InProcessEngine>),
    #[cfg(windows)]
    Pipe(PipeBackend),
}

/// 管道后端：持有一个可重连的 `PipeClient`（断线后整体替换，因 PipeClient 内部
/// 管道流不可复用）。`Mutex` 仅在重连写入时短暂持有，命令调用走 guard 内的引用。
#[cfg(windows)]
struct PipeBackend {
    client: Mutex<Arc<ff_ipc::PipeClient>>,
    pipe_name: String,
    /// 上次重连成功的时刻（毫秒）；用于退避，避免服务未起时忙重连。
    last_reconnect_ms: AtomicU64,
}

impl Backend {
    fn search(&self, req: SearchRequest) -> Result<SearchSummary, EngineError> {
        match self {
            Backend::InProcess(e) => e.search(req),
            #[cfg(windows)]
            Backend::Pipe(p) => p.with_reconnect(|c| c.search(req.clone())),
        }
    }
    fn range(
        &self,
        id: RequestId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ResultRow>, EngineError> {
        match self {
            Backend::InProcess(e) => e.range(id, offset, limit),
            #[cfg(windows)]
            Backend::Pipe(p) => p.with_reconnect(|c| c.range(id, offset, limit)),
        }
    }
    fn cancel(&self, id: RequestId) {
        match self {
            Backend::InProcess(e) => e.cancel(id),
            #[cfg(windows)]
            Backend::Pipe(p) => {
                let _ = p.with_reconnect(|c| {
                    c.cancel(id);
                    Ok(())
                });
            }
        }
    }
    fn status(&self) -> EngineStatus {
        match self {
            Backend::InProcess(e) => e.status(),
            #[cfg(windows)]
            Backend::Pipe(p) => p
                .with_reconnect(|c| c.status_checked())
                .unwrap_or_else(|_| EngineStatus {
                    volumes: Vec::new(),
                    total_entries: 0,
                    last_saved: None,
                    searching: false,
                    save_error: Some("服务不可用".into()),
                }),
        }
    }
    fn rescan(&self, volume: Option<VolumeId>) -> Result<(), EngineError> {
        match self {
            Backend::InProcess(e) => e.rescan(volume),
            #[cfg(windows)]
            Backend::Pipe(p) => p.with_reconnect(|c| c.rescan(volume.clone())),
        }
    }
    /// 已载入卷的 ID 列表。管道后端拿不到 VolumeId（协议只有显示字符串），返回空。
    fn volume_ids(&self) -> Vec<VolumeId> {
        match self {
            Backend::InProcess(e) => e.volume_ids(),
            #[cfg(windows)]
            Backend::Pipe(_) => Vec::new(),
        }
    }
    /// 把 UI 传入的盘符（`"C"` / `"C:"` / `"c:\\"`）解析成引擎需要的 [`VolumeId`]。
    ///
    /// 解析不到（空串 / 未知盘符 / 管道后端）时返回 `None`，调用方回退为「全部重建」。
    fn resolve_volume(&self, name: &str) -> Option<VolumeId> {
        let want = name
            .trim()
            .trim_end_matches(|c| c == '\\' || c == '/')
            .trim_end_matches(':')
            .chars()
            .next()?
            .to_ascii_uppercase();
        self.volume_ids()
            .into_iter()
            .find(|v| v.drive.map(|c| c.to_ascii_uppercase()) == Some(want))
    }
    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<(), EngineError> {
        match self {
            Backend::InProcess(e) => e.open(id, index, action),
            #[cfg(windows)]
            Backend::Pipe(p) => p.with_reconnect(|c| {
                let resolved = c.open_resolved(id, index, action)?;
                ff_engine::execute_open_checked(resolved.action, &resolved.row)
            }),
        }
    }
}

#[cfg(windows)]
impl PipeBackend {
    /// 在**重连保护**下执行一次管道调用：断线错误 → 退避重连 → 至多重试一次。
    /// 重连成功/失败分别通过 Tauri event 通知前端提示用户。
    fn with_reconnect<T>(
        &self,
        f: impl Fn(&ff_ipc::PipeClient) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let guard = self.client.lock().unwrap();
        match f(&guard) {
            Ok(v) => Ok(v),
            Err(e) if ff_ipc::PipeClient::is_disconnected(&e) => {
                drop(guard);
                let now = now_ms();
                let last = self.last_reconnect_ms.load(Ordering::Relaxed);
                // 1s 退避：服务刚崩溃时避免每个命令都触发一次 CreateFile。
                if now.saturating_sub(last) < 1000 {
                    return Err(e);
                }
                self.last_reconnect_ms.store(now, Ordering::Relaxed);
                dbg_log("pipe: disconnected, attempting reconnect");
                match ff_ipc::PipeClient::connect(&self.pipe_name) {
                    Ok(new_client) => {
                        *self.client.lock().unwrap() = Arc::new(new_client);
                        emit_service_msg("服务已重连");
                        let guard = self.client.lock().unwrap();
                        f(&guard)
                    }
                    Err(_) => {
                        emit_service_msg("服务已断开，正在重试");
                        Err(e)
                    }
                }
            }
            Err(e) => Err(e),
        }
    }
}

/// 通知前端服务状态变化（前端监听 `sift:service-status` 用 toast 提示）。
/// AppHandle 在 setup 时注入 `SERVICE_HANDLE`，重连前若尚未注入则忽略。
fn emit_service_msg(msg: &str) {
    if let Some(app) = SERVICE_HANDLE.get() {
        let _ = app.emit("sift:service-status", msg.to_string());
    }
}

/// 服务状态事件使用的 AppHandle（setup 时注入一次）。
static SERVICE_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 进程内/服务双后端的内核状态（注入到 Tauri 状态容器，命令据此访问）。
/// `Arc` 使 rescan 线程可克隆共享后端，无需把整个状态跨线程搬运。
struct EngineState {
    backend: Arc<Backend>,
}

// ───────────────────────── 引擎命令（镜像 EngineClient，DESIGN §9） ─────────────────────────
//
// P0-4：搜索 / 取行 / 打开都是毫秒级（引擎重活在后台线程跑），同步执行即可，
// 不会冻结 UI；真正会卡住主线程的是「重建索引」（全量扫描）与「Overlay 合并
// 持写锁」。因此 `rescan` 改为起线程后立即返回（下方），其余命令保持同步。
// 引擎侧的合并已在读锁下构建（见 InProcessEngine），进一步避免主线程阻塞。
//
// **M7 第 5 项修正（DECISIONS #95）**：`content:` 内容搜索要读磁盘文件，
// 实测单个查询可达数百毫秒到数秒（远超「毫秒级」假设）。同步命令会占住
// Tauri 的 IPC 处理线程，Windows 直接把窗口标成「未响应」——真机验收抓到。
// 因此 `search` 改为 async + `spawn_blocking`：引擎调用仍在阻塞线程池里跑，
// WebView 的 IPC 与渲染不被占住，用户还能按键触发取消。

#[tauri::command]
async fn search(
    state: tauri::State<'_, EngineState>,
    req: SearchRequest,
) -> Result<SearchSummary, String> {
    let backend = Arc::clone(&state.backend);
    tauri::async_runtime::spawn_blocking(move || backend.search(req).map_err(|e| e.to_string()))
        .await
        .map_err(|e| format!("搜索任务异常终止：{e}"))?
}

#[tauri::command]
fn range(
    state: tauri::State<EngineState>,
    id: RequestId,
    offset: usize,
    limit: usize,
) -> Result<Vec<ResultRow>, String> {
    // P1-8：引擎给的是 FILETIME，前端 `types.ts` 契约是 JS epoch 毫秒。
    // 必须在序列化前转换——FILETIME ≈1.34e17 超过 2^53，直接送进 JS 会先丢精度
    // 再被 `new Date()` 判为 Invalid Date，前端补算也救不回来（见 timeconv.rs）。
    let mut rows = state
        .backend
        .range(id, offset, limit)
        .map_err(|e| e.to_string())?;
    timeconv::convert_rows(&mut rows);
    Ok(rows)
}

#[tauri::command]
fn cancel(state: tauri::State<EngineState>, id: RequestId) {
    state.backend.cancel(id);
}

#[tauri::command]
fn status(state: tauri::State<EngineState>) -> EngineStatus {
    // P1-8：状态栏「已保存 HH:MM」同样需要 epoch 毫秒，转换口径与 `range` 一致。
    let mut s = state.backend.status();
    timeconv::convert_status(&mut s);
    s
}

#[tauri::command]
fn rescan(state: tauri::State<EngineState>, volume: Option<String>) -> Result<(), String> {
    // #6：`volume` 现按盘符解析（"C" / "C:" / "C:\" 均可），映射到带 serial 的
    // VolumeId 后只重建该卷。解析不到时回退为「全部重建」，与旧行为一致。
    let backend = state.backend.clone();
    let target = volume.as_deref().and_then(|v| backend.resolve_volume(v));
    // P0-4：起线程立即返回，避免在主线程跑全量扫描（否则托盘「重建索引」会让
    // 窗口与托盘无响应数秒到数十秒）。
    std::thread::spawn(move || {
        let _ = backend.rescan(target);
    });
    Ok(())
}

#[tauri::command]
fn open(
    state: tauri::State<EngineState>,
    id: RequestId,
    index: usize,
    action: OpenAction,
) -> Result<(), String> {
    state
        .backend
        .open(id, index, action)
        .map_err(|e| e.to_string())
}

// ───────────────────── M7 对话框集成（DESIGN §99 / DECISIONS #92） ─────────────────────
//
// 监听文件「打开 / 另存为」对话框前台事件（SetWinEventHook，不注入），用户显式触发后
// 把选中结果的完整路径经 UIA ValuePattern 回填到对话框「文件名」框。回填在 UI 进程
// 以当前用户身份执行，不经提权服务（DESIGN §12）。

/// 进程生命周期持有对话框观察器：一旦 Drop 会卸载钩子，故存入 Mutex 常驻。
/// （`DialogWatcher` 含 mpsc::Receiver 不实现 Sync，需 Mutex 包裹才能进静态区。）
static DIALOG_WATCHER: Mutex<Option<ff_dialog::DialogWatcher>> = Mutex::new(None);

/// 启动对话框监听：安装钩子并把事件转发为 Tauri 事件给前端提示。失败只记日志，不影响主流程。
fn start_dialog_watcher(app: &tauri::AppHandle) {
    let mut watcher = match ff_dialog::DialogWatcher::start() {
        Ok(w) => w,
        Err(e) => {
            dbg_log(&format!("dialog watcher start failed: {e}"));
            return;
        }
    };
    let rx = match watcher.take_receiver() {
        Some(rx) => rx,
        None => {
            dbg_log("dialog watcher receiver missing");
            return;
        }
    };
    // 观察器必须常驻（Drop 会卸载钩子）；放入静态 Mutex 槽直至进程退出。
    *DIALOG_WATCHER.lock().unwrap() = Some(watcher);
    let app = app.clone();
    std::thread::Builder::new()
        .name("sift-dialog-events".into())
        .spawn(move || {
            use ff_dialog::DialogEvent;
            while let Ok(evt) = rx.recv() {
                match evt {
                    DialogEvent::Opened(snap) => {
                        let _ = app.emit(
                            "sift:dialog-detected",
                            serde_json::json!({
                                "title": snap.title,
                                "kind": ff_dialog::kind_label(snap.kind),
                            }),
                        );
                    }
                    DialogEvent::Closed => {
                        let _ = app.emit("sift:dialog-closed", ());
                    }
                }
            }
        })
        .ok();
}

/// 把选中结果的完整路径回填到最近识别到的文件对话框「文件名」框。
/// `id`/`index` 定位结果行（复用引擎 range），路径在 Rust 侧拼（与 execute_open 同源）。
#[tauri::command]
fn fill_active_dialog(
    state: tauri::State<EngineState>,
    id: RequestId,
    index: usize,
) -> Result<(), String> {
    let rows = state
        .backend
        .range(id, index, 1)
        .map_err(|e| e.to_string())?;
    let row = rows
        .first()
        .ok_or_else(|| "结果行不存在或已过期".to_string())?;
    let path = ff_engine::full_path(row);
    ff_dialog::fill_active_dialog(&path).map_err(|e| e.to_string())
}

/// 当前是否存在可回填的文件对话框 —— 由 `ff_dialog` 的 `sift:dialog-detected` /
/// `sift:dialog-closed` 事件驱动前端提示，不再单独暴露 IPC 命令（#14 死代码清理：
/// 该命令注册后前端从未调用，留着只会让人误以为有一条可用的查询通道）。
// ───────────────────────── 窗口命令（OS 级 show/hide/toggle） ─────────────────────────
//
// 窗口动效（UI_SPEC §2.6 dur-window-in/out，2026-11-15 落地，DECISIONS #83）：
// show 后向前端发 `sift:shown`，前端重播 140ms 进入动画；hide 前发 `sift:hiding`，
// 等 90ms 退出动画播完再真隐藏——原生 hide 是瞬时的，直接调会把退出动画吞掉。

/// 隐藏挂起的代数计数：`hide_window` 调度延迟隐藏时记下当前代数，到点后代数
/// 一致才真隐藏；`show_window` 递增代数使所有挂起的隐藏作废（热键连按竞态）。
static HIDE_GEN: AtomicU64 = AtomicU64::new(0);

/// 是否有一个「已发出退出动画、正在等 90ms 后真隐藏」的挂起隐藏。
///
/// #33：`is_visible()` 在退出动画期间仍为 true，`toggle_window` 只看可见性的话，
/// 90ms 内连按两次热键会连续两次调度隐藏（而不是唤回），窗口最终消失。
/// 有了这个标志，第二次按热键能识别「正在隐藏」并按「显示」处理。
static HIDE_PENDING: AtomicBool = AtomicBool::new(false);

/// 退出动画时长（UI_SPEC §2.6 `dur-window-out`），Rust 侧等它播完再真隐藏。
const WINDOW_OUT_MS: u64 = 90;

#[tauri::command]
fn show_window(app: tauri::AppHandle) {
    dbg_log("show_window called");
    HIDE_PENDING.store(false, Ordering::SeqCst); // 取消「正在隐藏」状态
    HIDE_GEN.fetch_add(1, Ordering::SeqCst); // 作废挂起的延迟隐藏
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit("sift:shown", ());
    }
}

#[tauri::command]
fn hide_window(app: tauri::AppHandle) {
    dbg_log("hide_window called");
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.emit("sift:hiding", ());
        HIDE_PENDING.store(true, Ordering::SeqCst);
        let gen = HIDE_GEN.fetch_add(1, Ordering::SeqCst);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(WINDOW_OUT_MS));
            // fetch_add 返回旧值且自身已 +1：无后来者时 load() == gen + 1。
            // （曾写成 load() == gen，导致每次延迟隐藏 100% 被自己作废，#83。）
            let matched = HIDE_GEN.load(Ordering::SeqCst) == gen + 1;
            dbg_log(&format!(
                "delayed hide: gen_match={} (gen={}, now={})",
                matched,
                gen,
                HIDE_GEN.load(Ordering::SeqCst)
            ));
            if matched {
                HIDE_PENDING.store(false, Ordering::SeqCst);
                if let Some(w) = app.get_webview_window("main") {
                    let r = w.hide();
                    dbg_log(&format!("hide result ok={}", r.is_ok()));
                }
            }
            // matched == false：被 show_window 作废（它已把 HIDE_PENDING 置 false），
            // 或被后一次 hide_window 接管（它会把 HIDE_PENDING 重新置 true）。
        });
    }
}

#[tauri::command]
fn toggle_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        // #33：退出动画期间窗口仍然「可见」，此时连按热键应当唤回而不是再隐藏一次。
        let pending_hide = HIDE_PENDING.load(Ordering::SeqCst);
        if !pending_hide && w.is_visible().unwrap_or(false) {
            hide_window(app);
        } else {
            show_window(app);
        }
    }
}

// 判断当前前台窗口是否属于本进程。
//
// WebView2 子窗口（或输入法 textinputhost 关联行为）获得焦点时，父窗口会收到
// `WM_ACTIVATE(WA_INACTIVATE)` → Tauri `Focused(false)`，但那不是「用户切到别的
// 应用」，不应隐藏窗口（2026-10-25 实测：启动 2.3s 页面加载完就被误藏）。
// 用 user32 裸 extern 声明，避免为此引入 winapi/windows crate。
#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> *mut std::ffi::c_void;
    fn GetWindowThreadProcessId(hwnd: *mut std::ffi::c_void, pid: *mut u32) -> u32;
}

fn foreground_pid() -> u32 {
    // P2：非 Windows 平台下没有 user32，直接返回 0（不启用「前台属于本进程」豁免）。
    #[cfg(windows)]
    unsafe {
        // 启动瞬间 / Alt+Tab 切换中，GetForegroundWindow 会短暂返回 NULL，
        // 或返回非空 HWND 但 PID 仍为 0。此时「前台是谁」无法确定，调用方
        // 必须据此放弃失焦隐藏——否则刚显示的窗口会被误藏（真机实测，见
        // DECISIONS #96）。重试 3 次（共 ~90ms）跨过系统切换的瞬态；仍拿
        // 不到就返回 0，由 on_window_event 的守卫兜住。
        for _ in 0..3 {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid != 0 {
                    return pid;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        0 // 三次都没拿到有效 PID：前台无法确定，交给调用方保守处理。
    }
    #[cfg(not(windows))]
    0
}

/// 调试日志（热键/焦点链路排查用）：写 `%TEMP%\sift-debug.log`。
///
/// #7：默认**只在 debug 构建或设置 `SIFT_DEBUG=1` 时**写入——release 版常驻写盘
/// 既没必要（用户磁盘随时间被无意义事件日志占着），也让「临时定位后移除」这条
/// 注释永远无法兑现。需要现场诊断时设环境变量即可开启。
/// 文件超过 1MB 先删除再写，不会无限增长。
fn dbg_log(msg: &str) {
    use std::io::Write;
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    let enabled =
        *ENABLED.get_or_init(|| cfg!(debug_assertions) || std::env::var_os("SIFT_DEBUG").is_some());
    if !enabled {
        return;
    }
    // 后台索引线程与 UI 线程会并发写；不加锁时两行会交错成一团（真机见到
    // `[ts1] [ts2] msg1msg2`），诊断日志本身就不可读了。持锁覆盖"删旧文件 +
    // 打开 + 写一行"整个过程，保证每行完整。
    static LOG_LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = std::env::temp_dir().join("sift-debug.log");
    // 超过 1MB 先删除，避免无限膨胀（调试日志，丢旧内容无妨）。
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 1_048_576 {
            let _ = std::fs::remove_file(&path);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
}

fn rescan_all(app: &tauri::AppHandle) {
    // S4：托盘回调运行在 UI 线程，同步全量扫描会冻结窗口与托盘；与 `rescan` 命令
    // 路径保持一致——起线程后立即返回，扫描在后台进行。
    if let Some(state) = app.try_state::<EngineState>() {
        let backend = state.backend.clone();
        std::thread::spawn(move || {
            let _ = backend.rescan(None);
        });
    }
}

// ───────────────────────── 引擎构建（Windows 真实扫描 / 其他平台空引擎） ─────────────────────────

#[cfg(windows)]
fn build_engine() -> Arc<InProcessEngine> {
    use ff_ntfs::AutoVolumeScanner;
    // M7（DESIGN §5.4）：按卷的文件系统自动分派——NTFS 走 MFT + USN，
    // exFAT / FAT32 等走目录遍历回退（无增量，引擎按 30s 定期重走）。
    let engine = Arc::new(InProcessEngine::new(Box::new(AutoVolumeScanner)));

    // 索引放在运行程序所在磁盘根目录的 Sift 文件夹，避免 C: 的
    // `%LOCALAPPDATA%` 被大索引长期占满；同一 serial 始终覆盖同一文件。
    let save_dir = index_dir();
    let _ = std::fs::create_dir_all(&save_dir);
    engine.set_save_dir(&save_dir);

    // 启动路径上的所有重活——枚举卷、清理旧索引、load_all（真机 200MB / ~5s）、
    // 逐卷增量扫描——全部挪进后台线程。窗口创建不能等它们：否则双击 exe 后
    // 先看到透明窗口的阴影，约 5s 后才显示内容，看起来像「索引完才显现」
    // （DECISIONS #96）。引擎内部 volumes 由 RwLock 保护，后台写与 UI 的
    // status/search/range 读并发安全；加载完成前 UI 显示 Scanning（DESIGN §5.2）。
    let scan = engine.clone();
    std::thread::spawn(move || {
        // S3：卷枚举失败（瞬时 RPC / 权限故障）时绝不能用空集去清理——那会把所有
        // 既有 .ffix 误判为幽灵卷删光，毁掉秒级冷启动。失败时跳过清理，保留磁盘
        // 上的索引，仍尝试 load_all；本轮不扫描，下次启动枚举恢复后再收敛。
        let available = match ff_ntfs::win32::enumerate_volumes() {
            Ok(vols) if !vols.is_empty() => {
                let valid_serials: HashSet<u32> = vols.iter().map(|v| v.serial).collect();
                let removed = ff_engine::prune_index_dir(&save_dir, &valid_serials);
                if removed > 0 {
                    dbg_log(&format!("engine: pruned {removed} stale index/tmp file(s)"));
                }
                cleanup_legacy_indexes();
                vols
            }
            Ok(empty) => {
                // 枚举「成功但为空」：不得清理（否则空集删光所有合法索引），但仍保留
                // 已载入的磁盘索引供查询；本轮不扫描。
                dbg_log("engine: enumerate_volumes returned empty, skip prune");
                empty
            }
            Err(e) => {
                dbg_log(&format!(
                    "engine: enumerate_volumes failed, skip prune: {e}"
                ));
                Vec::new()
            }
        };
        if let Err(e) = scan.load_all(&save_dir) {
            dbg_log(&format!("engine: load_all failed (will full-scan): {}", e));
        } else {
            dbg_log(&format!(
                "engine: loaded persisted indexes from {:?}",
                save_dir
            ));
        }

        {
            let vols = available;
            dbg_log(&format!("engine: {} volume(s) found", vols.len()));
            let vids: Vec<ff_engine::VolumeId> = vols
                .iter()
                .map(|vi| ff_engine::VolumeId {
                    serial: vi.serial,
                    drive: vi.drive,
                })
                .collect();
            // 索引复用：load_all 已把可续读的磁盘索引置为 Ready，不再逐卷全量
            // 重扫覆盖掉它们。startup_reconcile 只挑出新卷 / 空索引 / 失效卷。
            // 复用的卷由后台 poll_volume 从 next_usn 续读关机期间的增量。
            let need_scan = scan.startup_reconcile(&vids);
            dbg_log(&format!(
                "engine: {} volume(s) reused from disk, {} need full scan",
                vids.len() - need_scan.len(),
                need_scan.len()
            ));
            // 只把真正要扫的卷置为「待扫描」，避免进度条把复用的 Ready 卷算成
            // 已完成又被拽回低位（实机进度回跳）；否则该卷会停在 Scanning 不恢复。
            scan.mark_pending(&need_scan);
            for vid in need_scan {
                dbg_log(&format!("engine: rescan volume {:?}", vid));
                if let Err(e) = scan.rescan(Some(vid)) {
                    dbg_log(&format!("engine: rescan error: {}", e));
                }
                dbg_log("engine: rescan done");
            }
        }
    });
    engine
}

#[cfg(windows)]
fn index_dir() -> PathBuf {
    let fallback = || {
        std::env::var("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join("Sift"))
            .unwrap_or_else(|_| std::env::temp_dir().join("Sift"))
    };
    let Ok(exe) = std::env::current_exe() else {
        return fallback();
    };
    match exe.components().next() {
        Some(std::path::Component::Prefix(prefix)) => {
            PathBuf::from(format!(r"{}\Sift", prefix.as_os_str().to_string_lossy()))
        }
        _ => fallback(),
    }
}

#[cfg(windows)]
fn cleanup_legacy_indexes() {
    let Ok(base) = std::env::var("LOCALAPPDATA") else {
        return;
    };
    let legacy = PathBuf::from(base).join("Sift");
    let Ok(entries) = std::fs::read_dir(legacy) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) == Some("ffix") {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(not(windows))]
fn build_engine() -> Arc<InProcessEngine> {
    // 非 Windows（CI / 开发机）只保证可编译，不挂真实扫描器。
    Arc::new(InProcessEngine::empty())
}

/// 选择引擎后端：服务模式（普通权限，经命名管道）优先，不可用则回退进程内引擎。
///
/// 探测方式：解析当前用户 SID → 构造管道名 → 尝试 `PipeClient::connect`（带短超时由
/// CreateFileW 自身的管道等待决定）。连接成功即持该连接作为初始 client；失败（服务
/// 未安装/未运行/权限不足）则回退 InProcess——安装服务前的应用行为与 M0–M5 完全一致。
#[cfg(windows)]
fn build_backend() -> Arc<Backend> {
    let pipe_name = ff_ipc::current_user_sid()
        .ok()
        .map(|sid| ff_ipc::pipe_name(&sid));
    if let Some(ref name) = pipe_name {
        match ff_ipc::PipeClient::connect(name) {
            Ok(client) => {
                dbg_log(&format!("backend: pipe service available ({name})"));
                return Arc::new(Backend::Pipe(PipeBackend {
                    client: Mutex::new(Arc::new(client)),
                    pipe_name: name.clone(),
                    last_reconnect_ms: AtomicU64::new(0),
                }));
            }
            Err(e) => dbg_log(&format!("backend: pipe unavailable ({name}): {e}")),
        }
    }
    dbg_log("backend: falling back to in-process engine");
    Arc::new(Backend::InProcess(build_engine()))
}

#[cfg(not(windows))]
fn build_backend() -> Arc<Backend> {
    Arc::new(Backend::InProcess(build_engine()))
}

// ───────────────────────── 托盘 ─────────────────────────

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "隐藏窗口", true, None::<&str>)?;
    let rescan = MenuItem::with_id(app, "rescan", "重建索引", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &hide,
            &rescan,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("窗口图标必须在 tauri.conf.json 的 bundle.icon 配置且文件存在");
    TrayIconBuilder::new()
        .icon(icon)
        .tooltip("Sift")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_window(app.clone()),
            "hide" => hide_window(app.clone()),
            "rescan" => rescan_all(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

// ───────────────────────── 入口 ─────────────────────────

pub fn run() {
    let t0 = std::time::Instant::now();
    let backend = build_backend();
    dbg_log(&format!(
        "startup: backend built @{}ms",
        t0.elapsed().as_millis()
    ));

    // #12：解析失败同样不能 panic（和「注册被占用」一样降级为无全局热键）。
    // 之前这里用 expect，常量写错一个字符就会让整个应用启动即崩。
    let shortcut = "CmdOrCtrl+Alt+Space".parse::<Shortcut>().ok();
    if shortcut.is_none() {
        dbg_log("global shortcut parse failed, continue without hotkey");
    }

    // 失焦自动隐藏的守卫：只有窗口「曾经获得过焦点」之后，失焦才隐藏。
    // 否则启动阶段窗口从未聚焦时收到的 Focused(false) 会把窗口立刻藏掉
    // （双击 exe 一闪即没的 UX bug，2026-10-25 实测修复）。
    let focused_once = Arc::new(AtomicBool::new(false));
    let focused_once_evt = focused_once.clone();

    // L2：全局快捷键可能被其他程序占用，注册失败不应让整个应用启动即崩。
    // 注册成功才装插件；失败仅记日志并继续（托盘、失焦隐藏、窗口命令仍可用）。
    let mut builder = tauri::Builder::default();
    // L2：全局快捷键可能被其他程序占用，注册失败不应让整个应用启动即崩。
    // 注册成功才装插件；失败（含解析失败）仅记日志并继续
    // （托盘、失焦隐藏、窗口命令仍可用）。
    let shortcut_builder = shortcut.and_then(
        |s| match tauri_plugin_global_shortcut::Builder::new().with_shortcuts([s]) {
            Ok(b) => Some(b),
            Err(e) => {
                dbg_log(&format!(
                    "global shortcut register failed, continue without it: {e}"
                ));
                None
            }
        },
    );
    if let Some(sb) = shortcut_builder {
        builder = builder.plugin(
            sb.with_handler(|app, _shortcut, event| {
                dbg_log("hotkey event received");
                if event.state() == ShortcutState::Pressed {
                    dbg_log("hotkey pressed -> toggle_window");
                    toggle_window(app.clone());
                }
            })
            .build(),
        );
    }
    dbg_log(&format!(
        "startup: plugins registered @{}ms (about to .run)",
        t0.elapsed().as_millis()
    ));

    builder
        .setup(move |app| {
            dbg_log(&format!("setup: begin @{}ms", t0.elapsed().as_millis()));
            // 服务状态事件需要 AppHandle；重连时由此通道通知前端 toast。
            let _ = SERVICE_HANDLE.set(app.handle().clone());
            app.manage(EngineState { backend });
            build_tray(app)?;
            // M7：启动文件对话框监听（失败不影响主流程）。
            start_dialog_watcher(app.handle());
            dbg_log("setup: done (tray built, engine managed, dialog watcher started)");
            Ok(())
        })
        .on_window_event(move |window, event| {
            // 失去焦点自动隐藏（DESIGN §11.2，默认开）——两个守卫：
            // ① 只有窗口曾获得过焦点后才允许隐藏（否则启动阶段从未聚焦的
            //    Focused(false) 会把窗口立刻藏掉，双击 exe 一闪即没）；
            // ② 前台窗口属于本进程时不隐藏（WebView2 子窗口/输入法抢焦点会
            //    让父窗口收到 WM_ACTIVATE(WA_INACTIVE)，那不是用户切走）。
            if let WindowEvent::Focused(focused) = event {
                let fg_pid = foreground_pid();
                dbg_log(&format!(
                    "window focused event: {} (self_pid={}, fg_pid={})",
                    focused,
                    std::process::id(),
                    fg_pid
                ));
                if *focused {
                    focused_once_evt.store(true, Ordering::SeqCst);
                } else if fg_pid == 0 {
                    // GetForegroundWindow 无法确定前台（启动瞬间 / 系统切换中）。
                    // 曾因缺这个守卫，启动 2.7s 后的假失焦把窗口误藏，用户看到
                    // 「窗口一闪就没、索引完才再出现」（DECISIONS #96）。
                    dbg_log("  -> ignore: foreground pid unknown (0)");
                } else if !focused_once_evt.load(Ordering::SeqCst) {
                    dbg_log("  -> ignore: never focused before");
                } else if fg_pid == std::process::id() {
                    dbg_log("  -> ignore: foreground is own process (webview child/IME)");
                } else {
                    dbg_log("  -> hide: focus moved to another app");
                    // 统一走 hide_window：让前端先播 90ms 退出动画再真隐藏（§2.6）。
                    let app = window.app_handle().clone();
                    hide_window(app);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            search,
            range,
            cancel,
            status,
            rescan,
            open,
            fill_active_dialog,
            show_window,
            hide_window,
            toggle_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sift");
}

fn main() {
    run();
}
