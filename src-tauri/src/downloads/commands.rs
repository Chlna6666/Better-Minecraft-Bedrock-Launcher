use std::fs;
use std::path::Path;
use num_cpus;
use crate::config::config::{read_config, ProxyConfig};
use tracing::{debug, info};

use reqwest::{ClientBuilder, Proxy};
// use 新的全局名字与 per-task 新建函数
use crate::downloads::cancel::new_cancel_flag;
use crate::downloads::manager::DownloaderManager;
use crate::downloads::WuClient::client::WuClient;
use crate::result::{CoreError, CoreResult};

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
    package_id: String,
    file_name: String,
    md5: Option<String>,
) -> Result<String, String> {

    // per-task cancel handle
    let cancel_flag = new_cancel_flag();

    let config = read_config().map_err(|e| e.to_string())?;
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

    // 只在这里构造一次 PathBuf（用于传给下载函数）
    let dest = downloads_dir.join(&file_name);

    let threads = if config.launcher.download.auto_thread_count {
        num_cpus::get()
    } else if config.launcher.download.multi_thread {
        config.launcher.download.max_threads as usize
    } else {
        1
    };

    let manager = DownloaderManager::with_client(client.clone());
    let wu_client = WuClient::with_client(client);

    // 获取下载 URL（传入 per-task cancel_flag）
    let url_result = wu_client
        .get_download_url(update_id, revision, Some(cancel_flag.clone()))
        .await
        .map_err(|e| format!("获取下载地址失败：{}", e))?;

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

    // 把 dest（owned PathBuf）移动给下载函数 —— **不再 clone dest**
    let download_result = manager
        .download_with_options(
            url,
            dest,                        // moved here, 不再可在本函数使用
            threads,
            md5,
            Some(cancel_flag.clone()),   // per-task cancel flag
        )
        .await;

    // 需要在后面做删除或返回路径字符串时，重新用相同的构造方式生成 PathBuf（没有 clone）
    let dest_after = downloads_dir.join(&file_name);

    match download_result {
        Ok(CoreResult::Success(_)) => {
            info!("下载完成 {}", file_name);
            Ok(dest_after.to_string_lossy().to_string())
        }
        Ok(CoreResult::Cancelled) => {
            debug!("下载被取消");
            let _ = fs::remove_file(&dest_after); // 删除部分下载文件（使用重新构造的路径）
            Ok("cancelled".into())
        }
        Ok(CoreResult::Error(err)) => {
            let _ = fs::remove_file(&dest_after);
            Err(format!("下载出错：{}", err))
        }
        Err(err) => {
            let _ = fs::remove_file(&dest_after);
            Err(format!("下载发生异常：{}", err))
        }
    }
}
