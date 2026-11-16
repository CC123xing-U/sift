//! ff-http —— Sift M6 本地 HTTP API（DESIGN §10.2 / §12）。
//!
//! 默认关闭、仅绑定 `127.0.0.1`、Bearer token、严格 Host、无宽松 CORS、
//! count/速率上限。纯 std 手写 HTTP/1.1，零新增依赖；安全逻辑为可独立单测
//! 的纯函数，并配真实 TCP 端到端测试覆盖所有拒绝路径。
//!
//! 服务以 SYSTEM 运行，本 API 只返回文件列表（`search`/`range`），不执行
//! 打开/剪贴板动作（DESIGN §12：动作由 UI 进程以当前用户身份执行）。
//! FILETIME 在 Rust 边界经 `ff_engine::timeconv` 转 epoch 毫秒后输出。

pub mod config;
pub mod fileconfig;
pub mod security;
pub mod server;
pub mod wire;

pub use config::{generate_token, HttpConfig, DEFAULT_COUNT, MAX_COUNT, MAX_OFFSET};
pub use fileconfig::{load_config, HttpFileConfig};
pub use security::{check_bearer, check_host, parse_count, parse_offset, parse_sort, RateLimiter};
pub use server::{bind, default_rate_limiter, serve_forever};
