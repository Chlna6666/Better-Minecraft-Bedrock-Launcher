// src/downloads/multi.rs
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::header::{self, HeaderMap};
use std::cmp::Ordering as CmpOrdering;
use std::fs::OpenOptions;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::{sleep, Duration};
use tracing::{debug, error, warn};

use crate::downloads::md5 as md5_utils;
use crate::downloads::single::download_file;
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{finish_task, is_cancelled, set_total, update_progress};

// =========================================================================
// 性能配置常量 (针对 2.5G+ 宽带优化)
// =========================================================================

// 写入通道容量：控制在 64 个，防止内存积压过多
// 内存占用估算：64 * 1MB = 64MB + Worker Buffers
const WRITE_CHANNEL_SIZE: usize = 64;

// [核心优化] Worker 内部聚合阈值提升至 1MB
// 减少 Channel 锁竞争和 CPU 上下文切换，大幅降低高带宽下的 CPU 占用
const WORKER_BATCH_SIZE: usize = 1024 * 1024;

// 磁盘写缓冲：8MB，确保大块连续写入，减少磁盘磁头寻道
const DISK_BUFFER_SIZE: usize = 8 * 1024 * 1024;

// 最小分片 10MB，避免碎片化
const MIN_CHUNK_SIZE: u64 = 10 * 1024 * 1024;
// 最小窃取大小
const MIN_STEAL_SIZE: u64 = 2 * 1024 * 1024;

// =========================================================================
// 消息定义
// =========================================================================

enum WriterMsg {
    /// (文件偏移量, 数据块)
    Write(u64, Vec<Bytes>),
    Close,
}

// =========================================================================
// 任务管理结构
// =========================================================================

#[derive(Debug)]
struct ChunkSlot {
    id: usize,
    start: u64,
    end: AtomicU64,
    current: AtomicU64,
    is_active: AtomicBool,
    is_done: AtomicBool,
}

struct AdaptiveTaskManager {
    slots: Mutex<Vec<Arc<ChunkSlot>>>,
}

impl AdaptiveTaskManager {
    fn new() -> Self {
        Self { slots: Mutex::new(Vec::new()) }
    }

    async fn add_tasks(&self, chunks: Vec<(u64, u64)>) {
        let mut guard = self.slots.lock().await;
        for (i, (s, e)) in chunks.into_iter().enumerate() {
            guard.push(Arc::new(ChunkSlot {
                id: i,
                start: s,
                end: AtomicU64::new(e),
                current: AtomicU64::new(s),
                is_active: AtomicBool::new(false),
                is_done: AtomicBool::new(false),
            }));
        }
    }

    async fn get_next_task(&self, worker_id: usize) -> Option<Arc<ChunkSlot>> {
        // 1. 领取常规任务
        {
            let guard = self.slots.lock().await;
            for slot in guard.iter() {
                if !slot.is_active.load(Ordering::Relaxed) && !slot.is_done.load(Ordering::Relaxed) {
                    slot.is_active.store(true, Ordering::Relaxed);
                    return Some(slot.clone());
                }
            }
        }
        // 2. 窃取任务
        self.steal_task(worker_id).await
    }

    async fn steal_task(&self, worker_id: usize) -> Option<Arc<ChunkSlot>> {
        let mut guard = self.slots.lock().await;
        if guard.iter().all(|s| s.is_done.load(Ordering::Relaxed)) {
            return None;
        }

        let mut best_victim = None;
        let mut max_rem = 0;

        for (idx, slot) in guard.iter().enumerate() {
            if slot.is_active.load(Ordering::Relaxed) && !slot.is_done.load(Ordering::Relaxed) {
                let curr = slot.current.load(Ordering::Relaxed);
                let end = slot.end.load(Ordering::Relaxed);
                if end > curr {
                    let rem = end - curr;
                    if rem > max_rem {
                        max_rem = rem;
                        best_victim = Some(idx);
                    }
                }
            }
        }

        if max_rem < MIN_STEAL_SIZE || best_victim.is_none() {
            return None;
        }

        let victim = guard[best_victim.unwrap()].clone();
        let v_curr = victim.current.load(Ordering::Relaxed);
        let v_end = victim.end.load(Ordering::Relaxed);
        let remaining = v_end.saturating_sub(v_curr);

        if remaining < MIN_STEAL_SIZE {
            return None;
        }

        let mid = v_curr + remaining / 2;
        victim.end.store(mid, Ordering::Relaxed);

        let new_slot = Arc::new(ChunkSlot {
            id: guard.len(),
            start: mid + 1,
            end: AtomicU64::new(v_end),
            current: AtomicU64::new(mid + 1),
            is_active: AtomicBool::new(true),
            is_done: AtomicBool::new(false),
        });

        debug!("Worker {} 窃取任务 #{} -> 新任务 #{}", worker_id, victim.id, new_slot.id);
        guard.push(new_slot.clone());
        Some(new_slot)
    }
}

// =========================================================================
// 主逻辑
// =========================================================================

pub async fn download_multi(
    client: reqwest::Client,
    task_id: &str,
    url: &str,
    dest: impl AsRef<Path>,
    threads: usize,
    headers: Option<HeaderMap>,
    md5_expected: Option<&str>,
) -> Result<CoreResult<()>, CoreError> {
    let task_id_owned = task_id.to_string();
    debug!("启动极致性能多线程下载: task={} threads={}", task_id, threads);

    // 1. 获取文件大小 + 解析最终 URL（必须！某些 CDN 对带 Range 的请求不返回 302，而是直接 404）
    //    例如 edge.forgecdn.net 在带 Range 时可能直接 404，但正常请求会 302 到 mediafilez.forgecdn.net。
    let mut effective_url: String = url.to_string();
    let mut head_req = client.head(url).header(header::ACCEPT_ENCODING, "identity");
    if let Some(h) = &headers { head_req = head_req.headers(h.clone()); }

    let total = match head_req.send().await {
        Ok(resp) => {
            effective_url = resp.url().to_string();
            resp.headers().get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        },
        Err(_) => 0,
    };

    if total == 0 {
        return download_file(client, task_id, url, dest, headers, md5_expected).await;
    }
    set_total(task_id, Some(total));

    // 2. 启动独立 Writer 线程
    let dest_buf = dest.as_ref().to_path_buf();
    let (tx, mut rx) = mpsc::channel::<WriterMsg>(WRITE_CHANNEL_SIZE);

    let writer_thread = thread::spawn(move || -> Result<(), std::io::Error> {
        if let Some(parent) = dest_buf.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&dest_buf)?;

        // [核心优化] 文件预分配
        if let Err(e) = file.set_len(total) {
            warn!("文件预分配失败（可能是磁盘空间不足或文件系统不支持）: {}", e);
        }

        // 8MB 缓冲区，配合 Smart Seek
        let mut writer = BufWriter::with_capacity(DISK_BUFFER_SIZE, file);
        let mut current_pos = 0u64; // 虚拟游标

        while let Some(msg) = rx.blocking_recv() {
            match msg {
                WriterMsg::Write(offset, batch) => {
                    if current_pos != offset {
                        writer.seek(SeekFrom::Start(offset))?;
                        current_pos = offset;
                    }
                    for chunk in batch {
                        writer.write_all(&chunk)?;
                        current_pos += chunk.len() as u64;
                    }
                }
                WriterMsg::Close => break,
            }
        }
        writer.flush()?;
        Ok(())
    });

    // 3. 任务分配
    let initial_chunk_size = (total / threads as u64).max(MIN_CHUNK_SIZE);
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < total {
        let end = (offset + initial_chunk_size - 1).min(total - 1);
        chunks.push((offset, end));
        offset = end + 1;
    }

    let manager = Arc::new(AdaptiveTaskManager::new());
    manager.add_tasks(chunks.clone()).await;

    let active_threads = threads.min(chunks.len()).max(1);

    let downloaded_global = Arc::new(AtomicU64::new(0));
    let client = Arc::new(client);
    let url = Arc::new(url.to_string());
    let error_occurred = Arc::new(Notify::new());
    let error_store = Arc::new(Mutex::new(None));

    // 4. 启动 Workers
    let mut handles = Vec::with_capacity(active_threads);

    for worker_id in 0..active_threads {
        let manager = manager.clone();
        let client = client.clone();
        let url = effective_url.clone();
        let downloaded_global = downloaded_global.clone();
        let task_id = task_id.to_string();
        let error_occurred = error_occurred.clone();
        let error_store = error_store.clone();
        let headers = headers.clone();
        let tx = tx.clone();

        handles.push(tokio::spawn(async move {
            let mut pending_progress = 0u64;
            let mut last_update_time = Instant::now();

            loop {
                if is_cancelled(&task_id) || error_store.lock().await.is_some() { return; }

                let slot = match manager.get_next_task(worker_id).await {
                    Some(s) => s,
                    None => {
                        let all_done = manager.slots.lock().await.iter().all(|s| s.is_done.load(Ordering::Relaxed));
                        if all_done { break; }
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };

                let mut attempts = 0;
                let mut success = false;

                'retry: while attempts < 10 {
                    if is_cancelled(&task_id) { return; }

                    let mut batch_buffer: Vec<Bytes> = Vec::with_capacity(16);
                    let mut batch_size = 0;

                    let start = slot.current.load(Ordering::Relaxed);
                    let end = slot.end.load(Ordering::Relaxed);

                    if start > end {
                        success = true;
                        break 'retry;
                    }

                    if attempts > 0 { sleep(Duration::from_millis(500 * attempts as u64)).await; }

                    let mut req = client.get(url.as_str())
                        .header(header::RANGE, format!("bytes={}-{}", start, end))
                        .header(header::ACCEPT_ENCODING, "identity");
                    if let Some(h) = &headers { req = req.headers(h.clone()); }

                    match req.send().await {
                        Ok(resp) => {
                            if !resp.status().is_success() {
                                attempts += 1;
                                continue;
                            }

                            let mut stream = resp.bytes_stream();
                            let mut local_curr = start;
                            let mut batch_start_offset = local_curr;

                            let mut stream_err = false;

                            while let Some(item) = stream.next().await {
                                if is_cancelled(&task_id) { return; }

                                let dynamic_end = slot.end.load(Ordering::Relaxed);
                                if local_curr > dynamic_end { break; }

                                match item {
                                    Ok(chunk) => {
                                        let len = chunk.len() as u64;
                                        let write_len = if local_curr + len - 1 > dynamic_end {
                                            (dynamic_end - local_curr + 1) as usize
                                        } else {
                                            len as usize
                                        };
                                        if write_len == 0 { break; }

                                        let data_to_send = if write_len < chunk.len() {
                                            chunk.slice(0..write_len)
                                        } else {
                                            chunk
                                        };

                                        batch_buffer.push(data_to_send);
                                        batch_size += write_len;
                                        local_curr += write_len as u64;

                                        downloaded_global.fetch_add(write_len as u64, Ordering::Relaxed);
                                        pending_progress += write_len as u64;

                                        if batch_size >= WORKER_BATCH_SIZE {
                                            let msg = WriterMsg::Write(batch_start_offset, batch_buffer);

                                            if tx.send(msg).await.is_err() {
                                                stream_err = true;
                                                // [修复 E0382]
                                                // 必须重新初始化，因为 batch_buffer 已经移入 msg。
                                                // 即使 break，之后的代码路径（!batch_buffer.is_empty()）也需要该变量有效。
                                                batch_buffer = Vec::new();
                                                break;
                                            }

                                            slot.current.store(batch_start_offset + batch_size as u64, Ordering::Relaxed);

                                            // 重置
                                            batch_buffer = Vec::with_capacity(16);
                                            batch_start_offset += batch_size as u64;
                                            batch_size = 0;
                                        }

                                        if pending_progress > 0 && (last_update_time.elapsed().as_millis() > 200 || pending_progress > 1024 * 1024) {
                                            update_progress(&task_id, pending_progress, Some(total), Some("downloading"));
                                            pending_progress = 0;
                                            last_update_time = Instant::now();
                                        }
                                    },
                                    Err(_) => { stream_err = true; break; }
                                }
                            }

                            // 循环结束后检查剩余数据
                            if !batch_buffer.is_empty() {
                                let msg = WriterMsg::Write(batch_start_offset, batch_buffer);
                                if tx.send(msg).await.is_err() {
                                    stream_err = true;
                                } else {
                                    slot.current.store(batch_start_offset + batch_size as u64, Ordering::Relaxed);
                                }
                            }

                            if !stream_err {
                                let final_end = slot.end.load(Ordering::Relaxed);
                                if local_curr >= final_end + 1 {
                                    success = true;
                                    break 'retry;
                                }
                            }
                        }
                        Err(_) => {}
                    }
                    attempts += 1;
                }

                if pending_progress > 0 {
                    update_progress(&task_id, pending_progress, Some(total), Some("downloading"));
                    pending_progress = 0;
                }

                if success {
                    slot.is_done.store(true, Ordering::Relaxed);
                    slot.is_active.store(false, Ordering::Relaxed);
                } else {
                    *error_store.lock().await = Some(CoreError::Other(format!("Task #{} failed", slot.id)));
                    error_occurred.notify_waiters();
                    return;
                }
            }
        }));
    }

    drop(tx);

    let mut tasks_fut = futures_util::future::join_all(handles);

    tokio::select! {
        _ = &mut tasks_fut => {},
        _ = error_occurred.notified() => {},
        _ = async {
            loop {
                if is_cancelled(&task_id_owned) { return; }
                sleep(Duration::from_millis(100)).await;
            }
        } => {}
    }

    if is_cancelled(task_id) {
        let _ = tokio::fs::remove_file(dest.as_ref()).await;
        return Ok(CoreResult::Cancelled);
    }

    if let Some(e) = error_store.lock().await.take() {
        return Err(e);
    }

    match writer_thread.join() {
        Ok(Ok(())) => {},
        Ok(Err(e)) => return Err(CoreError::Io(e)),
        Err(_) => return Err(CoreError::Other("Writer panic".into())),
    }

    let final_bytes = downloaded_global.load(Ordering::Relaxed);
    if final_bytes < total {
        return Err(CoreError::Other("Download incomplete".into()));
    }

    if let Some(expected) = md5_expected {
        update_progress(task_id, 0, Some(total), Some("verifying"));
        match md5_utils::verify_md5(dest.as_ref(), expected).await {
            Ok(true) => {},
            Ok(false) => return Err(CoreError::ChecksumMismatch("MD5 mismatch".into())),
            Err(e) => return Err(CoreError::Io(e)),
        }
    }

    finish_task(task_id, "completed", Some(dest.as_ref().to_string_lossy().to_string()));
    Ok(CoreResult::Success(()))
}
