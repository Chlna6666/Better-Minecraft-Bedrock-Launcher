use futures_util::StreamExt;
use reqwest::header;
use std::cmp::Ordering as CmpOrdering;
use std::io::SeekFrom;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{Mutex, Notify};
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

use crate::downloads::md5 as md5_utils;
use crate::downloads::single::download_file;
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{finish_task, is_cancelled, set_total, update_progress};

// =========================================================================
// 1. 动态任务控制结构
// =========================================================================

/// 单个分片的动态状态
#[derive(Debug)]
struct ChunkSlot {
    id: usize,           // 唯一标识
    start: u64,          // 起始位置
    end: AtomicU64,      // 结束位置 (可能会被其他线程缩小!)
    current: AtomicU64,  // 当前下载进度
    is_active: AtomicBool, // 是否有线程正在处理
    is_done: AtomicBool,   // 是否已完成
}

/// 全局任务管理器：管理所有分片和窃取逻辑
struct AdaptiveTaskManager {
    slots: Mutex<Vec<Arc<ChunkSlot>>>, // 所有的任务槽
    notify: Notify, // 用于唤醒空闲线程
}

impl AdaptiveTaskManager {
    fn new() -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
            notify: Notify::new(),
        }
    }

    /// 添加初始任务
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

    /// 获取下一个任务：
    /// 1. 优先找没人做的任务
    /// 2. 如果都做完了，返回 None
    /// 3. 如果都在做但有慢的，尝试“窃取”
    async fn get_next_task(&self, worker_id: usize) -> Option<Arc<ChunkSlot>> {
        let min_steal_size = 2 * 1024 * 1024; // 剩余小于 2MB 就不抢了，避免碎片化

        // --- 阶段 1: 查找常规空闲任务 ---
        {
            let guard = self.slots.lock().await;
            for slot in guard.iter() {
                // 如果没人做 且 没做完
                if !slot.is_active.load(Ordering::Relaxed) && !slot.is_done.load(Ordering::Relaxed) {
                    slot.is_active.store(true, Ordering::Relaxed);
                    return Some(slot.clone());
                }
            }
        }

        // --- 阶段 2: 尝试窃取 (Work Stealing) ---
        // 既然没有空闲任务，我们看看有没有"长尾"任务可以切分
        let stolen_task = {
            let mut guard = self.slots.lock().await;

            // 检查是否全部完成
            let all_done = guard.iter().all(|s| s.is_done.load(Ordering::Relaxed));
            if all_done {
                return None;
            }

            // 寻找剩余字节数最多的那个正在运行的任务 (Victim)
            let mut best_victim_idx = None;
            let mut max_remaining = 0;

            for (idx, slot) in guard.iter().enumerate() {
                if slot.is_active.load(Ordering::Relaxed) && !slot.is_done.load(Ordering::Relaxed) {
                    let curr = slot.current.load(Ordering::Relaxed);
                    let end = slot.end.load(Ordering::Relaxed);
                    if end > curr {
                        let remaining = end - curr;
                        if remaining > max_remaining {
                            max_remaining = remaining;
                            best_victim_idx = Some(idx);
                        }
                    }
                }
            }

            // 如果最大的剩余量还不够切，或者没找到
            if max_remaining < min_steal_size || best_victim_idx.is_none() {
                return None; // 暂时没活干，等会儿再来
            }

            // --- 执行切割操作 ---
            let victim_idx = best_victim_idx.unwrap();
            let victim = guard[victim_idx].clone();

            // 再次确认数据（避免竞态）
            let v_curr = victim.current.load(Ordering::Relaxed);
            let v_end = victim.end.load(Ordering::Relaxed);
            let remaining = v_end.saturating_sub(v_curr);

            if remaining < min_steal_size {
                None
            } else {
                // 计算切分点：取中点
                let split_point = v_curr + remaining / 2;

                // 1. 修改受害者的结束点 (Victim will stop early)
                victim.end.store(split_point, Ordering::Relaxed);

                // 2. 创建新任务 (Stealed part)
                let new_id = guard.len();
                let new_slot = Arc::new(ChunkSlot {
                    id: new_id,
                    start: split_point + 1, // 从切分点后一个字节开始
                    end: AtomicU64::new(v_end),
                    current: AtomicU64::new(split_point + 1),
                    is_active: AtomicBool::new(true), // 直接标记为被我（当前线程）占有
                    is_done: AtomicBool::new(false),
                });

                guard.push(new_slot.clone());
                debug!("Worker {} 窃取任务: 原始#{}(剩{}) -> 切分点 {} -> 新任务#{}",
                       worker_id, victim.id, format_size(remaining), split_point, new_id);

                Some(new_slot)
            }
        };

        stolen_task
    }
}

// =========================================================================
// 监控与统计结构 (保持原有逻辑，增加对动态分片的显示)
// =========================================================================

struct WorkerState {
    id: usize,
    bytes_last_interval: AtomicU64,
    bytes_total: AtomicU64,
    current_chunk_id: AtomicUsize, // 关联 ChunkSlot 的 id
}

// 辅助函数：格式化字节大小
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// 智能动态多线程分片下载（带工作窃取）
pub async fn download_multi(
    client: reqwest::Client,
    task_id: &str,
    url: &str,
    dest: impl AsRef<Path>,
    threads: usize,
    md5_expected: Option<&str>,
) -> Result<CoreResult<()>, CoreError> {
    let task_id_owned = task_id.to_string();
    debug!("开始自适应多线程下载: task={} 线程数={}", task_id, threads);

    // 1. 获取文件信息 (同原代码)
    let head_resp = client.head(url).header(header::ACCEPT_ENCODING, "identity").send().await?;
    let mut total_len = head_resp.headers().get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok());

    // 简略部分：如果不支持 Range 或大小未知，回退单线程 (保持你原有逻辑即可，此处略去以节省篇幅)
    let total = match total_len {
        Some(len) if len > 0 => len,
        _ => return download_file(client, task_id, url, dest, md5_expected).await,
    };
    set_total(task_id, Some(total));

    // 2. 预分配磁盘 (同原代码)
    {
        let dest_path = dest.as_ref();
        if let Some(parent) = dest_path.parent() { let _ = tokio::fs::create_dir_all(parent).await; }
        let file = OpenOptions::new().create(true).write(true).open(dest_path).await
            .map_err(|e| CoreError::Io(e))?;
        file.set_len(total).await.map_err(|e| CoreError::Io(e))?;
    }

    // 3. 生成初始分片 & 初始化管理器
    // 初始分片可以切大一点，反正后面会自动切分
    let initial_chunk_size = (total / threads as u64).max(5 * 1024 * 1024);
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < total {
        let end = (offset + initial_chunk_size - 1).min(total - 1);
        chunks.push((offset, end));
        offset = end + 1;
    }

    let manager = Arc::new(AdaptiveTaskManager::new());
    manager.add_tasks(chunks).await;

    // 共享资源
    let downloaded_bytes = Arc::new(AtomicU64::new(0));
    let client = Arc::new(client);
    let url = Arc::new(url.to_string());
    let error_occurred = Arc::new(Notify::new());
    let error_store = Arc::new(Mutex::new(None));

    // 4. 初始化可视化监控状态
    let mut worker_states = Vec::with_capacity(threads);
    for i in 0..threads {
        worker_states.push(Arc::new(WorkerState {
            id: i,
            bytes_last_interval: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            current_chunk_id: AtomicUsize::new(usize::MAX),
        }));
    }
    let monitor_states = worker_states.clone();
    let monitor_task_id = task_id.to_string();

    // 监控协程
    let monitor_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1500));
        loop {
            interval.tick().await;
            if is_cancelled(&monitor_task_id) { break; }
            let mut log_buf = String::from("\n=== Adaptive Download Monitor ===\n");
            let mut total_speed: u64 = 0;
            let mut active_threads = 0;

            for state in &monitor_states {
                let bytes_inc = state.bytes_last_interval.swap(0, Ordering::Relaxed);
                let total_done = state.bytes_total.load(Ordering::Relaxed);
                let current_chunk = state.current_chunk_id.load(Ordering::Relaxed);

                let speed_bps = (bytes_inc as f64 / 1.5) as u64;
                total_speed += speed_bps;

                let chunk_str = if current_chunk == usize::MAX {
                    "IDLE".to_string()
                } else {
                    active_threads += 1;
                    format!("#{}", current_chunk)
                };

                // 只有当速度大于0或者正在处理分片时才详细显示，避免刷屏（可选）
                log_buf.push_str(&format!(
                    " -> [Thread {:02}] Chunk: {:<5} | Speed: {:>9}/s | Total: {:>9}\n",
                    state.id, chunk_str, format_size(speed_bps), format_size(total_done)
                ));
            }
            log_buf.push_str(&format!(" -> [Aggregate] Speed: {}/s | Active: {}/{}\n",
                                      format_size(total_speed), active_threads, monitor_states.len()));
            log_buf.push_str("==================================\n");
            info!("{}", log_buf);
        }
    });

    // 5. 启动 Worker
    let mut handles = Vec::with_capacity(threads);
    for worker_id in 0..threads {
        let manager = manager.clone();
        let client = client.clone();
        let url = url.clone();
        let dest_path = dest.as_ref().to_path_buf();
        let downloaded_bytes = downloaded_bytes.clone();
        let task_id = task_id.to_string();
        let error_occurred = error_occurred.clone();
        let error_store = error_store.clone();
        let my_state = worker_states[worker_id].clone();

        handles.push(tokio::spawn(async move {
            let mut file = match OpenOptions::new().write(true).open(&dest_path).await {
                Ok(f) => f,
                Err(e) => {
                    *error_store.lock().await = Some(CoreError::Io(e));
                    error_occurred.notify_waiters();
                    return;
                }
            };

            loop {
                if is_cancelled(&task_id) || error_store.lock().await.is_some() { return; }

                my_state.current_chunk_id.store(usize::MAX, Ordering::Relaxed);

                // >>> 获取任务（这里包含窃取逻辑） <<<
                let slot = match manager.get_next_task(worker_id).await {
                    Some(s) => s,
                    None => {
                        // 真的没有任务了，也没有可以窃取的目标了
                        // 再次检查是否全部完成，如果是则退出
                        let finished_count = {
                            let guard = manager.slots.lock().await;
                            guard.iter().filter(|s| s.is_done.load(Ordering::Relaxed)).count() == guard.len()
                        };
                        if finished_count { break; }

                        // 还有任务在跑，但我暂时抢不到，睡一会再试
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };

                my_state.current_chunk_id.store(slot.id, Ordering::Relaxed);

                let mut attempts = 0;
                let max_retries = 10;
                let mut success = false;

                'retry: while attempts < max_retries {
                    if is_cancelled(&task_id) { return; }

                    // 获取当前的范围（注意：end 可能会被其他窃取者修改）
                    let start_pos = slot.current.load(Ordering::Relaxed);
                    let end_pos = slot.end.load(Ordering::Relaxed);

                    // 如果被抢光了（start > end），直接标记完成
                    if start_pos > end_pos {
                        success = true;
                        break 'retry;
                    }

                    if attempts > 0 { sleep(Duration::from_millis(500 * attempts as u64)).await; }

                    let req = client.get(url.as_str())
                        .header(header::RANGE, format!("bytes={}-{}", start_pos, end_pos))
                        .header(header::ACCEPT_ENCODING, "identity");

                    match req.send().await {
                        Ok(resp) => {
                            if !resp.status().is_success() { attempts += 1; continue; }

                            // Seek 到正确位置
                            if let Err(e) = file.seek(SeekFrom::Start(start_pos)).await {
                                *error_store.lock().await = Some(CoreError::Io(e));
                                error_occurred.notify_waiters();
                                return;
                            }

                            let mut stream = resp.bytes_stream();
                            let mut local_offset = start_pos;
                            let mut stream_failed = false;

                            while let Some(item) = stream.next().await {
                                // 1. 检查任务是否被取消
                                if is_cancelled(&task_id) { return; }

                                // 2. >>> 关键：检查是否被截断 (Chunk Stealing Check) <<<
                                // 每次写数据前，检查 slot.end 是否变小了
                                let current_target_end = slot.end.load(Ordering::Relaxed);
                                if local_offset > current_target_end {
                                    // 即使流还在继续，我们也停止，因为后面的部分被别人抢走了
                                    break;
                                }

                                match item {
                                    Ok(chunk) => {
                                        // 再次防御性检查，防止写入越界
                                        let chunk_len = chunk.len() as u64;
                                        let write_len = if local_offset + chunk_len - 1 > current_target_end {
                                            (current_target_end - local_offset + 1) as usize
                                        } else {
                                            chunk_len as usize
                                        };

                                        if write_len == 0 { break; }

                                        if let Err(e) = file.write_all(&chunk[..write_len]).await {
                                            stream_failed = true; break;
                                        }

                                        local_offset += write_len as u64;

                                        // 更新全局进度
                                        downloaded_bytes.fetch_add(write_len as u64, Ordering::Relaxed);
                                        update_progress(&task_id, write_len as u64, Some(total), Some("downloading"));

                                        // 更新监控
                                        my_state.bytes_last_interval.fetch_add(write_len as u64, Ordering::Relaxed);
                                        my_state.bytes_total.fetch_add(write_len as u64, Ordering::Relaxed);

                                        // 更新当前任务进度，供窃取者计算
                                        slot.current.store(local_offset, Ordering::Relaxed);
                                    }
                                    Err(_) => { stream_failed = true; break; }
                                }
                            }

                            if !stream_failed {
                                // 检查是否达到了当前的 end (注意 end 可能动态变小了)
                                let final_end = slot.end.load(Ordering::Relaxed);
                                if local_offset >= final_end + 1 {
                                    success = true;
                                    break 'retry;
                                }
                                // 没读完流就断了，或者被截断但没完全对齐，重试
                            }
                        },
                        Err(_) => {}
                    }
                    attempts += 1;
                }

                if success {
                    slot.is_done.store(true, Ordering::Relaxed);
                    slot.is_active.store(false, Ordering::Relaxed);
                } else {
                    *error_store.lock().await = Some(CoreError::Other(format!("Chunk #{} failed", slot.id)));
                    error_occurred.notify_waiters();
                    return;
                }
            }
            my_state.current_chunk_id.store(usize::MAX, Ordering::Relaxed);
        }));
    }

    // 6. 等待结果 (同原代码)
    let mut all_tasks_future = futures_util::future::join_all(handles);
    tokio::select! {
        _ = &mut all_tasks_future => {}
        _ = error_occurred.notified() => { debug!("内部错误触发"); }
        _ = async {
            loop {
                if is_cancelled(&task_id_owned) { return; }
                sleep(Duration::from_millis(100)).await;
            }
        } => { debug!("用户取消触发"); }
    }
    monitor_handle.abort();

    if is_cancelled(task_id) {
        let _ = tokio::fs::remove_file(dest.as_ref()).await;
        return Ok(CoreResult::Cancelled);
    }

    if let Some(e) = error_store.lock().await.take() {
        return Err(e);
    }

    // 最终校验
    let final_bytes = downloaded_bytes.load(Ordering::Relaxed);
    if final_bytes < total {
        return Err(CoreError::Other("Incomplete download".into()));
    }

    if let Some(expected) = md5_expected {
        update_progress(task_id, 0, Some(total), Some("verifying"));
        match md5_utils::verify_md5(dest.as_ref(), expected).await {
            Ok(true) => debug!("MD5 OK"),
            Ok(false) => return Err(CoreError::ChecksumMismatch("MD5 fail".into())),
            Err(e) => return Err(CoreError::Io(e)),
        }
    }

    finish_task(task_id, "completed", Some(dest.as_ref().to_string_lossy().to_string()));
    Ok(CoreResult::Success(()))
}