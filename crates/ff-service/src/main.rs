//! ff-service —— Sift M6 Windows 服务外壳（DESIGN §12）。
//!
//! 运行形态：
//! - `ff-service.exe`（无参数）：由 SCM 以 LocalSystem 启动，进入 `service_dispatcher`；
//!   服务持有引擎/索引、监听 USN、跑命名管道服务端（每连接一个 `EngineSession`）。
//! - `ff-service.exe install|uninstall|start|stop|status`：SCM 管理命令（需管理员权限）。
//!
//! 安全约束（DESIGN §10.1/§12、DECISIONS #86）：
//! - 管道 ACL 只放行「活动控制台登录用户」与 SYSTEM；服务为 SYSTEM 时不能用自身
//!   SID 命名管道，必须取交互用户 SID（`interactive_user_sid`）。
//! - `Open` 只解析出结果行经管道回给 UI，服务绝不代替用户打开文件/写剪贴板。

#![cfg(windows)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ff_engine::{EngineClient, InProcessEngine};
use ff_ipc::{
    current_user_sid, interactive_user_sid, pipe_acl_sddl, pipe_name, resolve_volume_id,
    serve_forever, PipeListener,
};
use ff_ntfs::AutoVolumeScanner;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceInfo, ServiceState,
        ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SERVICE_NAME: &str = "SiftService";
const DISPLAY_NAME: &str = "Sift Search Service";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        // 由 SCM 启动：阻塞直到服务停止。
        if let Err(e) = service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
            eprintln!("service_dispatcher::start failed: {e}");
        }
        return;
    }
    let cmd = args[0].to_string_lossy().to_ascii_lowercase();
    let result = match cmd.as_str() {
        "install" => install_service(),
        "uninstall" => uninstall_service(),
        "start" => manager_command("start"),
        "stop" => manager_command("stop"),
        "status" => manager_command("status"),
        other => Err(format!(
            "unknown command: {other}（可用 install/uninstall/start/stop/status）"
        )),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

// ───────────────────── SCM 管理命令（需管理员权限） ─────────────────────

fn open_manager(access: ServiceManagerAccess) -> windows_service::Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access)
}

fn install_service() -> Result<(), String> {
    let manager =
        open_manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
            .map_err(|e| format!("连接 SCM 失败（需管理员权限）: {e}"))?;
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: SERVICE_TYPE,
        start_type: windows_service::service::ServiceStartType::AutoStart,
        error_control: windows_service::service::ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    match manager.create_service(
        &info,
        windows_service::service::ServiceAccess::CHANGE_CONFIG,
    ) {
        Ok(service) => {
            service
                .set_description("Sift 本地搜索服务：持有索引并通过命名管道为 UI 提供搜索")
                .ok();
            println!("已安装服务 {SERVICE_NAME}（LocalSystem，开机自启）。用 net start {SERVICE_NAME} 启动。");
            Ok(())
        }
        Err(e) => Err(format!("创建服务失败: {e}")),
    }
}

fn uninstall_service() -> Result<(), String> {
    let manager = open_manager(ServiceManagerAccess::CONNECT).map_err(|e| e.to_string())?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            windows_service::service::ServiceAccess::DELETE,
        )
        .map_err(|e| format!("打开服务失败: {e}"))?;
    service.delete().map_err(|e| format!("删除服务失败: {e}"))?;
    println!("已标记删除服务 {SERVICE_NAME}。");
    Ok(())
}

fn manager_command(action: &str) -> Result<(), String> {
    let access = match action {
        "start" => windows_service::service::ServiceAccess::START,
        "stop" => windows_service::service::ServiceAccess::STOP,
        _ => windows_service::service::ServiceAccess::QUERY_STATUS,
    };
    let manager = open_manager(ServiceManagerAccess::CONNECT).map_err(|e| e.to_string())?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            access | windows_service::service::ServiceAccess::QUERY_STATUS,
        )
        .map_err(|e| format!("打开服务失败: {e}"))?;
    match action {
        "start" => service.start::<&str>(&[]).map_err(|e| e.to_string())?,
        "stop" => {
            service.stop().map_err(|e| e.to_string())?;
        }
        _ => {}
    }
    let status = service.query_status().map_err(|e| e.to_string())?;
    println!("{SERVICE_NAME}: {:?}", status.current_state);
    Ok(())
}

// ───────────────────── 服务主体 ─────────────────────

define_windows_service!(ffi_service_main, service_main_entry);

fn service_main_entry(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        eprintln!("SiftService error: {e}");
    }
}

fn run_service() -> windows_service::Result<()> {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_handler = stop.clone();

    let event_handler = move |control: ServiceControl| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop_handler.store(true, Ordering::Release);
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    // 引擎 + 索引（服务模式：%ProgramData%\Sift，DESIGN §6）。
    // 扫描器按卷的文件系统自动分派：NTFS 走 MFT + USN，其它走目录遍历回退（M7 §5.4）。
    let engine = Arc::new(InProcessEngine::new(Box::new(AutoVolumeScanner)));
    let save_dir = program_data_sift_dir();
    let _ = std::fs::create_dir_all(&save_dir);
    engine.set_save_dir(&save_dir);
    if let Err(e) = engine.load_all(&save_dir) {
        eprintln!("load_all failed (will full-scan): {e}");
    }
    // 后台全量扫描各卷（此处用卷枚举；失败则仅以已载入索引服务）。
    spawn_initial_scan(engine.clone(), save_dir);

    // 管道名与 ACL：优先活动登录用户 SID；无交互会话时回退 SYSTEM SID（仅 SYSTEM 客户端可用）。
    let sid = interactive_user_sid().or_else(|_| current_user_sid());
    let (name, sddl) = match &sid {
        Ok(s) => (pipe_name(s), pipe_acl_sddl(s)),
        Err(e) => {
            eprintln!("resolve user SID failed, service cannot start pipe: {e}");
            status_handle.set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::Stopped,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(1),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })?;
            return Ok(());
        }
    };
    let listener = PipeListener::new(name.clone(), sddl);
    let listen_engine = engine.clone();
    let listen_stop = stop.clone();
    let server = std::thread::spawn(move || {
        serve_forever(
            listen_engine,
            &listener,
            &listen_stop,
            resolve_volume_id,
            |e| eprintln!("[SiftService] connection error: {e}"),
        );
    });
    eprintln!("SiftService listening on {name}");

    // 本地 HTTP API（默认关闭，DESIGN §10.2）：仅当数据目录下 http.json 显式
    // 启用时才 bind 127.0.0.1:<port>；配置错误只记日志，不影响管道服务。
    let http_thread = start_http(&engine, &stop);

    // 等停止信号；管道 accept 线程靠 stop 旗标 + 一条占位连接退出。
    let _ = stop_rx.recv();
    stop.store(true, Ordering::Release);
    let _ = ff_ipc::connect(&name); // 唤醒阻塞中的 accept
    let _ = server.join();
    if let Some(t) = http_thread {
        let _ = t.join();
    }

    let _ = engine.save_all();
    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    Ok(())
}

/// 按 http.json 配置启动本地 HTTP server（默认关闭）。返回线程句柄供停机 join。
fn start_http(
    engine: &Arc<InProcessEngine>,
    stop: &Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    let config_path = program_data_sift_dir().join("http.json");
    let cfg = match ff_http::load_config(&config_path) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            eprintln!("SiftService HTTP disabled (no http.json or enabled=false)");
            return None;
        }
        Err(e) => {
            eprintln!("SiftService HTTP config error, not started: {e}");
            return None;
        }
    };
    let port = cfg.port;
    let listener = match ff_http::bind(port) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("SiftService HTTP bind 127.0.0.1:{port} failed: {e}");
            return None;
        }
    };
    let cfg = Arc::new(cfg);
    let rate = ff_http::default_rate_limiter(&cfg);
    let http_engine = engine.clone();
    let http_stop = stop.clone();
    let handle = std::thread::spawn(move || {
        ff_http::serve_forever(listener, cfg, http_engine, &http_stop, rate);
    });
    eprintln!("SiftService HTTP listening on 127.0.0.1:{port}");
    Some(handle)
}

fn program_data_sift_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("Sift")
}

fn spawn_initial_scan(engine: Arc<InProcessEngine>, save_dir: std::path::PathBuf) {
    std::thread::spawn(move || match ff_ntfs::win32::enumerate_volumes() {
        Ok(vols) if !vols.is_empty() => {
            // 与桌面端共用清理：删除幽灵卷 .ffix + 崩溃残留 .tmp。服务模式此前
            // 完全没有清理，换盘/拔盘/serial 变化的旧索引永久堆积（F1）。
            let valid: std::collections::HashSet<u32> = vols.iter().map(|v| v.serial).collect();
            let removed = ff_engine::prune_index_dir(&save_dir, &valid);
            if removed > 0 {
                eprintln!("pruned {removed} stale index/tmp file(s) from {save_dir:?}");
            }
            let vids: Vec<_> = vols
                .iter()
                .map(|v| ff_engine::VolumeId {
                    serial: v.serial,
                    drive: v.drive,
                })
                .collect();
            // 索引复用：load_all 已载入磁盘索引，startup_reconcile 只挑出
            // 新卷 / 空索引 / 失效卷做全量扫描；复用卷交给后台 USN 增量续读。
            let need_scan = engine.startup_reconcile(&vids);
            eprintln!(
                "startup: {} volume(s) reused, {} need full scan",
                vids.len() - need_scan.len(),
                need_scan.len()
            );
            engine.mark_pending(&need_scan);
            for vid in need_scan {
                if let Err(e) = engine.rescan(Some(vid.clone())) {
                    eprintln!("rescan {vid} failed: {e}");
                }
            }
        }
        Ok(empty) => eprintln!("enumerate_volumes returned empty, skip prune: {} volume(s)", empty.len()),
        Err(e) => eprintln!("enumerate_volumes failed: {e}"),
    });
}
