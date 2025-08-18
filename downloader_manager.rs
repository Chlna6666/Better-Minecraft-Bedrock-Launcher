use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use futures_util::StreamExt;
use reqwest::{header, Client};
use serde_json::json;
use tauri::{AppHandle};
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tokio::time::{sleep, Instant};
use tracing::{debug, info};

use crate::core::downloads::cancel::{is_cancelled, CANCEL_DOWNLOAD};
use crate::core::downloads::result::{DownloadError, DownloadResult};
use crate::core::minecraft::utils::{emit_progress, format_eta, format_speed};


pub struct DownloaderManager {
    client: Client
}

impl DownloaderManager {
    pub fn with_client(client: Client) -> Self {
        Self {
            client: client.clone(),
        }
    }


    /// 下载文件，支持配置线程数
    pub async fn download(
        &self,
        url: String,
        dest: impl AsRef<Path>,
        app_handle: AppHandle,
        threads: usize,

    ) -> Result<DownloadResult, DownloadError> {
        debug!("获取下载地址成功: {}", url);

        let mut retry = 0;
        loop {
            let result = if threads > 1 {
                debug!("开始多线程下载: 线程数 {}", threads);
                self.download_file_multi_thread(&url, &dest, app_handle.clone(), threads).await
            } else {
                debug!("开始单线程下载");
                self.download_file(&url, &dest, app_handle.clone()).await
            };

            match &result {
                Ok(DownloadResult::Success) => {
                    debug!("下载完成");
                    return result;
                },
                Ok(DownloadResult::Cancelled) => {
                    debug!("下载被用户取消");
                    return result;
                },
                _ if retry < 3 => {
                    retry += 1;
                    debug!("下载失败，尝试重试第 {} 次...", retry);
                    sleep(Duration::from_secs(1)).await;
                },
                _ => {
                    debug!("下载失败，终止尝试");
                    return result;
                },
            }
        }
    }

    pub async fn download_file(
        &self,
        url: &str,
        dest: impl AsRef<Path>,
        app_handle: AppHandle,
    ) -> Result<DownloadResult, DownloadError> {
        let mut retry = 0;
        loop {
            debug!("发送下载请求: {}", url);
            let response = self.client.get(url).send().await;

            match response {
                Ok(response) => {
                    debug!("响应成功，状态: {}", response.status());
                    let response = response.error_for_status()?;
                    let total_size = response.content_length().unwrap_or(0);
                    debug!("获取内容长度: {} bytes", total_size);
                    let mut file = File::create(dest.as_ref()).await?;
                    let mut downloaded: u64 = 0;
                    let start = Instant::now();
                    let mut last_emit = Instant::now() - Duration::from_secs(1);
                    let mut stream = response.bytes_stream();

                    while let Some(item) = stream.next().await {
                        if is_cancelled() {
                            debug!("下载被取消");
                            return Ok(DownloadResult::Cancelled);
                        }
                        let chunk = item?;
                        file.write_all(&chunk).await?;
                        downloaded += chunk.len() as u64;
                        
                        if last_emit.elapsed() >= Duration::from_millis(1000) || downloaded == total_size {
                            let elapsed = start.elapsed().as_secs_f64();
                            let speed_str = format_speed(downloaded, elapsed);
                            let eta_str = format_eta(Some(total_size), downloaded, elapsed);

                            emit_progress(
                                &app_handle,
                                downloaded,
                                Some(total_size),
                                Some(&speed_str),
                                Some(&eta_str),
                                Some(json!({ "stage": "downloading" })),
                            ).await;

                            last_emit = Instant::now();
                        }
                    }

                    file.flush().await?;
                    info!("下载成功: {}", url);
                    return Ok(DownloadResult::Success);
                }
                Err(e) if retry < 3 => {
                    retry += 1;
                    debug!("请求失败，重试第 {} 次: {:?}", retry, e);
                    sleep(Duration::from_secs(1)).await;
                }
                Err(e) => {
                    debug!("请求最终失败: {:?}", e);
                    return Err(DownloadError::Request(e));
                },
            }
        }
    }

    pub async fn download_file_multi_thread(
        &self,
        url: &str,
        dest: impl AsRef<Path>,
        app_handle: AppHandle,
        threads: usize,
    ) -> Result<DownloadResult, DownloadError> {
        debug!("准备开始多线程下载: URL = {}, 线程数 = {}", url, threads);

        let head = self.client.head(url).send().await?;
        if head.headers().get(header::ACCEPT_RANGES) != Some(&header::HeaderValue::from_static("bytes")) {
            debug!("服务器不支持分片下载，回退到单线程");
            return self.download_file(url, dest, app_handle).await;
        }

        let total_size = head.headers().get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or(DownloadError::UnknownContentLength)?;

        debug!("总文件大小: {} bytes", total_size);

        let file = Arc::new(RwLock::new(File::create(dest.as_ref()).await?));
        let downloaded = Arc::new(AtomicU64::new(0));
        let start_time = Instant::now();

        let chunk_size = (total_size + threads as u64 - 1) / threads as u64;

        let ranges: Vec<(u64, u64)> = (0..threads).map(|i| {
            let start = i as u64 * chunk_size;
            let end = if i == threads - 1 { total_size - 1 } else { start + chunk_size - 1 };
            (start, end)
        }).collect();

        let downloaded_clone = Arc::clone(&downloaded);
        let app_handle_clone = app_handle.clone();
        let monitor_handle = tokio::spawn(async move {
            loop {
                if is_cancelled() {
                    break;
                }
                let downloaded_now = downloaded_clone.load(Ordering::Relaxed);
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed_str = format_speed(downloaded_now, elapsed);
                let eta_str = format_eta(Some(total_size), downloaded_now, elapsed);

                emit_progress(
                    &app_handle_clone, downloaded_now,
                    Some(total_size), Some(&speed_str), Some(&eta_str),
                    Some(json!({ "threads": threads, "stage": "downloading" }))
                ).await;

                if downloaded_now >= total_size {
                    break;
                }

                sleep(Duration::from_millis(500)).await;
            }
        });

        let mut handles = Vec::with_capacity(threads);
        for (start, end) in ranges.into_iter() {
            let url = url.to_string();
            let file = Arc::clone(&file);
            let downloaded = Arc::clone(&downloaded);
            let client = self.client.clone();

            let handle = tokio::spawn(async move {
                let mut retry = 0;
                loop {
                    let range_header = format!("bytes={}-{}", start, end);
                    let resp = client.get(&url).header(header::RANGE, range_header.clone()).send().await;

                    match resp {
                        Ok(resp) => {
                            let mut stream = resp.error_for_status()?.bytes_stream();
                            let mut offset = start;
                            while let Some(chunk_res) = stream.next().await {
                                if is_cancelled() {
                                    return Ok(DownloadResult::Cancelled);
                                }
                                let chunk = chunk_res?;
                                {
                                    let mut file_lock = file.write().await;
                                    file_lock.seek(std::io::SeekFrom::Start(offset)).await?;
                                    file_lock.write_all(&chunk).await?;
                                }
                                offset += chunk.len() as u64;
                                downloaded.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                            }
                            return Ok(DownloadResult::Success);
                        }
                        Err(_) if retry < 3 => {
                            retry += 1;
                            sleep(Duration::from_secs(1)).await;
                        }
                        Err(e) => {
                            return Err(DownloadError::Request(e));
                        }
                    }
                }
            });

            handles.push(handle);
        }

        let mut had_error = false;
        for h in handles {
            match h.await {
                Ok(Ok(DownloadResult::Success)) => {}
                Ok(Ok(DownloadResult::Cancelled)) => {
                    had_error = true;
                }
                Ok(Ok(DownloadResult::Error(_))) => {
                    had_error = true;
                }
                Ok(Err(_)) => {
                    had_error = true;
                }
                Err(_) => {
                    had_error = true;
                }
            }
        }

        let _ = monitor_handle.await;

        if is_cancelled() {
            return Ok(DownloadResult::Cancelled);
        }

        if had_error {
            let _ = tokio::fs::remove_file(dest.as_ref()).await;
            return Ok(DownloadResult::Error(DownloadError::Other("Download cancelled".into())));
        }

        let elapsed = start_time.elapsed().as_secs_f64();
        let speed_str = format_speed(total_size, elapsed);

        emit_progress(
            &app_handle,
            total_size,
            Some(total_size),
            Some(&speed_str),
            Some("00:00:00"),
            Some(json!({ "threads": threads, "stage": "downloading" }))
        ).await;

        Ok(DownloadResult::Success)
    }

}

