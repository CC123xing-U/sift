// Sift 桌面外壳（Tauri v2）：承载进程内引擎，暴露 search/range/cancel/status/rescan/open
// 六个引擎命令 + show/hide/toggle_window 三个窗口命令；全局热键 Ctrl+Alt+Space 唤出/隐藏；
// 托盘菜单；失焦自动隐藏（DESIGN §11.2）。
//
// 平台：M0–M5 以管理员运行（DESIGN §12），引擎进程内直接调 NtfsVolumeScanner 读真实 NTFS 卷。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
    // 后台线程逐个卷触发全量扫描，避免阻塞窗口显示（DESIGN §5.2：扫描期间 UI 显示 Scanning）。
    let scan = engine.clone();
    std::thread::spawn(move || {
        if let Ok(vols) = ff_ntfs::win32::enumerate_volumes() {
            for vi in vols {
                let vid = ff_engine::VolumeId {
                    serial: vi.serial,
                    drive: vi.drive,
                };
                let _ = scan.rescan(Some(vid));
            }
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

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcuts([shortcut])
                .expect("全局快捷键注册失败")
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle_window(app.clone());
                    }
                })
                .build(),
        )
        .setup(move |app| {
            app.manage(EngineState { engine });
            build_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 失去焦点自动隐藏（DESIGN §11.2，默认开）。
            if let WindowEvent::Focused(false) = event {
                let _ = window.hide();
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
