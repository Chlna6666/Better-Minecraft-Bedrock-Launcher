use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::OnceCell;
use rayon::{ThreadPool, ThreadPoolBuilder};
use tokio::runtime::{Builder as TokioRuntimeBuilder, Handle, Runtime};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{debug, error, warn};

use super::task_manager::{
    TaskVisibility, create_task_with_details_and_visibility, finish_task, get_snapshot_arc,
    is_cancelled, register_task_abort_handle, remove_task, update_progress,
};

const DEFAULT_BLOCKING_TIMEOUT: Duration = Duration::from_secs(30);
const FALLBACK_LOGICAL_THREADS: usize = 2;
const MAX_CONCURRENT_DOWNLOAD_TASKS: usize = 2;
const MAX_CONCURRENT_ARCHIVE_TASKS: usize = 1;

static APP_RUNTIME: OnceCell<AppRuntime> = OnceCell::new();

pub struct AppRuntime {
    io: Runtime,
    download: Runtime,
    archive: Runtime,
    cpu: ThreadPool,
    blocking_slots: Arc<Semaphore>,
    download_slots: Arc<Semaphore>,
    archive_slots: Arc<Semaphore>,
}

impl AppRuntime {
    fn build() -> anyhow::Result<Self> {
        let logical_threads = std::thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(FALLBACK_LOGICAL_THREADS);
        let background_threads = background_thread_limit(logical_threads);
        let download_workers = logical_threads.saturating_sub(1).clamp(2, 6);
        let archive_workers = logical_threads.saturating_sub(1).clamp(2, 4);

        let io = TokioRuntimeBuilder::new_multi_thread()
            .enable_all()
            .worker_threads(background_threads)
            .max_blocking_threads(background_threads)
            .thread_stack_size(1024 * 1024)
            .thread_name("bmcbl-io")
            .build()?;
        let download = TokioRuntimeBuilder::new_multi_thread()
            .enable_all()
            .worker_threads(download_workers)
            .max_blocking_threads(logical_threads.saturating_add(2).clamp(4, 8))
            .thread_name("bmcbl-download")
            .build()?;
        let archive = TokioRuntimeBuilder::new_multi_thread()
            .enable_all()
            .worker_threads(archive_workers)
            .max_blocking_threads(logical_threads.saturating_add(1).clamp(4, 6))
            .thread_name("bmcbl-archive")
            .build()?;
        let cpu = ThreadPoolBuilder::new()
            .num_threads(logical_threads.max(1))
            .thread_name(|index| format!("bmcbl-cpu-{index}"))
            .build()?;

        Ok(Self {
            io,
            download,
            archive,
            cpu,
            blocking_slots: Arc::new(Semaphore::new(background_threads)),
            download_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOAD_TASKS)),
            archive_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_ARCHIVE_TASKS)),
        })
    }

    pub fn io_handle(&self) -> &Handle {
        self.io.handle()
    }

    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.io.block_on(future)
    }

    pub fn spawn_io<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.io.spawn(future)
    }

    fn spawn_download<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.download.spawn(future)
    }

    fn spawn_download_blocking<T, F>(&self, operation: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.download.spawn_blocking(operation)
    }

    fn spawn_archive<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.archive.spawn(future)
    }

    fn spawn_io_blocking<T, F>(&self, operation: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.io.spawn_blocking(operation)
    }

    fn spawn_archive_blocking<T, F>(&self, operation: F) -> JoinHandle<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.archive.spawn_blocking(operation)
    }

    fn spawn_cpu<T, F>(&self, operation: F) -> oneshot::Receiver<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.cpu.spawn(move || {
            if sender.send(operation()).is_err() {
                debug!("CPU task result receiver was released");
            }
        });
        receiver
    }
}

pub fn initialize_app_runtime() -> anyhow::Result<&'static AppRuntime> {
    APP_RUNTIME.get_or_try_init(AppRuntime::build)
}

pub fn app_runtime() -> Result<&'static AppRuntime, String> {
    APP_RUNTIME
        .get()
        .ok_or_else(|| "应用运行时尚未初始化".to_string())
}

pub fn io_handle() -> Result<Handle, String> {
    Ok(app_runtime()?.io_handle().clone())
}

pub fn spawn_io<F>(future: F) -> Result<JoinHandle<F::Output>, String>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    Ok(app_runtime()?.spawn_io(future))
}

pub fn spawn_download_blocking<T, F>(operation: F) -> Result<JoinHandle<T>, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    Ok(app_runtime()?.spawn_download_blocking(operation))
}

pub async fn run_cpu<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    app_runtime()?
        .spawn_cpu(operation)
        .await
        .map_err(|error| format!("CPU 任务异常结束: {error}"))
}

pub fn install_cpu<T, F>(operation: F) -> Result<T, String>
where
    T: Send,
    F: FnOnce() -> T + Send,
{
    Ok(app_runtime()?.cpu.install(operation))
}

pub async fn run_io_blocking<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    app_runtime()?
        .spawn_io_blocking(operation)
        .await
        .map_err(|error| format!("后台阻塞任务异常结束: {error}"))
}

pub async fn run_archive_blocking<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    app_runtime()?
        .spawn_archive_blocking(operation)
        .await
        .map_err(|error| format!("安装阻塞任务异常结束: {error}"))
}

pub fn spawn_download_task<F>(task_id: String, future: F) -> Result<AbortHandle, String>
where
    F: Future<Output = ()> + Send + 'static,
{
    let runtime = app_runtime()?;
    let task_slots = Arc::clone(&runtime.download_slots);
    let task_id_for_worker = task_id.clone();
    let join_handle = runtime.spawn_download(async move {
        update_progress(&task_id_for_worker, 0, None, Some("queued"));
        let Ok(_slot) = task_slots.acquire_owned().await else {
            if !is_cancelled(&task_id_for_worker) {
                finish_task(
                    &task_id_for_worker,
                    "error",
                    Some("下载队列已关闭".to_string()),
                );
            }
            return;
        };

        if !is_cancelled(&task_id_for_worker) {
            future.await;
        }
    });

    let abort_handle = join_handle.abort_handle();
    runtime.spawn_download(async move {
        match join_handle.await {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                error!(task_id = %task_id, ?error, "download task failed before finishing");
                finish_task(
                    &task_id,
                    "error",
                    Some(format!("下载任务异常结束: {error}")),
                );
            }
        }
    });

    Ok(abort_handle)
}

pub fn spawn_archive_task<F>(task_id: String, future: F) -> Result<(), String>
where
    F: Future<Output = ()> + Send + 'static,
{
    let runtime = app_runtime()?;
    let task_slots = Arc::clone(&runtime.archive_slots);
    let task_id_for_worker = task_id.clone();
    let join_handle = runtime.spawn_archive(async move {
        update_progress(&task_id_for_worker, 0, None, Some("queued"));
        let Ok(_slot) = task_slots.acquire_owned().await else {
            if !is_cancelled(&task_id_for_worker) {
                finish_task(
                    &task_id_for_worker,
                    "error",
                    Some("安装队列已关闭".to_string()),
                );
            }
            return;
        };

        if !is_cancelled(&task_id_for_worker) {
            future.await;
        }
    });

    runtime.spawn_archive(async move {
        match join_handle.await {
            Ok(()) => {
                let still_running =
                    get_snapshot_arc(&task_id).is_some_and(|snapshot| !snapshot.is_terminal());
                if still_running && !is_cancelled(&task_id) {
                    finish_task(&task_id, "error", Some("安装任务未完成就退出".to_string()));
                }
            }
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                error!(task_id = %task_id, ?error, "archive task failed before finishing");
                finish_task(
                    &task_id,
                    "error",
                    Some(format!("安装任务异常结束: {error}")),
                );
            }
        }
    });

    Ok(())
}

fn background_thread_limit(logical_threads: usize) -> usize {
    logical_threads.max(1).saturating_mul(2)
}

fn spawn_blocking_with_permit<T, F>(
    runtime: &AppRuntime,
    permit: OwnedSemaphorePermit,
    operation: F,
) -> JoinHandle<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    runtime.spawn_io_blocking(move || {
        let outcome = operation();
        drop(permit);
        outcome
    })
}

pub struct BlockingTaskOptions {
    pub title: &'static str,
    pub detail: Option<String>,
    pub timeout: Option<Duration>,
}

impl BlockingTaskOptions {
    pub fn hidden(title: &'static str) -> Self {
        Self {
            title,
            detail: None,
            timeout: Some(DEFAULT_BLOCKING_TIMEOUT),
        }
    }
}

struct HiddenTaskGuard {
    task_id: String,
    abort_handle: AbortHandle,
    completed: bool,
}

impl HiddenTaskGuard {
    fn complete(mut self, status: &str, message: Option<String>) {
        self.completed = true;
        finish_task(&self.task_id, status, message);
        remove_task(&self.task_id);
    }
}

impl Drop for HiddenTaskGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.abort_handle.abort();
        finish_task(
            &self.task_id,
            "cancelled",
            Some("调用方已取消任务".to_string()),
        );
        remove_task(&self.task_id);
    }
}

pub async fn run_blocking<T, F>(options: BlockingTaskOptions, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let runtime = app_runtime()?;
    let permit = Arc::clone(&runtime.blocking_slots)
        .acquire_owned()
        .await
        .map_err(|error| format!("后台任务队列已关闭: {error}"))?;
    let task_id = create_task_with_details_and_visibility(
        None,
        options.title,
        options.detail,
        "running",
        None,
        false,
        TaskVisibility::Hidden,
    );
    let started_at = Instant::now();
    let join_handle = spawn_blocking_with_permit(runtime, permit, operation);
    let guard = HiddenTaskGuard {
        task_id: task_id.clone(),
        abort_handle: join_handle.abort_handle(),
        completed: false,
    };
    register_task_abort_handle(task_id.clone(), guard.abort_handle.clone());

    debug!(
        task_id,
        title = options.title,
        "hidden blocking task started"
    );
    let joined = match options.timeout {
        Some(timeout) => tokio::time::timeout(timeout, join_handle)
            .await
            .map_err(|_| format!("{}超时（{} 秒）", options.title, timeout.as_secs())),
        None => Ok(join_handle.await),
    };

    if joined.is_err() {
        guard.abort_handle.abort();
    }
    let outcome = match joined {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => Err(format!("{}任务失败: {error}", options.title)),
        Err(error) => Err(error),
    };

    match &outcome {
        Ok(_) => {
            debug!(
                task_id,
                title = options.title,
                elapsed_ms = started_at.elapsed().as_millis(),
                "hidden blocking task completed"
            );
            guard.complete("completed", None);
        }
        Err(error) => {
            warn!(
                task_id,
                title = options.title,
                elapsed_ms = started_at.elapsed().as_millis(),
                %error,
                "hidden blocking task failed"
            );
            guard.complete("error", Some(error.clone()));
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;
    use crate::tasks::task_manager::render_snapshots_limited;

    fn initialize_test_runtime() {
        initialize_app_runtime().expect("application runtime should initialize");
    }

    #[tokio::test]
    async fn hidden_blocking_task_returns_result_without_rendering_snapshot() {
        initialize_test_runtime();
        let value = run_blocking(BlockingTaskOptions::hidden("隐藏任务测试"), || Ok(42))
            .await
            .expect("hidden task should complete");

        assert_eq!(value, 42);
        let snapshots = render_snapshots_limited(64, 64, 64);
        assert!(
            snapshots
                .active
                .iter()
                .chain(&snapshots.finished)
                .all(|snapshot| snapshot.title.as_ref() != "隐藏任务测试")
        );
    }

    #[tokio::test]
    async fn hidden_blocking_task_timeout_returns_error() {
        initialize_test_runtime();
        let mut options = BlockingTaskOptions::hidden("超时任务测试");
        options.timeout = Some(Duration::from_millis(1));

        let error = run_blocking(options, || {
            std::thread::sleep(Duration::from_millis(20));
            Ok(())
        })
        .await
        .expect_err("blocking task should time out");

        assert!(error.contains("超时"), "unexpected error: {error}");
    }

    #[test]
    fn background_thread_limit_is_twice_logical_threads() {
        assert_eq!(background_thread_limit(1), 2);
        assert_eq!(background_thread_limit(8), 16);
    }

    #[tokio::test]
    async fn blocking_permit_is_held_until_operation_exits() {
        initialize_test_runtime();
        let runtime = app_runtime().expect("application runtime should exist");
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&semaphore)
            .acquire_owned()
            .await
            .expect("test semaphore should remain open");
        let started = Arc::new(Barrier::new(2));
        let finish = Arc::new(Barrier::new(2));
        let join_handle = spawn_blocking_with_permit(runtime, permit, {
            let started = Arc::clone(&started);
            let finish = Arc::clone(&finish);
            move || {
                started.wait();
                finish.wait();
                Ok(())
            }
        });

        started.wait();
        assert_eq!(semaphore.available_permits(), 0);
        finish.wait();
        join_handle
            .await
            .expect("blocking task should join")
            .expect("blocking operation should succeed");
        assert_eq!(semaphore.available_permits(), 1);
    }
}
