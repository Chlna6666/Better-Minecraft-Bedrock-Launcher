use std::fs;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::downloads::integrity::verify_download_integrity;
use crate::downloads::manager::{DownloadOptions, DownloaderManager};
use crate::downloads::wu_client::client::WuClient;
use crate::http::proxy::get_download_client_for_proxy;
use crate::result::CoreResult;
use crate::tasks::task_manager::{
    create_task_with_details, finish_task, is_cancelled, register_task_abort_handle,
    update_progress,
};
use crate::utils::file_ops;

fn safe_file_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download.bin")
        .to_string()
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    let mut s = trimmed.replace(['\\', '/', ':', '*', '?', '\"', '<', '>', '|'], "_");
    while s.ends_with('.') {
        s.pop();
    }
    if s.is_empty() {
        "download.bin".to_string()
    } else {
        s
    }
}

fn task_target_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(file_name)
        .to_string()
}

fn remove_download_temp(dest: &Path) {
    let temp_dest = crate::downloads::manager::temp_download_path(dest);
    if temp_dest.exists() {
        let _ = fs::remove_file(temp_dest);
    }
}

fn downloads_dir() -> PathBuf {
    file_ops::downloads_dir()
}

async fn local_file_ok(dest: &Path, md5: &Option<String>) -> bool {
    if !dest.exists() {
        return false;
    }
    match verify_download_integrity(dest, md5.as_deref()).await {
        Ok(()) => true,
        Err(error) => {
            debug!(
                "local download integrity failed; removing corrupt file path={} error={}",
                dest.to_string_lossy(),
                error
            );
            let _ = fs::remove_file(dest);
            remove_download_temp(dest);
            false
        }
    }
}

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
    remove_download_temp(&dest);
    Ok(())
}

pub async fn download_appx(
    package_id: String,
    file_name: String,
    md5: Option<String>,
    force_download: Option<bool>,
    download_options: Option<DownloadOptions>,
) -> Result<String, String> {
    crate::downloads::register_download_task_stage_labels();
    let client =
        get_download_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let parts: Vec<&str> = package_id.split('_').collect();
    if parts.len() != 2 {
        return Err("package_id 格式无效，必须形如 `<id>_<revision>`".into());
    }
    let (update_id, revision) = (parts[0], parts[1]);

    let downloads_dir = downloads_dir();
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let safe_name = safe_file_name(&file_name);
    let dest = downloads_dir.join(&safe_name);

    let force = force_download.unwrap_or(false);
    if !force && local_file_ok(&dest, &md5).await {
        let task_id = create_task_with_details(
            None,
            "下载游戏包",
            Some(task_target_name(&safe_name)),
            "ready",
            None,
            true,
        );
        let dest_str = dest.to_string_lossy().to_string();
        finish_task(&task_id, "completed", Some(dest_str));
        return Ok(task_id);
    }
    if force && dest.exists() {
        let _ = fs::remove_file(&dest);
        remove_download_temp(&dest);
    }

    let task_id = create_task_with_details(
        None,
        "下载游戏包",
        Some(task_target_name(&safe_name)),
        "ready",
        None,
        true,
    );

    let update_id = update_id.to_string();
    let revision = revision.to_string();
    let dest_clone = dest.clone();
    let task_id_clone = task_id.clone();

    let abort_handle =
        match crate::downloads::runtime::spawn_download_task(task_id.clone(), async move {
            if is_cancelled(&task_id_clone) {
                finish_task(
                    &task_id_clone,
                    "cancelled",
                    Some("cancelled before start".into()),
                );
                return;
            }

            let wu_client = WuClient::with_client(client.clone());
            let file_result = match wu_client
                .get_download_files(&update_id, &revision, &task_id_clone)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    finish_task(
                        &task_id_clone,
                        "error",
                        Some(format!("获取下载地址失败: {error:?}")),
                    );
                    return;
                }
            };

            let candidates = match file_result {
                CoreResult::Success(files) => files.into_iter().map(|file| file.url).collect(),
                CoreResult::Cancelled => {
                    finish_task(&task_id_clone, "cancelled", Some("取消下载".into()));
                    return;
                }
                CoreResult::Error(error) => {
                    finish_task(&task_id_clone, "error", Some(format!("{error:?}")));
                    return;
                }
            };

            if is_cancelled(&task_id_clone) {
                finish_task(&task_id_clone, "cancelled", Some("取消下载".into()));
                return;
            }

            update_progress(&task_id_clone, 0, None, Some("starting"));
            let mut options = download_options.unwrap_or_default();
            if options.md5_expected.is_none() {
                options.md5_expected = md5;
            }
            let manager = DownloaderManager::with_client(client);
            let res = manager
                .download_with_url_candidates(
                    &task_id_clone,
                    candidates,
                    dest_clone.clone(),
                    &options,
                )
                .await;

            match res {
                Ok(CoreResult::Success(final_path)) => {
                    let dest_str = final_path.to_string_lossy().to_string();
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
        }) {
            Ok(abort_handle) => abort_handle,
            Err(error) => {
                finish_task(&task_id, "error", Some(error));
                return Ok(task_id);
            }
        };
    register_task_abort_handle(task_id.clone(), abort_handle);

    Ok(task_id)
}

pub async fn download_resource(
    url: String,
    file_name: String,
    md5: Option<String>,
    force_download: Option<bool>,
    download_options: Option<DownloadOptions>,
) -> Result<String, String> {
    crate::downloads::register_download_task_stage_labels();
    let client =
        get_download_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    let downloads_dir = downloads_dir();
    fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;
    let safe_name = safe_file_name(&file_name);
    let dest = downloads_dir.join(&safe_name);

    let force = force_download.unwrap_or(false);
    if !force && local_file_ok(&dest, &md5).await {
        let task_id = create_task_with_details(
            None,
            "下载资源文件",
            Some(task_target_name(&safe_name)),
            "ready",
            None,
            true,
        );
        let dest_str = dest.to_string_lossy().to_string();
        finish_task(&task_id, "completed", Some(dest_str));
        return Ok(task_id);
    }
    if force && dest.exists() {
        let _ = fs::remove_file(&dest);
        remove_download_temp(&dest);
    }

    let task_id = create_task_with_details(
        None,
        "下载资源文件",
        Some(task_target_name(&safe_name)),
        "ready",
        None,
        true,
    );

    let manager = DownloaderManager::with_client(client);
    let dest_clone = dest.clone();
    let task_id_clone = task_id.clone();

    let abort_handle =
        match crate::downloads::runtime::spawn_download_task(task_id.clone(), async move {
            if is_cancelled(&task_id_clone) {
                finish_task(
                    &task_id_clone,
                    "cancelled",
                    Some("cancelled before start".into()),
                );
                return;
            }

            update_progress(&task_id_clone, 0, None, Some("starting"));

            let mut options = download_options.unwrap_or_default();
            if options.md5_expected.is_none() {
                options.md5_expected = md5;
            }
            let res = manager
                .download_with_options(&task_id_clone, url, dest_clone.clone(), &options)
                .await;

            match res {
                Ok(CoreResult::Success(final_path)) => {
                    let dest_str = final_path.to_string_lossy().to_string();
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
        }) {
            Ok(abort_handle) => abort_handle,
            Err(error) => {
                finish_task(&task_id, "error", Some(error));
                return Ok(task_id);
            }
        };
    register_task_abort_handle(task_id.clone(), abort_handle);

    Ok(task_id)
}

/// Download a remote file into a temp cache directory.
///
/// This mirrors the upstream tauri command `download_resource_to_cache` and integrates with the
/// global task manager so GPUI can display progress.
pub async fn download_resource_to_cache(
    url: String,
    file_name: String,
    md5: Option<String>,
    download_options: Option<DownloadOptions>,
) -> Result<String, String> {
    crate::downloads::register_download_task_stage_labels();
    let client =
        get_download_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

    #[cfg(target_os = "linux")]
    let cache_dir = file_ops::cache_subdir("resource-downloads");
    #[cfg(not(target_os = "linux"))]
    let cache_dir = std::env::temp_dir().join("BMCBL").join("cache_downloads");
    fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    let safe_name = sanitize_filename(&file_name);
    let dest = cache_dir.join(&safe_name);

    let task_id = create_task_with_details(
        None,
        "下载缓存文件",
        Some(task_target_name(&safe_name)),
        "ready",
        None,
        true,
    );

    let manager = DownloaderManager::with_client(client);
    let dest_clone = dest.clone();
    let task_id_clone = task_id.clone();

    let abort_handle =
        match crate::downloads::runtime::spawn_download_task(task_id.clone(), async move {
            if is_cancelled(&task_id_clone) {
                finish_task(
                    &task_id_clone,
                    "cancelled",
                    Some("cancelled before start".into()),
                );
                return;
            }

            update_progress(&task_id_clone, 0, None, Some("starting"));

            let mut options = download_options.unwrap_or_default();
            if options.md5_expected.is_none() {
                options.md5_expected = md5;
            }
            let res = manager
                .download_with_options(&task_id_clone, url, dest_clone.clone(), &options)
                .await;

            match res {
                Ok(CoreResult::Success(final_path)) => {
                    let dest_str = final_path.to_string_lossy().to_string();
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
        }) {
            Ok(abort_handle) => abort_handle,
            Err(error) => {
                finish_task(&task_id, "error", Some(error));
                return Ok(task_id);
            }
        };
    register_task_abort_handle(task_id.clone(), abort_handle);

    Ok(task_id)
}

#[cfg(test)]
mod tests {
    use super::download_appx;
    use crate::tasks::task_manager::{TaskSnapshot, get_snapshot_arc, remove_task};
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Once;
    use std::time::{Duration, Instant};
    use tokio::time::sleep;

    const MC_1_21_93_X64_UPDATE_ID: &str = "9a1e10b3-e8e1-4d01-a2c0-c3ecac48fd13";
    const MC_1_21_93_X64_REVISION: &str = "1";
    const MC_1_21_93_X64_TEST_FILE: &str = "1.21.93-x64-test.appx";

    static LOG_INIT: Once = Once::new();

    fn init_test_logging() {
        LOG_INIT.call_once(|| {
            let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        });
    }

    async fn wait_task_finished_for_test(task_id: &str, timeout: Duration) -> Arc<TaskSnapshot> {
        let started_at = Instant::now();
        let mut last_log_at = Instant::now() - Duration::from_secs(60);

        loop {
            assert!(
                started_at.elapsed() < timeout,
                "download task timed out after {:?}: {task_id}",
                timeout
            );

            if let Some(snapshot) = get_snapshot_arc(task_id) {
                if last_log_at.elapsed() >= Duration::from_secs(10) {
                    println!(
                        "task={} status={} stage={} done={} total={:?} percent={:?} speed={:.2}B/s message={}",
                        snapshot.id,
                        snapshot.status,
                        snapshot.stage,
                        snapshot.done,
                        snapshot.total,
                        snapshot.percent,
                        snapshot.speed_bytes_per_sec,
                        snapshot.message.as_deref().unwrap_or("")
                    );
                    last_log_at = Instant::now();
                }

                if !matches!(
                    snapshot.status.as_ref(),
                    "running" | "paused" | "cancelling"
                ) {
                    return snapshot;
                }
            }

            sleep(Duration::from_millis(250)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "downloads a real 1GB+ Minecraft UWP package from Microsoft"]
    async fn download_minecraft_1_21_93_x64_from_wu_metadata() {
        init_test_logging();
        crate::utils::file_ops::create_initial_directories();

        let task_id = download_appx(
            format!("{MC_1_21_93_X64_UPDATE_ID}_{MC_1_21_93_X64_REVISION}"),
            MC_1_21_93_X64_TEST_FILE.to_string(),
            None,
            Some(true),
            None,
        )
        .await
        .expect("download task should be created");

        println!("created download task: {task_id}");
        let snapshot = wait_task_finished_for_test(&task_id, Duration::from_secs(45 * 60)).await;
        let status = snapshot.status.as_ref();
        let message = snapshot.message.as_deref().unwrap_or("");
        assert_eq!(status, "completed", "download task failed: {message}");
        assert!(
            Path::new(message).is_file(),
            "completed task did not return an existing file path: {message}"
        );

        remove_task(&task_id);
    }
}
