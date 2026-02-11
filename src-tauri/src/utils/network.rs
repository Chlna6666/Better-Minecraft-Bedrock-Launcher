// src-tauri/src/utils/network.rs

use crate::http::proxy::get_client_for_proxy;
use futures_util::future::join_all;
use reqwest::Url;
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::command;

#[command]
pub async fn test_network_connectivity(url: String) -> Result<u64, String> {
    // 1. 获取带有代理配置的 Client
    // 直接复用 http/proxy.rs 中的逻辑，这样测试结果与实际应用下载时的连通性一致
    let client = get_client_for_proxy().map_err(|e| e.to_string())?;

    let start = Instant::now();

    // 2. 发送请求
    // 使用 HEAD 请求减少流量
    // 显式设置 timeout(Duration::from_secs(5))，覆盖 Client 默认可能较长的超时
    let _response = client
        .head(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    // 3. 计算耗时 (毫秒)
    let duration = start.elapsed().as_millis() as u64;

    Ok(duration)
}

#[derive(Debug, Clone, Serialize)]
pub struct CdnProbeResult {
    pub base: String,
    pub url: String,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CdnProbeResponse {
    pub recommended_base: Option<String>,
    pub results: Vec<CdnProbeResult>,
}

#[command]
pub async fn probe_gdk_asset_cdns(original_url: String, bases: Vec<String>) -> Result<CdnProbeResponse, String> {
    let original = Url::parse(&original_url).map_err(|e| format!("Invalid original_url: {}", e))?;

    let client = get_client_for_proxy().map_err(|e| e.to_string())?;

    let mut candidates = Vec::new();
    for base in bases {
        let mut base_url = Url::parse(&base).map_err(|e| format!("Invalid base url '{}': {}", base, e))?;
        base_url.set_path(original.path());
        base_url.set_query(original.query());
        base_url.set_fragment(None);
        candidates.push((base, base_url));
    }

    let futures = candidates.into_iter().map(|(base, url)| {
        let client = client.clone();
        async move {
            let start = Instant::now();
            let res = client
                .head(url.clone())
                .timeout(Duration::from_secs(5))
                .send()
                .await;

            match res {
                Ok(_) => CdnProbeResult {
                    base,
                    url: url.to_string(),
                    latency_ms: Some(start.elapsed().as_millis() as u64),
                    error: None,
                },
                Err(e) => CdnProbeResult {
                    base,
                    url: url.to_string(),
                    latency_ms: None,
                    error: Some(e.to_string()),
                },
            }
        }
    });

    let mut results = join_all(futures).await;

    results.sort_by(|a, b| match (a.latency_ms, b.latency_ms) {
        (Some(la), Some(lb)) => la.cmp(&lb),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.base.cmp(&b.base),
    });

    let recommended_base = results.iter().find_map(|r| r.latency_ms.map(|_| r.base.clone()));

    Ok(CdnProbeResponse {
        recommended_base,
        results,
    })
}
