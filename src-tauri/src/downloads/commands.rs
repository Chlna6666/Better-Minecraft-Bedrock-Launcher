// src/downloads/commands.rs
use std::fs;
use std::path::PathBuf;
use tracing::{debug, error};

use crate::config::config::read_config;
use crate::downloads::manager::DownloaderManager;
use crate::downloads::wu_client::client::WuClient;
use crate::http::proxy::get_client_for_proxy;
use crate::result::CoreResult;
use crate::tasks::task_manager::{create_task, finish_task, update_progress};

#[tauri::command]
pub async fn download_appx(
    package_id: String,
    file_name: String,
    md5: Option<String>,
) -> Result<String, String> {
    let client = get_client_for_proxy()
        .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let parts: Vec<&str> = package_id.split('_').collect();
    if parts.len() != 2 {
        return Err("package_id 格式无效，必须形如 `<id>_<revision>`".into());
    }
    let (update_id, revision) = (parts[0], parts[1]);

    let downloads_dir = PathBuf::from("./BMCBL/downloads");
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let dest = downloads_dir.join(&file_name);

    // 命令层创建 task_id（因为需要在解析 URL 阶段也能被前端取消/轮询）
    let task_id = create_task("ready", None);

    // 获取下载 URL（WuClient 使用 task_id 检查取消/上报阶段）
    let wu_client = WuClient::with_client(client.clone());
    let url_result = wu_client
        .get_download_url(update_id, revision, &task_id)
        .await
        .map_err(|e| format!("获取下载地址失败：{}", e))?;

    let url = match url_result {
        CoreResult::Success(u) => u,
        CoreResult::Cancelled => {
            finish_task(&task_id, "cancelled", Some("取消下载".into()));
            return Ok("cancelled".into());
        }
        CoreResult::Error(e) => {
            finish_task(&task_id, "error", Some(format!("{:?}", e)));
            return Err(format!("获取下载地址失败：{}", e));
        }
    };

    // 使用 manager 启动实际下载任务（manager 负责 spawn 背景任务并返回 task_id）
    let manager = DownloaderManager::with_client(client.clone());

    // 这里我们已经有 task_id —— 想要 manager reuse 已有 task_id 的逻辑，
    // 我们使用 manager.start_download_for_existing_task（如果你没有，下面我给出 start_download_with_existing_task 的建议）
    // 为了最小变动，如果你的 manager 只有 start_download(url, dest, cfg, md5) 返回 task_id（它会自己创建 task_id），
    // 我们在这里直接调用 manager.download_with_options 在后台 spawn，并保持使用当前 task_id。
    let url_clone = url.clone();
    let dest_clone = dest.clone();
    let md5_clone = md5.clone();
    let task_id_clone = task_id.clone();
    let manager_clone = manager;

    // spawn background download while preserving this task_id
    tokio::spawn(async move {
        update_progress(&task_id_clone, 0, None, Some("starting"));

        let res = manager_clone
            .download_with_options(
                &task_id_clone,
                url_clone,
                dest_clone.clone(),
                md5_clone.as_deref(),
            )
            .await;

        match res {
            Ok(CoreResult::Success(_)) => {
                // 把下载后的本地路径放到 message 中，供前端读取并触发解压
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

/// 通用资源下载（前端只传 URL）
/// 此处直接把 task id 创建交给 manager.start_download（manager 会 spawn 并返回 task_id）
#[tauri::command]
pub async fn download_resource(
    url: String,
    file_name: String,
    md5: Option<String>,
) -> Result<String, String> {
    let client = get_client_for_proxy()
        .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let downloads_dir = PathBuf::from("./BMCBL/downloads");
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let dest = downloads_dir.join(&file_name);

    let manager = DownloaderManager::with_client(client);

    // start_download 会创建 task_id 并 spawn 后台任务
    let task_id = manager.start_download(
        url,
        dest,
        md5.clone(),
    );

    Ok(task_id)
}
