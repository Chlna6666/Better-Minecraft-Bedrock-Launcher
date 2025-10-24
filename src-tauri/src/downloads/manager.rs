// core/downloader_manager.rs
use reqwest::Client;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tracing::debug;
use crate::downloads::{cancel};
use crate::downloads::multi::download_multi;
use crate::downloads::single::download_file;
use crate::result::{CoreError, CoreResult};

pub struct DownloaderManager {
    client: Client,
}

impl DownloaderManager {
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// 兼容并扩展：可传入 md5_expected 与 cancel_flag
    pub async fn download_with_options(
        &self,
        url: String,
        dest: std::path::PathBuf,
        threads: usize,
        md5_expected: Option<String>,
        cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<CoreResult, CoreError> {
        let mut retry = 0;
        loop {
            debug!("DownloaderManager start download loop retry={}", retry);
            let res = if threads > 1 {
                // multi::download_multi 需要实现相同的参数签名（参见下面）
                download_multi(
                    self.client.clone(),
                    &url,
                    &dest,
                    threads,
                    md5_expected.as_deref(),
                    cancel_flag.clone(),
                )
                    .await
            } else {
                download_file(
                    self.client.clone(),
                    &url,
                    &dest,
                    md5_expected.as_deref(),
                    cancel_flag.clone(),
                )
                    .await
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

    /// 启动后台下载任务并返回一个取消句柄（Arc<AtomicBool>），方便前端保存并在需要时调用 `store` 并 set(true)
    pub fn start_download_task(
        &self,
        url: String,
        dest: impl AsRef<Path> + Send + 'static,
        threads: usize,
        md5_expected: Option<String>,
    ) -> Arc<AtomicBool> {
        let cancel_flag = cancel::new_cancel_flag();
        let cancel_flag_for_task = cancel_flag.clone(); // <-- clone before moving into closure

        let client = self.client.clone();
        let dest_buf = dest.as_ref().to_path_buf();
        // spawn a background task, move the cloned flag into the task
        tokio::spawn(async move {
            let mgr = DownloaderManager::with_client(client);
            let _ = mgr
                .download_with_options(
                    url,
                    dest_buf,
                    threads,
                    md5_expected,
                    Some(cancel_flag_for_task), // move cloned flag here
                )
                .await;
            // 结果可以发事件给前端（可通过 tauri::event::emit 等）
        });

        // return the original Arc so caller can cancel
        cancel_flag
    }
}