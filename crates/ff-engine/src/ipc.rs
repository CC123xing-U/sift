//! M6 shared pipe protocol: u32 little-endian byte length followed by UTF-8 JSON.
//!
//! Transport request ids correlate messages; search result ids belong to the engine.
//! Rust peers keep FILETIME intact. Only the final JS/HTTP boundary uses `timeconv`.
//! This module performs no Windows I/O and never executes shell/clipboard actions.

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::api::{
    EngineClient, EngineError, EngineStatus, OpenAction, RequestId, ResultRow, SearchRequest,
    SearchSummary, VolumeId,
};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_QUERY_BYTES: usize = 16 * 1024;
pub const MAX_RANGE_ROWS: usize = 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message<T> {
    pub protocol_version: u16,
    pub request_id: u64,
    pub body: T,
}

impl<T> Message<T> {
    pub fn new(request_id: u64, body: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            body,
        }
    }

    fn validate_header(&self) -> io::Result<()> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(invalid("unsupported IPC protocol version"));
        }
        if self.request_id == 0 {
            return Err(invalid("transport request_id must be nonzero"));
        }
        Ok(())
    }
}

pub type Request = Message<Command>;
pub type Response = Message<Reply>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Command {
    Search(SearchRequest),
    Range {
        id: RequestId,
        offset: usize,
        limit: usize,
    },
    Cancel {
        id: RequestId,
    },
    Status,
    /// The service resolves a display identifier to a known volume, not a device path.
    Rescan {
        volume: Option<String>,
    },
    /// Resolve a result for the UI; SYSTEM must not execute this action itself.
    Open {
        id: RequestId,
        index: usize,
        action: OpenAction,
    },
}

impl Command {
    pub fn validate(&self) -> io::Result<()> {
        match self {
            Self::Search(req) if req.query.len() > MAX_QUERY_BYTES => {
                Err(invalid("query exceeds IPC byte limit"))
            }
            Self::Range { offset, limit, .. }
                if *limit > MAX_RANGE_ROWS || offset.checked_add(*limit).is_none() =>
            {
                Err(invalid("invalid IPC result window"))
            }
            Self::Rescan {
                volume: Some(volume),
            } if volume.is_empty() || volume.len() > 128 => {
                Err(invalid("invalid volume identifier"))
            }
            Self::Open { index, .. } if index.checked_add(1).is_none() => {
                Err(invalid("invalid result index"))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "data", rename_all = "snake_case")]
pub enum Reply {
    Search(SearchSummary),
    Range(Vec<ResultRow>),
    Cancel,
    Status(EngineStatus),
    Rescan,
    Open(ResolvedAction),
    Error(RpcError),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedAction {
    pub action: OpenAction,
    pub row: ResultRow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    Cancelled,
    Io,
    Internal,
    InvalidRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl From<EngineError> for RpcError {
    fn from(error: EngineError) -> Self {
        let code = match &error {
            EngineError::NotFound => ErrorCode::NotFound,
            EngineError::Cancelled => ErrorCode::Cancelled,
            EngineError::Io(_) => ErrorCode::Io,
            EngineError::Internal(_) => ErrorCode::Internal,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

/// None is a clean disconnect before the next header. Truncated frames are errors.
/// Callers must discard the connection after any decoding/validation error.
pub fn read_request(reader: &mut impl Read) -> io::Result<Option<Request>> {
    let request: Option<Request> = read_message(reader)?;
    if let Some(request) = &request {
        request.body.validate()?;
    }
    Ok(request)
}

pub fn write_request(writer: &mut impl Write, request: &Request) -> io::Result<()> {
    request.body.validate()?;
    write_message(writer, request)
}

pub fn read_response(reader: &mut impl Read) -> io::Result<Option<Response>> {
    read_message(reader)
}

pub fn write_response(writer: &mut impl Write, response: &Response) -> io::Result<()> {
    write_message(writer, response)
}

/// 处理一个已建立连接，直到对端干净断开或帧解码失败。
///
/// 调用方应为每条连接传入独立的 `EngineSession`。`Open` 只解析并返回结果行，
/// 不调用 `EngineClient::open`，因此服务进程不会以 SYSTEM 身份执行 UI 动作。
pub fn serve_connection(
    client: &dyn EngineClient,
    reader: &mut impl Read,
    writer: &mut impl Write,
    resolve_volume: impl Fn(&str) -> Option<VolumeId>,
) -> io::Result<()> {
    while let Some(request) = read_request(reader)? {
        let reply = dispatch(client, request.body, &resolve_volume);
        let response = Message::new(request.request_id, reply);
        write_response(writer, &response)?;
    }
    Ok(())
}

fn dispatch(
    client: &dyn EngineClient,
    command: Command,
    resolve_volume: &impl Fn(&str) -> Option<VolumeId>,
) -> Reply {
    let result = match command {
        Command::Search(request) => client.search(request).map(Reply::Search),
        Command::Range { id, offset, limit } => client.range(id, offset, limit).map(Reply::Range),
        Command::Cancel { id } => {
            client.cancel(id);
            Ok(Reply::Cancel)
        }
        Command::Status => Ok(Reply::Status(client.status())),
        Command::Rescan { volume } => {
            let resolved = match volume {
                Some(name) => resolve_volume(&name).map(Some).ok_or(EngineError::NotFound),
                None => Ok(None),
            };
            resolved.and_then(|v| client.rescan(v).map(|()| Reply::Rescan))
        }
        Command::Open { id, index, action } => client
            .range(id, index, 1)
            .and_then(|mut rows| rows.pop().ok_or(EngineError::NotFound))
            .map(|row| Reply::Open(ResolvedAction { action, row })),
    };
    result.unwrap_or_else(|error| Reply::Error(error.into()))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn read_message<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<Message<T>>> {
    let mut header = [0_u8; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid("invalid IPC frame length"));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    let message: Message<T> =
        serde_json::from_slice(&payload).map_err(|e| invalid(e.to_string()))?;
    message.validate_header()?;
    Ok(Some(message))
}

// Stage at most one bounded frame before touching the socket, including its header.
// Serialization errors must not leave half a JSON frame on a healthy connection.
struct BoundedPayload(Vec<u8>);
impl Write for BoundedPayload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME_BYTES - self.0.len() {
            return Err(invalid("IPC frame exceeds byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_message<T: Serialize>(writer: &mut impl Write, message: &Message<T>) -> io::Result<()> {
    message.validate_header()?;
    let mut payload = BoundedPayload(Vec::new());
    serde_json::to_writer(&mut payload, message).map_err(|e| invalid(e.to_string()))?;
    writer.write_all(&(payload.0.len() as u32).to_le_bytes())?;
    writer.write_all(&payload.0)?;
    writer.flush()
}
