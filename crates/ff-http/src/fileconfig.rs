//! 本地 HTTP API 的开关配置文件（DESIGN §10.2「默认关闭，设置里开启」）。
//!
//! 配置文件位于服务数据目录（`%ProgramData%\Sift\http.json`）：
//!
//! ```json
//! { "enabled": true, "port": 9090, "rate_per_sec": 60 }
//! ```
//!
//! - 文件不存在 / `enabled=false` → 不启动 HTTP（默认关闭）；
//! - `enabled=true` 且 `token` 为空 → 生成随机 token 并写回文件，供管理端读取；
//! - 端口必须非 0；非法配置返回错误，服务记录后不启动 HTTP。
//!
//! 配置目录由安装器/服务以受 ACL 保护的账户写入；token 只存本地文件、不放 URL。
//! 复用 `ff-http` 已有的 `serde_json`，零新增依赖。

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{generate_token, HttpConfig, DEFAULT_RATE_PER_SEC};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpFileConfig {
    pub enabled: bool,
    pub port: u16,
    /// 空串表示首次启用：加载时生成并写回。
    #[serde(default)]
    pub token: String,
    /// 可选：覆盖默认速率（每秒请求数）。
    #[serde(default)]
    pub rate_per_sec: Option<u32>,
}

/// 从文件加载 HTTP 配置。返回 `Ok(None)` 表示不启用（默认状态）。
pub fn load_config(path: &Path) -> io::Result<Option<HttpConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    let mut file: HttpFileConfig = serde_json::from_str(&text).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("http.json 解析失败: {e}"),
        )
    })?;
    if !file.enabled {
        return Ok(None);
    }
    if file.port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "http.json: port 不能为 0",
        ));
    }
    let mut cfg = HttpConfig::new(
        file.port,
        if file.token.is_empty() {
            generate_token()
        } else {
            file.token.clone()
        },
    )
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(rate) = file.rate_per_sec {
        if rate == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "http.json: rate_per_sec 不能为 0",
            ));
        }
        cfg.rate_per_sec = rate;
    } else {
        cfg.rate_per_sec = DEFAULT_RATE_PER_SEC;
    }
    // 首次启用：把生成的 token 写回，便于管理端读取。
    if file.token.is_empty() {
        file.token = cfg.token.clone();
        let pretty = serde_json::to_string_pretty(&file)
            .map_err(|e| io::Error::other(format!("序列化 http.json: {e}")))?;
        fs::write(path, pretty)?;
    }
    Ok(Some(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sift_http_{}_{}.json", name, std::process::id()))
    }

    #[test]
    fn missing_file_is_disabled() {
        let p = tmp_path("missing");
        let _ = fs::remove_file(&p);
        assert!(load_config(&p).unwrap().is_none());
    }

    #[test]
    fn enabled_false_is_disabled() {
        let p = tmp_path("off");
        fs::write(&p, r#"{"enabled":false,"port":9090}"#).unwrap();
        assert!(load_config(&p).unwrap().is_none());
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn enabled_true_generates_and_persists_token() {
        let p = tmp_path("on");
        fs::write(&p, r#"{"enabled":true,"port":9090}"#).unwrap();
        let cfg = load_config(&p).unwrap().unwrap();
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.token.len(), 64);
        // 第二次读取应复用同一 token（写回生效）。
        let cfg2 = load_config(&p).unwrap().unwrap();
        assert_eq!(cfg2.token, cfg.token);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn rate_override_and_validation() {
        let p = tmp_path("rate");
        fs::write(
            &p,
            r#"{"enabled":true,"port":9090,"token":"abc","rate_per_sec":5}"#,
        )
        .unwrap();
        assert_eq!(load_config(&p).unwrap().unwrap().rate_per_sec, 5);
        fs::write(&p, r#"{"enabled":true,"port":0}"#).unwrap();
        assert!(load_config(&p).is_err());
        let _ = fs::remove_file(&p);
    }
}
