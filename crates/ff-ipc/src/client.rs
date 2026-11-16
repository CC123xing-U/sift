//! `EngineClient over Named Pipe`：让 UI 以普通权限经管道调用服务里的引擎。
//!
//! 关键约束（DESIGN §12 / DECISIONS #86）：
//! - 每条连接对应服务侧一个独立 `EngineSession`，客户端只管自己的结果集；
//! - `Open` 经 IPC 只拿到 `ResolvedAction`（结果行 + 动作），**本客户端不执行**
//!   打开/剪贴板动作——必须由 UI 进程以当前用户身份执行。这里 `open` 返回 Ok
//!   并向调用方暴露最近解析的动作（见 [`PipeClient::last_open`]）。

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ff_engine::api::{
    EngineError, EngineStatus, OpenAction, RequestId, Result, SearchRequest, SearchSummary,
    VolumeId,
};
use ff_engine::ipc::{read_response, write_request, Command, Message, Reply, ResolvedAction};
use ff_engine::{EngineClient, ResultRow};

use crate::pipe::{connect, PipeStream};

/// 经命名管道调用服务的引擎客户端。`Send + Sync`：内部互斥串行化单条连接的
/// 请求/响应（管道是全双工字节流，一次只能有一个未完成的 request/response 对）。
pub struct PipeClient {
    stream: Mutex<PipeStream>,
    next: AtomicU64,
    last_open: Mutex<Option<ResolvedAction>>,
}

impl PipeClient {
    /// 连接到服务端管道（`name` 为 [`crate::sid::pipe_name`] 的结果）。
    pub fn connect(name: &str) -> io::Result<Self> {
        let stream = connect(name)?;
        Ok(Self {
            stream: Mutex::new(stream),
            next: AtomicU64::new(1),
            last_open: Mutex::new(None),
        })
    }

    /// 取最近一次 `Open` 请求解析出的动作（UI 据此以自身身份执行）。
    pub fn last_open(&self) -> Option<ResolvedAction> {
        self.last_open.lock().unwrap().clone()
    }

    /// 是否已断线（最近一次命令因管道 I/O / 对端关闭失败）。
    ///
    /// `call` 把所有传输错误映射为 `EngineError::Io`，这里用消息前缀识别；
    /// 仅用于决定是否触发重连，不影响返回给前端的错误语义。
    pub fn is_disconnected(err: &ff_engine::EngineError) -> bool {
        matches!(err, ff_engine::EngineError::Io(msg) if msg.contains("pipe"))
    }

    /// 发送一条命令并读取对应响应；transport/协议错误映射为 `EngineError::Io`。
    fn call(&self, command: Command) -> Result<Reply> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut stream = self.stream.lock().unwrap();
        write_request(&mut *stream, &Message::new(id, command))
            .map_err(|e| EngineError::Io(format!("pipe write: {e}")))?;
        let response = read_response(&mut *stream)
            .map_err(|e| EngineError::Io(format!("pipe read: {e}")))?
            .ok_or_else(|| EngineError::Io("pipe closed by service".into()))?;
        if response.request_id != id {
            return Err(EngineError::Io(format!(
                "response id mismatch: {} != {id}",
                response.request_id
            )));
        }
        match response.body {
            Reply::Error(err) => Err(match err.code {
                ff_engine::ipc::ErrorCode::NotFound => EngineError::NotFound,
                ff_engine::ipc::ErrorCode::Cancelled => EngineError::Cancelled,
                other => EngineError::Io(format!("{:?}: {}", other, err.message)),
            }),
            other => Ok(other),
        }
    }

    /// 与 [`EngineClient::status`] 相同的协议调用，但把传输/协议错误以 `Result` 返回，
    /// 让 UI 后端能据此识别断线并触发重连（trait 方法为兼容契约吞掉错误）。
    pub fn status_checked(&self) -> Result<EngineStatus> {
        match self.call(Command::Status)? {
            Reply::Status(s) => Ok(s),
            other => Err(unexpected("status", other)),
        }
    }

    /// 解析打开动作并返回 [`ResolvedAction`]，同时更新 `last_open()`。
    ///
    /// 服务只解析不执行（DESIGN §12）；调用方（UI）拿返回值后以自身身份执行动作。
    pub fn open_resolved(
        &self,
        id: RequestId,
        index: usize,
        action: OpenAction,
    ) -> Result<ResolvedAction> {
        let resolved = match self.call(Command::Open { id, index, action })? {
            Reply::Open(r) => r,
            other => return Err(unexpected("open", other)),
        };
        *self.last_open.lock().unwrap() = Some(resolved.clone());
        Ok(resolved)
    }

    /// 把 `VolumeId` 编码成服务侧 resolver 可解析的字符串：`<serial>:<drive>`。
    fn encode_volume(volume: &VolumeId) -> String {
        match volume.drive {
            Some(c) => format!("{}:{c}", volume.serial),
            None => format!("{}:", volume.serial),
        }
    }
}

/// resolver 的默认实现：解析 `<serial>:<drive>`（服务侧用它把请求里的卷名转回 `VolumeId`）。
pub fn resolve_volume_id(text: &str) -> Option<VolumeId> {
    let (serial, drive) = text.split_once(':')?;
    let serial = serial.parse::<u32>().ok()?;
    let drive = drive.chars().next().filter(|c| c.is_ascii_alphabetic());
    Some(VolumeId {
        serial,
        drive: drive.map(|c| c.to_ascii_uppercase()),
    })
}

impl EngineClient for PipeClient {
    fn search(&self, req: SearchRequest) -> Result<SearchSummary> {
        match self.call(Command::Search(req))? {
            Reply::Search(s) => Ok(s),
            other => Err(unexpected("search", other)),
        }
    }

    fn range(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>> {
        match self.call(Command::Range { id, offset, limit })? {
            Reply::Range(rows) => Ok(rows),
            other => Err(unexpected("range", other)),
        }
    }

    fn cancel(&self, id: RequestId) {
        let _ = self.call(Command::Cancel { id });
    }

    fn status(&self) -> EngineStatus {
        self.status_checked().unwrap_or_else(|_| EngineStatus {
            volumes: Vec::new(),
            total_entries: 0,
            last_saved: None,
            searching: false,
            save_error: Some("pipe status request failed".into()),
        })
    }

    fn rescan(&self, volume: Option<VolumeId>) -> Result<()> {
        let volume = volume.as_ref().map(Self::encode_volume);
        match self.call(Command::Rescan { volume })? {
            Reply::Rescan => Ok(()),
            other => Err(unexpected("rescan", other)),
        }
    }

    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()> {
        self.open_resolved(id, index, action).map(|_| ())
    }
}

fn unexpected(method: &str, reply: Reply) -> EngineError {
    EngineError::Internal(format!("unexpected reply to {method}: {reply:?}"))
}
