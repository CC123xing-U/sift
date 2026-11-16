//! 本地 HTTP API 真实 TCP 端到端测试（DESIGN §10.2 验收）。
//!
//! 用 `InProcessEngine::empty()`（无需管理员、不真实扫盘）：
//! - 全部拒绝路径：错误/缺失 token、错误 Host、超限 count、速率耗尽；
//! - 正确请求返回 200 + 结构化 JSON；
//! - 无 `Access-Control-*` 头（无宽松 CORS）；
//! - 监听器只绑定 127.0.0.1。

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use ff_engine::InProcessEngine;
use ff_http::{bind, default_rate_limiter, serve_forever, HttpConfig};

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// 起一个绑定临时端口的 HTTP 服务；`rate` 可自定义以测速率拒绝。
    fn start(cfg: HttpConfig, rate: Arc<ff_http::RateLimiter>) -> Self {
        let listener = bind(0).expect("bind 127.0.0.1:0");
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let engine = Arc::new(InProcessEngine::empty());
        let cfg = Arc::new(cfg);
        std::thread::spawn(move || {
            serve_forever(listener, cfg, engine, &stop_thread, rate);
        });
        Server { port, stop }
    }

    fn request(&self, raw: &str) -> String {
        self.request_bytes(raw.as_bytes())
    }

    fn request_bytes(&self, raw: &[u8]) -> String {
        let mut stream = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)))
            .expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(raw).unwrap();
        stream.flush().unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn cfg() -> HttpConfig {
    HttpConfig::new(0, "s3cr3t-token".into()).unwrap()
}

/// 组装一条合法 GET；host / auth 可替换为非法值以测拒绝。
fn get(host: &str, auth: Option<&str>, path: &str) -> String {
    let mut s = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n");
    if let Some(a) = auth {
        s.push_str(&format!("Authorization: {a}\r\n"));
    }
    s.push_str("Connection: close\r\n\r\n");
    s
}

fn status_line(resp: &str) -> &str {
    resp.lines().next().unwrap_or("")
}

#[test]
fn listener_is_bound_to_loopback_only() {
    let listener = bind(0).unwrap();
    let addr = listener.local_addr().unwrap();
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(addr.ip(), Ipv4Addr::UNSPECIFIED, "不得绑定 0.0.0.0");
}

#[test]
fn valid_request_returns_200_json_without_cors_headers() {
    let srv = Server::start(cfg(), default_rate_limiter(&cfg()));
    let resp = srv.request(&get(
        "127.0.0.1:0",
        Some("Bearer s3cr3t-token"),
        "/search?q=ntdll",
    ));
    assert!(status_line(&resp).contains("200 OK"), "{resp}");
    assert!(resp.contains("\"total\":"), "{resp}");
    assert!(resp.contains("\"results\":"), "{resp}");
    assert!(resp.contains("\"offset\":0"), "{resp}");
    assert!(
        !resp.to_ascii_lowercase().contains("access-control-"),
        "禁止宽松 CORS: {resp}"
    );
    assert!(resp
        .to_ascii_lowercase()
        .contains("x-content-type-options: nosniff"));
}

#[test]
fn localhost_host_also_accepted() {
    let srv = Server::start(cfg(), default_rate_limiter(&cfg()));
    let resp = srv.request(&get(
        "LOCALHOST:0",
        Some("Bearer s3cr3t-token"),
        "/search?q=x",
    ));
    assert!(status_line(&resp).contains("200 OK"), "{resp}");
}

#[test]
fn wrong_or_missing_token_is_401() {
    let srv = Server::start(cfg(), default_rate_limiter(&cfg()));
    let no_auth = srv.request(&get("127.0.0.1:0", None, "/search?q=x"));
    assert!(status_line(&no_auth).contains("401"), "{no_auth}");
    assert!(no_auth
        .to_ascii_lowercase()
        .contains("www-authenticate: bearer"));
    let bad = srv.request(&get("127.0.0.1:0", Some("Bearer wrong"), "/search?q=x"));
    assert!(status_line(&bad).contains("401"), "{bad}");
    assert!(bad.contains("\"code\":\"unauthorized\""), "{bad}");
    // token 放查询串里无效。
    let in_url = srv.request(&get("127.0.0.1:0", None, "/search?q=x&token=s3cr3t-token"));
    assert!(status_line(&in_url).contains("401"), "{in_url}");
}

#[test]
fn rebinding_host_is_403() {
    let srv = Server::start(cfg(), default_rate_limiter(&cfg()));
    for host in ["evil.example:0", "localhost:9999", "0.0.0.0:0", "[::1]:0"] {
        let resp = srv.request(&get(host, Some("Bearer s3cr3t-token"), "/search?q=x"));
        assert!(
            status_line(&resp).contains("403"),
            "host={host} resp={resp}"
        );
    }
}

#[test]
fn count_over_limit_is_400() {
    let srv = Server::start(cfg(), default_rate_limiter(&cfg()));
    let resp = srv.request(&get(
        "127.0.0.1:0",
        Some("Bearer s3cr3t-token"),
        "/search?q=x&count=5000",
    ));
    assert!(status_line(&resp).contains("400"), "{resp}");
    assert!(resp.contains("count"), "{resp}");
}

#[test]
fn missing_q_and_bad_method_are_rejected() {
    let srv = Server::start(cfg(), default_rate_limiter(&cfg()));
    let no_q = srv.request(&get("127.0.0.1:0", Some("Bearer s3cr3t-token"), "/search"));
    assert!(status_line(&no_q).contains("400"), "{no_q}");

    let post = "POST /search?q=x HTTP/1.1\r\nHost: 127.0.0.1:0\r\nAuthorization: Bearer s3cr3t-token\r\nConnection: close\r\n\r\n";
    let resp = srv.request(post);
    assert!(status_line(&resp).contains("400"), "{resp}");
}

#[test]
fn rate_limit_exhaustion_is_429() {
    let cfg = cfg();
    // 窗口 60s、容量 2：前两次 OK，第三次 429。
    let limiter = Arc::new(ff_http::RateLimiter::new(2, Duration::from_secs(60)));
    let srv = Server::start(cfg, limiter);
    let first = srv.request(&get(
        "127.0.0.1:0",
        Some("Bearer s3cr3t-token"),
        "/search?q=a",
    ));
    let second = srv.request(&get(
        "127.0.0.1:0",
        Some("Bearer s3cr3t-token"),
        "/search?q=b",
    ));
    let third = srv.request(&get(
        "127.0.0.1:0",
        Some("Bearer s3cr3t-token"),
        "/search?q=c",
    ));
    assert!(status_line(&first).contains("200"), "{first}");
    assert!(status_line(&second).contains("200"), "{second}");
    assert!(status_line(&third).contains("429"), "{third}");
    assert!(third.contains("\"code\":\"rate_limited\""), "{third}");
}

#[test]
fn each_connection_gets_isolated_session() {
    // 空引擎：多个连接顺序搜索都返回 200，不因共享结果集互相作废。
    let srv = Server::start(cfg(), default_rate_limiter(&cfg()));
    for i in 0..5 {
        let path = format!("/search?q=q{i}&count=10");
        let resp = srv.request(&get("127.0.0.1:0", Some("Bearer s3cr3t-token"), &path));
        assert!(status_line(&resp).contains("200"), "i={i} {resp}");
    }
}
