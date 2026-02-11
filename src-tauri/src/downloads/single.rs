// src/downloads/single.rs
use crate::downloads::md5::verify_md5;
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{finish_task, is_cancelled, set_total, update_progress};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

// 单线程模式下，通道可以小一点
const CHANNEL_CAPACITY: usize = 50;
const BATCH_SIZE_THRESHOLD: usize = 512 * 1024;
const DISK_BUFFER_SIZE: usize = 4 * 1024 * 1024;

pub async fn download_file(
    client: reqwest::Client,
    task_id: &str,
    url: &str,
    dest: impl AsRef<Path>,
    headers: Option<HeaderMap>,
    md5_expected: Option<&str>,
) -> Result<CoreResult<()>, CoreError> {
    let dest_buf = dest.as_ref().to_path_buf();
    let mut retry = 0u8;

    loop {
        debug!(
            "开始单线程下载 (I/O Offload): task={} url={}，重试={}",
            task_id, url, retry
        );

        let mut req_builder = client.get(url);
        if let Some(h) = &headers {
            req_builder = req_builder.headers(h.clone());
        }

        match req_builder.send().await {
            Ok(resp) => {
                let resp = resp.error_for_status()?;
                let total = resp.content_length();
                set_total(&task_id, total);

                // =========================================================
                // 1. 启动独立的磁盘写入线程
                // =========================================================
                let (tx, mut rx) = mpsc::channel::<Vec<Bytes>>(CHANNEL_CAPACITY);
                let write_dest = dest_buf.clone();
                let task_id_clone = task_id.to_string();

                let writer_handle = thread::spawn(move || -> Result<(), std::io::Error> {
                    let file = OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(&write_dest)?;

                    // [核心优化] 预分配空间
                    if let Some(len) = total {
                        if let Err(e) = file.set_len(len) {
                            warn!("预分配磁盘空间失败: {}", e);
                        }
                    }

                    // 4MB 缓冲区
                    let mut writer = BufWriter::with_capacity(DISK_BUFFER_SIZE, file);

                    while let Some(batch) = rx.blocking_recv() {
                        for chunk in batch {
                            writer.write_all(&chunk)?;
                        }
                    }
                    writer.flush()?;
                    Ok(())
                });

                // =========================================================
                // 2. 网络下载与批处理
                // =========================================================
                let mut stream = resp.bytes_stream();
                let mut pending_bytes = 0u64;
                let mut last_update = Instant::now();

                let mut current_batch: Vec<Bytes> = Vec::with_capacity(16);
                let mut current_batch_size = 0;

                while let Some(item) = stream.next().await {
                    let chunk = match item {
                        Ok(c) => c,
                        Err(e) => {
                            return Err(CoreError::Request(e));
                        }
                    };

                    let len = chunk.len();
                    current_batch.push(chunk);
                    current_batch_size += len;
                    pending_bytes += len as u64;

                    if current_batch_size >= BATCH_SIZE_THRESHOLD {
                        if tx.send(current_batch).await.is_err() {
                            return Err(CoreError::Io(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "Disk writer thread died"
                            )));
                        }
                        current_batch = Vec::with_capacity(16);
                        current_batch_size = 0;
                    }

                    if last_update.elapsed().as_millis() > 200 {
                        update_progress(&task_id, pending_bytes, total, Some("downloading"));
                        pending_bytes = 0;
                        last_update = Instant::now();
                    }

                    if is_cancelled(&task_id) {
                        finish_task(&task_id, "cancelled", Some("user cancelled".into()));
                        return Ok(CoreResult::Cancelled);
                    }
                }

                if !current_batch.is_empty() {
                    let _ = tx.send(current_batch).await;
                }

                drop(tx);

                if pending_bytes > 0 {
                    update_progress(&task_id, pending_bytes, total, Some("downloading"));
                }

                // =========================================================
                // 3. 等待结果
                // =========================================================
                match writer_handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        finish_task(&task_id, "error", Some(format!("Disk write error: {}", e)));
                        return Err(CoreError::Io(e));
                    }
                    Err(_) => {
                        finish_task(&task_id, "error", Some("Writer thread panic".into()));
                        return Err(CoreError::Other("Writer thread panic".into()));
                    }
                }

                if let Some(expected) = md5_expected {
                    update_progress(&task_id, 0, total, Some("verifying"));
                    match verify_md5(&dest_buf, expected).await {
                        Ok(true) => debug!("MD5 OK"),
                        Ok(false) => {
                            finish_task(&task_id, "error", Some("md5 mismatch".into()));
                            return Err(CoreError::ChecksumMismatch(format!("expected {}", expected)));
                        }
                        Err(e) => {
                            finish_task(&task_id, "error", Some("md5 error".into()));
                            return Err(CoreError::Io(e));
                        }
                    }
                }

                finish_task(&task_id, "completed", Some(dest_buf.to_string_lossy().to_string()));
                return Ok(CoreResult::Success(()));
            }
            Err(e) if retry < 3 => {
                retry += 1;
                debug!("下载重试 {}/3: {}", retry, e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(e) => {
                finish_task(&task_id, "error", Some(format!("{}", e)));
                return Err(CoreError::Request(e));
            }
        }
    }
}