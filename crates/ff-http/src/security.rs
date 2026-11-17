//! 本地 HTTP API 的安全边界（DESIGN §10.2 硬性要求）。
//!
//! 这里全部是可独立单测的纯函数 / 小型状态机，不依赖 socket，便于把
//! 「错误 token / 错误 Host / 超限 count / 速率耗尽必须被拒」写成断言：
//!
//! 1. `check_host`：Host 头必须是 `127.0.0.1:<port>` 或 `localhost:<port>`，
//!    防 DNS rebinding；任何其它 host、缺省端口、多余端口、IPv6 全部拒绝。
//! 2. `check_bearer`：`Authorization: Bearer <token>`，恒定时间比较，防时序侧信道；
//!    token 只能放在头里，绝不接受查询串。
//! 3. `RateLimiter`：固定窗口计数，防本地脚本高频拉取文件列表。
//! 4. `parse_count` / `parse_offset` / `parse_sort`：入参上限与溢出校验。
//!
//! 不设置任何 CORS 头由 `server` 的响应构造保证（不输出 `Access-Control-*`），
//! 因此浏览器跨站脚本无法读取响应；这里不再重复。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ff_engine::{SortKey, SortSpec};

use crate::config::{MAX_COUNT, MAX_OFFSET};

/// Host 头合法返回 `Ok(())`，否则返回拒绝原因（便于日志/测试）。
pub fn check_host(host: Option<&str>, port: u16) -> Result<(), &'static str> {
    let host = host.ok_or("missing Host header")?;
    // HTTP/1.1 Host 允许尾随空白（OWS）；trim 后再严格比对。
    let host = host.trim();
    // DNS rebinding 关键：只接受两种字面量 + 精确端口，禁止逗号列表、userinfo、
    // IPv6、尾点、缺省端口。比较大小写不敏感以兼容 `LOCALHOST`（RFC 1035）。
    let expected_v4 = format!("127.0.0.1:{port}");
    let expected_local = format!("localhost:{port}");
    if host.eq_ignore_ascii_case(&expected_v4) || host.eq_ignore_ascii_case(&expected_local) {
        Ok(())
    } else {
        Err("Host header is not the local listener")
    }
}

/// 校验 `Authorization` 头；token 只认 Bearer 方案，恒定时间比较。
pub fn check_bearer(auth: Option<&str>, expected: &str) -> Result<(), &'static str> {
    let auth = auth.ok_or("missing Authorization header")?;
    let value = auth.trim();
    // 严格前缀匹配（大小写不敏感的方案名，RFC 7235），后接一个空格 + token。
    // 按字节切片：value 是 UTF-8 String，value[..7] 在第 7 字节落在多字节字符中间时会
    // panic（char boundary）。头值可由远端控制，故全程用 as_bytes() 比较。
    let bytes = value.as_bytes();
    if bytes.len() < 7 || !bytes[..7].eq_ignore_ascii_case(b"Bearer ") {
        return Err("not a Bearer credential");
    }
    let candidate = &bytes[7..];
    if constant_time_eq(candidate, expected.as_bytes()) {
        Ok(())
    } else {
        Err("invalid bearer token")
    }
}

/// 长度泄露不在威胁模型内（token 长度固定 64）；按字节累加差异避免提前退出。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 固定窗口速率限制器：每个 key（来源 IP）在 `window` 内最多 `capacity` 次。
/// 用 `Instant` 单调时钟，不受系统时间回拨影响。
pub struct RateLimiter {
    capacity: u32,
    window: Duration,
    state: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(capacity: u32, window: Duration) -> Self {
        Self {
            capacity,
            window,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// `per_sec` 便捷构造：1 秒窗口。
    pub fn per_sec(per_sec: u32) -> Self {
        Self::new(per_sec, Duration::from_secs(1))
    }

    /// 尝试消费一次配额；允许返回 `true`，超限返回 `false`。
    pub fn allow(&self, key: IpAddr, now: Instant) -> bool {
        let mut state = self.state.lock().unwrap();
        let entry = state.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            // 窗口过期：重置起点与计数。
            *entry = (now, 0);
        }
        if entry.1 >= self.capacity {
            false
        } else {
            entry.1 += 1;
            true
        }
    }

    /// 从 socket 地址取 key（HTTP 只绑 loopback，实际恒为 127.0.0.1）。
    pub fn key_of(addr: SocketAddr) -> IpAddr {
        if addr.ip().is_loopback() {
            // 归一化所有 loopback（::1 / 127.x）到 IPv4 回环，共用同一配额。
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            addr.ip()
        }
    }
}

/// 解析 `count`：缺省用 `default`；必须是十进制正整数，`>0` 且 `<=max_count`。
/// 显式上限（`max_count`）也受全局硬上限 [`MAX_COUNT`] 约束，防止调用方误配。
pub fn parse_count(
    raw: Option<&str>,
    default: usize,
    max_count: usize,
) -> Result<usize, &'static str> {
    let Some(raw) = raw else {
        return Ok(default.min(max_count).max(1));
    };
    let n = raw
        .parse::<usize>()
        .map_err(|_| "count must be a positive integer")?;
    if n == 0 {
        return Err("count must be greater than zero");
    }
    if n > max_count.min(MAX_COUNT) {
        return Err("count exceeds the allowed limit");
    }
    Ok(n)
}

/// 解析 `offset`：缺省 0；必须是十进制非负整数且 `<=MAX_OFFSET`。
pub fn parse_offset(raw: Option<&str>) -> Result<usize, &'static str> {
    let Some(raw) = raw else {
        return Ok(0);
    };
    let n = raw
        .parse::<usize>()
        .map_err(|_| "offset must be a non-negative integer")?;
    if n > MAX_OFFSET {
        return Err("offset exceeds the allowed limit");
    }
    Ok(n)
}

/// 解析 `sort`：形如 `score`（默认降序）或 `-name`（升序）。
/// 未知键返回错误，避免静默退化为错误排序。
pub fn parse_sort(raw: Option<&str>) -> Result<SortSpec, &'static str> {
    let Some(raw) = raw else {
        return Ok(SortSpec::default());
    };
    if raw.is_empty() {
        return Ok(SortSpec::default());
    }
    let (descending, key) = match raw.as_bytes()[0] {
        b'-' => (false, &raw[1..]),
        b'+' => (true, &raw[1..]),
        _ => (true, raw),
    };
    let key = match key {
        "score" => SortKey::Score,
        "name" => SortKey::Name,
        "path" => SortKey::Path,
        "size" => SortKey::Size,
        "modified" => SortKey::Modified,
        _ => return Err("unknown sort key"),
    };
    Ok(SortSpec { key, descending })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Host ────────────────────────────────────────────────
    #[test]
    fn host_accepts_only_local_literals_with_exact_port() {
        assert!(check_host(Some("127.0.0.1:8080"), 8080).is_ok());
        assert!(check_host(Some("localhost:8080"), 8080).is_ok());
        assert!(check_host(Some("LOCALHOST:8080"), 8080).is_ok());
        assert!(check_host(Some(" 127.0.0.1:8080 "), 8080).is_ok());
    }

    #[test]
    fn host_rejects_rebinding_variants() {
        assert!(check_host(None, 8080).is_err());
        assert!(check_host(Some("127.0.0.1:9090"), 8080).is_err());
        assert!(check_host(Some("127.0.0.1"), 8080).is_err());
        assert!(check_host(Some("evil.example:8080"), 8080).is_err());
        assert!(check_host(Some("[::1]:8080"), 8080).is_err());
        assert!(check_host(Some("127.0.0.1:8080,evil:8080"), 8080).is_err());
        assert!(check_host(Some("127.0.0.1:8080."), 8080).is_err());
        assert!(check_host(Some("user@127.0.0.1:8080"), 8080).is_err());
        assert!(check_host(Some("localhost:8080:8080"), 8080).is_err());
    }

    // ── Bearer ──────────────────────────────────────────────
    #[test]
    fn bearer_accepts_exact_token_only() {
        let token = "abc123TOKEN";
        assert!(check_bearer(Some("Bearer abc123TOKEN"), token).is_ok());
        assert!(check_bearer(Some("bearer abc123TOKEN"), token).is_ok());
        assert!(check_bearer(Some(" Bearer abc123TOKEN "), token).is_ok());
    }

    #[test]
    fn bearer_rejects_wrong_or_smuggled_token() {
        let token = "abc123TOKEN";
        assert!(check_bearer(None, token).is_err());
        assert!(check_bearer(Some("Bearer wrong"), token).is_err());
        assert!(check_bearer(Some("Basic abc123TOKEN"), token).is_err());
        assert!(check_bearer(Some("Bearer abc123TOKEN extra"), token).is_err());
        assert!(check_bearer(Some("Bearer "), token).is_err());
        // token 不得出现在查询串里——服务端根本只看 Authorization 头。
        assert!(check_bearer(Some("?token=abc123TOKEN"), token).is_err());
    }

    #[test]
    fn bearer_non_ascii_prefix_does_not_panic() {
        let token = "abc123TOKEN";
        // 第 7 字节落在多字节字符中间时，字符串切片 value[..7] 会 panic（char boundary）。
        // 头值可由远端控制，必须优雅返回 Err。
        assert!(check_bearer(Some("abcdefé"), token).is_err());
        assert!(check_bearer(Some("Bearer 你好"), token).is_err());
        assert!(check_bearer(Some("中文字符填充值超过七字节"), token).is_err());
    }

    // ── Rate limit ──────────────────────────────────────────
    #[test]
    fn rate_limiter_refuses_after_capacity_then_resets_after_window() {
        let limiter = RateLimiter::new(2, Duration::from_secs(1));
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let t0 = Instant::now();
        assert!(limiter.allow(ip, t0));
        assert!(limiter.allow(ip, t0 + Duration::from_millis(100)));
        assert!(
            !limiter.allow(ip, t0 + Duration::from_millis(200)),
            "第 3 次必须被拒"
        );
        assert!(
            limiter.allow(ip, t0 + Duration::from_secs(2)),
            "窗口过后恢复"
        );
    }

    #[test]
    fn rate_limiter_keys_are_independent() {
        let limiter = RateLimiter::per_sec(1);
        let a = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        let now = Instant::now();
        assert!(limiter.allow(a, now));
        assert!(!limiter.allow(a, now));
        assert!(limiter.allow(b, now));
    }

    #[test]
    fn loopback_sockets_share_one_bucket() {
        let v4 = SocketAddr::from(([127, 0, 0, 1], 1234));
        let v6 = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 1234));
        assert_eq!(RateLimiter::key_of(v4), RateLimiter::key_of(v6));
    }

    // ── 参数解析 ────────────────────────────────────────────
    #[test]
    fn count_bounds_and_defaults() {
        assert_eq!(parse_count(None, 100, 1000).unwrap(), 100);
        assert_eq!(parse_count(Some("25"), 100, 1000).unwrap(), 25);
        assert!(parse_count(Some("0"), 100, 1000).is_err());
        assert!(parse_count(Some("1001"), 100, 1000).is_err());
        assert!(parse_count(Some("-1"), 100, 1000).is_err());
        assert!(parse_count(Some("abc"), 100, 1000).is_err());
        // 调用方误配的上限不得超过硬上限。
        assert!(parse_count(Some("5000"), 100, usize::MAX).is_err());
    }

    #[test]
    fn offset_bounds_and_defaults() {
        assert_eq!(parse_offset(None).unwrap(), 0);
        assert_eq!(parse_offset(Some("20")).unwrap(), 20);
        assert!(parse_offset(Some("-3")).is_err());
        assert!(parse_offset(Some(&format!("{}", MAX_OFFSET + 1))).is_err());
    }

    #[test]
    fn sort_keys_and_direction() {
        assert_eq!(parse_sort(None).unwrap(), SortSpec::default());
        assert_eq!(
            parse_sort(Some("name")).unwrap(),
            SortSpec {
                key: SortKey::Name,
                descending: true
            }
        );
        assert_eq!(
            parse_sort(Some("-name")).unwrap(),
            SortSpec {
                key: SortKey::Name,
                descending: false
            }
        );
        assert_eq!(parse_sort(Some("modified")).unwrap().key, SortKey::Modified);
        assert!(parse_sort(Some("bogus")).is_err());
    }
}
