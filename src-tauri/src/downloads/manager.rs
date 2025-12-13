// src/downloads/manager.rs
use crate::config::config::read_config;
use crate::downloads::multi::download_multi;
use crate::downloads::single::download_file;
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{create_task, finish_task, update_progress};
use num_cpus;
use reqwest::Client;
use std::path::PathBuf;
use tracing::debug;

pub struct DownloaderManager {
    client: Client,
}

impl DownloaderManager {
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// 直接执行下载（不创建 task），使用已有 task_id（命令层传入）
    pub async fn download_with_options(
        &self,
        task_id: &str,
        url: String,
        dest: PathBuf,
        md5_expected: Option<&str>,
    ) -> Result<CoreResult, CoreError> {
        let config = read_config().map_err(|e| CoreError::Config(e.to_string()))?;

        let threads = if config.launcher.download.auto_thread_count {
            num_cpus::get()
        } else if config.launcher.download.multi_thread {
            config.launcher.download.max_threads as usize
        } else {
            1
        };

        update_progress(task_id, 0, None, Some("downloading"));

        let mut retry = 0usize;
        loop {
            debug!(
                "DownloaderManager start download loop retry={} threads={}",
                retry, threads
            );
            let res = if threads > 1 {
                download_multi(
                    self.client.clone(),
                    task_id,
                    &url,
                    &dest,
                    threads,
                    md5_expected,
                )
                .await
            } else {
                download_file(self.client.clone(), task_id, &url, &dest, md5_expected).await
            };

            match &res {
                Ok(CoreResult::Success(_)) | Ok(CoreResult::Cancelled) => return res,
                _ if retry < 3 => {
                    retry += 1;
                    debug!("DownloaderManager retrying {} for url={}", retry, url);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
                _ => return res,
            }
        }
    }

    /// manager 创建新的 task 并 spawn 后台执行，立即返回 task_id
    pub fn start_download(
        &self,
        url: String,
        dest: PathBuf,
        md5_expected: Option<String>,
    ) -> String {
        let task_id = create_task(None, "ready", None);
        let client = self.client.clone();

        // clones for task
        let url_clone = url.clone();
        let dest_clone = dest.clone();
        let md5_clone = md5_expected.clone();
        let task_id_clone = task_id.clone();

        tokio::spawn(async move {
            update_progress(&task_id_clone, 0, None, Some("starting"));

            let manager = DownloaderManager::with_client(client);

            let res = manager
                .download_with_options(
                    &task_id_clone,
                    url_clone,
                    dest_clone.clone(),
                    md5_clone.as_deref(),
                )
                .await;

            match res {
                Ok(CoreResult::Success(_)) => {
                    finish_task(&task_id_clone, "completed", None);
                }
                Ok(CoreResult::Cancelled) => {
                    finish_task(
                        &task_id_clone,
                        "cancelled",
                        Some("download cancelled".into()),
                    );
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

        task_id
    }
}
