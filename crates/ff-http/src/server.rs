//! 本地 HTTP server（DESIGN §10.2 / §12）。
//!
//! - 仅绑定 `127.0.0.1:<port>`；端口 0 用于测试拿临时端口。
//! - 每个 TCP 连接建一个 `EngineSession`（结果集/取消与管道、其它连接隔离）。
//! - 请求顺序：有界读头 → Host 校验 → Bearer 校验 → 速率限制 → 解析 query →
//!   `search` + `range` → FILETIME 转 epoch 毫秒 → JSON 响应。任一步失败都给
//!   结构化错误，且**不输出任何 `Access-Control-*` 头**（无宽松 CORS）。
//! - 服务进程（SYSTEM）经此 API 只返回文件列表，不执行打开/剪贴板动作（§12）。

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ff_engine::{EngineClient, InProcessEngine, SearchOptions, SearchRequest};

use crate::config::HttpConfig;
use crate::security::{self, RateLimiter};
use crate::wire::{self, ErrorBody, ParseReject, SearchResponse, MAX_HEAD_BYTES};

/// 单连接读超时，防慢速客户端长期占用线程。
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// 绑定本地监听器。`port=0` 时由 OS 分配临时端口，便于测试。
///
/// 强制使用 `127.0.0.1`，不接受配置其它地址（§10.2-1）：即使调用方误配也
/// 不会把文件列表暴露到局域网。
pub fn bind(port: u16) -> io::Result<TcpListener> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// 阻塞运行 HTTP accept 循环，直到 `stop` 置 true。
///
/// 每个连接一个线程 + 独立 `EngineSession`；单连接错误只影响该连接。
/// `engine` 与服务/管道共享同一索引。
pub fn serve_forever(
    listener: TcpListener,
    cfg: Arc<HttpConfig>,
    engine: Arc<InProcessEngine>,
    stop: &AtomicBool,
    rate: Arc<RateLimiter>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let cfg = cfg.clone();
                let eng = engine.clone();
                let rate = rate.clone();
                std::thread::spawn(move || {
                    let session = eng.create_session();
                    if let Err(e) = handle_connection(stream, peer, &cfg, &session, &rate) {
                        // 连接级错误只记日志，不终止监听（与管道 §10.1 一致）。
                        if !(e.kind() == io::ErrorKind::UnexpectedEof
                            || e.kind() == io::ErrorKind::ConnectionReset)
                        {
                            eprintln!("[SiftHttp] connection error from {peer}: {e}");
                        }
                    }
                    drop(session);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                eprintln!("[SiftHttp] accept error: {e}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// 处理单个连接：目前 HTTP/1.1 每连接一个请求，回复后关闭（Connection: close）。
fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    cfg: &HttpConfig,
    session: &dyn EngineClient,
    rate: &RateLimiter,
) -> io::Result<()> {
    let _ = stream.set_read_timeout(Some(CONNECTION_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECTION_TIMEOUT));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    // ── 1. 有界读请求头 ──────────────────────────────────
    let head = read_head(&mut reader)?;
    let (request_line, headers) = match head {
        Some(h) => h,
        None => return Ok(()), // 干净断开
    };

    // ── 2. 请求行 → 路径 + query ────────────────────────
    let (_path, raw_query) = match wire::parse_request_line(&request_line) {
        Ok(v) => v,
        Err(reject) => return write_reject(&mut writer, reject),
    };

    // ── 3. Host 校验（防 DNS rebinding） ─────────────────
    if let Err(reason) = security::check_host(header(&headers, "host"), cfg.port) {
        return write_json(
            &mut writer,
            403,
            "forbidden",
            &ErrorBody {
                error: reason.into(),
                code: "bad_host",
            },
        );
    }

    // ── 4. Bearer 鉴权（§10.2-2） ────────────────────────
    if let Err(reason) = security::check_bearer(header(&headers, "authorization"), &cfg.token) {
        let mut writer = writer;
        let body = ErrorBody {
            error: reason.into(),
            code: "unauthorized",
        };
        return write_json_with_auth(&mut writer, 401, "unauthorized", &body);
    }

    // ── 5. 速率限制（§10.2-5） ──────────────────────────
    if !rate.allow(RateLimiter::key_of(peer), Instant::now()) {
        return write_json(
            &mut writer,
            429,
            "too many requests",
            &ErrorBody {
                error: "rate limit exceeded".into(),
                code: "rate_limited",
            },
        );
    }

    // ── 6. 解析查询参数 ─────────────────────────────────
    let query = match wire::parse_search_query(&raw_query, cfg) {
        Ok(q) => q,
        Err(reject) => return write_reject(&mut writer, reject),
    };

    // ── 7. 调引擎：search + range ───────────────────────
    let req = SearchRequest {
        query: query.text,
        sort: query.sort,
        max_hits: None,
        opts: SearchOptions::default(),
    };
    let summary = session.search(req).map_err(io_engine)?;
    if let Some(pe) = summary.parse_error {
        return write_json(
            &mut writer,
            422,
            "unprocessable entity",
            &ErrorBody {
                error: format!("query parse error: {pe:?}"),
                code: "bad_query",
            },
        );
    }
    let rows = session
        .range(summary.id, query.offset, query.count)
        .map_err(io_engine)?;
    let body = SearchResponse {
        total: summary.total,
        offset: query.offset,
        results: wire::to_http_rows(&rows),
    };
    write_json(&mut writer, 200, "OK", &body)
}

/// 读请求头直到空行，带总字节上限；返回 `None` 表示对端干净断开。
fn read_head(reader: &mut impl BufRead) -> io::Result<Option<(String, HashMap<String, String>)>> {
    let mut line = String::new();
    let mut total = 0usize;
    let mut request_line = String::new();
    let mut headers = HashMap::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return if total == 0 {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete headers",
                ))
            };
        }
        total += n;
        if total > MAX_HEAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "headers too large",
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if request_line.is_empty() {
            request_line = trimmed.to_string();
        } else if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed header",
            ));
        }
    }
    if request_line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty request line",
        ));
    }
    Ok(Some((request_line, headers)))
}

type HashMap<K, V> = std::collections::HashMap<K, V>;

fn header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers.get(name).map(String::as_str)
}

fn io_engine(e: ff_engine::EngineError) -> io::Error {
    io::Error::other(format!("engine error: {e}"))
}

fn write_reject(writer: &mut impl Write, reject: ParseReject) -> io::Result<()> {
    match reject {
        ParseReject::BadRequest(msg) => write_json(
            writer,
            400,
            "bad request",
            &ErrorBody {
                error: msg.into(),
                code: "bad_request",
            },
        ),
        ParseReject::TooLarge => write_json(
            writer,
            413,
            "payload too large",
            &ErrorBody {
                error: "request too large".into(),
                code: "too_large",
            },
        ),
    }
}

fn write_json<T: serde::Serialize>(
    writer: &mut impl Write,
    status: u16,
    reason: &str,
    body: &T,
) -> io::Result<()> {
    write_json_headers(writer, status, reason, body, false)
}

fn write_json_with_auth<T: serde::Serialize>(
    writer: &mut impl Write,
    status: u16,
    reason: &str,
    body: &T,
) -> io::Result<()> {
    write_json_headers(writer, status, reason, body, true)
}

fn write_json_headers<T: serde::Serialize>(
    writer: &mut impl Write,
    status: u16,
    reason: &str,
    body: &T,
    auth: bool,
) -> io::Result<()> {
    let payload =
        serde_json::to_vec(body).map_err(|e| io::Error::other(format!("serialize: {e}")))?;
    write!(writer, "HTTP/1.1 {status} {reason}\r\n")?;
    write!(writer, "Content-Type: application/json; charset=utf-8\r\n")?;
    write!(writer, "Content-Length: {}\r\n", payload.len())?;
    write!(writer, "Cache-Control: no-store\r\n")?;
    write!(writer, "X-Content-Type-Options: nosniff\r\n")?;
    if auth {
        write!(writer, "WWW-Authenticate: Bearer\r\n")?;
    }
    // 刻意不输出任何 `Access-Control-Allow-*` / `Access-Control-Expose-*`：
    // 浏览器跨源脚本拿不到响应（DESIGN §10.2-4）。
    write!(writer, "Connection: close\r\n\r\n")?;
    writer.write_all(&payload)?;
    writer.flush()
}

/// 便捷入口：构造速率限制器（供服务与测试共用同一默认策略）。
pub fn default_rate_limiter(cfg: &HttpConfig) -> Arc<RateLimiter> {
    Arc::new(RateLimiter::per_sec(cfg.rate_per_sec))
}
