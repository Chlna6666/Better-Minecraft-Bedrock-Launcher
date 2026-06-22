use once_cell::sync::OnceCell;
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tracing::error;

use crate::tasks::task_manager::{finish_task, is_cancelled, update_progress};

use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime};
use tokio::sync::Semaphore;
use tokio::task::AbortHandle;

const MAX_CONCURRENT_DOWNLOAD_TASKS: usize = 2;

struct DownloadRuntime {
    runtime: Runtime,
    task_slots: Arc<Semaphore>,
}

static DOWNLOAD_RUNTIME: OnceCell<DownloadRuntime> = OnceCell::new();

fn build_download_runtime() -> Result<DownloadRuntime, String> {
    let available_threads = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(2);
    let worker_threads = available_threads.saturating_sub(1).clamp(2, 6);
    let blocking_threads = available_threads.saturating_add(2).clamp(4, 8);

    let runtime = TokioRuntimeBuilder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .max_blocking_threads(blocking_threads)
        .thread_name("bmcbl-download")
        .build()
        .map_err(|error| format!("创建下载运行时失败: {error}"))?;

    Ok(DownloadRuntime {
        runtime,
        task_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOAD_TASKS)),
    })
}

fn download_runtime() -> Result<&'static DownloadRuntime, String> {
    DOWNLOAD_RUNTIME.get_or_try_init(build_download_runtime)
}

pub fn spawn_download_task<F>(task_id: String, future: F) -> Result<AbortHandle, String>
where
    F: Future<Output = ()> + Send + 'static,
{
    let runtime = download_runtime()?;
    let task_slots = runtime.task_slots.clone();
    let task_id_for_worker = task_id.clone();
    let join_handle = runtime.runtime.spawn(async move {
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

        future.await;
    });

    let abort_handle = join_handle.abort_handle();
    runtime.runtime.spawn(async move {
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
