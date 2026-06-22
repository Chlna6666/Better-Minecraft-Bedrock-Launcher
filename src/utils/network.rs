use crate::http::proxy::get_blocking_client_for_proxy;
use crate::http::proxy::get_client_for_proxy;
use futures_util::future::join_all;
use reqwest::Url;
use serde::Serialize;
use std::time::{Duration, Instant};

pub fn test_network_connectivity_blocking(url: String) -> Result<u64, String> {
    let client = get_blocking_client_for_proxy().map_err(|e| e.to_string())?;

    let start = Instant::now();

    let _response = client
        .head(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .map_err(|e| e.to_string())?;

    Ok(start.elapsed().as_millis() as u64)
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

pub async fn probe_gdk_asset_cdns(
    original_url: String,
    bases: Vec<String>,
) -> Result<CdnProbeResponse, String> {
    let original = Url::parse(&original_url).map_err(|e| format!("Invalid original_url: {}", e))?;

    let client = get_client_for_proxy().map_err(|e| e.to_string())?;

    let mut candidates = Vec::new();
    for base in bases {
        let mut base_url =
            Url::parse(&base).map_err(|e| format!("Invalid base url '{}': {}", base, e))?;
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

    let recommended_base = results
        .iter()
        .find_map(|r| r.latency_ms.map(|_| r.base.clone()));

    Ok(CdnProbeResponse {
        recommended_base,
        results,
    })
}

pub fn probe_gdk_asset_cdns_blocking(
    original_url: String,
    bases: Vec<String>,
) -> Result<CdnProbeResponse, String> {
    let original = Url::parse(&original_url).map_err(|e| format!("Invalid original_url: {}", e))?;
    let client = get_blocking_client_for_proxy().map_err(|e| e.to_string())?;

    let mut results = Vec::with_capacity(bases.len());
    for base in bases {
        let mut base_url =
            Url::parse(&base).map_err(|e| format!("Invalid base url '{}': {}", base, e))?;
        base_url.set_path(original.path());
        base_url.set_query(original.query());
        base_url.set_fragment(None);

        let start = Instant::now();
        let response = client
            .head(base_url.clone())
            .timeout(Duration::from_secs(5))
            .send();

        results.push(match response {
            Ok(_) => CdnProbeResult {
                base,
                url: base_url.to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            },
            Err(error) => CdnProbeResult {
                base,
                url: base_url.to_string(),
                latency_ms: None,
                error: Some(error.to_string()),
            },
        });
    }

    results.sort_by(|a, b| match (a.latency_ms, b.latency_ms) {
        (Some(la), Some(lb)) => la.cmp(&lb),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.base.cmp(&b.base),
    });

    let recommended_base = results
        .iter()
        .find_map(|result| result.latency_ms.map(|_| result.base.clone()));

    Ok(CdnProbeResponse {
        recommended_base,
        results,
    })
}
