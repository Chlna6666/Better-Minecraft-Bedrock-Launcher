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

    // 1) HEAD 尝试获取总长度与快速判断是否支持 Range
    let head = client.head(url).header(header::ACCEPT_ENCODING, "identity").send().await?;
    // 读取 content-length（若不存在，后面会做进一步尝试）
    let maybe_len = head.headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    // 更稳健的接受-range 检查：如果 header 明确包含 bytes 则 OK；否则用小范围请求测试
    let accept_ranges_ok = head.headers()
        .get(header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok().map(|s| s.to_lowercase().contains("bytes")))
        .unwrap_or(false);

    // 如果 header 没有明确支持 Range，尝试使用 GET bytes=0-0 测试（部分服务器不返回 Accept-Ranges）
    let supports_range = if accept_ranges_ok {
        true
    } else {
        debug!("Accept-Ranges header 不明确，尝试小范围 GET 验证");
        let test = client
            .get(url)
            .header(header::RANGE, "bytes=0-0")
            .header(header::ACCEPT_ENCODING, "identity")
            .send()
            .await;
        match test {
            Ok(resp) => {
                // 206 Partial Content 表示支持 Range
                resp.status() == reqwest::StatusCode::PARTIAL_CONTENT
            }
            Err(_) => false,
        }
    };

    // 如果不支持 回退单线程实现（保持原行为）
    if !supports_range {
        debug!("服务器不支持分片下载，回退单线程");
        return super::single::download_file(client, url, dest, app).await;
    }

    // 如果 head 没给 content-length，再尝试用 GET (不请求全部，只请求 Content-Range)
    let total = if let Some(len) = maybe_len {
        len
    } else {
        debug!("HEAD 未提供 Content-Length，尝试通过范围请求获取总长度");
        let resp = client
            .get(url)
            .header(header::RANGE, "bytes=0-0")
            .header(header::ACCEPT_ENCODING, "identity")
            .send()
            .await?;
        // Content-Range: bytes 0-0/12345
        if let Some(cr) = resp.headers().get(header::CONTENT_RANGE) {
            let s = cr.to_str().ok().unwrap_or_default();
            // 解析末尾的总长度
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
    // 安全地把 chunk_count 转为 usize（通常在 64-bit 环境足够）
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
            Some(json!({"stage":"finished"})),
        ).await;
    });

    // 6) 启动 worker tasks（每个 worker 打开自己的文件句柄，避免全局 Mutex）
    let client = Arc::new(client);
    let url = Arc::new(url.to_string());
    let mut handles = Vec::with_capacity(threads);

    // 常量：写缓冲阈值（64 KiB）和每分片最大重试次数与重试回退
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
            // 每个 worker 打开自己的异步文件句柄用于写入（重用）
            let mut file = match OpenOptions::new().write(true).open(&dest_path).await {
                Ok(f) => f,
                Err(e) => {
                    debug!("worker {} 打开文件失败: {:?}", worker_id, e);
                    return Err(CoreError::Io(e));
                }
            };

            // worker 循环从全局索引获取任务
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

                    let resp_res = req.send().await;

                    match resp_res {
                        Ok(rsp) => {
                            // 检查状态
                            let rsp = match rsp.error_for_status() {
                                Ok(r) => r,
                                Err(e) => {
                                    debug!("worker {} response 状态错误: {:?}, 尝试重试", worker_id, e);
                                    if attempt < MAX_RETRY {
                                        attempt += 1;
                                        let backoff = BASE_RETRY_DELAY_SECS.saturating_pow(attempt as u32);
                                        sleep(Duration::from_secs(backoff)).await;
                                        continue;
                                    } else {
                                        return Err(CoreError::Request(e));
                                    }
                                }
                            };

                            // 206 是正常；某些服务器可能返回 200 （整体返回），在这种情况下我们也要能处理，但需谨慎
                            let status = rsp.status();
                            if status != reqwest::StatusCode::PARTIAL_CONTENT && status != reqwest::StatusCode::OK {
                                debug!("worker {} 非预期状态: {}", worker_id, status);
                                if attempt < MAX_RETRY {
                                    attempt += 1;
                                    sleep(Duration::from_secs(BASE_RETRY_DELAY_SECS * (attempt as u64))).await;
                                    continue;
                                } else {
                                    return Err(CoreError::Other(format!("Unexpected status {}", status)));
                                }
                            }

                            // 读取流并缓冲写入；若流中出现错误 -> 触发本分片重试
                            let mut stream = rsp.bytes_stream();
                            let mut write_offset = start_byte;
                            let mut local_buffer = Vec::with_capacity(BUFFER_FLUSH_THRESHOLD);
                            let mut received_for_this_partition: u64 = 0;
                            let mut stream_error = false;

                            while let Some(chunk_res) = stream.next().await {
                                if is_cancelled() {
                                    debug!("worker {} 在流处理时被取消", worker_id);
                                    return Ok(CoreResult::Cancelled);
                                }
                                let chunk = match chunk_res {
                                    Ok(c) => c,
                                    Err(e) => {
                                        debug!("worker {} 读取 chunk 失败: {:?}", worker_id, e);
                                        stream_error = true;
                                        break;
                                    }
                                };
                                // 累积到本地 buffer
                                local_buffer.extend_from_slice(&chunk);
                                received_for_this_partition += chunk.len() as u64;

                                // 如果缓冲超过阈值，则一次性写入
                                if local_buffer.len() >= BUFFER_FLUSH_THRESHOLD {
                                    // seek + write
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
                            } // stream loop

                            if stream_error {
                                // 流错误 -> 触发本分片重试（删除/回退已写入的部分由覆盖解决）
                                debug!("worker {} 分片读取中断，将重试分片 {}/{}", worker_id, i + 1, chunk_count);
                                if attempt < MAX_RETRY {
                                    attempt += 1;
                                    let backoff = BASE_RETRY_DELAY_SECS.saturating_pow(attempt as u32);
                                    sleep(Duration::from_secs(backoff)).await;
                                    // 注意：已写入的字节会被后续覆盖（因为我们针对相同偏移写入），无需显式回滚
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

                            // 检查该分片是否收到了足够字节（有些服务器可能返回小于请求长度的分片）
                            if received_for_this_partition < expected_len {
                                debug!("worker {} 分片 {}/{} 长度不够: recv={} expected={}", worker_id, i + 1, chunk_count, received_for_this_partition, expected_len);
                                if attempt < MAX_RETRY {
                                    attempt += 1;
                                    sleep(Duration::from_secs(BASE_RETRY_DELAY_SECS * (attempt as u64))).await;
                                    continue;
                                } else {
                                    return Err(CoreError::Other(format!("Partition {} incomplete", i)));
                                }
                            }

                            debug!("worker {} 完成分片 {}/{}", worker_id, i + 1, chunk_count);
                            break; // 分片成功，处理下一个分片
                        } // Ok(resp)
                        Err(e) => {
                            // 网络错误：重试若干次
                            if attempt < MAX_RETRY {
                                attempt += 1;
                                debug!("worker {} 分片 {} 网络错误，重试 {}/{}: {:?}", worker_id, i + 1, attempt, MAX_RETRY, e);
                                let backoff = BASE_RETRY_DELAY_SECS.saturating_pow(attempt as u32);
                                sleep(Duration::from_secs(backoff)).await;
                                continue;
                            } else {
                                debug!("worker {} 分片 {} 最终失败: {:?}", worker_id, i + 1, e);
                                return Err(CoreError::Request(e));
                            }
                        }
                    } // match resp_res
                } // partition retry loop
            } // worker while

            // worker 正常退出（所有分片完成）
            Ok(CoreResult::Success(()))
        })); // spawn
    }

    // 等待所有 worker 完成
    let mut failed = false;
    for h in handles {
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
                // worker 内部返回 Err(CoreError)
                debug!("worker 返回错误: {:?}", e);
                failed = true;
            }
            Err(e) => {
                // task join error
                debug!("worker task join 错误: {:?}", e);
                failed = true;
            }
        }
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
        // 删除残留并报错
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