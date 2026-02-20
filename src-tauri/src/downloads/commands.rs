// src/downloads/commands.rs
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tracing::{debug, error};

use crate::config::config::read_config;
use crate::downloads::manager::DownloaderManager;
use crate::downloads::md5::verify_md5;
use crate::downloads::wu_client::client::WuClient;
use crate::http::proxy::get_client_for_proxy;
use crate::result::CoreResult;
use crate::tasks::task_manager::{create_task, finish_task, is_cancelled, update_progress};
use crate::utils::file_ops;

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    let mut s = trimmed.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
    while s.ends_with('.') {
        s.pop();
    }
    if s.is_empty() {
        "download.bin".to_string()
    } else {
        s
    }
}

fn safe_file_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download.bin")
        .to_string()
}

fn downloads_dir() -> PathBuf {
    file_ops::bmcbl_subdir("downloads")
}

async fn local_file_ok(dest: &PathBuf, md5: &Option<String>) -> bool {
    if !dest.exists() {
        return false;
    }
    if let Some(expected) = md5.as_deref() {
        verify_md5(dest, expected).await.unwrap_or(false)
    } else {
        true
    }
}

#[tauri::command]
pub async fn local_download_path(
    file_name: String,
    md5: Option<String>,
) -> Result<Option<String>, String> {
    let downloads_dir = downloads_dir();
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let safe_name = safe_file_name(&file_name);
    let dest = downloads_dir.join(&safe_name);
    if local_file_ok(&dest, &md5).await {
        Ok(Some(dest.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn delete_local_download(file_name: String) -> Result<(), String> {
    let downloads_dir = downloads_dir();
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let safe_name = safe_file_name(&file_name);
    let dest = downloads_dir.join(&safe_name);

    if dest.exists() {
        if dest.is_file() {
            fs::remove_file(&dest).map_err(|e| e.to_string())?;
            debug!("Deleted local download: {}", dest.to_string_lossy());
        } else {
            return Err("目标不是文件，无法删除".into());
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn download_appx(
    package_id: String,
    file_name: String,
    md5: Option<String>,
    force_download: Option<bool>,
) -> Result<String, String> {
    let client = get_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let parts: Vec<&str> = package_id.split('_').collect();
    if parts.len() != 2 {
        return Err("package_id 格式无效，必须形如 `<id>_<revision>`".into());
    }
    let (update_id, revision) = (parts[0], parts[1]);

    let downloads_dir = downloads_dir();
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let safe_name = safe_file_name(&file_name);
    let dest = downloads_dir.join(&safe_name);

    // 如果本地已存在且校验通过，直接跳过下载，走本地安装/解压流程
    let force = force_download.unwrap_or(false);
    if !force && local_file_ok(&dest, &md5).await {
        let task_id = create_task(None, "ready", None);
        let dest_str = dest.to_string_lossy().to_string();
        finish_task(&task_id, "completed", Some(dest_str));
        return Ok(task_id);
    }
    if force && dest.exists() {
        let _ = fs::remove_file(&dest);
    }

    // 1. 创建 Task
    let task_id = create_task(None, "ready", None);

    // 2. 获取下载 URL
    let wu_client = WuClient::with_client(client.clone());
    let url_result = wu_client
        .get_download_url(update_id, revision, &task_id)
        .await
        .map_err(|e| format!("获取下载地址失败：{}", e))?;

    let url = match url_result {
        CoreResult::Success(u) => {
            // 拿到 URL 后立即检查取消
            if is_cancelled(&task_id) {
                finish_task(&task_id, "cancelled", Some("取消下载".into()));
                return Ok(task_id);
            }
            u
        },
        CoreResult::Cancelled => {
            finish_task(&task_id, "cancelled", Some("取消下载".into()));
            return Ok(task_id);
        }
        CoreResult::Error(e) => {
            finish_task(&task_id, "error", Some(format!("{:?}", e)));
            return Err(format!("获取下载地址失败：{}", e));
        }
    };

    let manager = DownloaderManager::with_client(client.clone());
    let url_clone = url.clone();
    let dest_clone = dest.clone();
    let md5_clone = md5.clone();
    let task_id_clone = task_id.clone();
    let manager_clone = manager;

    // 3. 启动后台下载任务
    tokio::spawn(async move {
        // 二次检查取消
        if is_cancelled(&task_id_clone) {
            finish_task(&task_id_clone, "cancelled", Some("download cancelled before start".into()));
            return;
        }

        update_progress(&task_id_clone, 0, None, Some("starting"));

        let res = manager_clone
            .download_with_options(
                &task_id_clone,
                url_clone,
                dest_clone.clone(),
                None, // headers
                md5_clone.as_deref(),
            )
            .await;

        match res {
            Ok(CoreResult::Success(_)) => {
                // 下载成功：将路径回传给前端，用于后续解压
                let dest_str = dest_clone.to_string_lossy().to_string();
                finish_task(&task_id_clone, "completed", Some(dest_str));
            }
            Ok(CoreResult::Cancelled) => {
                finish_task(&task_id_clone, "cancelled", Some("download cancelled".into()));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
            Ok(CoreResult::Error(err)) => {
                finish_task(&task_id_clone, "error", Some(format!("{:?}", err)));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
            Err(e) => {
                finish_task(&task_id_clone, "error", Some(format!("{:?}", e)));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
        }
    });

    Ok(task_id)
}

/// 通用资源下载（前端 GDK 下载走这里）
#[tauri::command]
pub async fn download_resource(
    url: String,
    file_name: String,
    md5: Option<String>,
    force_download: Option<bool>,
) -> Result<String, String> {
    let client = get_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let downloads_dir = downloads_dir();
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let safe_name = safe_file_name(&file_name);
    let dest = downloads_dir.join(&safe_name);

    // 如果本地已存在且校验通过，直接跳过下载
    let force = force_download.unwrap_or(false);
    if !force && local_file_ok(&dest, &md5).await {
        let task_id = create_task(None, "ready", None);
        let dest_str = dest.to_string_lossy().to_string();
        finish_task(&task_id, "completed", Some(dest_str));
        return Ok(task_id);
    }
    if force && dest.exists() {
        let _ = fs::remove_file(&dest);
    }

    let task_id = create_task(None, "ready", None);

    let manager = DownloaderManager::with_client(client);
    let dest_clone = dest.clone();
    let task_id_clone = task_id.clone();
    let md5_clone = md5.clone();

    // 2. 手动 Spawn，确保我们能控制 finish_task 的行为
    tokio::spawn(async move {
        // 启动前检查取消
        if is_cancelled(&task_id_clone) {
            finish_task(&task_id_clone, "cancelled", Some("cancelled before start".into()));
            return;
        }

        update_progress(&task_id_clone, 0, None, Some("starting"));

        // 使用 download_with_options (这会调用 download_file)
        let res = manager.download_with_options(
            &task_id_clone,
            url,
            dest_clone.clone(),
            None, // headers
            md5_clone.as_deref()
        ).await;

        // 3. 显式处理结果，确保 Success 时带上文件路径
        match res {
            Ok(CoreResult::Success(_)) => {
                // [修复核心] 必须在这里显式传入路径，前端才能收到 message 并触发解压
                let dest_str = dest_clone.to_string_lossy().to_string();
                debug!("GDK/Resource 下载完成，发送路径: {}", dest_str);
                finish_task(&task_id_clone, "completed", Some(dest_str));
            }
            Ok(CoreResult::Cancelled) => {
                finish_task(&task_id_clone, "cancelled", Some("user cancelled".into()));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
            Ok(CoreResult::Error(e)) => {
                finish_task(&task_id_clone, "error", Some(format!("{:?}", e)));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
            Err(e) => {
                finish_task(&task_id_clone, "error", Some(format!("{:?}", e)));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
        }
    });

    Ok(task_id)
}

/// 通用资源下载到系统缓存目录（CurseForge 下载默认走这里）
#[tauri::command]
pub async fn download_resource_to_cache(
    url: String,
    file_name: String,
    md5: Option<String>,
) -> Result<String, String> {
    let client = get_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let cache_dir = std::env::temp_dir().join("BMCBL").join("cache_downloads");
    fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    let safe_name = sanitize_filename(&file_name);
    let dest = cache_dir.join(&safe_name);

    let task_id = create_task(None, "ready", None);

    let manager = DownloaderManager::with_client(client);
    let dest_clone = dest.clone();
    let task_id_clone = task_id.clone();
    let md5_clone = md5.clone();

    tokio::spawn(async move {
        if is_cancelled(&task_id_clone) {
            finish_task(&task_id_clone, "cancelled", Some("cancelled before start".into()));
            return;
        }

        update_progress(&task_id_clone, 0, None, Some("starting"));

        let res = manager
            .download_with_options(
                &task_id_clone,
                url,
                dest_clone.clone(),
                None,
                md5_clone.as_deref(),
            )
            .await;

        match res {
            Ok(CoreResult::Success(_)) => {
                let dest_str = dest_clone.to_string_lossy().to_string();
                debug!("Resource 下载完成(缓存)，发送路径: {}", dest_str);
                finish_task(&task_id_clone, "completed", Some(dest_str));
            }
            Ok(CoreResult::Cancelled) => {
                finish_task(&task_id_clone, "cancelled", Some("user cancelled".into()));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
            Ok(CoreResult::Error(e)) => {
                finish_task(&task_id_clone, "error", Some(format!("{:?}", e)));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
            Err(e) => {
                finish_task(&task_id_clone, "error", Some(format!("{:?}", e)));
                let _ = tokio::fs::remove_file(&dest_clone).await;
            }
        }
    });

    Ok(task_id)
}
