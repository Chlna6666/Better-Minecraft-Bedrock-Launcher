// src/archive/zip.rs
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration as StdDuration;
use std::time::Instant as StdInstant;

use zip::ZipArchive;

use tokio::task;
use tracing::{debug, info};

use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{
    TaskControl, TaskVisualization, finish_task, is_cancelled, is_cancelled_fast,
    set_task_visualization, set_total, task_control, task_visualization_enabled, update_progress,
};

/// 解压进度/可视化上报节流：每累计 1MB 或每 200ms 上报一次
const EXTRACT_PROGRESS_EMIT_BYTES: u64 = 1024 * 1024;
const EXTRACT_EMIT_INTERVAL_MS: u64 = 200;
const EXTRACT_COPY_BUFFER_SIZE: usize = 64 * 1024;

/// 预扫描阶段收集的文件条目计划
struct FileEntryPlan {
    index: usize,
    size: u64,
    out_path: PathBuf,
    display_name: String,
}

/// 并行解压的共享上下文（生命周期限定在协调线程栈上，worker 只持引用）
struct ParallelExtractContext<'a> {
    archive_path: &'a Path,
    task_id: &'a str,
    control: Option<&'a TaskControl>,
    force_replace: bool,
    total_bytes: u64,
    entry_total: u64,
    file_entries: &'a [FileEntryPlan],
    /// 按 chunk 索引分发工作：worker 领取一段连续的条目区间，
    /// 减少每个 worker 重复打开归档的次数
    next_chunk: AtomicUsize,
    chunk_size: usize,
    worker_total: usize,
    active_workers: AtomicUsize,
    finished_entries: AtomicU64,
    /// 已解压但尚未上报的字节数（由抢到上报权的 worker 统一冲刷）
    pending_bytes: AtomicU64,
    last_progress_emit_ms: AtomicU64,
    last_visual_emit_ms: AtomicU64,
    started_at: StdInstant,
    error: StdMutex<Option<String>>,
    has_error: AtomicBool,
}

impl ParallelExtractContext<'_> {
    fn is_cancelled(&self) -> bool {
        self.control.is_some_and(is_cancelled_fast)
    }

    fn has_failed(&self) -> bool {
        self.has_error.load(Ordering::Relaxed)
    }

    fn record_error(&self, message: String) {
        let mut slot = self
            .error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(message);
        }
        self.has_error.store(true, Ordering::Relaxed);
    }

    fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    /// 累计进度并按 1MB/200ms 节流上报；通过 CAS 抢占上报权，避免多 worker 重复发事件。
    fn add_progress(&self, bytes: u64, force: bool) {
        if bytes > 0 {
            self.pending_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
        let pending = self.pending_bytes.load(Ordering::Relaxed);
        if pending == 0 {
            return;
        }
        let now = self.now_ms();
        let last = self.last_progress_emit_ms.load(Ordering::Relaxed);
        if !force
            && pending < EXTRACT_PROGRESS_EMIT_BYTES
            && now.saturating_sub(last) < EXTRACT_EMIT_INTERVAL_MS
        {
            return;
        }
        if self
            .last_progress_emit_ms
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
            && !force
        {
            return;
        }
        let delta = self.pending_bytes.swap(0, Ordering::Relaxed);
        if delta > 0 {
            update_progress(
                self.task_id,
                delta,
                Some(self.total_bytes),
                Some("extracting"),
            );
        }
    }

    /// 可视化事件按 200ms 节流发送（原实现每个条目发两次）。
    fn maybe_emit_visualization(&self, current_item: &str, force: bool) {
        if !task_visualization_enabled() {
            return;
        }
        let now = self.now_ms();
        let last = self.last_visual_emit_ms.load(Ordering::Relaxed);
        if !force && now.saturating_sub(last) < EXTRACT_EMIT_INTERVAL_MS {
            return;
        }
        if self
            .last_visual_emit_ms
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
            && !force
        {
            return;
        }
        set_task_visualization(
            self.task_id,
            Some(TaskVisualization {
                worker_total: Some(self.worker_total as u32),
                worker_active: Some(self.active_workers.load(Ordering::Relaxed) as u32),
                unit_label: Some("文件".to_string()),
                unit_total: Some(self.entry_total),
                unit_done: Some(self.finished_entries.load(Ordering::Relaxed)),
                current_item: Some(current_item.to_string()),
                downloaded_ranges: None,
                threads: None,
            }),
        );
    }
}

/// 创建输出文件。返回 Ok(None) 表示文件已存在且不需要覆盖（视为完成）。
/// 通过 create/create_new 的返回值区分存在性，避免每条目一次 exists() stat。
fn create_output_file(out_path: &Path, force_replace: bool) -> Result<Option<File>, String> {
    if force_replace {
        match File::create(out_path) {
            Ok(file) => Ok(Some(file)),
            Err(_) if out_path.is_dir() => {
                fs::remove_dir_all(out_path).map_err(|error| {
                    format!("删除已有目录失败: {} ({error})", out_path.display())
                })?;
                File::create(out_path)
                    .map(Some)
                    .map_err(|error| format!("创建文件失败: {} ({error})", out_path.display()))
            }
            Err(error) => Err(format!("创建文件失败: {} ({error})", out_path.display())),
        }
    } else {
        match File::create_new(out_path) {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(format!("创建文件失败: {} ({error})", out_path.display())),
        }
    }
}

fn open_archive_for_worker(archive_path: &Path) -> Result<ZipArchive<File>, String> {
    let file = File::open(archive_path)
        .map_err(|error| format!("打开安装包失败：{} ({})", error, archive_path.display()))?;
    ZipArchive::new(file).map_err(|error| {
        format!(
            "创建 ZipArchive 失败：{} ({})",
            error,
            archive_path.display()
        )
    })
}

fn extract_single_entry(
    context: &ParallelExtractContext<'_>,
    archive: &mut ZipArchive<File>,
    plan: &FileEntryPlan,
    copy_buffer: &mut [u8],
) -> Result<(), String> {
    context.maybe_emit_visualization(&plan.display_name, false);

    let mut entry = archive
        .by_index(plan.index)
        .map_err(|error| format!("读取 zip 条目失败: {} ({error})", plan.display_name))?;

    let file = match create_output_file(&plan.out_path, context.force_replace)? {
        Some(file) => file,
        None => {
            // 已存在且不强制替换：视为已完成此 entry 的大小
            context.finished_entries.fetch_add(1, Ordering::Relaxed);
            context.add_progress(plan.size, false);
            return Ok(());
        }
    };

    let mut writer = BufWriter::new(file);
    loop {
        // 取消检查：缓存的 TaskControl 原子标志，无全局锁
        if context.is_cancelled() {
            return Ok(());
        }

        let bytes_read = entry
            .read(copy_buffer)
            .map_err(|error| format!("读取压缩条目失败: {} ({error})", plan.display_name))?;
        if bytes_read == 0 {
            break;
        }

        writer
            .write_all(&copy_buffer[..bytes_read])
            .map_err(|error| format!("写入文件失败: {} ({error})", plan.out_path.display()))?;
        context.add_progress(bytes_read as u64, false);
    }

    writer
        .flush()
        .map_err(|error| format!("刷新文件失败: {} ({error})", plan.out_path.display()))?;
    context.finished_entries.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// 单个解压 worker：领取 chunk（连续条目区间）直至工作耗尽。
/// ZipArchive 不能跨线程共享，每个 worker 独立打开自己的 File + ZipArchive；
/// 惰性打开：确认真的领到工作后才打开，避免空跑 worker 重复解析中央目录。
fn extraction_worker(context: &ParallelExtractContext<'_>) {
    context.active_workers.fetch_add(1, Ordering::Relaxed);
    let mut archive: Option<ZipArchive<File>> = None;
    let mut copy_buffer = vec![0u8; EXTRACT_COPY_BUFFER_SIZE];

    loop {
        if context.is_cancelled() || context.has_failed() {
            break;
        }
        let chunk_index = context.next_chunk.fetch_add(1, Ordering::Relaxed);
        let start = chunk_index.saturating_mul(context.chunk_size);
        if start >= context.file_entries.len() {
            break;
        }
        let end = (start + context.chunk_size).min(context.file_entries.len());

        if archive.is_none() {
            match open_archive_for_worker(context.archive_path) {
                Ok(opened) => archive = Some(opened),
                Err(message) => {
                    context.record_error(message);
                    break;
                }
            }
        }
        let Some(archive) = archive.as_mut() else {
            break;
        };

        for plan in &context.file_entries[start..end] {
            if context.is_cancelled() || context.has_failed() {
                break;
            }
            if let Err(message) = extract_single_entry(context, archive, plan, &mut copy_buffer) {
                context.record_error(message);
                break;
            }
        }
    }

    context.active_workers.fetch_sub(1, Ordering::Relaxed);
}

fn extract_zip_parallel_blocking(
    archive_path: &Path,
    destination: &str,
    force_replace: bool,
    task_id: &str,
) -> Result<(), String> {
    let mut archive = open_archive_for_worker(archive_path)?;

    // 1) 预扫描中央目录：收集条目信息并计算总大小
    //    display_name 在此处一次性转换，避免解压循环里重复 to_string_lossy 分配。
    let dest_root = Path::new(destination);
    let entry_count = archive.len();
    let mut total: u64 = 0;
    let mut file_entries: Vec<FileEntryPlan> = Vec::with_capacity(entry_count);
    let mut dir_paths = Vec::new();
    // 归档内重复的输出路径必须去重：并行时两个 worker 写同一文件会互相踩踏。
    // 语义与串行版本一致：force_replace 时后出现的条目覆盖先出现的，否则先出现的生效。
    let mut planned_paths: std::collections::HashMap<PathBuf, usize> =
        std::collections::HashMap::with_capacity(entry_count);
    let mut skipped_duplicate_bytes: u64 = 0;
    let mut skipped_duplicate_entries: u64 = 0;
    for i in 0..entry_count {
        let entry = archive
            .by_index(i)
            .map_err(|error| format!("读取 zip 条目 #{i} 失败: {error}"))?;
        let size = entry.size();
        let name = entry
            .mangled_name()
            .map_err(|error| format!("解析 zip 条目路径失败 #{i}: {error}"))?;
        let is_dir = entry.is_dir();
        total = total.saturating_add(size);

        let out_path = dest_root.join(&name);
        if is_dir {
            dir_paths.push(out_path);
            continue;
        }
        match planned_paths.entry(out_path) {
            std::collections::hash_map::Entry::Occupied(existing) => {
                let replaced_size = if force_replace {
                    let plan = &mut file_entries[*existing.get()];
                    let previous_size = plan.size;
                    plan.index = i;
                    plan.size = size;
                    plan.display_name = name.to_string_lossy().into_owned();
                    previous_size
                } else {
                    size
                };
                skipped_duplicate_bytes = skipped_duplicate_bytes.saturating_add(replaced_size);
                skipped_duplicate_entries += 1;
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                let plan_out_path = slot.key().clone();
                slot.insert(file_entries.len());
                file_entries.push(FileEntryPlan {
                    index: i,
                    size,
                    display_name: name.to_string_lossy().into_owned(),
                    out_path: plan_out_path,
                });
            }
        }
    }
    drop(planned_paths);
    drop(archive);

    // 设置 task_manager 的 total（线程安全）
    set_total(task_id, Some(total));
    let entry_total = entry_count as u64;

    // 取一次 Arc<TaskControl>，后续取消检查走原子标志（无全局 RwLock）
    let control = task_control(task_id);
    let cancelled = || control.as_deref().is_some_and(is_cancelled_fast);

    if task_visualization_enabled() {
        set_task_visualization(
            task_id,
            Some(TaskVisualization {
                worker_total: Some(1),
                worker_active: Some(1),
                unit_label: Some("文件".to_string()),
                unit_total: Some(entry_total),
                unit_done: Some(0),
                current_item: Some("等待解压文件".to_string()),
                downloaded_ranges: None,
                threads: None,
            }),
        );
    }

    let start = StdInstant::now();

    // 2) 串行建目录：目录条目 + 所有文件的父目录。
    //    HashSet 去重，消除逐条目 create_dir_all 带来的冗余 stat。
    let mut created_dirs: HashSet<PathBuf> = HashSet::new();
    for dir_path in &dir_paths {
        if cancelled() {
            debug!("解压已被取消（检测到 task cancelled）");
            finish_task(task_id, "cancelled", Some("user cancelled".into()));
            return Ok(());
        }
        if created_dirs.insert(dir_path.clone()) {
            fs::create_dir_all(dir_path)
                .map_err(|error| format!("创建目录失败: {} ({error})", dir_path.display()))?;
        }
    }
    for plan in &file_entries {
        if cancelled() {
            debug!("解压已被取消（检测到 task cancelled）");
            finish_task(task_id, "cancelled", Some("user cancelled".into()));
            return Ok(());
        }
        if let Some(parent) = plan.out_path.parent()
            && !created_dirs.contains(parent)
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建父目录失败: {} ({error})", parent.display()))?;
            created_dirs.insert(parent.to_path_buf());
        }
    }
    drop(created_dirs);

    // 3) 并行解压文件条目（AppRuntime 的 rayon 池）
    let logical_threads = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2);
    let worker_total = logical_threads.min(file_entries.len().max(1)).max(1);
    let chunk_size = (file_entries.len() / (worker_total * 8)).max(1);

    let context = ParallelExtractContext {
        archive_path,
        task_id,
        control: control.as_deref(),
        force_replace,
        total_bytes: total,
        entry_total,
        file_entries: &file_entries,
        next_chunk: AtomicUsize::new(0),
        chunk_size,
        worker_total,
        active_workers: AtomicUsize::new(0),
        // 目录条目与去重跳过的重复条目直接计为已完成
        finished_entries: AtomicU64::new(dir_paths.len() as u64 + skipped_duplicate_entries),
        pending_bytes: AtomicU64::new(skipped_duplicate_bytes),
        last_progress_emit_ms: AtomicU64::new(0),
        last_visual_emit_ms: AtomicU64::new(0),
        started_at: start,
        error: StdMutex::new(None),
        has_error: AtomicBool::new(false),
    };

    if !file_entries.is_empty() {
        let context_ref = &context;
        let ran_on_pool = crate::tasks::runtime::install_cpu(|| {
            rayon::scope(|scope| {
                for _ in 0..worker_total {
                    scope.spawn(|_| extraction_worker(context_ref));
                }
            })
        })
        .is_ok();
        if !ran_on_pool {
            // 运行时不可用（如个别测试环境）：退化为当前线程串行解压
            extraction_worker(context_ref);
        }
    }

    if cancelled() {
        debug!("解压已被取消（检测到 task cancelled）");
        finish_task(task_id, "cancelled", Some("user cancelled".into()));
        return Ok(());
    }

    if let Some(error) = context
        .error
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        return Err(error);
    }

    // 冲刷剩余进度，并保持与旧实现一致的收尾进度事件
    context.add_progress(0, true);
    update_progress(task_id, 0, Some(total), Some("extracting"));

    info!(
        "解压完成，总计 {} bytes, {} 个条目, {} 线程, 总耗时 {:.2} 秒",
        total,
        entry_total,
        worker_total,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

/// 从磁盘路径解压 zip 到 destination（并行版本）。
///
/// 与 [`extract_zip`] 的任务事件语义一致（开始/完成/失败/取消由调用方与本函数
/// 按相同约定处理），但按条目区间在 AppRuntime 的 rayon 池上并行 inflate + 写盘。
pub async fn extract_zip_from_path(
    archive_path: impl AsRef<Path>,
    destination: &str,
    force_replace: bool,
    task_id: String,
) -> Result<CoreResult<()>, CoreError> {
    crate::archive::register_archive_task_stage_labels();
    let archive_path = archive_path.as_ref().to_path_buf();
    let dest_string = destination.to_string();
    let task_id_for_block = task_id.clone();

    let handle = task::spawn_blocking(move || -> Result<(), String> {
        extract_zip_parallel_blocking(
            &archive_path,
            &dest_string,
            force_replace,
            &task_id_for_block,
        )
    });

    match handle.await {
        Ok(Ok(())) => {
            if is_cancelled(&task_id) {
                return Ok(CoreResult::Cancelled);
            }
            Ok(CoreResult::Success(()))
        }
        Ok(Err(error)) => Err(CoreError::Other(error)),
        Err(join_err) => Err(CoreError::Other(format!("join error: {}", join_err))),
    }
}

/// 将 archive 解压到 destination（串行兼容版本）。
/// 注意：新增参数 `task_id`（拥有所有权的 String），用于取消/进度上报。
/// 已有归档文件路径时请优先使用 [`extract_zip_from_path`]（并行解压）。
pub async fn extract_zip<R: Read + Seek + Send + 'static>(
    mut archive: ZipArchive<R>,
    destination: &str,
    force_replace: bool,
    task_id: String,
) -> Result<CoreResult<()>, CoreError> {
    crate::archive::register_archive_task_stage_labels();
    // spawn_blocking 内执行实际解压（IO 密集）
    let dest_string = destination.to_string();
    let task_id_clone_for_block = task_id.clone();

    let handle = task::spawn_blocking(move || -> Result<(), String> {
        // 1) 收集条目并计算总大小
        let mut total: u64 = 0;
        let mut entries = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let e = archive
                .by_index(i)
                .map_err(|error| format!("读取 zip 条目 #{i} 失败: {error}"))?;
            let size = e.size();
            let name = e
                .mangled_name()
                .map_err(|error| format!("解析 zip 条目路径失败 #{i}: {error}"))?;
            let is_dir = e.is_dir();
            entries.push((i, name, size, is_dir));
            total = total.saturating_add(size);
        }

        // 设置 task_manager 的 total（线程安全）
        set_total(&task_id_clone_for_block, Some(total));
        let entry_total = entries.len() as u64;
        if task_visualization_enabled() {
            set_task_visualization(
                &task_id_clone_for_block,
                Some(TaskVisualization {
                    worker_total: Some(1),
                    worker_active: Some(1),
                    unit_label: Some("文件".to_string()),
                    unit_total: Some(entry_total),
                    unit_done: Some(0),
                    current_item: Some("等待解压文件".to_string()),
                    downloaded_ranges: None,
                    threads: None,
                }),
            );
        }

        let start = StdInstant::now();
        let mut pending_progress = 0u64;
        let mut finished_entries = 0u64;
        let mut last_progress_emit = StdInstant::now();

        // 逐项解压
        for (idx, name, size, is_dir) in entries {
            let display_name = name.to_string_lossy().to_string();
            if task_visualization_enabled() {
                set_task_visualization(
                    &task_id_clone_for_block,
                    Some(TaskVisualization {
                        worker_total: Some(1),
                        worker_active: Some(1),
                        unit_label: Some("文件".to_string()),
                        unit_total: Some(entry_total),
                        unit_done: Some(finished_entries),
                        current_item: Some(display_name.clone()),
                        downloaded_ranges: None,
                        threads: None,
                    }),
                );
            }

            // 取消检查（使用 task_manager）
            if is_cancelled(&task_id_clone_for_block) {
                debug!("解压已被取消（检测到 task cancelled）");
                // 写最终状态并返回（注意：finish_task 也可以放到 async 侧，但这里放在 blocking 侧也可以）
                finish_task(
                    &task_id_clone_for_block,
                    "cancelled",
                    Some("user cancelled".into()),
                );
                return Ok(());
            }

            let mut entry = archive
                .by_index(idx)
                .map_err(|error| format!("读取 zip 条目失败: {display_name} ({error})"))?;
            let out_path = Path::new(&dest_string).join(&name);

            if is_dir {
                if let Some(p) = out_path.parent() {
                    fs::create_dir_all(p)
                        .map_err(|error| format!("创建父目录失败: {} ({error})", p.display()))?;
                }
                fs::create_dir_all(&out_path)
                    .map_err(|error| format!("创建目录失败: {} ({error})", out_path.display()))?;
                finished_entries = finished_entries.saturating_add(1);
                continue;
            }

            if out_path.exists() {
                if force_replace {
                    if out_path.is_dir() {
                        fs::remove_dir_all(&out_path).map_err(|error| {
                            format!("删除已有目录失败: {} ({error})", out_path.display())
                        })?;
                    } else {
                        fs::remove_file(&out_path).map_err(|error| {
                            format!("删除已有文件失败: {} ({error})", out_path.display())
                        })?;
                    }
                } else {
                    // 已存在：视为已完成此 entry 的大小
                    pending_progress = pending_progress.saturating_add(size);
                    if pending_progress >= 1024 * 1024
                        || last_progress_emit.elapsed() >= StdDuration::from_millis(200)
                    {
                        update_progress(
                            &task_id_clone_for_block,
                            pending_progress,
                            Some(total),
                            Some("extracting"),
                        );
                        pending_progress = 0;
                        last_progress_emit = StdInstant::now();
                    }
                    finished_entries = finished_entries.saturating_add(1);
                    continue;
                }
            }

            if let Some(p) = out_path.parent() {
                fs::create_dir_all(p)
                    .map_err(|error| format!("创建父目录失败: {} ({error})", p.display()))?;
            }

            let f = File::create(&out_path)
                .map_err(|error| format!("创建文件失败: {} ({error})", out_path.display()))?;
            let mut writer = BufWriter::new(f);

            let mut buf = [0u8; 64 * 1024];
            loop {
                // 取消检查
                if is_cancelled(&task_id_clone_for_block) {
                    debug!("解压在写入过程中被取消");
                    finish_task(
                        &task_id_clone_for_block,
                        "cancelled",
                        Some("user cancelled".into()),
                    );
                    return Ok(());
                }

                let bytes_read = entry
                    .read(&mut buf)
                    .map_err(|error| format!("读取压缩条目失败: {display_name} ({error})"))?;
                if bytes_read == 0 {
                    break;
                }

                writer
                    .write_all(&buf[..bytes_read])
                    .map_err(|error| format!("写入文件失败: {} ({error})", out_path.display()))?;
                pending_progress = pending_progress.saturating_add(bytes_read as u64);
                if pending_progress >= 1024 * 1024
                    || last_progress_emit.elapsed() >= StdDuration::from_millis(200)
                {
                    update_progress(
                        &task_id_clone_for_block,
                        pending_progress,
                        Some(total),
                        Some("extracting"),
                    );
                    pending_progress = 0;
                    last_progress_emit = StdInstant::now();
                }
            }

            writer
                .flush()
                .map_err(|error| format!("刷新文件失败: {} ({error})", out_path.display()))?;
            finished_entries = finished_entries.saturating_add(1);
            if task_visualization_enabled() {
                set_task_visualization(
                    &task_id_clone_for_block,
                    Some(TaskVisualization {
                        worker_total: Some(1),
                        worker_active: Some(1),
                        unit_label: Some("文件".to_string()),
                        unit_total: Some(entry_total),
                        unit_done: Some(finished_entries),
                        current_item: Some(display_name),
                        downloaded_ranges: None,
                        threads: None,
                    }),
                );
            }
        }

        if pending_progress > 0 {
            update_progress(
                &task_id_clone_for_block,
                pending_progress,
                Some(total),
                Some("extracting"),
            );
        }

        update_progress(&task_id_clone_for_block, 0, Some(total), Some("extracting"));

        info!(
            "解压完成，总计 {} bytes, 总耗时 {:.2} 秒",
            total,
            start.elapsed().as_secs_f64()
        );
        Ok(())
    });

    // 等待 blocking 任务完成
    match handle.await {
        Ok(Ok(())) => {
            // 如果调用方在 async 侧想再次检查取消可以用 is_cancelled(task_id)
            if is_cancelled(&task_id) {
                return Ok(CoreResult::Cancelled);
            }
            Ok(CoreResult::Success(()))
        }
        Ok(Err(error)) => Err(CoreError::Other(error)),
        Err(join_err) => Err(CoreError::Other(format!("join error: {}", join_err))),
    }
}
