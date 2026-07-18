use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{debug, warn};

use super::task_manager::{
    TaskVisibility, create_task_with_details_and_visibility, finish_task,
    register_task_abort_handle, remove_task,
};

const DEFAULT_BLOCKING_TIMEOUT: Duration = Duration::from_secs(30);
const FALLBACK_LOGICAL_THREADS: usize = 2;
static BACKGROUND_THREAD_LIMIT: LazyLock<usize> = LazyLock::new(|| {
    background_thread_limit(
        std::thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(FALLBACK_LOGICAL_THREADS),
    )
});
static BUSINESS_BLOCKING_LIMIT: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(*BACKGROUND_THREAD_LIMIT)));

fn background_thread_limit(logical_threads: usize) -> usize {
    logical_threads.max(1).saturating_mul(2)
}

fn spawn_blocking_with_permit<T, F>(
    permit: OwnedSemaphorePermit,
    operation: F,
) -> JoinHandle<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let outcome = operation();
        drop(permit);
        outcome
    })
}

pub fn build_launcher_runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    let background_threads = *BACKGROUND_THREAD_LIMIT;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(background_threads)
        .max_blocking_threads(background_threads)
        .thread_stack_size(1024 * 1024)
        .thread_name("bmcbl-runtime")
        .build()
        .map_err(Into::into)
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
    let permit = Arc::clone(&BUSINESS_BLOCKING_LIMIT)
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
    let join_handle = spawn_blocking_with_permit(permit, operation);
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

    #[tokio::test]
    async fn hidden_blocking_task_returns_result_without_rendering_snapshot() {
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
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&semaphore)
            .acquire_owned()
            .await
            .expect("test semaphore should remain open");
        let started = Arc::new(Barrier::new(2));
        let finish = Arc::new(Barrier::new(2));
        let join_handle = spawn_blocking_with_permit(permit, {
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
