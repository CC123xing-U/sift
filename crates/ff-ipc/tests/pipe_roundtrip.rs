//! 真实 OS 命名管道端到端验收（DESIGN §10.1：不把纯内存编解码当作 OS 管道验收）。
//!
//! 同一进程内：启动 server 线程（PipeListener + serve_forever + InProcessEngine），
//! 客户端经 `PipeClient` 连接，验证 Search/Range/Status/Open/Cancel 全链路、
//! 会话隔离与请求 id 关联。管道名带随机后缀避免并行测试冲突。

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ff_core::{EntryInput, IndexBuilder, VolumeInfo};
use ff_engine::api::{OpenAction, SearchOptions, SearchRequest, SortSpec, VolumeId};
use ff_engine::{EngineClient, InProcessEngine};
use ff_ipc::{
    current_user_sid, pipe_acl_sddl, resolve_volume_id, serve_forever, PipeClient, PipeListener,
};

fn sample_index() -> ff_core::VolumeIndex {
    let mut b = IndexBuilder::new(VolumeInfo {
        drive: Some('C'),
        ..Default::default()
    });
    let dir = |frn: u64, parent: u64, name: &str| EntryInput {
        frn,
        parent_frn: parent,
        name: name.into(),
        is_dir: true,
        attrs: 0x10,
        size: 0,
        mtime: 100,
        ctime: 100,
        atime: 100,
    };
    let file = |frn: u64, parent: u64, name: &str, size: u64| EntryInput {
        frn,
        parent_frn: parent,
        name: name.into(),
        is_dir: false,
        attrs: 0,
        size,
        mtime: 200,
        ctime: 200,
        atime: 200,
    };
    b.push(dir(5, 0, "."));
    b.push(dir(100, 5, "Windows"));
    b.push(file(101, 100, "readme.txt", 1024));
    b.push(dir(200, 5, "用户"));
    b.push(file(201, 200, "文档.txt", 2048));
    b.finish()
}

fn req(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        sort: SortSpec::default(),
        max_hits: None,
        opts: SearchOptions::default(),
    }
}

struct Server {
    pipe_name: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

/// 等服务线程创建好管道实例后再连（消除 accept 启动竞态）。
fn connect_retry(name: &str) -> PipeClient {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match PipeClient::connect(name) {
            Ok(c) => return c,
            Err(e) if std::time::Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
                let _ = e;
            }
            Err(e) => panic!("client connect timeout: {e}"),
        }
    }
}

impl Server {
    fn start() -> Self {
        let sid = current_user_sid().unwrap();
        let suffix = std::process::id();
        let name = format!(
            r"\\.\pipe\sift-test-{sid}-{suffix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                & 0xffff
        );
        let listener = PipeListener::new(name.clone(), pipe_acl_sddl(&sid));

        let engine = InProcessEngine::empty();
        engine.add_volume(
            VolumeId {
                serial: 1,
                drive: Some('C'),
            },
            sample_index(),
        );

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = thread::spawn(move || {
            serve_forever(
                Arc::new(engine),
                &listener,
                &stop_thread,
                resolve_volume_id,
                |e| eprintln!("[pipe test] connection error: {e}"),
            );
        });
        Self {
            pipe_name: name,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // 唤醒阻塞的 accept：连一条占位管道让循环走到 stop 判断后退出。
        let _ = ff_ipc::connect(&self.pipe_name);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[test]
fn pipe_roundtrip_search_range_status_open() {
    let server = Server::start();
    let client = connect_retry(&server.pipe_name);

    // Search
    let sum = client.search(req("readme")).expect("search");
    assert_eq!(sum.total, 1, "应命中 readme.txt");

    // Range
    let rows = client.range(sum.id, 0, 10).expect("range");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "readme.txt");
    assert_eq!(rows[0].size, Some(1024));

    // Status
    let status = client.status();
    assert_eq!(status.total_entries, 5, "索引 5 条");

    // Open：返回解析行，不执行动作；客户端可读到 last_open
    client.open(sum.id, 0, OpenAction::CopyPath).expect("open");
    let resolved = client.last_open().expect("resolved action");
    assert_eq!(resolved.row.name, "readme.txt");
    assert_eq!(resolved.action, OpenAction::CopyPath);

    // Cancel：无对应进行中请求时应静默（管道不返回错误）
    client.cancel(sum.id);
}

#[test]
fn two_clients_have_isolated_sessions() {
    let server = Server::start();
    let a = connect_retry(&server.pipe_name);
    let b = connect_retry(&server.pipe_name);

    let sum_a = a.search(req("readme")).unwrap();
    let sum_b = b.search(req("文档")).unwrap();
    assert_eq!(sum_a.total, 1);
    assert_eq!(sum_b.total, 1);

    // 两个连接各自的引擎会话保留自己的结果集，互不覆盖。
    assert_eq!(a.range(sum_a.id, 0, 10).unwrap()[0].name, "readme.txt");
    assert_eq!(b.range(sum_b.id, 0, 10).unwrap()[0].name, "文档.txt");
    thread::sleep(Duration::from_millis(10));
}

#[test]
fn resolve_volume_id_roundtrip() {
    let v = VolumeId {
        serial: 0x9EA0C151,
        drive: Some('C'),
    };
    let encoded = format!("{}:{}", v.serial, 'C');
    assert_eq!(resolve_volume_id(&encoded), Some(v));
    assert_eq!(
        resolve_volume_id("2650120001:"),
        Some(VolumeId {
            serial: 2650120001,
            drive: None
        })
    );
    assert_eq!(resolve_volume_id("garbage"), None);
}
