use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use futures_util::StreamExt;
use tokio::fs::{OpenOptions, File};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::time::{sleep, Instant, Duration};
use tracing::{debug, info};
use tauri::AppHandle;
use serde_json::json;
use reqwest::header;
use std::io::Write as StdWrite;

use crate::core::downloads::cancel::is_cancelled;
use crate::core::result::{CoreError, CoreResult};
use crate::core::minecraft::utils::{emit_progress, format_eta, format_speed};

/// 优化后的多线程分片下载
pub async fn download_multi(
    client: reqwest::Client,
    url: &str,
    dest: impl AsRef<Path>,
    app: AppHandle,
    threads: usize,
) -> Result<CoreResult<()>, CoreError> {
    debug!("开始多线程下载: url={}, 线程数={}", url, threads);

    // helper: 取消等待 future（每 50ms 轮询 is_cancelled）
    async fn cancelled_future() {
        use tokio::time::sleep;
        use std::time::Duration;
        while !is_cancelled() {
            sleep(Duration::from_millis(50)).await;
        }
    }

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
        // 用 select 包裹以便能快速取消
        let get_fut = client
            .get(url)
            .header(header::RANGE, "bytes=0-0")
            .header(header::ACCEPT_ENCODING, "identity")
            .send();
        tokio::select! {
            _ = cancelled_future() => {
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
        return super::single::download_file(client, url, dest, app).await;
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
            _ = cancelled_future() => {
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
    let downloaded = Arc::new(AtomicU64::new(0));
    let index = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();

    // 5) 进度监控 task（可被 abort）
    let mon_dl = downloaded.clone();
    let mon_app = app.clone();
    let monitor_handle = tokio::spawn(async move {
        while !is_cancelled() && mon_dl.load(Ordering::Relaxed) < total {
            let done = mon_dl.load(Ordering::Relaxed);
            let elapsed = start.elapsed().as_secs_f64();
            let _ = emit_progress(
                &mon_app,
                done,
                Some(total),
                Some(&format_speed(done, elapsed)),
                Some(&format_eta(Some(total), done, elapsed)),
                Some(json!({"stage":"downloading"})),
            ).await;
            sleep(Duration::from_millis(500)).await;
        }
        // 结束前发送一次最终进度
        let done = mon_dl.load(Ordering::Relaxed);
        let elapsed = start.elapsed().as_secs_f64();
        let _ = emit_progress(
            &mon_app,
            done,
            Some(total),
            Some(&format_speed(done, elapsed)),
            Some(&format_eta(Some(total), done, elapsed)),
            Some(json!({"stage": if is_cancelled() { "cancelled" } else { "finished" }})),
        ).await;
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

        handles.push(tokio::spawn(async move {
            debug!("worker {} 启动", worker_id);

            let mut file = match OpenOptions::new().write(true).open(&dest_path).await {
                Ok(f) => f,
                Err(e) => {
                    debug!("worker {} 打开文件失败: {:?}", worker_id, e);
                    return Err(CoreError::Io(e));
                }
            };

            while !is_cancelled() {
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
                loop {
                    if is_cancelled() {
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
                        _ = cancelled_future() => {
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
                                            _ = cancelled_future() => {
                                                debug!("worker {} 在重试等待中检测到取消", worker_id);
                                                return Ok(CoreResult::Cancelled);
                                            }
                                            _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                        }
                                        continue;
                                    } else {
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
                                        _ = cancelled_future() => {
                                            debug!("worker {} 在重试等待中检测到取消", worker_id);
                                            return Ok(CoreResult::Cancelled);
                                        }
                                        _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                    }
                                    continue;
                                } else {
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
                                    _ = cancelled_future() => {
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
                                            if let Err(e) = file.seek(std::io::SeekFrom::Start(write_offset)).await {
                                                debug!("worker {} seek 失败: {:?}", worker_id, e);
                                                return Err(CoreError::Io(e));
                                            }
                                            if let Err(e) = file.write_all(&local_buffer).await {
                                                debug!("worker {} 写入失败: {:?}", worker_id, e);
                                                return Err(CoreError::Io(e));
                                            }
                                            write_offset += local_buffer.len() as u64;
                                            downloaded.fetch_add(local_buffer.len() as u64, Ordering::Relaxed);
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
                                    attempt += 1;
                                    let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                    tokio::select! {
                                        _ = cancelled_future() => {
                                            debug!("worker {} 在重试等待中检测到取消", worker_id);
                                            return Ok(CoreResult::Cancelled);
                                        }
                                        _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                    }
                                    continue;
                                } else {
                                    return Err(CoreError::Other(format!("Partition {} read failed after retries", i)));
                                }
                            }

                            // flush any remaining buffer
                            if !local_buffer.is_empty() {
                                if let Err(e) = file.seek(std::io::SeekFrom::Start(write_offset)).await {
                                    debug!("worker {} seek 失败: {:?}", worker_id, e);
                                    return Err(CoreError::Io(e));
                                }
                                if let Err(e) = file.write_all(&local_buffer).await {
                                    debug!("worker {} 写入失败: {:?}", worker_id, e);
                                    return Err(CoreError::Io(e));
                                }
                                write_offset += local_buffer.len() as u64;
                                downloaded.fetch_add(local_buffer.len() as u64, Ordering::Relaxed);
                                local_buffer.clear();
                            }

                            if received_for_this_partition < expected_len {
                                debug!("worker {} 分片 {}/{} 长度不够: recv={} expected={}", worker_id, i + 1, chunk_count, received_for_this_partition, expected_len);
                                if attempt < MAX_RETRY {
                                    attempt += 1;
                                    let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                    tokio::select! {
                                        _ = cancelled_future() => {
                                            debug!("worker {} 在重试等待中检测到取消", worker_id);
                                            return Ok(CoreResult::Cancelled);
                                        }
                                        _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                    }
                                    continue;
                                } else {
                                    return Err(CoreError::Other(format!("Partition {} incomplete", i)));
                                }
                            }

                            debug!("worker {} 完成分片 {}/{}", worker_id, i + 1, chunk_count);
                            break; // 分片成功，处理下一个分片
                        } // Ok(resp)
                        Err(e) => {
                            if attempt < MAX_RETRY {
                                attempt += 1;
                                debug!("worker {} 分片 {} 网络错误，重试 {}/{}: {:?}", worker_id, i + 1, attempt, MAX_RETRY, e);
                                let backoff_secs = (BASE_RETRY_DELAY_SECS << attempt).min(30);
                                tokio::select! {
                                    _ = cancelled_future() => {
                                        debug!("worker {} 在网络重试等待中检测到取消", worker_id);
                                        return Ok(CoreResult::Cancelled);
                                    }
                                    _ = sleep(Duration::from_secs(backoff_secs)) => {}
                                }
                                continue;
                            } else {
                                debug!("worker {} 分片 {} 最终失败: {:?}", worker_id, i + 1, e);
                                return Err(CoreError::Request(e));
                            }
                        }
                    } // match resp_res
                } // partition retry loop
            } // worker while

            Ok(CoreResult::Success(()))
        }));
    }

    // 等待所有 worker 完成 — 但如果检测到取消，尽快 abort 剩余 tasks 并返回 Cancelled
    let mut failed = false;
    // 使用索引循环，这样可以在检测到取消时中止剩下的 handles
    let mut idx = 0usize;
    while idx < handles.len() {
        let h = handles.remove(0);
        if is_cancelled() {
            debug!("外部取消检测到，立即中止剩余 worker 和 monitor");
            monitor_handle.abort();
            // abort the current and remaining one(s)
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
        idx += 1;
    }

    if is_cancelled() {
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
