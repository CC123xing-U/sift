// Sift 桌面外壳（Tauri v2）：承载进程内引擎，暴露 search/range/cancel/status/rescan/open
// 六个引擎命令 + show/hide/toggle_window 三个窗口命令；全局热键 Ctrl+Alt+Space 唤出/隐藏；
// 托盘菜单；失焦自动隐藏（DESIGN §11.2）。
//
// 平台：M0–M5 以管理员运行（DESIGN §12），引擎进程内直接调 NtfsVolumeScanner 读真实 NTFS 卷。
// 注意：无条件 windows 子系统——debug 构建也不弹控制台（GUI 应用不需要；
// 若日后要调试日志，用 dbgview 或改为 cfg_attr 条件编译）。
#![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ff_engine::api::{
    EngineClient, EngineStatus, OpenAction, RequestId, ResultRow, SearchRequest, SearchSummary,
};
use ff_engine::InProcessEngine;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};

/// 进程内引擎的托管状态（注入到 Tauri 状态容器，命令据此访问）。
struct EngineState {
    engine: Arc<InProcessEngine>,
}

// ───────────────────────── 引擎命令（镜像 EngineClient，DESIGN §9） ─────────────────────────

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
    state
        .engine
        .range(id, offset, limit)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel(state: tauri::State<EngineState>, id: RequestId) {
    state.engine.cancel(id);
}

#[tauri::command]
fn status(state: tauri::State<EngineState>) -> EngineStatus {
    state.engine.status()
}

#[tauri::command]
fn rescan(state: tauri::State<EngineState>, volume: Option<String>) -> Result<(), String> {
    // M3：引擎公开 API 仅支持「按 VolumeId 全量重建」或「全部重建」，没有「按盘符查卷」的查询。
    // 真实 UI 的 rescan 按钮不带参数（重建全部）；带盘符的单卷重建需引擎暴露卷列表（留待后续）。
    // 此处忽略 `volume` 参数，统一重建全部——行为正确，只是粒度粗。
    let _ = volume;
    state.engine.rescan(None).map_err(|e| e.to_string())
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

#[tauri::command]
fn show_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[tauri::command]
fn hide_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
fn toggle_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

// 判断当前前台窗口是否属于本进程。
//
// WebView2 子窗口（或输入法 textinputhost 关联行为）获得焦点时，父窗口会收到
// `WM_ACTIVATE(WA_INACTIVATE)` → Tauri `Focused(false)`，但那不是「用户切到别的
// 应用」，不应隐藏窗口（2026-10-25 实测：启动 2.3s 页面加载完就被误藏）。
// 用 user32 裸 extern 声明，避免为此引入 winapi/windows crate。
#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> *mut std::ffi::c_void;
    fn GetWindowThreadProcessId(hwnd: *mut std::ffi::c_void, pid: *mut u32) -> u32;
}

fn foreground_pid() -> u32 {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return 0;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid
    }
}

/// 临时调试日志（热键/焦点链路排查用，定位后移除）：写 %TEMP%\sift-debug.log。
fn dbg_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("sift-debug.log"))
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
        dbg_log(&format!("engine: loaded persisted indexes from {:?}", save_dir));
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
                    let _ = window.hide();
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
