// src/http/proxy.rs
use crate::config::config::{ProxyConfig, ProxyType, read_config};
use crate::http::request::{DEFAULT_USER_AGENT, GLOBAL_CLIENT};
use crate::result::CoreError;
use once_cell::sync::Lazy;
use reqwest::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, HeaderMap, HeaderValue, PRAGMA,
};
use reqwest::{Client, Proxy, blocking::Client as BlockingClient};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{debug, error, warn};

/// 全局 client 缓存（key -> Client）
/// 优化：使用 LRU 策略管理客户端缓存，避免无限增长
static CLIENT_CACHE: Lazy<Mutex<HashMap<String, Client>>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // 明确创建 no-proxy client（不受 HTTP(S)_PROXY 等环境变量影响）
    let no_proxy_client = build_optimized_client(|builder| builder.no_proxy());

    m.insert("no_proxy".to_string(), no_proxy_client);
    Mutex::new(m)
});

static DOWNLOAD_CLIENT_CACHE: Lazy<Mutex<HashMap<String, Client>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static BLOCKING_CLIENT_CACHE: Lazy<Mutex<HashMap<String, BlockingClient>>> = Lazy::new(|| {
    let mut m = HashMap::new();
    let no_proxy_client = build_optimized_blocking_client(|builder| builder.no_proxy());
    m.insert("no_proxy".to_string(), no_proxy_client);
    Mutex::new(m)
});

const DOWNLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 构建优化的 reqwest Client，具有更好的连接池和复用性能
fn build_optimized_client(
    configure: impl FnOnce(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> Client {
    let builder = reqwest::Client::builder()
        // 启用压缩支持，减少传输数据量
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .zstd(true)
        // 连接超时设置
        .connect_timeout(Duration::from_secs(10))
        // 连接池优化：增加每个主机的最大空闲连接数
        .pool_max_idle_per_host(16)
        // 连接池空闲超时：保持连接更长时间以便复用
        .pool_idle_timeout(Duration::from_secs(90))
        // TCP keepalive 保持连接活跃
        .tcp_keepalive(Duration::from_secs(60))
        // 启用 HTTP/2 支持（如果服务器支持）
        .http2_adaptive_window(true)
        .http2_keep_alive_interval(Duration::from_secs(30))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_keep_alive_while_idle(true)
        // 设置 User-Agent
        .user_agent(DEFAULT_USER_AGENT.as_str())
        // 超时配置
        .timeout(Duration::from_secs(60));

    configure(builder).build().unwrap_or_else(|e| {
        error!("Failed to build optimized client: {}", e);
        GLOBAL_CLIENT.clone()
    })
}

fn build_optimized_blocking_client(
    configure: impl FnOnce(reqwest::blocking::ClientBuilder) -> reqwest::blocking::ClientBuilder,
) -> BlockingClient {
    let builder = reqwest::blocking::Client::builder()
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .zstd(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(16)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .http2_adaptive_window(true)
        .user_agent(DEFAULT_USER_AGENT.as_str())
        .timeout(Duration::from_secs(60));

    configure(builder).build().unwrap_or_else(|e| {
        error!("Failed to build optimized blocking client: {}", e);
        get_no_proxy_blocking_client()
    })
}

fn normalize_proxy_url_for_key(s: &str) -> String {
    s.trim().to_string()
}

/// 如果用户写了 `127.0.0.1:7899` 等无 scheme 的地址，为不同类型自动补 scheme。
/// 返回 (normalized_for_reqwest, normalized_for_cache_key)
fn normalize_and_prepare_url(kind: &ProxyType, raw: &str) -> (String, String) {
    let t = raw.trim();
    if t.is_empty() {
        return (String::new(), String::new());
    }

    // Already has scheme?
    if t.contains("://") {
        return (t.to_string(), t.to_string());
    }

    match kind {
        ProxyType::Http => {
            let with = format!("http://{}", t);
            (with.clone(), with)
        }
        ProxyType::Socks5 => {
            let with = format!("socks5://{}", t);
            (with.clone(), with)
        }
        _ => (t.to_string(), t.to_string()),
    }
}

fn client_key_from_config(cfg: &ProxyConfig) -> String {
    match cfg.proxy_type {
        ProxyType::None => "no_proxy".to_string(),
        ProxyType::System => "system_proxy".to_string(),
        ProxyType::Http => {
            let (_, key_url) = normalize_and_prepare_url(&ProxyType::Http, &cfg.http_proxy_url);
            if key_url.is_empty() {
                "no_proxy".to_string()
            } else {
                format!("http:{}", normalize_proxy_url_for_key(&key_url))
            }
        }
        ProxyType::Socks5 => {
            let (_, key_url) = normalize_and_prepare_url(&ProxyType::Socks5, &cfg.socks_proxy_url);
            if key_url.is_empty() {
                "no_proxy".to_string()
            } else {
                format!("socks:{}", normalize_proxy_url_for_key(&key_url))
            }
        }
    }
}

pub fn download_request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers
}

pub fn apply_download_request_headers(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder.headers(download_request_headers())
}

pub fn validate_download_response_headers(
    url: &str,
    response: &reqwest::Response,
) -> Result<(), CoreError> {
    validate_download_response_header_map(url, response.headers())
}

pub fn validate_download_response_header_map(
    url: &str,
    headers: &HeaderMap,
) -> Result<(), CoreError> {
    let Some(value) = headers.get(CONTENT_ENCODING) else {
        return Ok(());
    };

    let encoding = value.to_str().map_err(|error| {
        CoreError::Other(format!(
            "download response has invalid Content-Encoding for {url}: {error}"
        ))
    })?;

    let transformed = encoding
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .any(|value| !value.eq_ignore_ascii_case("identity"));

    if transformed {
        return Err(CoreError::Other(format!(
            "download response was transformed by Content-Encoding={encoding} for {url}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{download_request_headers, validate_download_response_header_map};
    use reqwest::header::{
        ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, HeaderMap, HeaderValue, PRAGMA,
    };

    #[test]
    fn download_request_headers_mark_binary_downloads_as_identity() {
        let headers = download_request_headers();

        assert_eq!(
            headers
                .get(ACCEPT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            Some("identity")
        );
        assert_eq!(
            headers
                .get(CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache, no-transform")
        );
        assert_eq!(
            headers.get(PRAGMA).and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
    }

    #[test]
    fn validate_download_response_header_map_accepts_identity_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));

        validate_download_response_header_map("https://example.com/package.appx", &headers)
            .expect("identity encoding should be accepted");
    }

    #[test]
    fn validate_download_response_header_map_rejects_transformed_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let error =
            validate_download_response_header_map("https://example.com/package.appx", &headers)
                .expect_err("transformed encoding should be rejected");

        assert!(error.to_string().contains("Content-Encoding=gzip"));
    }
}

/// 从配置文件读取当前 proxy 配置：若读取失败则返回默认 ProxyConfig（即 None）
fn load_current_proxy_config() -> ProxyConfig {
    match read_config() {
        Ok(conf) => conf.launcher.download.proxy,
        Err(err) => {
            error!(
                "Failed to read config when building http client for proxy: {:?}",
                err
            );
            ProxyConfig::default()
        }
    }
}

/// 清空缓存（并重建 no_proxy 项），可在配置变更时调用
pub fn clear_client_cache() {
    retain_client_cache_entries("CLIENT_CACHE", &CLIENT_CACHE, |key| key == "no_proxy");
    retain_client_cache_entries("DOWNLOAD_CLIENT_CACHE", &DOWNLOAD_CLIENT_CACHE, |key| {
        key == "download:no_proxy"
    });
    retain_client_cache_entries("BLOCKING_CLIENT_CACHE", &BLOCKING_CLIENT_CACHE, |key| {
        key == "no_proxy"
    });
    prewarm_current_proxy_clients_in_background();
}

/// 强制重建 no_proxy client 并放回缓存（如果你希望在运行时刷新）
pub fn rebuild_no_proxy_client_in_cache() {
    let new_no_proxy = build_optimized_client(|builder| builder.no_proxy());
    if let Ok(mut cache) = CLIENT_CACHE.lock() {
        cache.insert("no_proxy".to_string(), new_no_proxy);
        debug!("Rebuilt no_proxy client in cache");
    } else {
        error!("Failed to lock CLIENT_CACHE to rebuild no_proxy");
    }
}

/// 返回不使用代理（且不读环境变量 proxy）的 client。
pub fn get_no_proxy_client() -> Client {
    if let Ok(cache) = CLIENT_CACHE.lock() {
        if let Some(c) = cache.get("no_proxy") {
            return c.clone();
        }
    }

    build_optimized_client(|builder| builder.no_proxy())
}

/// 构造一个 no-proxy client，并将指定 host 绑定到固定 IP（常用于 Cloudflare 优选 IP）。
pub fn build_no_proxy_client_with_resolve(host: &str, addr: SocketAddr) -> Client {
    build_optimized_client(|builder| builder.resolve(host, addr).no_proxy())
}

/// 返回当前配置对应的 reqwest::Client（内部读取配置）
/// 调用处请不要使用 GLOBAL_CLIENT，改为使用本函数返回的 client。
pub fn get_client_for_proxy() -> Result<Client, CoreError> {
    let cfg = load_current_proxy_config();
    let key = client_key_from_config(&cfg);

    // 1) 尝试从缓存读取
    {
        let cache = CLIENT_CACHE
            .lock()
            .map_err(|e| CoreError::Config(format!("client cache lock poisoned: {}", e)))?;
        if let Some(client) = cache.get(&key) {
            tracing::trace!("复用已缓存 client key={}", key);
            return Ok(client.clone());
        }
    }

    // 2) 缓存未命中，构造新的 client（基于常用参数）
    let client = {
        let mut builder = reqwest::Client::builder()
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .zstd(true)
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(16)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .http2_adaptive_window(true)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .http2_keep_alive_while_idle(true)
            .user_agent(DEFAULT_USER_AGENT.as_str())
            .timeout(Duration::from_secs(60));

        builder = match cfg.proxy_type {
            ProxyType::None => {
                debug!("Building client: None -> no_proxy()");
                builder.no_proxy()
            }
            ProxyType::System => {
                debug!("Building client: System -> using environment proxy settings");
                builder
            }
            ProxyType::Http => {
                let (normalized, _) =
                    normalize_and_prepare_url(&ProxyType::Http, &cfg.http_proxy_url);
                if normalized.is_empty() {
                    debug!("HTTP proxy selected but http_proxy_url empty -> fallback to no_proxy");
                    builder.no_proxy()
                } else {
                    debug!("Building client with HTTP proxy: {}", normalized);
                    let proxy =
                        Proxy::all(&normalized).map_err(|e| CoreError::Config(e.to_string()))?;
                    builder.proxy(proxy)
                }
            }
            ProxyType::Socks5 => {
                let (normalized, _) =
                    normalize_and_prepare_url(&ProxyType::Socks5, &cfg.socks_proxy_url);
                if normalized.is_empty() {
                    debug!(
                        "SOCKS5 proxy selected but socks_proxy_url empty -> fallback to no_proxy"
                    );
                    builder.no_proxy()
                } else {
                    debug!("Building client with SOCKS5 proxy: {}", normalized);
                    let proxy =
                        Proxy::all(&normalized).map_err(|e| CoreError::Config(e.to_string()))?;
                    builder.proxy(proxy)
                }
            }
        };

        builder
            .build()
            .map_err(|e| CoreError::Config(e.to_string()))?
    };

    // 3) 写回缓存
    {
        let mut cache = CLIENT_CACHE
            .lock()
            .map_err(|e| CoreError::Config(format!("client cache lock poisoned: {}", e)))?;
        cache.insert(key.clone(), client.clone());
    }

    debug!("创建并缓存 client key={}", key);
    Ok(client)
}

/// 返回当前代理配置对应的下载专用 client。
///
/// 普通请求 client 有 60 秒总超时，下载 1GB+ 大文件时会把一个仍在传输的请求
/// 中断。下载 client 只保留连接超时，并把总请求超时放宽到 6 小时。
pub fn get_download_client_for_proxy() -> Result<Client, CoreError> {
    let cfg = load_current_proxy_config();
    let key = format!("download:{}", client_key_from_config(&cfg));

    {
        let cache = DOWNLOAD_CLIENT_CACHE.lock().map_err(|e| {
            CoreError::Config(format!("download client cache lock poisoned: {}", e))
        })?;
        if let Some(client) = cache.get(&key) {
            tracing::trace!("复用已缓存 download client key={}", key);
            return Ok(client.clone());
        }
    }

    let mut builder = reqwest::Client::builder()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .http1_only()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .pool_max_idle_per_host(16)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .user_agent(DEFAULT_USER_AGENT.as_str())
        .timeout(DOWNLOAD_REQUEST_TIMEOUT);

    builder = match cfg.proxy_type {
        ProxyType::None => builder.no_proxy(),
        ProxyType::System => builder,
        ProxyType::Http => {
            let (normalized, _) = normalize_and_prepare_url(&ProxyType::Http, &cfg.http_proxy_url);
            if normalized.is_empty() {
                builder.no_proxy()
            } else {
                let proxy =
                    Proxy::all(&normalized).map_err(|e| CoreError::Config(e.to_string()))?;
                builder.proxy(proxy)
            }
        }
        ProxyType::Socks5 => {
            let (normalized, _) =
                normalize_and_prepare_url(&ProxyType::Socks5, &cfg.socks_proxy_url);
            if normalized.is_empty() {
                builder.no_proxy()
            } else {
                let proxy =
                    Proxy::all(&normalized).map_err(|e| CoreError::Config(e.to_string()))?;
                builder.proxy(proxy)
            }
        }
    };

    let client = builder
        .build()
        .map_err(|e| CoreError::Config(e.to_string()))?;

    {
        let mut cache = DOWNLOAD_CLIENT_CACHE.lock().map_err(|e| {
            CoreError::Config(format!("download client cache lock poisoned: {}", e))
        })?;
        cache.insert(key.clone(), client.clone());
    }

    debug!("创建并缓存 download client key={}", key);
    Ok(client)
}

/// 构造一个与当前代理配置一致的 blocking client。
pub fn get_blocking_client_for_proxy() -> Result<BlockingClient, CoreError> {
    let cfg = load_current_proxy_config();
    let key = client_key_from_config(&cfg);
    {
        let cache = BLOCKING_CLIENT_CACHE.lock().map_err(|e| {
            CoreError::Config(format!("blocking client cache lock poisoned: {}", e))
        })?;
        if let Some(client) = cache.get(&key) {
            tracing::trace!("复用已缓存 blocking client key={}", key);
            return Ok(client.clone());
        }
    }

    let mut builder = reqwest::blocking::Client::builder()
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .zstd(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(16)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .http2_adaptive_window(true)
        .user_agent(DEFAULT_USER_AGENT.as_str())
        .timeout(Duration::from_secs(60));

    builder = match cfg.proxy_type {
        ProxyType::None => builder.no_proxy(),
        ProxyType::System => builder,
        ProxyType::Http => {
            let (normalized, _) = normalize_and_prepare_url(&ProxyType::Http, &cfg.http_proxy_url);
            if normalized.is_empty() {
                builder.no_proxy()
            } else {
                let proxy =
                    Proxy::all(&normalized).map_err(|e| CoreError::Config(e.to_string()))?;
                builder.proxy(proxy)
            }
        }
        ProxyType::Socks5 => {
            let (normalized, _) =
                normalize_and_prepare_url(&ProxyType::Socks5, &cfg.socks_proxy_url);
            if normalized.is_empty() {
                builder.no_proxy()
            } else {
                let proxy =
                    Proxy::all(&normalized).map_err(|e| CoreError::Config(e.to_string()))?;
                builder.proxy(proxy)
            }
        }
    };

    let client = builder
        .build()
        .map_err(|e: reqwest::Error| CoreError::Config(e.to_string()))?;

    {
        let mut cache = BLOCKING_CLIENT_CACHE.lock().map_err(|e| {
            CoreError::Config(format!("blocking client cache lock poisoned: {}", e))
        })?;
        cache.insert(key.clone(), client.clone());
    }

    debug!("创建并缓存 blocking client key={}", key);
    Ok(client)
}

/// 构建当前代理配置下的常用 client，但不发起任何网络请求。
pub fn prewarm_current_proxy_clients() -> Result<(), CoreError> {
    get_client_for_proxy()?;
    get_download_client_for_proxy()?;
    get_blocking_client_for_proxy()?;
    Ok(())
}

pub fn prewarm_current_proxy_clients_in_background() {
    fn prewarm() {
        match prewarm_current_proxy_clients() {
            Ok(()) => debug!("HTTP proxy clients prewarmed"),
            Err(error) => warn!("HTTP proxy client prewarm failed: {error}"),
        }
    }

    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            drop(handle.spawn_blocking(prewarm));
        }
        Err(_) => {
            if let Err(error) = std::thread::Builder::new()
                .name("bmcbl-http-client-prewarm".to_string())
                .spawn(prewarm)
            {
                error!("Failed to spawn HTTP proxy client prewarm thread: {error}");
            }
        }
    }
}

fn retain_client_cache_entries<T>(
    cache_name: &str,
    cache: &Lazy<Mutex<HashMap<String, T>>>,
    keep_entry: impl Fn(&str) -> bool,
) {
    let Some(cache) = Lazy::get(cache) else {
        debug!("{cache_name} not initialized; skipping clear");
        return;
    };

    match cache.lock() {
        Ok(mut cache) => {
            let previous_len = cache.len();
            cache.retain(|key, _| keep_entry(key));
            debug!(
                "{cache_name} cleared stale proxy clients: before={} after={}",
                previous_len,
                cache.len()
            );
        }
        Err(error) => error!("Failed to lock {cache_name} to clear it: {error}"),
    }
}

fn get_no_proxy_blocking_client() -> BlockingClient {
    if let Ok(cache) = BLOCKING_CLIENT_CACHE.lock()
        && let Some(client) = cache.get("no_proxy")
    {
        return client.clone();
    }

    build_optimized_blocking_client(|builder| builder.no_proxy())
}

/// 异步调试函数：使用当前配置的 client 访问 httpbin.org/ip，便于判断是否走代理
/// 在 tokio/async runtime 中调用：例如在初始化流程或 debug 命令中调用。
pub async fn debug_check_proxy() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("debug_check_proxy: start");
    let client =
        get_client_for_proxy().map_err(|e| format!("get_client_for_proxy err: {:?}", e))?;
    debug!("debug_check_proxy: got client, sending request to httpbin.org/ip ...");

    let resp = client
        .get("https://httpbin.org/ip")
        .send()
        .await
        .map_err(|e| format!("request err: {:?}", e))?;
    let body = resp
        .text()
        .await
        .map_err(|e| format!("read body err: {:?}", e))?;
    debug!("httpbin.org/ip => {}", body);
    Ok(())
}
