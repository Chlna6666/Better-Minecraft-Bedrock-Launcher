use once_cell::sync::Lazy;
use reqwest::{Client, header::{HeaderMap, HeaderName, HeaderValue}, Url};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tauri::command;

/// 单例 Client（复用连接池，提高性能）
static CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(8)
        .build()
        .expect("reqwest client")
});

#[derive(Deserialize)]
pub struct FetchOptions {
    /// GET/POST/HEAD/... 默认 "GET"
    pub method: Option<String>,
    /// 自定义 headers, e.g. { "User-Agent": "My/1.0" }
    pub headers: Option<HashMap<String, String>>,
    /// 超时时间（毫秒）
    pub timeout_ms: Option<u64>,
    /// 是否跟随重定向（默认 true）
    pub allow_redirects: Option<bool>,
    /// 可选主机白名单，若提供且不包含请求 host 则拒绝（支持带端口或不带端口）
    pub allowed_hosts: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct FetchResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String, // 文本（JSON / TXT / HTML 等）
}

/// fetch_remote 提供给前端调用
#[command]
pub async fn fetch_remote(url: String, options: Option<FetchOptions>) -> Result<FetchResponse, String> {
    // 解析 URL
    let parsed = Url::parse(&url).map_err(|e| format!("invalid url: {}", e))?;
    // 检查白名单（如果提供）
    if let Some(opts) = &options {
        if let Some(allow) = &opts.allowed_hosts {
            let host = parsed.host_str().unwrap_or("");
            let port = parsed.port_or_known_default();
            let host_port = match port {
                Some(p) => format!("{}:{}", host, p),
                None => host.to_string(),
            };

            let allowed = allow.iter().any(|a| {
                // 支持只写 host 也能匹配（如 "data.mcappx.com" 或 "data.mcappx.com:443"）
                if a == &host_port { return true; }
                if a == host { return true; }
                false
            });

            if !allowed {
                return Err(format!("host not allowed: {}", host));
            }
        }
    }

    // 构造请求
    let method = options
        .as_ref()
        .and_then(|o| o.method.as_ref().map(|s| s.to_uppercase()))
        .unwrap_or_else(|| "GET".to_string());
    let mut req_builder = CLIENT.request(method.parse().unwrap_or(reqwest::Method::GET), parsed.clone());

    // 自定义 headers
    if let Some(opts) = &options {
        if let Some(hs) = &opts.headers {
            let mut header_map = HeaderMap::new();
            for (k, v) in hs {
                if let Ok(name) = HeaderName::from_bytes(k.as_bytes()) {
                    if let Ok(value) = HeaderValue::from_str(v) {
                        header_map.insert(name, value);
                    }
                }
            }
            if !header_map.is_empty() {
                req_builder = req_builder.headers(header_map);
            }
        }
    }

    // 超时（按次）
    if let Some(opts) = &options {
        if let Some(ms) = opts.timeout_ms {
            req_builder = req_builder.timeout(Duration::from_millis(ms));
        }
    }

    // 重定向策略（默认跟随）
    if let Some(opts) = &options {
        if let Some(allow) = opts.allow_redirects {
            if !allow {
                // 若不允许重定向，使用临时 client 禁止重定向
                let temp_client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|e| e.to_string())?;
                // 重新创建请求并发出
                let request = temp_client
                    .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET), parsed);
                let resp = request.send().await.map_err(|e| e.to_string())?;
                let status = resp.status().as_u16();
                let mut headers_out = HashMap::new();
                for (k, v) in resp.headers().iter() {
                    if let Ok(s) = v.to_str() {
                        headers_out.insert(k.to_string(), s.to_string());
                    }
                }

                // 统一以文本形式返回（适用于 JSON/TXT/HTML）
                let text = resp.text().await.map_err(|e| e.to_string())?;
                return Ok(FetchResponse { status, headers: headers_out, body: text });
            }
        }
    }

    // 默认分支（使用全局 CLIENT）
    let resp = req_builder.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();

    let mut headers_out = HashMap::new();
    for (k, v) in resp.headers().iter() {
        if let Ok(s) = v.to_str() {
            headers_out.insert(k.to_string(), s.to_string());
        }
    }

    // 统一以文本形式返回（适用于 JSON/TXT/HTML）
    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok(FetchResponse { status, headers: headers_out, body: text })
}
