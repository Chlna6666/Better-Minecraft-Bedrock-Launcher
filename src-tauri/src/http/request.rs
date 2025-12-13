// src/http/request.rs
use once_cell::sync::Lazy;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, Method, Url,
};
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

pub(crate) static GLOBAL_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(8)
        .build()
        .expect("reqwest client")
});

/// 构造并发送请求的选项（与 UI 兼容）
pub struct RequestOptions<'a> {
    pub method: &'a str,
    pub headers: Option<&'a HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub allow_redirects: Option<bool>,
    // 这里不直接带 allowed_hosts（由外层提前校验 URL 白名单）
}

fn build_header_map(hs: Option<&HashMap<String, String>>) -> HeaderMap {
    let mut header_map = HeaderMap::new();
    if let Some(hs) = hs {
        for (k, v) in hs {
            if let Ok(name) = HeaderName::from_bytes(k.as_bytes()) {
                if let Ok(value) = HeaderValue::from_str(v) {
                    header_map.insert(name, value);
                }
            }
        }
    }
    header_map
}

/// 发送请求的通用函数。
/// - `client` 为预构建好的 reqwest::Client（例如由 build_client_with_proxy 构建）
/// - 当 `allow_redirects == Some(false)` 时，会临时使用 new client 禁止重定向（因为 redirect 策略是 client 级别）
pub async fn send_request_with_options(
    client: &Client,
    url: &Url,
    opts: &RequestOptions<'_>,
) -> Result<reqwest::Response, String> {
    // 解析 method
    let method = Method::from_bytes(opts.method.as_bytes()).unwrap_or(Method::GET);

    // 如果用户显式要求禁止重定向，我们创建一个临时 client（基于原 client 的常用配置无法直接复制），
    // 这里做一个简单实现：仅在需要禁止重定向时构建一个短生命周期 client，
    // 否则使用传入的 client 复用连接池。
    let allow_redirects = opts.allow_redirects.unwrap_or(true);
    if !allow_redirects {
        debug!("创建临时 client 禁止重定向");
        let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());

        // 如果 timeout_ms 提供，我们可以在请求上设置，但部分 reqwest 版本对 client.timeout 与 request.timeout 行为不同，
        // 这里优先在 request 上设置 timeout（下面会设置 request.timeout）
        let temp_client = builder
            .build()
            .map_err(|e| format!("构建临时 client 失败: {}", e))?;

        let mut rb = temp_client.request(method, url.clone());
        let header_map = build_header_map(opts.headers);
        if !header_map.is_empty() {
            rb = rb.headers(header_map);
        }
        if let Some(ms) = opts.timeout_ms {
            rb = rb.timeout(Duration::from_millis(ms));
        }

        return rb.send().await.map_err(|e| e.to_string());
    }

    // 默认使用传入 client
    let mut rb = client.request(method, url.clone());
    let header_map = build_header_map(opts.headers);
    if !header_map.is_empty() {
        rb = rb.headers(header_map);
    }
    if let Some(ms) = opts.timeout_ms {
        rb = rb.timeout(Duration::from_millis(ms));
    }

    rb.send().await.map_err(|e| e.to_string())
}
