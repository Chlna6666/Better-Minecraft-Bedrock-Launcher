use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use num_cpus;
use crate::config::config::{read_config, ProxyConfig};
use tauri::AppHandle;
use tracing::{debug, info};




use reqwest::{ClientBuilder, Proxy};
use crate::core::downloads::cancel::CANCEL_DOWNLOAD;
use crate::core::downloads::manager::DownloaderManager;
use crate::core::downloads::WuClient::client::WuClient;
use crate::core::result::{CoreError, CoreResult};

fn apply_proxy_settings(client_builder: ClientBuilder, proxy_config: &ProxyConfig) -> Result<ClientBuilder, CoreError> {
    match proxy_config {
        ProxyConfig { disable_all_proxy: true, .. } => {
            debug!("禁用所有代理");
            Ok(client_builder.no_proxy())
        }
        ProxyConfig { use_system_proxy: true, .. } => {
            debug!("使用系统代理");
            Ok(client_builder)
        }
        ProxyConfig { enable_custom_proxy: true, custom_proxy_url, .. } if !custom_proxy_url.is_empty() => {
            debug!("使用自定义代理 URL: {}", custom_proxy_url);
            let proxy = Proxy::all(custom_proxy_url).map_err(|e| CoreError::Config(e.to_string()))?;
            Ok(client_builder.proxy(proxy))
        }
        ProxyConfig { enable_http_proxy: true, http_proxy_url, .. } if !http_proxy_url.is_empty() => {
            debug!("使用 HTTP 代理 URL: {}", http_proxy_url);
            let proxy = Proxy::http(http_proxy_url).map_err(|e| CoreError::Config(e.to_string()))?;
            Ok(client_builder.proxy(proxy))
        }
        ProxyConfig { enable_socks_proxy: true, socks_proxy_url, .. } if !socks_proxy_url.is_empty() => {
            debug!("使用 SOCKS 代理 URL: {}", socks_proxy_url);
            let proxy = Proxy::all(socks_proxy_url).map_err(|e| CoreError::Config(e.to_string()))?;
            Ok(client_builder.proxy(proxy))
        }
        _ => {
            debug!("不使用任何代理");
            Ok(client_builder.no_proxy())
        }
    }
}

fn build_client_with_proxy(proxy_config: &ProxyConfig) -> Result<reqwest::Client, CoreError> {
    let client_builder = reqwest::Client::builder();
    let configured_builder = apply_proxy_settings(client_builder, proxy_config)?;
    configured_builder
        .build()
        .map_err(|e| CoreError::Config(e.to_string()))
}

#[tauri::command]
pub async fn download_appx(
    app: AppHandle,
    package_id: String,
    file_name: String,
) -> Result<String, String> {
    CANCEL_DOWNLOAD.store(false, Ordering::Relaxed);
    let config = read_config().map_err(|e| e.to_string())?;

    // 构建带代理的客户端
    let client = build_client_with_proxy(&config.launcher.download.proxy)
        .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    // 拆解 package_id
    let parts: Vec<&str> = package_id.split('_').collect();
    if parts.len() != 2 {
        return Err("package_id 格式无效，必须形如 `<id>_<revision>`".into());
    }
    let (update_id, revision) = (parts[0], parts[1]);

    let downloads_dir = Path::new("./BMCBL/downloads");
    fs::create_dir_all(downloads_dir).map_err(|e| e.to_string())?;
    let dest = downloads_dir.join(&file_name);

    let threads = if config.launcher.download.auto_thread_count {
        num_cpus::get()
    } else if config.launcher.download.multi_thread {
        config.launcher.download.max_threads as usize
    } else {
        1
    };

    // 新建 DownloaderManager，注入带代理的客户端
    let manager = DownloaderManager::with_client(client.clone());
    let wu_client = WuClient::with_client(client);

    // 注意：get_download_url 现在返回 Result<DownloadResult<String>, DownloadError>
    let url_result = wu_client
        .get_download_url(update_id, revision)
        .await
        .map_err(|e| format!("获取下载地址失败：{}", e))?;

    // 处理 DownloadResult
    let url = match url_result {
        CoreResult::Success(u) => u,
        CoreResult::Cancelled => {
            debug!("获取下载地址时被取消");
            return Ok("cancelled".into());
        }
        CoreResult::Error(e) => {
            return Err(format!("获取下载地址失败：{}", e));
        }
    };

    debug!("拿到下载 URL：{}", url);

    let download_result = manager
        .download(url, &dest, app.clone(), threads)
        .await;

    match download_result {
        Ok(CoreResult::Success(_)) => {
            info!("下载完成 {}", file_name);
            Ok(dest.to_string_lossy().to_string())
        }
        Ok(CoreResult::Cancelled) => {
            debug!("下载被取消");
            Ok("cancelled".into())
        }
        Ok(CoreResult::Error(err)) => Err(format!("下载出错：{}", err)),
        Err(err) => Err(format!("下载发生异常：{}", err)),
    }
}
