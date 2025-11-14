// src/http/commands.rs
use reqwest::{Url};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;
use crate::config::config::{read_config};
use crate::http::proxy::{get_client_for_proxy};
use crate::http::request::{send_request_with_options, RequestOptions, GLOBAL_CLIENT};

#[derive(Deserialize)]
pub struct FetchOptions {
    pub method: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub allow_redirects: Option<bool>,
    pub allowed_hosts: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct FetchResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[tauri::command]
pub async fn fetch_remote(url: String, options: Option<FetchOptions>) -> Result<FetchResponse, String> {
    let parsed = Url::parse(&url).map_err(|e| format!("invalid url: {}", e))?;
    
    if let Some(opts) = &options {
        if let Some(allow) = &opts.allowed_hosts {
            let host = parsed.host_str().unwrap_or("");
            let port = parsed.port_or_known_default();
            let host_port = match port {
                Some(p) => format!("{}:{}", host, p),
                None => host.to_string(),
            };

            let allowed = allow.iter().any(|a| {
                if a == &host_port { return true; }
                if a == host { return true; }
                false
            });

            if !allowed {
                return Err(format!("host not allowed: {}", host));
            }
        }
    }
    
    let client = match read_config() {
        Ok(cfg) => {
            get_client_for_proxy().unwrap_or_else(|e| {
                debug!("构建代理 client 失败，回退到全局 client: {:?}", e);
                GLOBAL_CLIENT.clone()
            })
        }
        Err(_) => GLOBAL_CLIENT.clone(),
    };

    // 构造 RequestOptions
    let method = options
        .as_ref()
        .and_then(|o| o.method.as_ref().map(|s| s.as_str()))
        .unwrap_or("GET");

    let req_opts = RequestOptions {
        method,
        headers: options.as_ref().and_then(|o| o.headers.as_ref()),
        timeout_ms: options.as_ref().and_then(|o| o.timeout_ms),
        allow_redirects: options.as_ref().and_then(|o| o.allow_redirects),
    };

    // 发送请求
    let resp = send_request_with_options(&client, &parsed, &req_opts).await?;
    let status = resp.status().as_u16();

    let mut headers_out = HashMap::new();
    for (k, v) in resp.headers().iter() {
        if let Ok(s) = v.to_str() {
            headers_out.insert(k.to_string(), s.to_string());
        }
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok(FetchResponse { status, headers: headers_out, body: text })
}
