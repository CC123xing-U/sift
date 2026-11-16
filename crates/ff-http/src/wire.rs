//! HTTP/1.1 请求解析与响应序列化（DESIGN §10.2）。
//!
//! 手写、有界、零依赖：请求头与查询串都有字节上限，防止慢速/超大请求打爆服务。
//! 只支持 `GET /search`；任何其它方法/路径由 `server` 拒绝。
//! 响应 DTO 按 §10.2 固定字段；`mtime` 在 Rust 侧经 `timeconv` 转 epoch 毫秒。

use std::collections::HashMap;

use ff_engine::{timeconv, ResultRow, SortSpec};
use serde::Serialize;

use crate::config::{self, HttpConfig};
use crate::security;

/// 请求头总字节上限（含请求行）。查询串走 URL，16 KiB 与 IPC 查询上限对齐。
pub const MAX_HEAD_BYTES: usize = 16 * 1024;
pub const MAX_QUERY_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub struct SearchQuery {
    pub text: String,
    pub offset: usize,
    pub count: usize,
    pub sort: SortSpec,
}

#[derive(Debug)]
pub enum ParseReject {
    BadRequest(&'static str),
    TooLarge,
}

/// 从请求行（如 `GET /search?q=ntdll&count=10 HTTP/1.1`）解析出路径与原始 query。
pub fn parse_request_line(line: &str) -> Result<(String, String), ParseReject> {
    let mut parts = line.split_whitespace();
    let method = parts
        .next()
        .ok_or(ParseReject::BadRequest("empty request line"))?;
    let target = parts
        .next()
        .ok_or(ParseReject::BadRequest("missing request target"))?;
    let version = parts
        .next()
        .ok_or(ParseReject::BadRequest("missing HTTP version"))?;
    if parts.next().is_some() {
        return Err(ParseReject::BadRequest("malformed request line"));
    }
    if method != "GET" {
        return Err(ParseReject::BadRequest("only GET is supported"));
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(ParseReject::BadRequest("unsupported HTTP version"));
    }
    if target.len() > MAX_QUERY_BYTES + 32 {
        return Err(ParseReject::TooLarge);
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    if !path.eq_ignore_ascii_case("/search") {
        return Err(ParseReject::BadRequest("unknown path (only /search)"));
    }
    if query.len() > MAX_QUERY_BYTES {
        return Err(ParseReject::TooLarge);
    }
    Ok((path.to_string(), query.to_string()))
}

/// 解析 query 串为 [`SearchQuery`]。重复键取最后一个；未知键忽略。
pub fn parse_search_query(raw: &str, cfg: &HttpConfig) -> Result<SearchQuery, ParseReject> {
    let mut params: HashMap<String, String> = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        let key = percent_decode(k)
            .map_err(|_| ParseReject::BadRequest("invalid percent escape in key"))?;
        let value = percent_decode(v)
            .map_err(|_| ParseReject::BadRequest("invalid percent escape in value"))?;
        params.insert(key, value);
    }
    let text = params.get("q").cloned().unwrap_or_default();
    if text.is_empty() {
        return Err(ParseReject::BadRequest("missing or empty q"));
    }
    if text.len() > MAX_QUERY_BYTES {
        return Err(ParseReject::TooLarge);
    }
    let offset = security::parse_offset(params.get("offset").map(String::as_str))
        .map_err(ParseReject::BadRequest)?;
    let count = security::parse_count(
        params.get("count").map(String::as_str),
        config::DEFAULT_COUNT,
        cfg.max_count,
    )
    .map_err(ParseReject::BadRequest)?;
    let sort = security::parse_sort(params.get("sort").map(String::as_str))
        .map_err(ParseReject::BadRequest)?;
    Ok(SearchQuery {
        text,
        offset,
        count,
        sort,
    })
}

/// 百分号解码失败（非法 `%` 转义或非 UTF-8 字节序列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PercentDecodeError;

impl std::fmt::Display for PercentDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid percent escape or non-UTF-8 byte in query")
    }
}
impl std::error::Error for PercentDecodeError {}

/// 百分号解码：`%XX` → 字节，`+` → 空格。非法转义返回 `Err`。
pub fn percent_decode(s: &str) -> Result<String, PercentDecodeError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(PercentDecodeError);
                }
                let hi = hex_val(bytes[i + 1]).ok_or(PercentDecodeError)?;
                let lo = hex_val(bytes[i + 2]).ok_or(PercentDecodeError)?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| PercentDecodeError)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// §10.2 规定的单条结果 JSON。
#[derive(Clone, Debug, Serialize)]
pub struct HttpRow {
    pub name: String,
    /// 完整路径（dir + name）。
    pub path: String,
    pub is_dir: bool,
    /// `null` 表示元数据未补齐（STALE_META）。
    pub size: Option<u64>,
    /// epoch 毫秒或 `null`；由 FILETIME 转换而来。
    pub mtime: Option<i64>,
}

/// `GET /search` 的成功响应体。
#[derive(Clone, Debug, Serialize)]
pub struct SearchResponse {
    pub total: usize,
    pub offset: usize,
    pub results: Vec<HttpRow>,
}

/// 结构化错误响应（供客户端区分 401/403/429/400 等）。
#[derive(Clone, Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub code: &'static str,
}

/// 把引擎结果行映射为 JS 边界 DTO：拼完整路径 + FILETIME 转 epoch 毫秒。
pub fn to_http_rows(rows: &[ResultRow]) -> Vec<HttpRow> {
    rows.iter()
        .map(|r| HttpRow {
            path: ff_engine::full_path(r),
            name: r.name.clone(),
            is_dir: r.is_dir,
            size: r.size,
            mtime: r.mtime.and_then(timeconv::filetime_to_epoch_ms),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn cfg() -> HttpConfig {
        HttpConfig::new(8080, "tok".into()).unwrap()
    }

    #[test]
    fn request_line_parsing() {
        let (p, q) = parse_request_line("GET /search?q=a%20b&count=5 HTTP/1.1").unwrap();
        assert_eq!(p, "/search");
        assert_eq!(q, "q=a%20b&count=5");
        assert!(parse_request_line("POST /search HTTP/1.1").is_err());
        assert!(parse_request_line("GET /status HTTP/1.1").is_err());
        assert!(parse_request_line("GET /search HTTP/2.0").is_err());
    }

    #[test]
    fn percent_decode_plus_and_hex() {
        assert_eq!(percent_decode("ntdll").unwrap(), "ntdll");
        assert_eq!(percent_decode("a+b").unwrap(), "a b");
        assert_eq!(percent_decode("%E4%B8%AD%E6%96%87").unwrap(), "中文");
        assert!(percent_decode("%ZZ").is_err());
        assert!(percent_decode("%A").is_err());
    }

    #[test]
    fn query_parses_all_fields() {
        let q = parse_search_query("q=ntdll&offset=20&count=30&sort=-size", &cfg()).unwrap();
        assert_eq!(q.text, "ntdll");
        assert_eq!(q.offset, 20);
        assert_eq!(q.count, 30);
        assert_eq!(q.sort.key, ff_engine::SortKey::Size);
        assert!(!q.sort.descending);
    }

    #[test]
    fn query_rejects_missing_q_and_over_limit_count() {
        assert!(parse_search_query("count=5", &cfg()).is_err());
        assert!(parse_search_query("q=x&count=100000", &cfg()).is_err());
        assert!(parse_search_query("q=x&offset=-1", &cfg()).is_err());
    }

    #[test]
    fn rows_convert_path_and_mtime() {
        let rows = vec![ResultRow {
            name: "a.txt".into(),
            dir: r"D:\Work".into(),
            is_dir: false,
            size: Some(10),
            mtime: Some(134_116_992_000_000_000), // 2026-01-01
            name_matches: Vec::<Range<u32>>::new(),
        }];
        let http = to_http_rows(&rows);
        assert_eq!(http[0].path, r"D:\Work\a.txt");
        assert_eq!(http[0].mtime, Some(1_767_225_600_000));
        assert_eq!(http[0].size, Some(10));
    }

    #[test]
    fn error_body_serializes() {
        let json = serde_json::to_string(&ErrorBody {
            error: "bad token".into(),
            code: "unauthorized",
        })
        .unwrap();
        assert!(json.contains("\"code\":\"unauthorized\""));
    }
}
