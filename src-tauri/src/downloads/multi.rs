use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use futures_util::StreamExt;
use tokio::fs::{OpenOptions, File};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::time::{sleep, Duration};
use tracing::debug;
use serde_json::json;
use reqwest::header;
use std::io::Write as StdWrite;

use crate::downloads::cancel::is_cancelled;
use crate::downloads::md5 as md5_utils;
use crate::result::{CoreError, CoreResult};
use crate::progress::download_progress::{report_progress, DownloadProgress};

/// 优化后的多线程分片下载，支持 md5 校验与 per-task cancel_flag
pub async fn download_multi(
    client: reqwest::Client,
    url: &str,
    dest: impl AsRef<Path>,
    threads: usize,
    md5_expected: Option<&str>,
    cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<CoreResult<()>, CoreError> {
    debug!("开始多线程下载: url={}, 线程数={}, md5={:?}", url, threads, md5_expected);

    // 1) HEAD 尝试获取总长度与快速判断是否支持 Range
    let head = client.head(url)
        .header(header::ACCEPT_ENCODING, "identity")
        .send()
        .await?;
    let maybe_len = head.headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let accept_ranges_ok = head.headers()
        .get(header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok().map(|s| s.to_lowercase().contains("bytes")))
        .unwrap_or(false);

    let supports_range = if accept_ranges_ok {
        true
    } else {
        debug!("Accept-Ranges header 不明确，尝试小范围 GET 验证");
        let get_fut = client
            .get(url)
            .header(header::RANGE, "bytes=0-0")
            .header(header::ACCEPT_ENCODING, "identity")
            .send();
        tokio::select! {
            // 每次创建新的取消等待 future（捕获 clone）
            _ = {
                let cancel_check = cancel_flag.clone();
                async move {
                    while !is_cancelled(cancel_check.as_ref()) {
                        sleep(Duration::from_millis(50)).await;
                    }
                }
            } => {
                debug!("取消检测到：在 range 检查阶段退出");
                return Ok(CoreResult::Cancelled);
            }
            res = get_fut => {
                match res {
                    Ok(resp) => resp.status() == reqwest::StatusCode::PARTIAL_CONTENT,
                    Err(_) => false,
                }
            }
        }
    };

    if !supports_range {
        debug!("服务器不支持分片下载，回退单线程");
        // 调用 single::download_file 时也把 md5_expected 与 cancel_flag 传入（single 需相应支持）
        return super::single::download_file(client, url, dest, md5_expected, cancel_flag).await;
    }

    // 若 head 没给 content-length，再尝试用 GET bytes=0-0 获取 total
    let total = if let Some(len) = maybe_len {
        len
    } else {
        debug!("HEAD 未提供 Content-Length，尝试通过范围请求获取总长度");
        let get_fut = client
            .get(url)
            .header(header::RANGE, "bytes=0-0")
            .header(header::ACCEPT_ENCODING, "identity")
            .send();
        let resp = tokio::select! {
            _ = {
                let cancel_check = cancel_flag.clone();
                async move {
                    while !is_cancelled(cancel_check.as_ref()) {
                        sleep(Duration::from_millis(50)).await;
                    }
                }
            } => {
                debug!("取消检测到：在获取 Content-Range 阶段退出");
                return Ok(CoreResult::Cancelled);
            }
            r = get_fut => r?,
        };
        if let Some(cr) = resp.headers().get(header::CONTENT_RANGE) {
            let s = cr.to_str().ok().unwrap_or_default();
            if let Some(idx) = s.rfind('/') {
                if let Ok(len) = s[idx+1..].parse::<u64>() {
                    len
                } else {
                    return Err(CoreError::UnknownContentLength);
                }
            } else {
                return Err(CoreError::UnknownContentLength);
            }
        } else {
            return Err(CoreError::UnknownContentLength);
        }
    };
    debug!("总文件大小: {} 字节", total);

    // 2) 预分配文件长度（使用 std::fs）
    {
        use std::fs::OpenOptions as StdOpen;
        let p = dest.as_ref();
        let mut stdf = StdOpen::new()
            .create(true)
            .write(true)
            .open(p)?;
        stdf.set_len(total)?;
        stdf.flush()?;
    }
    debug!("已预分配文件: {} 字节", total);

    // 3) 计算 partition（细粒度以缓解长尾）
    const SPLIT_FACTOR: u64 = 8;
    let min_chunk: u64 = 1 * 1024 * 1024; // 最小 1 MiB
    let estimated_chunk = (total / (threads as u64 * SPLIT_FACTOR)).max(min_chunk);
    let chunk_size = estimated_chunk;
    let chunk_count_u64 = (total + chunk_size - 1) / chunk_size;
    if chunk_count_u64 == 0 {
        return Err(CoreError::Other("计算分片失败: chunk_count == 0".into()));
    }
    let chunk_count = usize::try_from(chunk_count_u64).map_err(|_| CoreError::Other("分片数过大".into()))?;
    debug!("分片大小: {} 字节, 分片数量: {}", chunk_size, chunk_count);

    // 4) 共享进度与索引
    let progress = DownloadProgress::new(total);
    let downloaded = progress.downloaded_arc(); // Arc<AtomicU64>
    let index = Arc::new(AtomicUsize::new(0));

    // 5) 进度监控 task（可被 abort）
    let mut mon_progress = progress; // monitor 持有 progress，用于 mut 更新 prev 等
    let cancel_flag_for_monitor = cancel_flag.clone();
    let monitor_handle = tokio::spawn(async move {
        loop {
            // 先检查取消标志（使用传入的 cancel_flag）
            if is_cancelled(cancel_flag_for_monitor.as_ref()) {
                break;
            }

            // 读取当前已下载字节
            let done = mon_progress.downloaded.load(Ordering::Relaxed);

            // 如果 total > 0 并且完成了，则退出循环（会在循环外发送最终进度）
            if mon_progress.total > 0 && done >= mon_progress.total {
                break;
            }

            // 若达到发送节流条件则发送（report_progress 会负责 mark_emitted / 更新 prev）
            if mon_progress.should_emit() {
                let _ = report_progress(&mut mon_progress, json!({"stage": "downloading"})).await;
            }

            sleep(Duration::from_millis(200)).await;
        }

        // 循环结束：发送一次最终状态（cancelled 或 finished）
        let final_stage = if is_cancelled(cancel_flag_for_monitor.as_ref()) { json!("cancelled") } else { json!("finished") };
        let _ = report_progress(&mut mon_progress, final_stage).await;
    });

    // 6) 启动 worker tasks（每个 worker 打开自己的文件句柄，避免全局 Mutex）
    let client = Arc::new(client);
    let url = Arc::new(url.to_string());
    let mut handles = Vec::with_capacity(threads);

    const BUFFER_FLUSH_THRESHOLD: usize = 64 * 1024;
    const MAX_RETRY: usize = 4;
    const BASE_RETRY_DELAY_SECS: u64 = 1;

    for worker_id in 0..threads {
        let client = client.clone();
        let url = url.clone();
        let downloaded = downloaded.clone();
        let index = index.clone();
        let dest_path = dest.as_ref().to_owned();
        let cancel_flag_worker = cancel_flag.clone();

        handles.push(tokio::spawn(async move {
            debug!("worker {} 启动", worker_id);

            let mut file = match OpenOptions::new().write(true).open(&dest_path).await {
                Ok(f) => f,
                Err(e) => {
                    debug!("worker {} 打开文件失败: {:?}", worker_id, e);
                    return Err(CoreError::Io(e));
                }
            };

            while !is_cancelled(cancel_flag_worker.as_ref()) {
                let i = index.fetch_add(1, Ordering::Relaxed);
                if i >= chunk_count {
                    debug!("worker {} 没有更多分片，退出", worker_id);
                    break;
                }
                let start_byte = i as u64 * chunk_size;
                let end_byte = (start_byte + chunk_size - 1).min(total - 1);
                let expected_len = end_byte - start_byte + 1;
                debug!(
                    "worker {} 处理分片 {}/{} ({:?}-{:?}) len={}",
                    worker_id,
                    i + 1,
                    chunk_count,
                    start_byte,
                    end_byte,
                    expected_len
                );

                // partition 级别重试
                let mut attempt = 0usize;
                // partition_committed 用于记录已计入全局 downloaded 的字节（针对本分片）
                let mut partition_committed: u64 = 0;

                loop {
                    if is_cancelled(cancel_flag_worker.as_ref()) {
                        debug!("worker {} 检测到取消", worker_id);
                        return Ok(CoreResult::Cancelled);
                    }

                    let range_header = format!("bytes={}-{}", start_byte, end_byte);
                    let req = client
                        .get(&*url)
                        .header(header::RANGE, range_header.clone())
                        .header(header::ACCEPT_ENCODING, "identity");

                    // 使用 select 包裹 send()，使得可以快速响应取消
                    let send_fut = req.send();
                    let resp_res = tokio::select! {
                        _ = {
                            let cancel_check = cancel_flag_worker.clone();
                            async move {
                                while !is_cancelled(cancel_check.as_ref()) {
                                    sleep(Duration::from_millis(50)).await;
                                }
                            }
                        } => {
                            debug!("worker {} 在 send 前检测到取消", worker_id);
                            return Ok(CoreResult::Cancelled);
                        }
                        r = send_fut => r
                    };

                    match resp_res {
                        Ok(rsp) => {
                            let rsp = match rsp.error_for_status() {
                                Ok(r) => r,
                                Err(e) => {
                                    debug!("worker {} response 状态错误: {:?}, 尝试重试", worker_id, e);
                                    if attempt < MAX_RETRY {
                                        attempt += 1;
                                        let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                        tokio::select! {
                                            _ = {
                                                let cancel_check = cancel_flag_worker.clone();
                                                async move {
                                                    while !is_cancelled(cancel_check.as_ref()) {
                                                        sleep(Duration::from_millis(50)).await;
                                                    }
                                                }
                                            } => {
                                                debug!("worker {} 在重试等待中检测到取消", worker_id);
                                                return Ok(CoreResult::Cancelled);
                                            }
                                            _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                        }
                                        continue;
                                    } else {
                                        if partition_committed > 0 {
                                            downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                            partition_committed = 0;
                                        }
                                        return Err(CoreError::Request(e));
                                    }
                                }
                            };

                            let status = rsp.status();
                            if status != reqwest::StatusCode::PARTIAL_CONTENT && status != reqwest::StatusCode::OK {
                                debug!("worker {} 非预期状态: {}", worker_id, status);
                                if attempt < MAX_RETRY {
                                    attempt += 1;
                                    let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                    tokio::select! {
                                        _ = {
                                            let cancel_check = cancel_flag_worker.clone();
                                            async move {
                                                while !is_cancelled(cancel_check.as_ref()) {
                                                    sleep(Duration::from_millis(50)).await;
                                                }
                                            }
                                        } => {
                                            debug!("worker {} 在重试等待中检测到取消", worker_id);
                                            return Ok(CoreResult::Cancelled);
                                        }
                                        _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                    }
                                    continue;
                                } else {
                                    if partition_committed > 0 {
                                        downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                        partition_committed = 0;
                                    }
                                    return Err(CoreError::Other(format!("Unexpected status {}", status)));
                                }
                            }

                            let mut stream = rsp.bytes_stream();
                            let mut write_offset = start_byte;
                            let mut local_buffer = Vec::with_capacity(BUFFER_FLUSH_THRESHOLD);
                            let mut received_for_this_partition: u64 = 0;
                            let mut stream_error = false;

                            loop {
                                // 在读取流时也要可取消
                                let next_chunk_opt = tokio::select! {
                                    _ = {
                                        let cancel_check = cancel_flag_worker.clone();
                                        async move {
                                            while !is_cancelled(cancel_check.as_ref()) {
                                                sleep(Duration::from_millis(50)).await;
                                            }
                                        }
                                    } => {
                                        debug!("worker {} 在读取流时检测到取消", worker_id);
                                        return Ok(CoreResult::Cancelled);
                                    }
                                    c = stream.next() => c
                                };

                                match next_chunk_opt {
                                    Some(chunk_res) => {
                                        let chunk = match chunk_res {
                                            Ok(c) => c,
                                            Err(e) => {
                                                debug!("worker {} 读取 chunk 失败: {:?}", worker_id, e);
                                                stream_error = true;
                                                break;
                                            }
                                        };
                                        local_buffer.extend_from_slice(&chunk);
                                        received_for_this_partition += chunk.len() as u64;

                                        if local_buffer.len() >= BUFFER_FLUSH_THRESHOLD {
                                            // 写入磁盘并同时更新全局计数与本分片计数
                                            if let Err(e) = file.seek(std::io::SeekFrom::Start(write_offset)).await {
                                                debug!("worker {} seek 失败: {:?}", worker_id, e);
                                                if partition_committed > 0 {
                                                    downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                                    partition_committed = 0;
                                                }
                                                return Err(CoreError::Io(e));
                                            }
                                            if let Err(e) = file.write_all(&local_buffer).await {
                                                debug!("worker {} 写入失败: {:?}", worker_id, e);
                                                if partition_committed > 0 {
                                                    downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                                    partition_committed = 0;
                                                }
                                                return Err(CoreError::Io(e));
                                            }
                                            write_offset += local_buffer.len() as u64;

                                            // 更新计数：把此次写入的字节既加入全局计数，也加入 partition_committed
                                            downloaded.fetch_add(local_buffer.len() as u64, Ordering::Relaxed);
                                            partition_committed += local_buffer.len() as u64;

                                            local_buffer.clear();
                                        }
                                    }
                                    None => {
                                        // 流结束
                                        break;
                                    }
                                }
                            } // stream loop

                            if stream_error {
                                debug!("worker {} 分片读取中断，将重试分片 {}/{}", worker_id, i + 1, chunk_count);
                                if attempt < MAX_RETRY {
                                    if partition_committed > 0 {
                                        debug!("worker {} 在重试前回退已计入字节: {}", worker_id, partition_committed);
                                        downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                        partition_committed = 0;
                                    }
                                    attempt += 1;
                                    let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                    tokio::select! {
                                        _ = {
                                            let cancel_check = cancel_flag_worker.clone();
                                            async move {
                                                while !is_cancelled(cancel_check.as_ref()) {
                                                    sleep(Duration::from_millis(50)).await;
                                                }
                                            }
                                        } => {
                                            debug!("worker {} 在重试等待中检测到取消", worker_id);
                                            return Ok(CoreResult::Cancelled);
                                        }
                                        _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                    }
                                    continue;
                                } else {
                                    if partition_committed > 0 {
                                        downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                        partition_committed = 0;
                                    }
                                    return Err(CoreError::Other(format!("Partition {} read failed after retries", i)));
                                }
                            }

                            // flush any remaining buffer
                            if !local_buffer.is_empty() {
                                if let Err(e) = file.seek(std::io::SeekFrom::Start(write_offset)).await {
                                    debug!("worker {} seek 失败: {:?}", worker_id, e);
                                    if partition_committed > 0 {
                                        downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                        partition_committed = 0;
                                    }
                                    return Err(CoreError::Io(e));
                                }
                                if let Err(e) = file.write_all(&local_buffer).await {
                                    debug!("worker {} 写入失败: {:?}", worker_id, e);
                                    if partition_committed > 0 {
                                        downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                        partition_committed = 0;
                                    }
                                    return Err(CoreError::Io(e));
                                }
                                write_offset += local_buffer.len() as u64;

                                // 更新计数（此次 flush 写入）
                                downloaded.fetch_add(local_buffer.len() as u64, Ordering::Relaxed);
                                partition_committed += local_buffer.len() as u64;
                                local_buffer.clear();
                            }

                            if received_for_this_partition < expected_len {
                                debug!("worker {} 分片 {}/{} 长度不够: recv={} expected={}", worker_id, i + 1, chunk_count, received_for_this_partition, expected_len);
                                if attempt < MAX_RETRY {
                                    if partition_committed > 0 {
                                        downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                        partition_committed = 0;
                                    }
                                    attempt += 1;
                                    let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                    tokio::select! {
                                        _ = {
                                            let cancel_check = cancel_flag_worker.clone();
                                            async move {
                                                while !is_cancelled(cancel_check.as_ref()) {
                                                    sleep(Duration::from_millis(50)).await;
                                                }
                                            }
                                        } => {
                                            debug!("worker {} 在重试等待中检测到取消", worker_id);
                                            return Ok(CoreResult::Cancelled);
                                        }
                                        _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                    }
                                    continue;
                                } else {
                                    if partition_committed > 0 {
                                        downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                        partition_committed = 0;
                                    }
                                    return Err(CoreError::Other(format!("Partition {} incomplete", i)));
                                }
                            }

                            debug!("worker {} 完成分片 {}/{}", worker_id, i + 1, chunk_count);
                            // 分片成功（partition_committed 已经包含了本分片所有写入并计入 global downloaded）
                            break; // 分片成功，处理下一个分片
                        } // Ok(resp)
                        Err(e) => {
                            if attempt < MAX_RETRY {
                                attempt += 1;
                                debug!("worker {} 分片 {} 网络错误，重试 {}/{}: {:?}", worker_id, i + 1, attempt, MAX_RETRY, e);
                                let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                tokio::select! {
                                    _ = {
                                        let cancel_check = cancel_flag_worker.clone();
                                        async move {
                                            while !is_cancelled(cancel_check.as_ref()) {
                                                sleep(Duration::from_millis(50)).await;
                                            }
                                        }
                                    } => {
                                        debug!("worker {} 在网络重试等待中检测到取消", worker_id);
                                        return Ok(CoreResult::Cancelled);
                                    }
                                    _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                }
                                continue;
                            } else {
                                debug!("worker {} 分片 {} 最终失败: {:?}", worker_id, i + 1, e);
                                if partition_committed > 0 {
                                    downloaded.fetch_sub(partition_committed, Ordering::Relaxed);
                                    partition_committed = 0;
                                }
                                return Err(CoreError::Request(e));
                            }
                        }
                    } // match resp_res
                } // partition retry loop
            } // worker while

            Ok(CoreResult::Success(()))
        } ));
    }

    // 等待所有 worker 完成 — 但如果检测到取消，尽快 abort 剩余 tasks 并返回 Cancelled
    let mut failed = false;
    while let Some(h) = handles.pop() {
        if is_cancelled(cancel_flag.as_ref()) {
            debug!("外部取消检测到，立即中止剩余 worker 和 monitor");
            monitor_handle.abort();
            h.abort();
            for remaining in handles.into_iter() {
                remaining.abort();
            }
            return Ok(CoreResult::Cancelled);
        }

        match h.await {
            Ok(Ok(CoreResult::Success(_))) => { /* 单个 worker 成功退出 */ },
            Ok(Ok(CoreResult::Cancelled)) => {
                debug!("下载被取消，清理并返回");
                monitor_handle.abort();
                return Ok(CoreResult::Cancelled);
            }
            Ok(Ok(CoreResult::Error(e))) => {
                debug!("worker 返回 CoreResult::Error: {:?}", e);
                failed = true;
            }
            Ok(Err(e)) => {
                debug!("worker 返回错误: {:?}", e);
                failed = true;
            }
            Err(e) => {
                debug!("worker task join 错误: {:?}", e);
                failed = true;
            }
        }
    }

    if is_cancelled(cancel_flag.as_ref()) {
        debug!("外部取消检测到，退出");
        monitor_handle.abort();
        return Ok(CoreResult::Cancelled);
    }

    if failed {
        debug!("部分分片失败，删除残留文件");
        monitor_handle.abort();
        let _ = tokio::fs::remove_file(dest.as_ref()).await;
        return Err(CoreError::Other("多线程下载失败".into()));
    }

    // 最后一次完整性检查：确保下载字节等于 total
    let done = downloaded.load(Ordering::Relaxed);
    if done != total {
        debug!("下载完成但字节数不一致: done={} total={}", done, total);
        monitor_handle.abort();
        let _ = tokio::fs::remove_file(dest.as_ref()).await;
        return Err(CoreError::Other(format!("下载大小校验失败: {} != {}", done, total)));
    }

    // 如果用户提供了 md5_expected，则进行最终 md5 校验
    if let Some(expected) = md5_expected {
        debug!("开始 md5 校验: expect={}", expected);
        match md5_utils::verify_md5(dest.as_ref(), expected).await {
            Ok(true) => {
                debug!("md5 校验通过");
            }
            Ok(false) => {
                debug!("md5 校验失败，删除目标文件");
                monitor_handle.abort();
                let _ = tokio::fs::remove_file(dest.as_ref()).await;
                return Err(CoreError::ChecksumMismatch(format!("md5 mismatch for {:?}", dest.as_ref())));
            }
            Err(e) => {
                debug!("md5 计算失败: {:?}", e);
                monitor_handle.abort();
                let _ = tokio::fs::remove_file(dest.as_ref()).await;
                return Err(CoreError::Io(e));
            }
        }
    }

    // 执行 fsync 确保所有写操作落盘
    if let Ok(final_file) = File::open(dest.as_ref()).await {
        if let Err(e) = final_file.sync_all().await {
            debug!("fsync 失败: {:?}", e);
            // 不致命，但提醒
        }
    }

    monitor_handle.abort();
    debug!("所有工作线程完成，下载成功");
    Ok(CoreResult::Success(()))
}
