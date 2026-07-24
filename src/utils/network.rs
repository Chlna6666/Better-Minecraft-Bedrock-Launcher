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

    let mut unique_bases = Vec::new();
    for base in bases {
        let trimmed = base.trim_end_matches('/').to_string();
        if !unique_bases
            .iter()
            .any(|b: &String| b.eq_ignore_ascii_case(&trimmed))
        {
            unique_bases.push(trimmed);
        }
    }

    let mut candidates = Vec::new();
    for base in unique_bases {
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
            let mut res = client
                .head(url.clone())
                .timeout(Duration::from_secs(5))
                .send()
                .await;

            let mut is_405 = false;
            if let Ok(ref resp) = res {
                if resp.status().as_u16() == 405 {
                    is_405 = true;
                }
            }

            if is_405 {
                res = client
                    .get(url.clone())
                    .header("Range", "bytes=0-0")
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
            }

            match res {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() || status.is_redirection() || status.as_u16() == 206 {
                        CdnProbeResult {
                            base,
                            url: url.to_string(),
                            latency_ms: Some(start.elapsed().as_millis() as u64),
                            error: None,
                        }
                    } else {
                        CdnProbeResult {
                            base,
                            url: url.to_string(),
                            latency_ms: None,
                            error: Some(format!("HTTP {}", status)),
                        }
                    }
                }
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

    let mut unique_bases = Vec::new();
    for base in bases {
        let trimmed = base.trim_end_matches('/').to_string();
        if !unique_bases
            .iter()
            .any(|b: &String| b.eq_ignore_ascii_case(&trimmed))
        {
            unique_bases.push(trimmed);
        }
    }

    let mut results = Vec::with_capacity(unique_bases.len());
    for base in unique_bases {
        let mut base_url =
            Url::parse(&base).map_err(|e| format!("Invalid base url '{}': {}", base, e))?;
        base_url.set_path(original.path());
        base_url.set_query(original.query());
        base_url.set_fragment(None);

        let start = Instant::now();
        let mut response = client
            .head(base_url.clone())
            .timeout(Duration::from_secs(5))
            .send();

        if let Ok(ref resp) = response {
            if resp.status().as_u16() == 405 {
                response = client
                    .get(base_url.clone())
                    .header("Range", "bytes=0-0")
                    .timeout(Duration::from_secs(5))
                    .send();
            }
        }

        results.push(match response {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() || status.is_redirection() || status.as_u16() == 206 {
                    CdnProbeResult {
                        base,
                        url: base_url.to_string(),
                        latency_ms: Some(start.elapsed().as_millis() as u64),
                        error: None,
                    }
                } else {
                    CdnProbeResult {
                        base,
                        url: base_url.to_string(),
                        latency_ms: None,
                        error: Some(format!("HTTP {}", status)),
                    }
                }
            }
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
