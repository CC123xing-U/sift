//! 本地 HTTP API 的运行配置（DESIGN §10.2）。
//!
//! 默认关闭：调用方（ff-service）只有读到「启用」配置时才应构造启用的
//! [`HttpConfig`] 并调用 `server::bind`。本模块只保存配置与生成 token，
//! 不触碰网络。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单次 `/search` 允许返回的最大行数（DESIGN §10.2 第 5 条，与 §10.1 的
/// `MAX_RANGE_ROWS` 对齐）。
pub const MAX_COUNT: usize = 1000;
/// 单次 `/search` 允许的最大 offset，防止 `offset + count` 溢出或超大跳跃。
pub const MAX_OFFSET: usize = 10_000_000;
/// 默认 count。
pub const DEFAULT_COUNT: usize = 100;
/// 默认每秒请求数（按来源 IP 计；HTTP 仅监听 127.0.0.1，实际是全局）。
pub const DEFAULT_RATE_PER_SEC: u32 = 60;

#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// 监听端口。`server::bind` 固定绑定 127.0.0.1，不接受其它地址（§10.2-1）。
    pub port: u16,
    /// Bearer token。由 [`generate_token`] 产生；空串表示未启用鉴权，
    /// 调用方不应在启用状态下构造空 token（server 会拒绝启动）。
    pub token: String,
    /// `count` 上限，默认 [`MAX_COUNT`]。
    pub max_count: usize,
    /// 每秒允许的请求数，默认 [`DEFAULT_RATE_PER_SEC`]。
    pub rate_per_sec: u32,
}

impl HttpConfig {
    /// 构造配置；空 token 返回错误（启用鉴权是硬性要求，§10.2-2）。
    pub fn new(port: u16, token: String) -> Result<Self, &'static str> {
        if token.is_empty() {
            return Err("HTTP token 不能为空");
        }
        Ok(Self {
            port,
            token,
            max_count: MAX_COUNT,
            rate_per_sec: DEFAULT_RATE_PER_SEC,
        })
    }
}

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成一次性的随机 Bearer token（64 个十六进制字符 = 256 bit）。
///
/// 只用 std：混合单调时间纳秒、进程 id、静态变量地址与进程内原子计数器，
/// 再经 xorshift64* 扩散。token 只在进程启动时生成、仅保存在服务进程与
/// （可选）受 ACL 保护的配置文件中，配合速率限制足以阻止本地猜测；不依赖
/// 新 crate（DESIGN §0.2 / DEPENDENCIES 零新增依赖约束）。
pub fn generate_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let seq = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let addr = &TOKEN_COUNTER as *const _ as usize as u64;
    let mut x =
        now ^ ((std::process::id() as u64) << 32) ^ addr ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    if x == 0 {
        x = 0xD1B5_4A32_D192_ED03;
    }
    let mut out = String::with_capacity(64);
    for _ in 0..4 {
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let mixed = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.push_str(&format!("{mixed:016x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "两次生成必须不同");
    }

    #[test]
    fn empty_token_rejected() {
        assert!(HttpConfig::new(8080, String::new()).is_err());
    }

    #[test]
    fn defaults_match_spec() {
        let c = HttpConfig::new(8080, "t".into()).unwrap();
        assert_eq!(c.max_count, MAX_COUNT);
        assert_eq!(c.rate_per_sec, DEFAULT_RATE_PER_SEC);
    }
}
