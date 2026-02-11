// src/http/proxy.rs
use crate::config::config::{read_config, ProxyConfig, ProxyType};
use crate::http::request::{DEFAULT_USER_AGENT, GLOBAL_CLIENT};
use crate::result::CoreError;
use once_cell::sync::Lazy;
use reqwest::{Client, Proxy};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{debug, error};

/// 全局 client 缓存（key -> Client）
static CLIENT_CACHE: Lazy<Mutex<HashMap<String, Client>>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // 明确创建 no-proxy client（不受 HTTP(S)_PROXY 等环境变量影响）
    let no_proxy_client = Client::builder()
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(8)
        .user_agent(DEFAULT_USER_AGENT.as_str())
        .no_proxy()
        .build()
        .unwrap_or_else(|e| {
            error!("Failed to build no-proxy client during init: {}", e);
            GLOBAL_CLIENT.clone()
        });

    m.insert("no_proxy".to_string(), no_proxy_client);
    Mutex::new(m)
});

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
    if let Ok(mut cache) = CLIENT_CACHE.lock() {
        cache.clear();

        let fallback = Client::builder()
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(8)
            .user_agent(DEFAULT_USER_AGENT.as_str())
            .no_proxy()
            .build()
            .unwrap_or_else(|e| {
                error!("Failed to rebuild fallback no-proxy client: {}", e);
                GLOBAL_CLIENT.clone()
            });
        cache.insert("no_proxy".to_string(), fallback);
        debug!("CLIENT_CACHE cleared and no_proxy rebuilt");
    } else {
        error!("Failed to lock CLIENT_CACHE to clear it");
    }
}

/// 强制重建 no_proxy client 并放回缓存（如果你希望在运行时刷新）
pub fn rebuild_no_proxy_client_in_cache() {
    let new_no_proxy = Client::builder()
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(8)
        .user_agent(DEFAULT_USER_AGENT.as_str())
        .no_proxy()
        .build()
        .unwrap_or_else(|e| {
            error!("Failed to build no-proxy client: {}", e);
            GLOBAL_CLIENT.clone()
        });
    if let Ok(mut cache) = CLIENT_CACHE.lock() {
        cache.insert("no_proxy".to_string(), new_no_proxy);
        debug!("Rebuilt no_proxy client in cache");
    } else {
        error!("Failed to lock CLIENT_CACHE to rebuild no_proxy");
    }
}

/// 返回当前配置对应的 reqwest::Client（内部读取配置）
/// 调用处请不要使用 GLOBAL_CLIENT，改为使用本函数返回的 client。
pub fn get_client_for_proxy() -> Result<Client, CoreError> {
    let cfg = load_current_proxy_config();
    debug!(
        "get_client_for_proxy: proxy_type={:?}, http='{}', socks='{}'",
        cfg.proxy_type, cfg.http_proxy_url, cfg.socks_proxy_url
    );

    let key = client_key_from_config(&cfg);
    debug!("Computed client cache key = {}", key);

    // 1) 尝试从缓存读取
    {
        let cache = CLIENT_CACHE
            .lock()
            .map_err(|e| CoreError::Config(format!("client cache lock poisoned: {}", e)))?;
        if let Some(client) = cache.get(&key) {
            debug!("复用已缓存 client key={}", key);
            return Ok(client.clone());
        }
    }

    // 2) 缓存未命中，构造新的 client（基于常用参数）
    let mut builder = Client::builder()
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(8)
        .user_agent(DEFAULT_USER_AGENT.as_str());

    builder = match cfg.proxy_type {
        ProxyType::None => {
            debug!("Building client: None -> no_proxy()");
            builder.no_proxy()
        }
        ProxyType::System => {
            debug!("Building client: System -> using environment proxy settings");
            builder // 使用环境变量 HTTP_PROXY/HTTPS_PROXY 等
        }
        ProxyType::Http => {
            let (normalized, _) = normalize_and_prepare_url(&ProxyType::Http, &cfg.http_proxy_url);
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
                debug!("SOCKS5 proxy selected but socks_proxy_url empty -> fallback to no_proxy");
                builder.no_proxy()
            } else {
                debug!("Building client with SOCKS5 proxy: {}", normalized);
                let proxy =
                    Proxy::all(&normalized).map_err(|e| CoreError::Config(e.to_string()))?;
                builder.proxy(proxy)
            }
        }
    };

    let client = builder
        .build()
        .map_err(|e| CoreError::Config(e.to_string()))?;

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
