//! 命名管道服务端循环（DESIGN §10.1 / §12）。
//!
//! 每条连接：
//!   1. `listen.accept()` 得到已连接的 `PipeStream`；
//!   2. `engine.create_session()` 建独立会话（结果/取消/TTL 隔离，共享索引）；
//!   3. 把连接交给独立线程跑 `ff_engine::ipc::serve_connection`，主循环立刻回到
//!      accept 创建下一个管道实例——因此支持多客户端并发（PIPE_UNLIMITED_INSTANCES）；
//!   4. 会话随 `EngineSession` drop 自动释放；连接流 drop 关闭句柄。
//!
//! 单条连接出错（坏帧 / I/O）只丢弃该连接并记日志，不终止监听循环——符合 §10.1
//! 「读写错误后丢弃连接」的契约。

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use ff_engine::ipc::serve_connection;
use ff_engine::InProcessEngine;

use crate::pipe::PipeListener;

/// 阻塞运行管道监听循环，直到 `stop` 置为 true。
///
/// - `engine`：共享引擎（服务持有，持有索引与 USN 监听）；
/// - `listener`：已绑定名字 + ACL 的监听端；
/// - `stop`：停止旗标；置位后需再有一次连入唤醒阻塞中的 accept（服务关闭时停止
///   监听即可，测试里用一条占位连接唤醒）；
/// - `resolve_volume`：卷名 → `VolumeId` 的解析器（见 `client::resolve_volume_id`）；
/// - `on_error`：连接级错误回调（服务用它写 tracing 日志）。
pub fn serve_forever<F, E>(
    engine: Arc<InProcessEngine>,
    listener: &PipeListener,
    stop: &AtomicBool,
    resolve_volume: F,
    on_error: E,
) where
    F: Fn(&str) -> Option<ff_engine::VolumeId> + Send + Sync + Copy + 'static,
    E: Fn(io::Error) + Send + Sync + Copy + 'static,
{
    while !stop.load(Ordering::Acquire) {
        let stream = match listener.accept() {
            Ok(s) => s,
            Err(e) => {
                on_error(e);
                continue;
            }
        };
        if stop.load(Ordering::Acquire) {
            return;
        }

        // 每连接独立会话；reader/writer 用两份指向同一管道端的句柄。
        let session = engine.create_session();
        let writer = match stream.try_clone() {
            Ok(w) => w,
            Err(e) => {
                on_error(e);
                continue;
            }
        };
        let mut reader = stream;
        let mut writer = writer;
        let resolve = resolve_volume;
        let report = on_error;
        thread::spawn(move || {
            if let Err(e) = serve_connection(&session, &mut reader, &mut writer, resolve) {
                report(e);
            }
        });
    }
}
