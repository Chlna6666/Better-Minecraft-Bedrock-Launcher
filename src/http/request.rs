// src/http/request.rs
use bytes::Bytes;
use once_cell::sync::Lazy;
use reqwest::blocking::Client as BlockingClient;
use reqwest::{
    Client, Method, Url,
    header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT},
};
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

pub(crate) static DEFAULT_USER_AGENT: Lazy<String> =
    Lazy::new(|| format!("BMCBL/{}", crate::utils::app_info::get_version()));

/// 全局默认客户端，使用优化的连接池配置
pub(crate) static GLOBAL_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        // 启用压缩支持
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .zstd(true)
        // 连接超时
        .connect_timeout(Duration::from_secs(10))
        // 连接池优化：增加空闲连接数，提高复用率
        .pool_max_idle_per_host(16)
        // 连接池空闲超时
        .pool_idle_timeout(Duration::from_secs(90))
        // TCP keepalive
        .tcp_keepalive(Duration::from_secs(60))
        // HTTP/2 优化
        .http2_adaptive_window(true)
        .http2_keep_alive_interval(Duration::from_secs(30))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_keep_alive_while_idle(true)
        // User-Agent
        .user_agent(DEFAULT_USER_AGENT.as_str())
        // 总超时
        .timeout(Duration::from_secs(60))
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

pub fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
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
    header_map.insert(
        USER_AGENT,
        HeaderValue::from_str(DEFAULT_USER_AGENT.as_str())
            .unwrap_or_else(|_| HeaderValue::from_static("BMCBL/unknown")),
    );
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
        let builder = reqwest::Client::builder()
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .zstd(true)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(DEFAULT_USER_AGENT.as_str());

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

pub fn send_blocking_request_with_options(
    client: &BlockingClient,
    url: &Url,
    opts: &RequestOptions<'_>,
) -> Result<reqwest::blocking::Response, String> {
    let method = Method::from_bytes(opts.method.as_bytes()).unwrap_or(Method::GET);
    let allow_redirects = opts.allow_redirects.unwrap_or(true);
    if !allow_redirects {
        let temp_client = reqwest::blocking::Client::builder()
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .zstd(true)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(DEFAULT_USER_AGENT.as_str())
            .build()
            .map_err(|e| format!("构建临时 blocking client 失败: {}", e))?;

        let mut request = temp_client.request(method, url.clone());
        let header_map = build_header_map(opts.headers);
        if !header_map.is_empty() {
            request = request.headers(header_map);
        }
        if let Some(ms) = opts.timeout_ms {
            request = request.timeout(Duration::from_millis(ms));
        }
        return request.send().map_err(|e| e.to_string());
    }

    let mut request = client.request(method, url.clone());
    let header_map = build_header_map(opts.headers);
    if !header_map.is_empty() {
        request = request.headers(header_map);
    }
    if let Some(ms) = opts.timeout_ms {
        request = request.timeout(Duration::from_millis(ms));
    }

    request.send().map_err(|e| e.to_string())
}

pub async fn fetch_bytes_with_retry(
    client: &Client,
    url: &Url,
    opts: &RequestOptions<'_>,
    attempts: usize,
    retry_delay: Duration,
) -> Result<Bytes, String> {
    let mut last_error = None;

    for attempt in 0..attempts {
        match send_request_with_options(client, url, opts).await {
            Ok(response) => {
                let status = response.status();
                match response.bytes().await {
                    Ok(bytes) if status.is_success() => return Ok(bytes),
                    Ok(bytes) => {
                        let body = String::from_utf8_lossy(&bytes).into_owned();
                        let error = format!("bad status loading {url}: {status} {body}");
                        last_error = Some(error.clone());
                        if !should_retry_status(status) || attempt + 1 >= attempts {
                            return Err(error);
                        }
                    }
                    Err(error) => {
                        let error = format!("reading response body failed for {url}: {error}");
                        last_error = Some(error.clone());
                        if attempt + 1 >= attempts {
                            return Err(error);
                        }
                    }
                }
            }
            Err(error) => {
                last_error = Some(error.clone());
                if attempt + 1 >= attempts {
                    return Err(error);
                }
            }
        }

        tokio::time::sleep(retry_delay).await;
    }

    Err(last_error.unwrap_or_else(|| format!("request failed for {url}")))
}

pub fn fetch_bytes_with_retry_blocking(
    client: &BlockingClient,
    url: &Url,
    opts: &RequestOptions<'_>,
    attempts: usize,
    retry_delay: Duration,
) -> Result<Bytes, String> {
    let mut last_error = None;

    for attempt in 0..attempts {
        match send_blocking_request_with_options(client, url, opts) {
            Ok(response) => {
                let status = response.status();
                match response.bytes() {
                    Ok(bytes) if status.is_success() => return Ok(bytes),
                    Ok(bytes) => {
                        let body = String::from_utf8_lossy(&bytes).into_owned();
                        let error = format!("bad status loading {url}: {status} {body}");
                        last_error = Some(error.clone());
                        if !should_retry_status(status) || attempt + 1 >= attempts {
                            return Err(error);
                        }
                    }
                    Err(error) => {
                        let error = format!("reading response body failed for {url}: {error}");
                        last_error = Some(error.clone());
                        if attempt + 1 >= attempts {
                            return Err(error);
                        }
                    }
                }
            }
            Err(error) => {
                last_error = Some(error.clone());
                if attempt + 1 >= attempts {
                    return Err(error);
                }
            }
        }

        std::thread::sleep(retry_delay);
    }

    Err(last_error.unwrap_or_else(|| format!("request failed for {url}")))
}
