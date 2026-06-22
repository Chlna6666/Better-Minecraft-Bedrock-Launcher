use once_cell::sync::OnceCell;
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime};
use tokio::sync::Semaphore;
use tracing::error;

use crate::tasks::task_manager::{finish_task, get_snapshot_arc, is_cancelled, update_progress};

const MAX_CONCURRENT_ARCHIVE_TASKS: usize = 1;

struct ArchiveRuntime {
    runtime: Runtime,
    task_slots: Arc<Semaphore>,
}

static ARCHIVE_RUNTIME: OnceCell<ArchiveRuntime> = OnceCell::new();

fn build_archive_runtime() -> Result<ArchiveRuntime, String> {
    let available_threads = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(2);
    let worker_threads = available_threads.saturating_sub(1).clamp(2, 4);
    let blocking_threads = available_threads.saturating_add(1).clamp(4, 6);

    let runtime = TokioRuntimeBuilder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .max_blocking_threads(blocking_threads)
        .thread_name("bmcbl-archive")
        .build()
        .map_err(|error| format!("创建安装运行时失败: {error}"))?;

    Ok(ArchiveRuntime {
        runtime,
        task_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_ARCHIVE_TASKS)),
    })
}

fn archive_runtime() -> Result<&'static ArchiveRuntime, String> {
    ARCHIVE_RUNTIME.get_or_try_init(build_archive_runtime)
}

pub fn spawn_archive_task<F>(task_id: String, future: F) -> Result<(), String>
where
    F: Future<Output = ()> + Send + 'static,
{
    let runtime = archive_runtime()?;
    let task_slots = runtime.task_slots.clone();
    let task_id_for_worker = task_id.clone();
    let join_handle = runtime.runtime.spawn(async move {
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

        if is_cancelled(&task_id_for_worker) {
            return;
        }

        future.await;
    });

    runtime.runtime.spawn(async move {
        match join_handle.await {
            Ok(()) => {
                let still_running = get_snapshot_arc(&task_id).is_some_and(|snapshot| {
                    matches!(snapshot.status.as_ref(), "running" | "paused")
                });
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
