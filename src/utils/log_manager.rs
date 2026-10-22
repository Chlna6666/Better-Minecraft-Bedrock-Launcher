use crate::tasks;
use chrono::{Local, NaiveDate};
use std::cmp::Reverse;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tracing_subscriber::fmt::MakeWriter;

const ACTIVE_LOG_FILE: &str = "latest.log";
const PREVIOUS_LOG_FILE: &str = "previous.log";
const ARCHIVE_DIRECTORY: &str = "archive";
const PENDING_EXTENSION: &str = "pending";
const ARCHIVE_EXTENSION: &str = "zst";
const ARCHIVE_WAKE_CAPACITY: usize = 1;
const AUXILIARY_LOG_SETTLE_AGE: Duration = Duration::from_mins(10);

// Bounded log queue. Emitting threads only copy bytes and try_send; a dedicated
// writer thread owns file rotation, archive wake-ups and durable flush. Formatting
// and disk latency must never run on the UI/GPU-owner threads: a single slow append
// previously stalled a 37-element view render for ~9 ms while tracing was enabled.
const ASYNC_LOG_QUEUE_CAPACITY: usize = 4096;
const ASYNC_LOG_BATCH_BYTES: usize = 64 * 1024;
const ASYNC_LOG_BATCH_INTERVAL: Duration = Duration::from_millis(100);
const ASYNC_LOG_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

enum LogMessage {
    Bytes(Vec<u8>),
    Flush(SyncSender<()>),
}

// Shutdown/panic-time handle that can drain the queue without holding App state.
#[derive(Clone)]
struct LogFlushHandle {
    sender: SyncSender<LogMessage>,
    dropped: Arc<AtomicU64>,
}

static LOG_FLUSH_HANDLE: OnceLock<LogFlushHandle> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
struct LogRetentionPolicy {
    max_active_bytes: u64,
    max_previous_bytes: u64,
    max_archive_bytes: u64,
    max_archive_files: usize,
    max_archive_age: Duration,
    compression_level: i32,
}

impl From<&crate::config::config::LogManagementConfig> for LogRetentionPolicy {
    fn from(config: &crate::config::config::LogManagementConfig) -> Self {
        let config = config.normalized();
        Self {
            max_active_bytes: u64::from(config.active_file_size_mb) * 1024 * 1024,
            max_previous_bytes: 4 * 1024 * 1024,
            max_archive_bytes: u64::from(config.max_total_size_mb) * 1024 * 1024,
            max_archive_files: usize::try_from(config.max_archive_files).unwrap_or(usize::MAX),
            max_archive_age: Duration::from_secs(u64::from(config.retention_days) * 24 * 60 * 60),
            compression_level: config.compression_level,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ManagedLogWriter {
    state: Arc<Mutex<ActiveLog>>,
    sender: SyncSender<LogMessage>,
    dropped: Arc<AtomicU64>,
    _worker: Option<Arc<ArchiveWorker>>,
    _writer_thread: Option<Arc<JoinHandle<()>>>,
}

struct ActiveLog {
    path: PathBuf,
    archive_directory: PathBuf,
    file: Option<File>,
    bytes_written: u64,
    opened_on: NaiveDate,
    next_sequence: u32,
    policy: Arc<RwLock<LogRetentionPolicy>>,
    archive_wake: Option<mpsc::Sender<()>>,
}

struct ArchiveWorker {
    _abort_handle: AbortHandle,
}

struct LogController {
    policy: Arc<RwLock<LogRetentionPolicy>>,
    archive_wake: mpsc::Sender<()>,
}

static LOG_CONTROLLER: OnceLock<LogController> = OnceLock::new();

pub(crate) struct BufferedLogEvent {
    sender: SyncSender<LogMessage>,
    dropped: Arc<AtomicU64>,
    bytes: Vec<u8>,
}

impl ManagedLogWriter {
    pub(crate) fn initialize(
        logs_directory: &Path,
        config: &crate::config::config::LogManagementConfig,
    ) -> io::Result<Self> {
        let policy = LogRetentionPolicy::from(config);
        fs::create_dir_all(logs_directory)?;
        let archive_directory = logs_directory.join(ARCHIVE_DIRECTORY);
        fs::create_dir_all(&archive_directory)?;

        let active_path = logs_directory.join(ACTIVE_LOG_FILE);
        let previous_path = logs_directory.join(PREVIOUS_LOG_FILE);
        if active_path.is_file() {
            copy_file_tail(&active_path, &previous_path, policy.max_previous_bytes)?;
            stage_log_file(&active_path, &archive_directory, 0)?;
        }

        let file = open_active_log(&active_path)?;
        let bytes_written = file.metadata()?.len();
        let policy = Arc::new(RwLock::new(policy));
        let (archive_wake, worker) =
            match start_archive_worker(logs_directory.to_path_buf(), Arc::clone(&policy)) {
                Ok((sender, worker)) => (Some(sender), Some(Arc::new(worker))),
                Err(error) => {
                    eprintln!("Failed to start log archive maintenance: {error}");
                    (None, None)
                }
            };

        let state = Arc::new(Mutex::new(ActiveLog {
            path: active_path,
            archive_directory,
            file: Some(file),
            bytes_written,
            opened_on: Local::now().date_naive(),
            next_sequence: 1,
            policy: Arc::clone(&policy),
            archive_wake,
        }));
        let (sender, receiver) = std::sync::mpsc::sync_channel(ASYNC_LOG_QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer_thread = start_async_writer(Arc::clone(&state), receiver, Arc::clone(&dropped))?;
        let writer = Self {
            state,
            sender: sender.clone(),
            dropped: dropped.clone(),
            _worker: worker,
            _writer_thread: Some(Arc::new(writer_thread)),
        };
        if let Some(archive_wake) = writer
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .archive_wake
            .clone()
            && LOG_CONTROLLER
                .set(LogController {
                    policy,
                    archive_wake,
                })
                .is_err()
        {
            eprintln!("Log manager controller was already initialized");
        }
        let _ = LOG_FLUSH_HANDLE.set(LogFlushHandle { sender, dropped });
        writer.wake_archive_worker();
        Ok(writer)
    }

    fn wake_archive_worker(&self) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.wake_archive_worker();
    }
}

fn start_async_writer(
    state: Arc<Mutex<ActiveLog>>,
    receiver: std::sync::mpsc::Receiver<LogMessage>,
    dropped: Arc<AtomicU64>,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("bmcbl-log-writer".to_owned())
        .spawn(move || run_log_writer(state, receiver, dropped))
}

fn run_log_writer(
    state: Arc<Mutex<ActiveLog>>,
    receiver: std::sync::mpsc::Receiver<LogMessage>,
    dropped: Arc<AtomicU64>,
) {
    let mut batch: Vec<u8> = Vec::with_capacity(ASYNC_LOG_BATCH_BYTES);
    let mut last_write = Instant::now();
    loop {
        if batch.is_empty() {
            match receiver.recv() {
                Ok(message) => {
                    handle_log_message(message, &state, &mut batch, &dropped, &mut last_write)
                }
                Err(_) => break,
            }
            continue;
        }

        match receiver.recv_timeout(ASYNC_LOG_BATCH_INTERVAL) {
            Ok(message) => {
                handle_log_message(message, &state, &mut batch, &dropped, &mut last_write)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                write_log_batch(&state, &mut batch, &dropped);
                last_write = Instant::now();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                write_log_batch(&state, &mut batch, &dropped);
                break;
            }
        }
    }
}

fn handle_log_message(
    message: LogMessage,
    state: &Mutex<ActiveLog>,
    batch: &mut Vec<u8>,
    dropped: &AtomicU64,
    last_write: &mut Instant,
) {
    match message {
        LogMessage::Bytes(bytes) => {
            batch.extend_from_slice(&bytes);
            if batch.len() >= ASYNC_LOG_BATCH_BYTES
                || last_write.elapsed() >= ASYNC_LOG_BATCH_INTERVAL
            {
                write_log_batch(state, batch, dropped);
                *last_write = Instant::now();
            }
        }
        LogMessage::Flush(acknowledge) => {
            write_log_batch(state, batch, dropped);
            let flushed = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .flush_file();
            if let Err(error) = flushed {
                eprintln!("Failed to flush application log file: {error}");
            }
            let _ = acknowledge.try_send(());
            *last_write = Instant::now();
        }
    }
}

fn write_log_batch(state: &Mutex<ActiveLog>, batch: &mut Vec<u8>, dropped: &AtomicU64) {
    let dropped_events = dropped.swap(0, Ordering::Relaxed);
    if dropped_events > 0 {
        batch.extend_from_slice(
            format!(
                "bmcbl log writer: dropped {dropped_events} events because the async queue was full\n"
            )
            .as_bytes(),
        );
    }
    if batch.is_empty() {
        return;
    }

    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Err(error) = state.write_event(batch) {
        eprintln!("Failed to write application log event: {error}");
    }
    batch.clear();
}

fn flush_log_sender(sender: &SyncSender<LogMessage>, timeout: Duration) -> bool {
    // Capacity 1 keeps the acknowledgement valid even if the writer finishes
    // before this thread starts waiting. A rendezvous channel (capacity 0)
    // would drop that fast acknowledgement and report a false flush timeout.
    let (acknowledge, acknowledged) = std::sync::mpsc::sync_channel(1);
    let deadline = Instant::now() + timeout;
    loop {
        match sender.try_send(LogMessage::Flush(acknowledge.clone())) {
            Ok(()) => break,
            Err(TrySendError::Full(_)) => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }

    acknowledged
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .is_ok()
}

/// Drains queued records from shutdown, panic or explicit diagnostic paths.
/// Ordinary emitters never call this function.
pub(crate) fn flush_async_logs(timeout: Duration) -> bool {
    let Some(handle) = LOG_FLUSH_HANDLE.get() else {
        return true;
    };
    let dropped = handle.dropped.swap(0, Ordering::Relaxed);
    let flushed = flush_log_sender(&handle.sender, timeout);
    if dropped > 0 {
        eprintln!("BMCBL log queue dropped {dropped} events because the writer could not keep up");
    }
    flushed
}

/// Bounded shutdown flush used by the application exit path.
pub(crate) fn flush_async_logs_now() {
    let _ = flush_async_logs(ASYNC_LOG_FLUSH_TIMEOUT);
}

impl<'writer> MakeWriter<'writer> for ManagedLogWriter {
    type Writer = BufferedLogEvent;

    fn make_writer(&'writer self) -> Self::Writer {
        BufferedLogEvent {
            sender: self.sender.clone(),
            dropped: Arc::clone(&self.dropped),
            bytes: Vec::with_capacity(512),
        }
    }
}

impl Write for BufferedLogEvent {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for BufferedLogEvent {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }

        // The emitting thread must never wait on disk, rotation or a mutex held
        // by the writer. A full queue drops the record and increments a counter
        // that the writer reports in the next successful batch.
        match self
            .sender
            .try_send(LogMessage::Bytes(std::mem::take(&mut self.bytes)))
        {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

impl ActiveLog {
    fn write_event(&mut self, bytes: &[u8]) -> io::Result<()> {
        let current_date = Local::now().date_naive();
        let incoming_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let max_active_bytes = self
            .policy
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .max_active_bytes;
        if should_rotate(
            self.bytes_written,
            incoming_bytes,
            self.opened_on,
            current_date,
            max_active_bytes,
        ) {
            self.rotate(current_date)?;
        }

        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("active log file is unavailable"))?;
        file.write_all(bytes)?;
        self.bytes_written = self.bytes_written.saturating_add(incoming_bytes);
        Ok(())
    }

    fn flush_file(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }

    fn rotate(&mut self, current_date: NaiveDate) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }

        let staged = if self.bytes_written > 0 {
            match stage_log_file(&self.path, &self.archive_directory, self.next_sequence) {
                Ok(()) => true,
                Err(error) => {
                    self.file = Some(open_active_log(&self.path)?);
                    self.bytes_written = self
                        .file
                        .as_ref()
                        .and_then(|file| file.metadata().ok())
                        .map_or(0, |metadata| metadata.len());
                    return Err(error);
                }
            }
        } else {
            false
        };

        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.file = Some(open_active_log(&self.path)?);
        self.bytes_written = 0;
        self.opened_on = current_date;
        if staged {
            self.wake_archive_worker();
        }
        Ok(())
    }

    fn wake_archive_worker(&self) {
        let Some(sender) = &self.archive_wake else {
            return;
        };
        match sender.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
            Err(mpsc::error::TrySendError::Closed(())) => {
                eprintln!("Log archive maintenance task stopped unexpectedly");
            }
        }
    }
}

fn should_rotate(
    bytes_written: u64,
    incoming_bytes: u64,
    opened_on: NaiveDate,
    current_date: NaiveDate,
    max_active_bytes: u64,
) -> bool {
    if bytes_written == 0 {
        return false;
    }
    opened_on != current_date || bytes_written.saturating_add(incoming_bytes) > max_active_bytes
}

fn open_active_log(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)
}

fn copy_file_tail(source: &Path, destination: &Path, max_bytes: u64) -> io::Result<()> {
    let mut source_file = File::open(source)?;
    let source_length = source_file.metadata()?.len();
    let start = source_length.saturating_sub(max_bytes);
    source_file.seek(SeekFrom::Start(start))?;

    let mut destination_file = File::create(destination)?;
    io::copy(&mut source_file, &mut destination_file)?;
    destination_file.flush()
}

fn stage_log_file(source: &Path, archive_directory: &Path, sequence: u32) -> io::Result<()> {
    let metadata = source.metadata()?;
    if metadata.len() == 0 {
        return Ok(());
    }

    let timestamp_millis = metadata
        .modified()
        .unwrap_or_else(|_| SystemTime::now())
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let process_id = std::process::id();
    for collision in 0..100_u16 {
        let pending_name = format!(
            "bmcbl-{timestamp_millis:013}-{process_id}-{sequence:08}-{collision:02}.log.pending"
        );
        let pending_path = archive_directory.join(pending_name);
        if pending_path.exists() {
            continue;
        }
        return fs::rename(source, pending_path);
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique pending log archive name",
    ))
}

fn start_archive_worker(
    logs_directory: PathBuf,
    policy: Arc<RwLock<LogRetentionPolicy>>,
) -> Result<(mpsc::Sender<()>, ArchiveWorker), String> {
    let (sender, mut receiver) = mpsc::channel(ARCHIVE_WAKE_CAPACITY);
    let task = tasks::runtime::spawn_io(async move {
        while receiver.recv().await.is_some() {
            let logs_directory = logs_directory.clone();
            let policy = *policy
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let result = tasks::runtime::run_archive_blocking(move || {
                maintain_archives(&logs_directory, policy)
            })
            .await;
            match result {
                Ok(Ok(report)) => {
                    if report.archived > 0 || report.removed > 0 {
                        tracing::info!(
                            archived = report.archived,
                            removed = report.removed,
                            "log archive maintenance completed"
                        );
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!(%error, "log archive maintenance failed");
                }
                Err(error) => {
                    tracing::warn!(%error, "log archive maintenance task failed");
                }
            }
        }
    })?;
    let abort_handle = task.abort_handle();
    drop(task);
    Ok((
        sender,
        ArchiveWorker {
            _abort_handle: abort_handle,
        },
    ))
}

#[derive(Default)]
struct MaintenanceReport {
    archived: usize,
    removed: usize,
}

fn maintain_archives(
    logs_directory: &Path,
    policy: LogRetentionPolicy,
) -> io::Result<MaintenanceReport> {
    let archive_directory = logs_directory.join(ARCHIVE_DIRECTORY);
    let mut report = MaintenanceReport::default();
    stage_legacy_and_auxiliary_logs(logs_directory, &archive_directory)?;
    for pending_path in files_with_extension(&archive_directory, PENDING_EXTENSION)? {
        archive_pending_file(&pending_path, policy.compression_level)?;
        report.archived = report.archived.saturating_add(1);
    }
    report.removed = enforce_retention(&archive_directory, policy)?;
    Ok(report)
}

fn stage_legacy_and_auxiliary_logs(
    logs_directory: &Path,
    archive_directory: &Path,
) -> io::Result<()> {
    let mut sequence = 10_000_u32;
    for entry in fs::read_dir(logs_directory)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file()
            || path.file_name().is_some_and(|name| {
                name == ACTIVE_LOG_FILE
                    || name == PREVIOUS_LOG_FILE
                    || name == "ui_foreground_stall.log"
            })
            || path.extension().is_none_or(|extension| extension != "log")
        {
            continue;
        }
        stage_log_file(&path, archive_directory, sequence)?;
        sequence = sequence.wrapping_add(1);
    }

    let proton_directory = logs_directory.join("proton");
    if !proton_directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(proton_directory)? {
        let path = entry?.path();
        if !path.is_file() || is_recently_modified(&path, AUXILIARY_LOG_SETTLE_AGE) {
            continue;
        }
        stage_log_file(&path, archive_directory, sequence)?;
        sequence = sequence.wrapping_add(1);
    }
    Ok(())
}

fn archive_pending_file(pending_path: &Path, compression_level: i32) -> io::Result<()> {
    let archive_path = pending_path.with_extension(ARCHIVE_EXTENSION);
    if archive_path.is_file() {
        return fs::remove_file(pending_path);
    }
    let temporary_path = pending_path.with_extension("zst.tmp");
    let mut source = File::open(pending_path)?;
    let temporary = File::create(&temporary_path)?;
    let mut encoder = zstd::stream::write::Encoder::new(temporary, compression_level)?;
    io::copy(&mut source, &mut encoder)?;
    let mut compressed = encoder.finish()?;
    compressed.flush()?;
    fs::rename(&temporary_path, &archive_path)?;
    fs::remove_file(pending_path)
}

fn enforce_retention(archive_directory: &Path, policy: LogRetentionPolicy) -> io::Result<usize> {
    let now = SystemTime::now();
    let mut records = Vec::new();
    let mut removed = 0_usize;

    for path in files_with_extension(archive_directory, ARCHIVE_EXTENSION)? {
        let metadata = path.metadata()?;
        let modified = archive_source_time(&path)
            .or_else(|| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let expired = now
            .duration_since(modified)
            .is_ok_and(|age| age > policy.max_archive_age);
        if expired {
            fs::remove_file(&path)?;
            removed = removed.saturating_add(1);
        } else {
            records.push(ArchiveRecord {
                path,
                bytes: metadata.len(),
                modified,
            });
        }
    }

    records.sort_by_key(|record| Reverse(record.modified));
    let mut retained_bytes = 0_u64;
    for (index, record) in records.into_iter().enumerate() {
        let exceeds_count = index >= policy.max_archive_files;
        let exceeds_bytes = retained_bytes.saturating_add(record.bytes) > policy.max_archive_bytes;
        if exceeds_count || exceeds_bytes {
            fs::remove_file(record.path)?;
            removed = removed.saturating_add(1);
        } else {
            retained_bytes = retained_bytes.saturating_add(record.bytes);
        }
    }

    Ok(removed)
}

fn archive_source_time(path: &Path) -> Option<SystemTime> {
    let filename = path.file_name()?.to_str()?;
    let timestamp_millis = filename.split('-').nth(1)?.parse::<u64>().ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_millis(timestamp_millis))
}

struct ArchiveRecord {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

fn files_with_extension(directory: &Path, extension: &str) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|value| value == extension) {
            paths.push(path);
        }
    }
    Ok(paths)
}

pub(crate) fn apply_runtime_config(config: &crate::config::config::LogManagementConfig) {
    let Some(controller) = LOG_CONTROLLER.get() else {
        return;
    };
    *controller
        .policy
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = LogRetentionPolicy::from(config);
    match controller.archive_wake.try_send(()) {
        Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
        Err(mpsc::error::TrySendError::Closed(())) => {
            eprintln!("Could not apply log retention settings: archive worker stopped");
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LogStorageStats {
    pub file_count: usize,
    pub archive_count: usize,
    pub pending_count: usize,
    pub total_bytes: u64,
    pub active_bytes: u64,
    pub previous_bytes: u64,
    pub oldest_archive: Option<SystemTime>,
}

pub(crate) fn inspect_log_storage() -> io::Result<LogStorageStats> {
    let logs_directory = crate::utils::file_ops::logs_dir();
    let mut stats = LogStorageStats::default();
    inspect_directory(&logs_directory, &logs_directory, &mut stats)?;
    Ok(stats)
}

fn inspect_directory(
    logs_directory: &Path,
    directory: &Path,
    stats: &mut LogStorageStats,
) -> io::Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            inspect_directory(logs_directory, &path, stats)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let metadata = path.metadata()?;
        let bytes = metadata.len();
        stats.file_count = stats.file_count.saturating_add(1);
        stats.total_bytes = stats.total_bytes.saturating_add(bytes);
        if path == logs_directory.join(ACTIVE_LOG_FILE) {
            stats.active_bytes = bytes;
        } else if path == logs_directory.join(PREVIOUS_LOG_FILE) {
            stats.previous_bytes = bytes;
        }
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(ARCHIVE_EXTENSION) => {
                stats.archive_count = stats.archive_count.saturating_add(1);
                if let Some(modified) =
                    archive_source_time(&path).or_else(|| metadata.modified().ok())
                {
                    stats.oldest_archive = Some(
                        stats
                            .oldest_archive
                            .map_or(modified, |oldest| oldest.min(modified)),
                    );
                }
            }
            Some(PENDING_EXTENSION) => {
                stats.pending_count = stats.pending_count.saturating_add(1);
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LogCleanupReport {
    pub removed_files: usize,
    pub freed_bytes: u64,
    pub failed_files: usize,
}

pub(crate) fn clear_inactive_logs() -> io::Result<LogCleanupReport> {
    let logs_directory = crate::utils::file_ops::logs_dir();
    let active_path = logs_directory.join(ACTIVE_LOG_FILE);
    let mut report = LogCleanupReport::default();
    clear_directory_files(&logs_directory, &active_path, &mut report)?;
    Ok(report)
}

fn clear_directory_files(
    directory: &Path,
    active_path: &Path,
    report: &mut LogCleanupReport,
) -> io::Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            clear_directory_files(&path, active_path, report)?;
            continue;
        }
        if !path.is_file() || path == active_path {
            continue;
        }
        if path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "proton")
            && is_recently_modified(&path, AUXILIARY_LOG_SETTLE_AGE)
        {
            continue;
        }
        let bytes = path.metadata().map_or(0, |metadata| metadata.len());
        match fs::remove_file(&path) {
            Ok(()) => {
                report.removed_files = report.removed_files.saturating_add(1);
                report.freed_bytes = report.freed_bytes.saturating_add(bytes);
            }
            Err(error) => {
                report.failed_files = report.failed_files.saturating_add(1);
                eprintln!("Failed to remove inactive log {}: {error}", path.display());
            }
        }
    }
    Ok(())
}

fn is_recently_modified(path: &Path, settle_age: Duration) -> bool {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < settle_age)
}

#[cfg(test)]
#[path = "log_manager_tests.rs"]
mod tests;
