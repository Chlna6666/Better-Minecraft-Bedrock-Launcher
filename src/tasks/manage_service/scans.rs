use std::sync::LazyLock;

use rayon::{ThreadPool, ThreadPoolBuilder};

use crate::tasks::runtime::BlockingTaskOptions;
use crate::tasks::runtime::blocking;

// Core pack readers use nested Rayon iterators. Installing their work in this
// shared pool bounds parsing/preview CPU use across foreground and warmup scans.
static POOL: LazyLock<Result<ThreadPool, String>> = LazyLock::new(|| {
    ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(1, |count| count.get().min(2)))
        .thread_name(|index| format!("bmcbl-manage-{index}"))
        .build()
        .map_err(|error| format!("创建管理扫描线程池失败: {error}"))
});

pub(super) async fn run<T, F>(title: &'static str, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    blocking::run(BlockingTaskOptions::hidden(title), move || {
        POOL.as_ref().map_err(Clone::clone)?.install(operation)
    })
    .await
}
