// src/downloads/single.rs
use crate::downloads::integrity::{has_zip_header, should_verify_zip_during_download};
use crate::downloads::md5::{is_md5_digest, verify_md5};
use crate::http::proxy::{apply_download_request_headers, validate_download_response_headers};
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{
    TaskControl, TaskVisualization, ThreadVisualization, is_cancelled_fast,
    maybe_set_task_visualization, set_task_visualization, set_total, update_progress,
    wait_until_active_fast,
};
use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::OpenOptions as TokioOpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tracing::debug;

const DISK_BUFFER_SIZE: usize = 4 * 1024 * 1024;
const DOWNLOAD_REQUEST_TIMEOUT_SECS: u64 = 6 * 60 * 60;

fn build_single_download_visualization(
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
) -> TaskVisualization {
    let thread_total = total_bytes.unwrap_or(downloaded_bytes.max(1));
    let thread_done = downloaded_bytes.min(thread_total);

    TaskVisualization {
        worker_total: Some(1),
        worker_active: Some(1),
        unit_label: Some("分片".to_string()),
        unit_total: Some(1),
        unit_done: Some(u64::from(thread_done >= thread_total && thread_total > 0)),
        current_item: Some("单线程顺序下载".to_string()),
        threads: Some(vec![ThreadVisualization {
            index: 0,
            label: Some("线程 1".to_string()),
            active: true,
            done: thread_done,
            total: thread_total,
            current_item: None,
        }]),
    }
}

fn clear_single_visualization(task_id: &str) {
    set_task_visualization(task_id, None);
}

pub async fn download_file(
    client: reqwest::Client,
    task_control: Arc<TaskControl>,
    task_id: &str,
    url: &str,
    dest: impl AsRef<Path>,
    headers: Option<HeaderMap>,
    md5_expected: Option<&str>,
) -> Result<CoreResult<()>, CoreError> {
    let dest_buf = dest.as_ref().to_path_buf();
    let should_check_zip_header = should_verify_zip_during_download(dest_buf.as_path());
    let mut retry = 0u8;
    let mut resume_from = tokio::fs::metadata(&dest_buf)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    loop {
        if !wait_until_active_fast(task_control.as_ref()).await {
            return Ok(CoreResult::Cancelled);
        }

        debug!(
            "开始单线程下载 (I/O Offload): task={} url={}，重试={}",
            task_id, url, retry
        );

        let mut req_builder = client
            .get(url)
            .timeout(Duration::from_secs(DOWNLOAD_REQUEST_TIMEOUT_SECS));
        if let Some(h) = &headers {
            req_builder = req_builder.headers(h.clone());
        }
        req_builder = apply_download_request_headers(req_builder);

        match req_builder.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && resume_from > 0 {
                    if let Some(expected) = md5_expected
                        .map(str::trim)
                        .filter(|value| is_md5_digest(value))
                    {
                        if !verify_md5(&dest_buf, expected).await.unwrap_or(false) {
                            return Err(CoreError::ChecksumMismatch(format!(
                                "expected {}",
                                expected
                            )));
                        }
                    }
                    set_task_visualization(task_id, None);
                    return Ok(CoreResult::Success(()));
                }

                let resp = resp.error_for_status()?;
                validate_download_response_headers(url, &resp)?;
                let supports_resume =
                    status == reqwest::StatusCode::PARTIAL_CONTENT && resume_from > 0;
                if !supports_resume {
                    resume_from = 0;
                }

                let total = match (supports_resume, resp.content_length()) {
                    (true, Some(remaining)) => Some(remaining.saturating_add(resume_from)),
                    _ => resp.content_length(),
                };
                set_total(&task_id, total);
                if resume_from > 0 {
                    update_progress(&task_id, resume_from, total, Some("downloading"));
                }
                let _ = maybe_set_task_visualization(task_id, || {
                    Some(build_single_download_visualization(resume_from, total))
                });

                let mut open_options = TokioOpenOptions::new();
                open_options.create(true).write(true);
                if supports_resume {
                    open_options.append(true);
                } else {
                    open_options.truncate(true);
                }
                let file = open_options.open(&dest_buf).await.map_err(CoreError::Io)?;
                let mut writer = BufWriter::with_capacity(DISK_BUFFER_SIZE, file);

                let mut stream = resp.bytes_stream();
                let mut downloaded_bytes = resume_from;
                let mut checked_zip_header =
                    !should_check_zip_header || (supports_resume && resume_from > 0);
                let mut last_update = Instant::now();
                let mut pending_progress = 0u64;

                while let Some(item) = stream.next().await {
                    if !wait_until_active_fast(task_control.as_ref()).await {
                        clear_single_visualization(task_id);
                        return Ok(CoreResult::Cancelled);
                    }

                    let chunk = match item {
                        Ok(c) => c,
                        Err(e) => {
                            clear_single_visualization(task_id);
                            return Err(CoreError::Request(e));
                        }
                    };

                    if !checked_zip_header && !chunk.is_empty() {
                        checked_zip_header = true;
                        if !has_zip_header(&chunk) {
                            clear_single_visualization(task_id);
                            return Err(CoreError::ChecksumMismatch(format!(
                                "invalid zip header for {}",
                                dest_buf.display()
                            )));
                        }
                    }

                    let len = chunk.len();
                    writer.write_all(&chunk).await.map_err(CoreError::Io)?;
                    pending_progress += len as u64;
                    downloaded_bytes = downloaded_bytes.saturating_add(len as u64);

                    if last_update.elapsed().as_millis() > 200 {
                        update_progress(task_id, pending_progress, total, Some("downloading"));
                        let _ = maybe_set_task_visualization(task_id, || {
                            Some(build_single_download_visualization(downloaded_bytes, total))
                        });
                        pending_progress = 0;
                        last_update = Instant::now();
                    }

                    if is_cancelled_fast(task_control.as_ref()) {
                        clear_single_visualization(task_id);
                        return Ok(CoreResult::Cancelled);
                    }
                }

                if pending_progress > 0 {
                    update_progress(task_id, pending_progress, total, Some("downloading"));
                    let _ = maybe_set_task_visualization(task_id, || {
                        Some(build_single_download_visualization(downloaded_bytes, total))
                    });
                }

                writer.flush().await.map_err(CoreError::Io)?;
                drop(writer);
                clear_single_visualization(task_id);

                if should_check_zip_header && !checked_zip_header {
                    return Err(CoreError::ChecksumMismatch(format!(
                        "empty zip response for {}",
                        dest_buf.display()
                    )));
                }

                if let Some(expected_total) = total {
                    if downloaded_bytes != expected_total {
                        return Err(CoreError::Other(format!(
                            "download size mismatch: expected {expected_total} bytes, received {downloaded_bytes} bytes"
                        )));
                    }

                    let file_len = tokio::fs::metadata(&dest_buf)
                        .await
                        .map_err(CoreError::Io)?
                        .len();
                    if file_len != expected_total {
                        return Err(CoreError::Other(format!(
                            "download file size mismatch: expected {expected_total} bytes, file has {file_len} bytes"
                        )));
                    }
                }

                if let Some(expected) = md5_expected
                    .map(str::trim)
                    .filter(|value| is_md5_digest(value))
                {
                    update_progress(&task_id, 0, total, Some("verifying"));
                    match verify_md5(&dest_buf, expected).await {
                        Ok(true) => debug!("MD5 OK"),
                        Ok(false) => {
                            return Err(CoreError::ChecksumMismatch(format!(
                                "expected {}",
                                expected
                            )));
                        }
                        Err(e) => {
                            return Err(CoreError::Io(e));
                        }
                    }
                }

                return Ok(CoreResult::Success(()));
            }
            Err(e) if retry < 3 => {
                retry += 1;
                debug!("下载重试 {}/3: {}", retry, e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(e) => {
                return Err(CoreError::Request(e));
            }
        }
    }
}
