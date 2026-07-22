// src/downloads/multi.rs
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{self, HeaderMap, HeaderValue};
use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::task::JoinSet;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, warn};

use crate::downloads::md5::{is_md5_digest, verify_md5};
use crate::downloads::single::download_file;
use crate::http::proxy::{apply_download_request_headers, validate_download_response_headers};
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{
    TaskControl, TaskVisualization, ThreadVisualization, is_cancelled_fast, set_task_visualization,
    set_total, task_visualization_enabled, update_progress, wait_until_active_fast,
};

// =========================================================================
// 性能配置常量 (针对 2.5G+ 宽带优化)
// =========================================================================

const WORKER_BATCH_SIZE: usize = 1024 * 1024;
const VISUALIZATION_EMIT_INTERVAL_MS: u64 = 250;
const WRITE_CHANNEL_SIZE: usize = 64;

const MIN_CHUNK_SIZE: u64 = 10 * 1024 * 1024;
const RANGE_REQUEST_TIMEOUT_SECS: u64 = 5 * 60;
const DOWNLOAD_METADATA_TIMEOUT_SECS: u64 = 30;
const RANGE_REQUEST_ATTEMPTS: usize = 10;

#[derive(Clone, Debug)]
struct ChunkState {
    start: u64,
    end: u64,
}

// =========================================================================
// 任务管理结构
// =========================================================================

#[derive(Debug)]
struct ChunkSlot {
    id: usize,
    start: u64,
    end: u64,
    is_active: AtomicBool,
    is_done: AtomicBool,
}

struct AdaptiveTaskManager {
    slots: Mutex<Vec<Arc<ChunkSlot>>>,
    total_slots: AtomicUsize,
}

impl AdaptiveTaskManager {
    fn new() -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
            total_slots: AtomicUsize::new(0),
        }
    }

    async fn add_tasks(&self, chunks: Vec<ChunkState>) {
        self.total_slots.store(chunks.len(), Ordering::Relaxed);
        let mut guard = self.slots.lock().await;
        for (i, chunk) in chunks.into_iter().enumerate() {
            guard.push(Arc::new(ChunkSlot {
                id: i,
                start: chunk.start,
                end: chunk.end,
                is_active: AtomicBool::new(false),
                is_done: AtomicBool::new(false),
            }));
        }
    }

    async fn get_next_task(&self) -> Option<Arc<ChunkSlot>> {
        let guard = self.slots.lock().await;
        for slot in guard.iter() {
            if !slot.is_active.load(Ordering::Relaxed) && !slot.is_done.load(Ordering::Relaxed) {
                slot.is_active.store(true, Ordering::Relaxed);
                return Some(slot.clone());
            }
        }
        None
    }

    fn slot_total(&self) -> usize {
        self.total_slots.load(Ordering::Relaxed)
    }
}

struct WorkerActivityGuard<'a> {
    active_workers: &'a AtomicUsize,
}

impl<'a> WorkerActivityGuard<'a> {
    fn new(active_workers: &'a AtomicUsize) -> Self {
        active_workers.fetch_add(1, Ordering::Relaxed);
        Self { active_workers }
    }
}

impl Drop for WorkerActivityGuard<'_> {
    fn drop(&mut self) {
        self.active_workers.fetch_sub(1, Ordering::Relaxed);
    }
}

fn build_download_visualization(
    worker_total: usize,
    worker_active: usize,
    unit_total: usize,
    unit_done: usize,
    current_item: impl Into<Option<String>>,
    threads: Option<Vec<ThreadVisualization>>,
) -> TaskVisualization {
    TaskVisualization {
        worker_total: Some(worker_total as u32),
        worker_active: Some(worker_active as u32),
        unit_label: Some("分片".to_string()),
        unit_total: Some(unit_total as u64),
        unit_done: Some(unit_done as u64),
        current_item: current_item.into(),
        threads,
    }
}

fn build_thread_visualizations(worker_total: usize) -> Vec<ThreadVisualization> {
    (0..worker_total)
        .map(|index| ThreadVisualization {
            index: index as u32,
            label: Some(format!("线程 {}", index + 1)),
            active: false,
            done: 0,
            total: 0,
            current_item: None,
        })
        .collect()
}

fn build_initial_chunk_states(total: u64, threads: usize) -> Vec<ChunkState> {
    let initial_chunk_size = (total / threads.max(1) as u64).max(MIN_CHUNK_SIZE);
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < total {
        let end = (offset + initial_chunk_size - 1).min(total - 1);
        chunks.push(ChunkState { start: offset, end });
        offset = end + 1;
    }
    chunks
}

fn inclusive_range_len(start: u64, end: u64) -> u64 {
    if start > end { 0 } else { end - start + 1 }
}

#[derive(Clone, Copy)]
struct ParsedContentRange {
    start: u64,
    end: u64,
    total: Option<u64>,
}

fn parse_content_range(value: &HeaderValue) -> Option<ParsedContentRange> {
    let value = value.to_str().ok()?.trim();
    let range = value.strip_prefix("bytes ")?;
    let (range_bounds, total_text) = range.split_once('/')?;
    let (start_text, end_text) = range_bounds.split_once('-')?;
    let start = start_text.parse::<u64>().ok()?;
    let end = end_text.parse::<u64>().ok()?;
    let total = if total_text == "*" {
        None
    } else {
        Some(total_text.parse::<u64>().ok()?)
    };

    Some(ParsedContentRange { start, end, total })
}

fn content_range_matches(headers: &HeaderMap, start: u64, end: u64, total: u64) -> bool {
    headers
        .get(header::CONTENT_RANGE)
        .and_then(parse_content_range)
        .is_some_and(|range| range.start == start && range.end == end && range.total == Some(total))
}

fn should_fallback_to_single_thread(error: &CoreError) -> bool {
    match error {
        CoreError::Request(_) | CoreError::Timeout | CoreError::ChecksumMismatch(_) => true,
        CoreError::Other(message) => {
            message.contains("分片")
                || message.contains("range")
                || message.contains("Download incomplete")
        }
        CoreError::Io(_)
        | CoreError::Xml(_)
        | CoreError::Zip(_)
        | CoreError::BadUpdateIdentity
        | CoreError::UnknownContentLength
        | CoreError::Join(_)
        | CoreError::Config(_) => false,
    }
}

async fn resolve_reliable_range_url(
    client: &reqwest::Client,
    url: &str,
    headers: Option<&HeaderMap>,
    total: u64,
) -> Option<String> {
    let mut request = client
        .get(url)
        .header(header::RANGE, "bytes=0-0")
        .timeout(Duration::from_secs(DOWNLOAD_METADATA_TIMEOUT_SECS));
    if let Some(headers) = headers {
        request = request.headers(headers.clone());
    }
    request = apply_download_request_headers(request);

    match request.send().await {
        Ok(response) => {
            if let Err(error) = validate_download_response_headers(url, &response) {
                warn!("server range probe returned transformed response: {error}");
                return None;
            }
            let status = response.status();
            let content_length_is_valid = response.content_length().is_none_or(|len| len == 1);
            if status == StatusCode::PARTIAL_CONTENT
                && content_length_is_valid
                && content_range_matches(response.headers(), 0, 0, total)
            {
                Some(response.url().to_string())
            } else {
                warn!(
                    "server range probe failed: status={} content_length={:?}",
                    status,
                    response.content_length()
                );
                None
            }
        }
        Err(error) => {
            warn!("server range probe request failed: {error}");
            None
        }
    }
}

async fn all_slots_done(manager: &AdaptiveTaskManager) -> bool {
    manager
        .slots
        .lock()
        .await
        .iter()
        .all(|slot| slot.is_done.load(Ordering::Relaxed))
}

async fn update_thread_visualization(
    thread_visualizations: &Mutex<Vec<ThreadVisualization>>,
    worker_index: usize,
    active: bool,
    done: u64,
    total: u64,
    current_item: Option<String>,
) {
    let mut guard = thread_visualizations.lock().await;
    if let Some(thread) = guard.get_mut(worker_index) {
        thread.active = active;
        thread.done = done.min(total);
        thread.total = total;
        thread.current_item = current_item;
    }
}

async fn set_download_visualization(
    task_id: &str,
    worker_total: usize,
    worker_active: usize,
    unit_total: usize,
    unit_done: usize,
    current_item: Option<String>,
    thread_visualizations: &Mutex<Vec<ThreadVisualization>>,
) {
    if !task_visualization_enabled() {
        return;
    }

    let threads = thread_visualizations.lock().await.clone();
    set_task_visualization(
        task_id,
        Some(build_download_visualization(
            worker_total,
            worker_active,
            unit_total,
            unit_done,
            current_item,
            Some(threads),
        )),
    );
}

async fn set_download_visualization_throttled(
    task_id: &str,
    worker_total: usize,
    worker_active: usize,
    unit_total: usize,
    unit_done: usize,
    current_item: Option<String>,
    thread_visualizations: &Mutex<Vec<ThreadVisualization>>,
    last_emit_at: &Mutex<Instant>,
    force_emit: bool,
) -> bool {
    if !task_visualization_enabled() {
        return false;
    }

    let should_emit = {
        let mut last_emit_at = last_emit_at.lock().await;
        if force_emit
            || last_emit_at.elapsed() >= Duration::from_millis(VISUALIZATION_EMIT_INTERVAL_MS)
        {
            *last_emit_at = Instant::now();
            true
        } else {
            false
        }
    };

    if !should_emit {
        return false;
    }

    set_download_visualization(
        task_id,
        worker_total,
        worker_active,
        unit_total,
        unit_done,
        current_item,
        thread_visualizations,
    )
    .await;
    true
}

enum WriterMsg {
    Write { offset: u64, chunks: Vec<Bytes> },
}

async fn remove_download_file_if_exists(path: &Path) -> Result<(), CoreError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CoreError::Io(error)),
    }
}

async fn prepare_direct_output(dest: &Path) -> Result<(), CoreError> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(CoreError::Io)?;
    }
    remove_download_file_if_exists(dest).await
}

#[cfg(windows)]
fn write_all_at(file: &StdFile, mut offset: u64, mut buffer: &[u8]) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    use std::os::windows::fs::FileExt as _;

    while !buffer.is_empty() {
        let written = file.seek_write(buffer, offset)?;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write positioned download chunk",
            ));
        }
        offset = offset.saturating_add(written as u64);
        buffer = &buffer[written..];
    }

    Ok(())
}

#[cfg(unix)]
fn write_all_at(file: &StdFile, mut offset: u64, mut buffer: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt as _;

    while !buffer.is_empty() {
        let written = file.write_at(buffer, offset)?;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write positioned download chunk",
            ));
        }
        offset = offset.saturating_add(written as u64);
        buffer = &buffer[written..];
    }

    Ok(())
}

fn spawn_direct_writer(
    dest: std::path::PathBuf,
    total: u64,
    mut rx: mpsc::Receiver<WriterMsg>,
) -> thread::JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = StdOpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&dest)?;
        if let Err(error) = file.set_len(total) {
            warn!(
                "download writer failed to preallocate file path={} size={} error={}",
                dest.to_string_lossy(),
                total,
                error
            );
        }

        while let Some(message) = rx.blocking_recv() {
            match message {
                WriterMsg::Write { offset, chunks } => {
                    let mut current_offset = offset;
                    for chunk in chunks {
                        write_all_at(&file, current_offset, &chunk)?;
                        current_offset = current_offset.saturating_add(chunk.len() as u64);
                    }
                }
            }
        }

        Ok(())
    })
}

fn log_writer_cleanup_result(result: thread::Result<io::Result<()>>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!("download writer stopped during cleanup: {error}"),
        Err(_) => warn!("download writer thread panicked during cleanup"),
    }
}

async fn download_multi_partitioned(
    client: reqwest::Client,
    task_control: Arc<TaskControl>,
    task_id: &str,
    url: String,
    dest_path: Arc<std::path::PathBuf>,
    threads: usize,
    headers: Option<HeaderMap>,
    md5_expected: Option<&str>,
    total: u64,
) -> Result<CoreResult<()>, CoreError> {
    let chunks = build_initial_chunk_states(total, threads);
    let active_threads = threads.min(chunks.len()).max(1);
    let planned_chunks = chunks.len();
    let first_chunk_size = chunks
        .first()
        .map(|chunk| inclusive_range_len(chunk.start, chunk.end))
        .unwrap_or(0);
    debug!(
        "multi download chunk plan: task={} total={} requested_threads={} active_threads={} chunks={} first_chunk_size={}",
        task_id, total, threads, active_threads, planned_chunks, first_chunk_size
    );
    prepare_direct_output(dest_path.as_path()).await?;

    let manager = Arc::new(AdaptiveTaskManager::new());
    manager.add_tasks(chunks).await;

    let completed_slots = Arc::new(AtomicUsize::new(0));
    let active_workers = Arc::new(AtomicUsize::new(0));
    let thread_visualizations = Arc::new(Mutex::new(build_thread_visualizations(active_threads)));
    let visualization_last_emit_at = Arc::new(Mutex::new(
        Instant::now() - Duration::from_millis(VISUALIZATION_EMIT_INTERVAL_MS),
    ));
    if task_visualization_enabled() {
        let threads_snapshot = thread_visualizations.lock().await.clone();
        set_task_visualization(
            task_id,
            Some(build_download_visualization(
                active_threads,
                0,
                manager.slot_total(),
                0,
                None,
                Some(threads_snapshot),
            )),
        );
    }

    let downloaded_global = Arc::new(AtomicU64::new(0));
    let client = Arc::new(client);
    let error_occurred = Arc::new(Notify::new());
    let error_store = Arc::new(Mutex::new(None));
    let (write_tx, write_rx) = mpsc::channel::<WriterMsg>(WRITE_CHANNEL_SIZE);
    let writer_thread = spawn_direct_writer(dest_path.as_ref().clone(), total, write_rx);
    let mut workers = JoinSet::new();

    for worker_id in 0..active_threads {
        let manager = manager.clone();
        let client = client.clone();
        let url = url.clone();
        let task_control = task_control.clone();
        let downloaded_global = downloaded_global.clone();
        let completed_slots = completed_slots.clone();
        let active_workers = active_workers.clone();
        let thread_visualizations = thread_visualizations.clone();
        let visualization_last_emit_at = visualization_last_emit_at.clone();
        let task_id = task_id.to_string();
        let error_occurred = error_occurred.clone();
        let error_store = error_store.clone();
        let headers = headers.clone();
        let write_tx = write_tx.clone();

        workers.spawn(async move {
            let mut pending_progress = 0u64;
            let mut last_update_time = Instant::now();

            loop {
                if !wait_until_active_fast(task_control.as_ref()).await {
                    return;
                }

                if is_cancelled_fast(task_control.as_ref()) || error_store.lock().await.is_some() {
                    return;
                }

                let slot = match manager.get_next_task().await {
                    Some(slot) => slot,
                    None => {
                        if all_slots_done(manager.as_ref()).await {
                            let (thread_done, thread_total) = {
                                let guard = thread_visualizations.lock().await;
                                guard
                                    .get(worker_id)
                                    .map_or((0, 0), |thread| (thread.done, thread.total))
                            };
                            update_thread_visualization(
                                thread_visualizations.as_ref(),
                                worker_id,
                                false,
                                thread_done,
                                thread_total,
                                None,
                            )
                            .await;
                            break;
                        }
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };

                let active_guard = WorkerActivityGuard::new(active_workers.as_ref());
                let slot_start = slot.start;
                let slot_end = slot.end;
                let slot_total = inclusive_range_len(slot_start, slot_end);
                let mut reported_slot_bytes = 0u64;

                update_thread_visualization(
                    thread_visualizations.as_ref(),
                    worker_id,
                    true,
                    0,
                    slot_total,
                    None,
                )
                .await;
                set_download_visualization_throttled(
                    &task_id,
                    active_threads,
                    active_workers.load(Ordering::Relaxed),
                    manager.slot_total(),
                    completed_slots.load(Ordering::Relaxed),
                    None,
                    thread_visualizations.as_ref(),
                    visualization_last_emit_at.as_ref(),
                    true,
                )
                .await;

                let mut attempts = 0;
                let mut success = false;
                let mut last_error = None::<String>;

                'retry: while attempts < RANGE_REQUEST_ATTEMPTS {
                    if !wait_until_active_fast(task_control.as_ref()).await {
                        return;
                    }

                    if is_cancelled_fast(task_control.as_ref()) {
                        return;
                    }

                    if attempts > 0 {
                        sleep(Duration::from_millis(500 * attempts as u64)).await;
                    }

                    let mut req = client
                        .get(url.as_str())
                        .header(header::RANGE, format!("bytes={slot_start}-{slot_end}"))
                        .timeout(Duration::from_secs(RANGE_REQUEST_TIMEOUT_SECS));
                    if let Some(headers) = &headers {
                        req = req.headers(headers.clone());
                    }
                    req = apply_download_request_headers(req);

                    let resp = match req.send().await {
                        Ok(resp) => resp,
                        Err(error) => {
                            last_error = Some(format!("range request failed: {error}"));
                            attempts += 1;
                            continue;
                        }
                    };
                    if let Err(error) = validate_download_response_headers(url.as_str(), &resp) {
                        last_error = Some(format!("transformed range response: {error}"));
                        attempts += 1;
                        continue;
                    }
                    if resp.url().as_str() != url.as_str() {
                        last_error = Some(format!(
                            "range response url changed: expected={} actual={}",
                            url,
                            resp.url()
                        ));
                        warn!(
                            "range response url changed for task={} slot={} start={} end={} expected={} actual={}",
                            task_id,
                            slot.id,
                            slot_start,
                            slot_end,
                            url,
                            resp.url()
                        );
                        attempts += 1;
                        continue;
                    }

                    if resp.status() != StatusCode::PARTIAL_CONTENT
                        || !content_range_matches(resp.headers(), slot_start, slot_end, total)
                    {
                        last_error = Some(format!(
                            "invalid range response status={} content_range={:?}",
                            resp.status(),
                            resp.headers().get(header::CONTENT_RANGE)
                        ));
                        warn!(
                            "invalid range response for task={} slot={} start={} end={} status={} content_range={:?}",
                            task_id,
                            slot.id,
                            slot_start,
                            slot_end,
                            resp.status(),
                            resp.headers().get(header::CONTENT_RANGE),
                        );
                        attempts += 1;
                        continue;
                    }

                    if resp.content_length().is_some_and(|len| len != slot_total) {
                        last_error =
                            Some(format!("invalid range length: {:?}", resp.content_length()));
                        warn!(
                            "invalid range length for task={} slot={} start={} end={} content_length={:?}",
                            task_id,
                            slot.id,
                            slot_start,
                            slot_end,
                            resp.content_length(),
                        );
                        attempts += 1;
                        continue;
                    }

                    let mut stream = resp.bytes_stream();
                    let mut local_curr = slot_start;
                    let mut batch_start_offset = local_curr;
                    let mut batch_chunks: Vec<Bytes> = Vec::with_capacity(16);
                    let mut batch_size = 0usize;
                    let mut stream_err = false;

                    while let Some(item) = stream.next().await {
                        if !wait_until_active_fast(task_control.as_ref()).await {
                            return;
                        }

                        if is_cancelled_fast(task_control.as_ref()) {
                            return;
                        }

                        let chunk = match item {
                            Ok(chunk) => chunk,
                            Err(error) => {
                                last_error = Some(format!("read range stream failed: {error}"));
                                stream_err = true;
                                break;
                            }
                        };

                        if chunk.is_empty() {
                            continue;
                        }

                        if local_curr > slot_end {
                            last_error = Some(format!(
                                "range body exceeded requested length: expected {slot_total} bytes"
                            ));
                            break 'retry;
                        }

                        let remaining = slot_end.saturating_sub(local_curr).saturating_add(1);
                        let chunk_len = chunk.len() as u64;
                        if chunk_len > remaining {
                            last_error = Some(format!(
                                "range body exceeded requested length: expected {slot_total} bytes"
                            ));
                            break 'retry;
                        }

                        batch_size += chunk.len();
                        batch_chunks.push(chunk);
                        local_curr = local_curr.saturating_add(chunk_len);

                        let current_slot_done =
                            local_curr.saturating_sub(slot_start).min(slot_total);
                        let reportable_bytes =
                            current_slot_done.saturating_sub(reported_slot_bytes);
                        if reportable_bytes > 0 {
                            reported_slot_bytes = current_slot_done;
                            downloaded_global.fetch_add(reportable_bytes, Ordering::Relaxed);
                            pending_progress = pending_progress.saturating_add(reportable_bytes);
                        }

                        if batch_size >= WORKER_BATCH_SIZE {
                            let next_batch_start = batch_start_offset + batch_size as u64;
                            let chunks = std::mem::take(&mut batch_chunks);
                            if write_tx
                                .send(WriterMsg::Write {
                                    offset: batch_start_offset,
                                    chunks,
                                })
                                .await
                                .is_err()
                            {
                                last_error = Some("download writer stopped".to_string());
                                stream_err = true;
                                break;
                            }
                            batch_start_offset = next_batch_start;
                            batch_size = 0;
                        }

                        if pending_progress > 0
                            && (last_update_time.elapsed().as_millis() > 200
                                || pending_progress > WORKER_BATCH_SIZE as u64)
                        {
                            update_thread_visualization(
                                thread_visualizations.as_ref(),
                                worker_id,
                                true,
                                current_slot_done,
                                slot_total,
                                None,
                            )
                            .await;
                            update_progress(
                                &task_id,
                                pending_progress,
                                Some(total),
                                Some("downloading"),
                            );
                            set_download_visualization_throttled(
                                &task_id,
                                active_threads,
                                active_workers.load(Ordering::Relaxed),
                                manager.slot_total(),
                                completed_slots.load(Ordering::Relaxed),
                                None,
                                thread_visualizations.as_ref(),
                                visualization_last_emit_at.as_ref(),
                                false,
                            )
                            .await;
                            pending_progress = 0;
                            last_update_time = Instant::now();
                        }
                    }

                    if !stream_err {
                        if !batch_chunks.is_empty() {
                            let next_batch_start = batch_start_offset + batch_size as u64;
                            let chunks = std::mem::take(&mut batch_chunks);
                            if write_tx
                                .send(WriterMsg::Write {
                                    offset: batch_start_offset,
                                    chunks,
                                })
                                .await
                                .is_err()
                            {
                                last_error = Some("download writer stopped".to_string());
                                stream_err = true;
                            }
                        }
                    }

                    if !stream_err {
                        let expected_written = local_curr.saturating_sub(slot_start);
                        if expected_written == slot_total {
                            success = true;
                            break 'retry;
                        }

                        last_error = Some(format!(
                            "range size mismatch: expected {slot_total}, stream wrote {expected_written}"
                        ));
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
                    let completed = completed_slots.fetch_add(1, Ordering::Relaxed) + 1;
                    drop(active_guard);
                    update_thread_visualization(
                        thread_visualizations.as_ref(),
                        worker_id,
                        false,
                        slot_total,
                        slot_total,
                        None,
                    )
                    .await;
                    set_download_visualization_throttled(
                        &task_id,
                        active_threads,
                        active_workers.load(Ordering::Relaxed),
                        manager.slot_total(),
                        completed,
                        None,
                        thread_visualizations.as_ref(),
                        visualization_last_emit_at.as_ref(),
                        true,
                    )
                    .await;
                } else {
                    drop(active_guard);
                    let (thread_done, thread_total) = {
                        let guard = thread_visualizations.lock().await;
                        guard
                            .get(worker_id)
                            .map_or((0, 0), |thread| (thread.done, thread.total))
                    };
                    update_thread_visualization(
                        thread_visualizations.as_ref(),
                        worker_id,
                        false,
                        thread_done,
                        thread_total,
                        None,
                    )
                    .await;
                    *error_store.lock().await = Some(CoreError::Other(format!(
                        "分片 #{} 下载失败: {}",
                        slot.id,
                        last_error.unwrap_or_else(|| "unknown error".to_string())
                    )));
                    error_occurred.notify_waiters();
                    return;
                }
            }
        });
    }

    let mut all_workers_finished = false;
    let mut join_all_workers = Box::pin(async {
        while let Some(result) = workers.join_next().await {
            if let Err(error) = result {
                error!("download worker join error: {error}");
            }
        }
    });
    drop(write_tx);
    let cancel_task_control = task_control.clone();

    tokio::select! {
        _ = &mut join_all_workers => {
            all_workers_finished = true;
        },
        _ = error_occurred.notified() => {},
        _ = async {
            loop {
                if is_cancelled_fast(cancel_task_control.as_ref()) {
                    return;
                }
                sleep(Duration::from_millis(100)).await;
            }
        } => {}
    }
    drop(join_all_workers);

    if !all_workers_finished {
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }

    if let Some(error) = error_store.lock().await.take() {
        set_task_visualization(task_id, None);
        log_writer_cleanup_result(writer_thread.join());
        remove_download_file_if_exists(dest_path.as_path()).await?;
        return Err(error);
    }

    if is_cancelled_fast(task_control.as_ref()) {
        set_task_visualization(task_id, None);
        log_writer_cleanup_result(writer_thread.join());
        remove_download_file_if_exists(dest_path.as_path()).await?;
        return Ok(CoreResult::Cancelled);
    }

    if !all_slots_done(manager.as_ref()).await {
        set_task_visualization(task_id, None);
        log_writer_cleanup_result(writer_thread.join());
        remove_download_file_if_exists(dest_path.as_path()).await?;
        return Err(CoreError::Other("Download incomplete".into()));
    }

    match writer_thread.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            set_task_visualization(task_id, None);
            remove_download_file_if_exists(dest_path.as_path()).await?;
            return Err(CoreError::Io(error));
        }
        Err(_) => {
            set_task_visualization(task_id, None);
            remove_download_file_if_exists(dest_path.as_path()).await?;
            return Err(CoreError::Other("download writer thread panicked".into()));
        }
    }

    let actual_len = tokio::fs::metadata(dest_path.as_path())
        .await
        .map_err(CoreError::Io)?
        .len();
    if actual_len != total {
        set_task_visualization(task_id, None);
        return Err(CoreError::Other(format!(
            "Download size mismatch: expected {total} bytes, got {actual_len} bytes"
        )));
    }

    if let Some(expected) = md5_expected
        .map(str::trim)
        .filter(|value| is_md5_digest(value))
    {
        update_progress(task_id, 0, Some(total), Some("verifying"));
        match verify_md5(dest_path.as_path(), expected).await {
            Ok(true) => {}
            Ok(false) => {
                set_task_visualization(task_id, None);
                return Err(CoreError::ChecksumMismatch("MD5 mismatch".into()));
            }
            Err(error) => {
                set_task_visualization(task_id, None);
                return Err(CoreError::Io(error));
            }
        }
    }

    debug!(
        "multi download completed: task={} bytes_written_this_run={} write=direct-offset",
        task_id,
        downloaded_global.load(Ordering::Relaxed)
    );
    set_task_visualization(task_id, None);
    Ok(CoreResult::Success(()))
}

// =========================================================================
// 主逻辑
// =========================================================================

pub async fn download_multi(
    client: reqwest::Client,
    task_control: Arc<TaskControl>,
    task_id: &str,
    url: &str,
    dest: impl AsRef<Path>,
    threads: usize,
    headers: Option<HeaderMap>,
    md5_expected: Option<&str>,
) -> Result<CoreResult<()>, CoreError> {
    crate::downloads::register_download_task_stage_labels();
    let threads = threads.max(1);
    debug!(
        "启动极致性能多线程下载: task={} threads={}",
        task_id, threads
    );
    let dest_path = Arc::new(dest.as_ref().to_path_buf());

    // 1. 获取文件大小 + 解析最终 URL（必须！某些 CDN 对带 Range 的请求不返回 302，而是直接 404）
    //    例如 edge.forgecdn.net 在带 Range 时可能直接 404，但正常请求会 302 到 mediafilez.forgecdn.net。
    let mut head_req = client
        .head(url)
        .timeout(Duration::from_secs(DOWNLOAD_METADATA_TIMEOUT_SECS));
    if let Some(h) = &headers {
        head_req = head_req.headers(h.clone());
    }
    head_req = apply_download_request_headers(head_req);

    let total = match head_req.send().await {
        Ok(resp) => {
            if let Err(error) = validate_download_response_headers(url, &resp) {
                warn!("download metadata request returned transformed response: {error}");
                return download_file(
                    client,
                    task_control,
                    task_id,
                    url,
                    dest_path.as_path(),
                    headers,
                    md5_expected,
                )
                .await;
            }
            resp.headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        }
        Err(_) => 0,
    };

    if total == 0 {
        return download_file(
            client,
            task_control,
            task_id,
            url,
            dest_path.as_path(),
            headers,
            md5_expected,
        )
        .await;
    }
    set_total(task_id, Some(total));

    let Some(range_url) = resolve_reliable_range_url(&client, url, headers.as_ref(), total).await
    else {
        warn!(
            "server does not reliably support ranged downloads; falling back to single-thread mode"
        );
        return download_file(
            client,
            task_control,
            task_id,
            url,
            dest_path.as_path(),
            headers,
            md5_expected,
        )
        .await;
    };
    debug!(
        "multi download range url resolved: task={} total={} range_url={}",
        task_id, total, range_url
    );

    let result = download_multi_partitioned(
        client.clone(),
        task_control.clone(),
        task_id,
        range_url,
        dest_path,
        threads,
        headers.clone(),
        md5_expected,
        total,
    )
    .await;

    match result {
        Err(error) if should_fallback_to_single_thread(&error) => {
            warn!(
                "multi-thread download failed; falling back to single-thread mode: {}",
                error
            );
            update_progress(task_id, 0, Some(total), Some("single_thread_fallback"));
            remove_download_file_if_exists(dest.as_ref()).await?;
            download_file(
                client,
                task_control,
                task_id,
                url,
                dest,
                headers,
                md5_expected,
            )
            .await
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::CoreResult;
    use crate::tasks::task_manager::{create_task_with_options, task_control};
    use std::io::{Error as IoError, ErrorKind};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;

    #[test]
    fn large_files_are_partitioned_by_thread_count() {
        let total = 938_519_123;
        let chunks = build_initial_chunk_states(total, 16);

        assert_eq!(chunks.len(), 17);
        assert_eq!(chunks.first().map(|chunk| chunk.start), Some(0));
        assert_eq!(
            chunks
                .first()
                .map(|chunk| inclusive_range_len(chunk.start, chunk.end)),
            Some(total / 16)
        );
        assert_eq!(chunks.last().map(|chunk| chunk.end), Some(total - 1));
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| inclusive_range_len(chunk.start, chunk.end))
                .sum::<u64>(),
            total
        );
    }

    #[test]
    fn small_files_are_not_split_below_minimum_chunk_size() {
        let total = MIN_CHUNK_SIZE - 1;
        let chunks = build_initial_chunk_states(total, 16);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start, 0);
        assert_eq!(chunks[0].end, total - 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn multi_download_writes_ranges_to_expected_offsets() {
        let data_len = usize::try_from(MIN_CHUNK_SIZE * 2 + 123_457)
            .expect("test payload length should fit usize");
        let data = Arc::new(build_test_payload(data_len));
        let mut context = md5::Context::new();
        context.consume(data.as_slice());
        let expected_md5 = format!("{:x}", context.compute());

        let (url, server_handle) = spawn_range_server(data.clone())
            .await
            .expect("range test server should start");
        let dest = temp_test_path("bmcbl-multi-range.bin");
        remove_test_file_if_exists(&dest).await;

        let task_id = unique_task_id("multi-range-test");
        create_task_with_options(Some(task_id.clone()), "downloading", None, true);
        let control = task_control(&task_id).expect("task control should exist");
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client should build");

        let result = download_multi(
            client,
            control,
            &task_id,
            &url,
            &dest,
            4,
            None,
            Some(expected_md5.as_str()),
        )
        .await
        .expect("multi download should not return a transport error");

        assert!(matches!(result, CoreResult::Success(())));
        let downloaded = tokio::fs::read(&dest)
            .await
            .expect("downloaded file should be readable");
        assert_eq!(downloaded, data.as_slice());

        remove_test_file_if_exists(&dest).await;
        server_handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn multi_download_falls_back_to_single_when_range_body_exceeds_request() {
        let data_len =
            usize::try_from(MIN_CHUNK_SIZE + 4096).expect("test payload length should fit usize");
        let data = Arc::new(build_test_payload(data_len));
        let mut context = md5::Context::new();
        context.consume(data.as_slice());
        let expected_md5 = format!("{:x}", context.compute());

        let (url, server_handle) = spawn_malformed_range_server(data.clone())
            .await
            .expect("range test server should start");
        let dest = temp_test_path("bmcbl-multi-range-fallback.bin");
        remove_test_file_if_exists(&dest).await;

        let task_id = unique_task_id("multi-range-fallback-test");
        create_task_with_options(Some(task_id.clone()), "downloading", None, true);
        let control = task_control(&task_id).expect("task control should exist");
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client should build");

        let result = download_multi(
            client,
            control,
            &task_id,
            &url,
            &dest,
            4,
            None,
            Some(expected_md5.as_str()),
        )
        .await
        .expect("fallback download should not return a transport error");

        assert!(matches!(result, CoreResult::Success(())));
        let downloaded = tokio::fs::read(&dest)
            .await
            .expect("downloaded file should be readable");
        assert_eq!(downloaded, data.as_slice());

        remove_test_file_if_exists(&dest).await;
        server_handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn direct_writer_preserves_out_of_order_offsets() {
        let data = build_test_payload(64 * 1024 + 513);
        let dest = temp_test_path("bmcbl-direct-writer-offsets.bin");
        remove_test_file_if_exists(&dest).await;

        let (write_tx, write_rx) = mpsc::channel::<WriterMsg>(8);
        let writer_thread = spawn_direct_writer(dest.clone(), data.len() as u64, write_rx);

        let middle_start = 4096usize;
        let tail_start = 48 * 1024usize;
        write_tx
            .send(WriterMsg::Write {
                offset: tail_start as u64,
                chunks: vec![Bytes::copy_from_slice(&data[tail_start..])],
            })
            .await
            .expect("tail write should enqueue");
        write_tx
            .send(WriterMsg::Write {
                offset: 0,
                chunks: vec![
                    Bytes::copy_from_slice(&data[..middle_start]),
                    Bytes::copy_from_slice(&data[middle_start..tail_start]),
                ],
            })
            .await
            .expect("head write should enqueue");
        drop(write_tx);

        writer_thread
            .join()
            .expect("writer thread should not panic")
            .expect("writer thread should flush successfully");
        let written = tokio::fs::read(&dest)
            .await
            .expect("written file should be readable");
        assert_eq!(written, data);

        remove_test_file_if_exists(&dest).await;
    }

    fn build_test_payload(len: usize) -> Vec<u8> {
        (0..len)
            .map(|index| {
                let value = index as u64;
                value.wrapping_mul(31).wrapping_add(value >> 7) as u8
            })
            .collect()
    }

    #[derive(Clone, Copy)]
    enum TestRangeMode {
        Exact,
        ExtraByteAfterRange,
    }

    async fn spawn_range_server(data: Arc<Vec<u8>>) -> std::io::Result<(String, JoinHandle<()>)> {
        spawn_range_server_with_mode(data, TestRangeMode::Exact).await
    }

    async fn spawn_malformed_range_server(
        data: Arc<Vec<u8>>,
    ) -> std::io::Result<(String, JoinHandle<()>)> {
        spawn_range_server_with_mode(data, TestRangeMode::ExtraByteAfterRange).await
    }

    async fn spawn_range_server_with_mode(
        data: Arc<Vec<u8>>,
        mode: TestRangeMode,
    ) -> std::io::Result<(String, JoinHandle<()>)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let data = data.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_range_connection(stream, data, mode).await {
                        debug!("range test server request failed: {}", error);
                    }
                });
            }
        });

        Ok((format!("http://{address}/file.bin"), handle))
    }

    async fn handle_range_connection(
        mut stream: TcpStream,
        data: Arc<Vec<u8>>,
        mode: TestRangeMode,
    ) -> std::io::Result<()> {
        let request = read_http_request(&mut stream).await?;
        let request_text = String::from_utf8_lossy(&request);
        let request_line = request_text.lines().next().unwrap_or_default();
        let total = data.len() as u64;

        if request_line.starts_with("HEAD ") {
            let response =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
            stream.write_all(response.as_bytes()).await?;
            return Ok(());
        }

        if !request_line.starts_with("GET ") {
            stream
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n")
                .await?;
            return Ok(());
        }

        let Some((start, end)) = request_text.lines().find_map(parse_range_header) else {
            let response =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n");
            stream.write_all(response.as_bytes()).await?;
            stream.write_all(data.as_slice()).await?;
            return Ok(());
        };

        if start > end || end >= total {
            let response = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await?;
            return Ok(());
        }

        let start_index = usize::try_from(start).map_err(|error| {
            IoError::new(
                ErrorKind::InvalidInput,
                format!("range start too large: {error}"),
            )
        })?;
        let end_index = usize::try_from(end).map_err(|error| {
            IoError::new(
                ErrorKind::InvalidInput,
                format!("range end too large: {error}"),
            )
        })?;
        let body = &data[start_index..=end_index];
        let body_len = body.len();
        let extra_range_byte =
            matches!(mode, TestRangeMode::ExtraByteAfterRange) && !(start == 0 && end == 0);
        if extra_range_byte {
            let response = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await?;
            stream.write_all(body).await?;
            stream.write_all(&[0]).await?;
            return Ok(());
        }

        let response = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {body_len}\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(response.as_bytes()).await?;
        stream.write_all(body).await
    }

    async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await?;
            if read == 0 {
                return Ok(request);
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                return Ok(request);
            }
            if request.len() > 16 * 1024 {
                return Err(IoError::new(
                    ErrorKind::InvalidData,
                    "test HTTP request headers exceeded 16 KiB",
                ));
            }
        }
    }

    fn parse_range_header(line: &str) -> Option<(u64, u64)> {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("range") {
            return None;
        }
        let value = value.trim().strip_prefix("bytes=")?;
        let (start, end) = value.split_once('-')?;
        Some((start.parse().ok()?, end.parse().ok()?))
    }

    fn temp_test_path(file_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{}-{}", unique_task_id("download"), file_name))
    }

    fn unique_task_id(prefix: &str) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        format!("{prefix}-{}-{timestamp}", std::process::id())
    }

    async fn remove_test_file_if_exists(path: &Path) {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => panic!("failed to remove test file {}: {error}", path.display()),
        }
    }
}
