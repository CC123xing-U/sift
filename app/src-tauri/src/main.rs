// Sift 桌面外壳（Tauri v2）：承载进程内引擎，暴露 search/range/cancel/status/rescan/open
// 六个引擎命令 + show/hide/toggle_window 三个窗口命令；全局热键 Ctrl+Alt+Space 唤出/隐藏；
// 托盘菜单；失焦自动隐藏（DESIGN §11.2）。
//
// 平台：M0–M5 以管理员运行（DESIGN §12），引擎进程内直接调 NtfsVolumeScanner 读真实 NTFS 卷。
// 注意：无条件 windows 子系统——debug 构建也不弹控制台（GUI 应用不需要；
// 若日后要调试日志，用 dbgview 或改为 cfg_attr 条件编译）。
#![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use ff_engine::timeconv;

use ff_engine::api::{
    EngineClient, EngineStatus, OpenAction, RequestId, ResultRow, SearchRequest, SearchSummary,
};
use ff_engine::InProcessEngine;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};

/// 进程内引擎的托管状态（注入到 Tauri 状态容器，命令据此访问）。
struct EngineState {
    engine: Arc<InProcessEngine>,
}

// ───────────────────────── 引擎命令（镜像 EngineClient，DESIGN §9） ─────────────────────────
//
// P0-4：搜索 / 取行 / 打开都是毫秒级（引擎重活在后台线程跑），同步执行即可，
// 不会冻结 UI；真正会卡住主线程的是「重建索引」（全量扫描）与「Overlay 合并
// 持写锁」。因此 `rescan` 改为起线程后立即返回（下方），其余命令保持同步。
// 引擎侧的合并已在读锁下构建（见 InProcessEngine），进一步避免主线程阻塞。

#[tauri::command]
fn search(state: tauri::State<EngineState>, req: SearchRequest) -> Result<SearchSummary, String> {
    state.engine.search(req).map_err(|e| e.to_string())
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
        .engine
        .range(id, offset, limit)
        .map_err(|e| e.to_string())?;
    timeconv::convert_rows(&mut rows);
    Ok(rows)
}

#[tauri::command]
fn cancel(state: tauri::State<EngineState>, id: RequestId) {
    state.engine.cancel(id);
}

#[tauri::command]
fn status(state: tauri::State<EngineState>) -> EngineStatus {
    // P1-8：状态栏「已保存 HH:MM」同样需要 epoch 毫秒，转换口径与 `range` 一致。
    let mut s = state.engine.status();
    timeconv::convert_status(&mut s);
    s
}

#[tauri::command]
fn rescan(state: tauri::State<EngineState>, volume: Option<String>) -> Result<(), String> {
    // M3：引擎公开 API 仅支持「按 VolumeId 全量重建」或「全部重建」，没有「按盘符查卷」的查询。
    // 真实 UI 的 rescan 按钮不带参数（重建全部）；带盘符的单卷重建需引擎暴露卷列表（留待后续）。
    // 此处忽略 `volume` 参数，统一重建全部——行为正确，只是粒度粗。
    let _ = volume;
    // P0-4：起线程立即返回，避免在主线程跑全量扫描（否则托盘「重建索引」会让
    // 窗口与托盘无响应数秒到数十秒）。
    let engine = state.engine.clone();
    std::thread::spawn(move || {
        let _ = engine.rescan(None);
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
        .engine
        .open(id, index, action)
        .map_err(|e| e.to_string())
}

// ───────────────────────── 窗口命令（OS 级 show/hide/toggle） ─────────────────────────
//
// 窗口动效（UI_SPEC §2.6 dur-window-in/out，2026-11-15 落地，DECISIONS #83）：
// show 后向前端发 `sift:shown`，前端重播 140ms 进入动画；hide 前发 `sift:hiding`，
// 等 90ms 退出动画播完再真隐藏——原生 hide 是瞬时的，直接调会把退出动画吞掉。

/// 隐藏挂起的代数计数：`hide_window` 调度延迟隐藏时记下当前代数，到点后代数
/// 一致才真隐藏；`show_window` 递增代数使所有挂起的隐藏作废（热键连按竞态）。
static HIDE_GEN: AtomicU64 = AtomicU64::new(0);

/// 退出动画时长（UI_SPEC §2.6 `dur-window-out`），Rust 侧等它播完再真隐藏。
const WINDOW_OUT_MS: u64 = 90;

#[tauri::command]
fn show_window(app: tauri::AppHandle) {
    dbg_log("show_window called");
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
                if let Some(w) = app.get_webview_window("main") {
                    let r = w.hide();
                    dbg_log(&format!("hide result ok={}", r.is_ok()));
                }
            }
        });
    }
}

#[tauri::command]
fn toggle_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) {
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
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return 0;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid
    }
    #[cfg(not(windows))]
    0
}

/// 临时调试日志（热键/焦点链路排查用，定位后移除）：写 %TEMP%\sift-debug.log。
///
/// P2：无大小上限会随焦点/热键事件无限增长，这里超过 1MB 就先删除旧文件再写。
fn dbg_log(msg: &str) {
    use std::io::Write;
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
    if let Some(state) = app.try_state::<EngineState>() {
        let _ = state.engine.rescan(None);
    }
}

// ───────────────────────── 引擎构建（Windows 真实扫描 / 其他平台空引擎） ─────────────────────────

#[cfg(windows)]
fn build_engine() -> Arc<InProcessEngine> {
    use ff_ntfs::NtfsVolumeScanner;
    let engine = Arc::new(InProcessEngine::new(Box::new(NtfsVolumeScanner)));

    // P2 修复：接线持久化（DESIGN §6：开发期 %LOCALAPPDATA%\Sift\）。
    // 先尝试载入已有索引（秒级），再后台全量重扫（保证新鲜度）。
    let save_dir = std::env::var("LOCALAPPDATA")
        .map(|base| std::path::PathBuf::from(base).join("Sift"))
        .unwrap_or_else(|_| std::env::temp_dir().join("Sift"));
    let _ = std::fs::create_dir_all(&save_dir);
    engine.set_save_dir(&save_dir);
    if let Err(e) = engine.load_all(&save_dir) {
        dbg_log(&format!("engine: load_all failed (will full-scan): {}", e));
    } else {
        dbg_log(&format!(
            "engine: loaded persisted indexes from {:?}",
            save_dir
        ));
    }

    // 后台线程逐个卷触发全量扫描，避免阻塞窗口显示（DESIGN §5.2：扫描期间 UI 显示 Scanning）。
    let scan = engine.clone();
    std::thread::spawn(move || {
        match ff_ntfs::win32::enumerate_volumes() {
            Ok(vols) => {
                dbg_log(&format!("engine: {} volume(s) found", vols.len()));
                let vids: Vec<ff_engine::VolumeId> = vols
                    .iter()
                    .map(|vi| ff_engine::VolumeId {
                        serial: vi.serial,
                        drive: vi.drive,
                    })
                    .collect();
                // 先把本轮要扫的卷全部置为「待扫描」：否则未轮到的卷仍是 Ready
                // （索引自磁盘载入），全局进度会把它们算作已完成，每开扫一个新卷
                // 就被拽回低位（实机：70 多 → 90 多 → 退回 70 多 反复回跳）。
                scan.mark_pending(&vids);
                for vid in vids {
                    dbg_log(&format!("engine: rescan volume {:?}", vid));
                    if let Err(e) = scan.rescan(Some(vid)) {
                        dbg_log(&format!("engine: rescan error: {}", e));
                    }
                    dbg_log("engine: rescan done");
                }
            }
            Err(e) => dbg_log(&format!("engine: enumerate_volumes failed: {}", e)),
        }
    });
    engine
}

#[cfg(not(windows))]
fn build_engine() -> Arc<InProcessEngine> {
    // 非 Windows（CI / 开发机）只保证可编译，不挂真实扫描器。
    Arc::new(InProcessEngine::empty())
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
    let engine = build_engine();

    let shortcut = "CmdOrCtrl+Alt+Space"
        .parse::<Shortcut>()
        .expect("全局快捷键解析失败");

    // 失焦自动隐藏的守卫：只有窗口「曾经获得过焦点」之后，失焦才隐藏。
    // 否则启动阶段窗口从未聚焦时收到的 Focused(false) 会把窗口立刻藏掉
    // （双击 exe 一闪即没的 UX bug，2026-10-25 实测修复）。
    let focused_once = Arc::new(AtomicBool::new(false));
    let focused_once_evt = focused_once.clone();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcuts([shortcut])
                .expect("全局快捷键注册失败")
                .with_handler(|app, _shortcut, event| {
                    dbg_log("hotkey event received");
                    if event.state() == ShortcutState::Pressed {
                        dbg_log("hotkey pressed -> toggle_window");
                        toggle_window(app.clone());
                    }
                })
                .build(),
        )
        .setup(move |app| {
            dbg_log("setup: begin");
            app.manage(EngineState { engine });
            build_tray(app)?;
            dbg_log("setup: done (tray built, engine managed)");
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
