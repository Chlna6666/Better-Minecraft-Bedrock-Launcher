use std::{
    error::Error,
    fmt,
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{FutureExt, future::BoxFuture};
use thiserror::Error;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

mod default_provider;

pub use default_provider::DefaultRuntimeProvider;

/// Physical runtime sizing and concurrency budgets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    /// Tokio worker threads used for asynchronous application work.
    pub io_worker_threads: NonZeroUsize,
    /// Maximum synchronous operations admitted to Tokio's blocking pool.
    pub max_blocking_tasks: NonZeroUsize,
    /// Rayon workers used for CPU-bound operations.
    pub cpu_worker_threads: NonZeroUsize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        let logical_cores = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
        let background_threads =
            NonZeroUsize::new(logical_cores.get().saturating_mul(2)).unwrap_or(logical_cores);

        Self {
            io_worker_threads: background_threads,
            max_blocking_tasks: background_threads,
            cpu_worker_threads: logical_cores,
        }
    }
}

/// Metadata attached to a blocking operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockingTaskOptions {
    /// Diagnostic name used to identify the operation.
    pub label: Arc<str>,
}

impl BlockingTaskOptions {
    /// Creates options with a stable diagnostic label.
    #[must_use]
    pub fn new(label: impl Into<Arc<str>>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl Default for BlockingTaskOptions {
    fn default() -> Self {
        Self::new("blocking")
    }
}

/// Failures in runtime construction, scheduling, or shutdown.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// A Tokio runtime could not be built.
    #[error("failed to build application runtime: {0}")]
    Build(#[source] std::io::Error),
    /// The provider no longer accepts new tasks.
    #[error("application runtime is closed")]
    Closed,
    /// Runtime coordination state was poisoned by a panic.
    #[error("application runtime coordination state is poisoned")]
    Poisoned,
}

/// An operation failure distinct from cancellation and successful completion.
#[derive(Debug, Error)]
pub enum TaskError {
    /// The operation returned an application error.
    #[error("application task failed: {0}")]
    Operation(#[source] Box<dyn Error + Send + Sync>),
    /// The scheduled worker exited before publishing a terminal result.
    #[error("application task worker exited before producing a result")]
    Join,
    /// The operation panicked before publishing a terminal result.
    #[error("application task panicked")]
    Panic,
}

/// The explicit terminal state of an application task.
#[derive(Debug)]
pub enum TaskOutcome<T> {
    /// The operation completed successfully.
    Completed(T),
    /// The task or one of its parent scopes was cancelled.
    Cancelled,
    /// The operation or its worker failed.
    Failed(TaskError),
}

impl<T> TaskOutcome<T> {
    /// Returns whether this outcome is a successful completion.
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        matches!(self, Self::Completed(_))
    }

    /// Returns whether this outcome represents cancellation.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// A provider-owned scheduled task that can be aborted when supported.
pub struct ScheduledTask {
    abort_handle: Option<tokio::task::AbortHandle>,
}

impl ScheduledTask {
    pub(crate) fn abortable(abort_handle: tokio::task::AbortHandle) -> Self {
        Self {
            abort_handle: Some(abort_handle),
        }
    }

    pub(crate) const fn cooperative_only() -> Self {
        Self { abort_handle: None }
    }

    /// Requests immediate cancellation when the physical executor supports it.
    pub fn abort(&self) {
        if let Some(abort_handle) = &self.abort_handle {
            abort_handle.abort();
        }
    }
}

impl fmt::Debug for ScheduledTask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScheduledTask")
            .field("abortable", &self.abort_handle.is_some())
            .finish()
    }
}

/// Result of attempting to converge the application runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownReport {
    /// Whether every tracked Tokio task exited before the deadline.
    pub async_tasks_drained: bool,
    /// CPU tasks still running after the deadline.
    pub remaining_cpu_tasks: usize,
    /// Total time spent waiting and stopping runtime workers.
    pub elapsed: Duration,
}

impl ShutdownReport {
    /// Returns whether all tracked execution domains converged in time.
    #[must_use]
    pub const fn drained(self) -> bool {
        self.async_tasks_drained && self.remaining_cpu_tasks == 0
    }
}

/// Replaceable physical execution provider used by [`ApplicationRuntime`].
pub trait RuntimeProvider: Send + Sync + 'static {
    /// Schedules an asynchronous IO-oriented future.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Closed`] once shutdown begins.
    fn spawn_io(&self, future: BoxFuture<'static, ()>) -> Result<ScheduledTask, RuntimeError>;

    /// Schedules a synchronous operation on a bounded blocking pool.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Closed`] once shutdown begins.
    fn spawn_blocking(
        &self,
        operation: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<ScheduledTask, RuntimeError>;

    /// Schedules a CPU-bound operation.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Closed`] once shutdown begins.
    fn spawn_cpu(
        &self,
        operation: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<ScheduledTask, RuntimeError>;

    /// Stops accepting new work without blocking the caller.
    fn request_shutdown(&self);

    /// Waits for tracked work and stops physical executors.
    ///
    /// This synchronous operation must run outside an asynchronous runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when provider coordination state is poisoned.
    fn shutdown(&self, timeout: Duration) -> Result<ShutdownReport, RuntimeError>;
}

/// Application-scoped access to background execution domains.
#[derive(Clone)]
pub struct ApplicationRuntime {
    provider: Arc<dyn RuntimeProvider>,
    root_cancellation: CancellationToken,
}

impl ApplicationRuntime {
    /// Builds the default Tokio/Rayon provider.
    ///
    /// # Errors
    ///
    /// Returns an error when a physical runtime or thread pool cannot be built.
    pub fn new(config: RuntimeConfig) -> Result<Self, RuntimeError> {
        let provider = Arc::new(DefaultRuntimeProvider::new(config)?);
        Ok(Self::with_provider(provider))
    }

    /// Creates an application runtime from a custom provider.
    #[must_use]
    pub fn with_provider(provider: Arc<dyn RuntimeProvider>) -> Self {
        Self {
            provider,
            root_cancellation: CancellationToken::new(),
        }
    }

    /// Creates a task scope tied to application shutdown.
    #[must_use]
    pub fn application_scope(&self) -> TaskScope {
        TaskScope {
            runtime: self.clone(),
            cancellation: self.root_cancellation.child_token(),
        }
    }

    /// Returns a read-only token that observes application shutdown.
    #[must_use]
    pub fn shutdown_token(&self) -> ShutdownToken {
        ShutdownToken(self.root_cancellation.clone())
    }

    /// Stops accepting work and cooperatively cancels application scopes.
    pub fn request_shutdown(&self) {
        self.root_cancellation.cancel();
        self.provider.request_shutdown();
    }

    /// Converges and stops the physical execution provider.
    ///
    /// # Errors
    ///
    /// Returns an error when provider coordination state is poisoned.
    pub fn shutdown(&self, timeout: Duration) -> Result<ShutdownReport, RuntimeError> {
        self.request_shutdown();
        self.provider.shutdown(timeout)
    }
}

/// A read-only application shutdown signal.
#[derive(Clone, Debug)]
pub struct ShutdownToken(CancellationToken);

impl ShutdownToken {
    /// Returns whether shutdown has been requested.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Waits until application shutdown is requested.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

/// Cooperative cancellation signal scoped to one application task.
#[derive(Clone, Debug)]
pub struct TaskCancellation(CancellationToken);

impl TaskCancellation {
    /// Returns whether this task has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Waits until this task or one of its parent scopes is cancelled.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

/// A structured cancellation domain for related application tasks.
#[derive(Clone)]
pub struct TaskScope {
    runtime: ApplicationRuntime,
    cancellation: CancellationToken,
}

impl TaskScope {
    /// Creates a nested scope cancelled with its parent.
    #[must_use]
    pub fn child_scope(&self) -> Self {
        Self {
            runtime: self.runtime.clone(),
            cancellation: self.cancellation.child_token(),
        }
    }

    /// Requests cooperative cancellation for this scope and its children.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// Waits until this scope or one of its parents is cancelled.
    pub async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }

    /// Returns a cloneable cooperative cancellation token for a worker.
    ///
    /// The token is scoped to this task scope and is cancelled when the scope
    /// or any parent scope is cancelled.
    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Schedules an asynchronous application operation.
    ///
    /// # Errors
    ///
    /// Returns an error after application shutdown begins.
    pub fn spawn_io<F, T, E>(&self, future: F) -> Result<AppTask<T>, RuntimeError>
    where
        F: Future<Output = Result<T, E>> + Send + 'static,
        T: Send + 'static,
        E: Error + Send + Sync + 'static,
    {
        self.spawn_io_with_cancellation(|_| future)
    }

    /// Schedules an asynchronous operation with its task-scoped cancellation
    /// signal.
    ///
    /// The supplied callback is invoked once on the application runtime. The
    /// returned [`TaskCancellation`] is cancelled when the returned
    /// [`AppTask`] is cancelled, when this scope is cancelled, or during
    /// application shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error after application shutdown begins.
    pub fn spawn_io_with_cancellation<F, Fut, T, E>(
        &self,
        operation: F,
    ) -> Result<AppTask<T>, RuntimeError>
    where
        F: FnOnce(TaskCancellation) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        T: Send + 'static,
        E: Error + Send + Sync + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let cancellation = self.cancellation.child_token();
        let task_cancellation = cancellation.clone();
        let task_context = TaskCancellation(cancellation.clone());
        let worker = async move {
            let future = operation(task_context);
            let outcome = tokio::select! {
                biased;
                () = task_cancellation.cancelled() => TaskOutcome::Cancelled,
                result = std::panic::AssertUnwindSafe(future).catch_unwind() => match result {
                    Ok(result) => map_operation_result(result),
                    Err(_panic) => TaskOutcome::Failed(TaskError::Panic),
                },
            };
            publish_outcome(sender, outcome);
        };
        let scheduled = self.runtime.provider.spawn_io(Box::pin(worker))?;

        Ok(AppTask::new(receiver, scheduled, cancellation))
    }

    /// Schedules a synchronous operation on the bounded blocking domain.
    ///
    /// Cancellation stops waiting for the result but cannot preempt synchronous
    /// code already running.
    ///
    /// # Errors
    ///
    /// Returns an error after application shutdown begins.
    pub fn spawn_blocking<F, T, E>(&self, operation: F) -> Result<AppTask<T>, RuntimeError>
    where
        F: FnOnce() -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: Error + Send + Sync + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let cancellation = self.cancellation.child_token();
        let task_cancellation = cancellation.clone();
        let worker = move || {
            let outcome = if task_cancellation.is_cancelled() {
                TaskOutcome::Cancelled
            } else {
                let outcome = match catch_unwind_operation(operation) {
                    Ok(result) => map_operation_result(result),
                    Err(_panic) => TaskOutcome::Failed(TaskError::Panic),
                };
                if task_cancellation.is_cancelled() {
                    TaskOutcome::Cancelled
                } else {
                    outcome
                }
            };
            publish_outcome(sender, outcome);
        };
        let scheduled = self.runtime.provider.spawn_blocking(Box::new(worker))?;

        Ok(AppTask::new(receiver, scheduled, cancellation))
    }

    /// Schedules an operation on the CPU-bound Rayon domain.
    ///
    /// Cancellation stops waiting for the result but cannot preempt a Rayon
    /// closure already running.
    ///
    /// # Errors
    ///
    /// Returns an error after application shutdown begins.
    pub fn spawn_cpu<F, T, E>(&self, operation: F) -> Result<AppTask<T>, RuntimeError>
    where
        F: FnOnce() -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: Error + Send + Sync + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let cancellation = self.cancellation.child_token();
        let task_cancellation = cancellation.clone();
        let worker = move || {
            let outcome = if task_cancellation.is_cancelled() {
                TaskOutcome::Cancelled
            } else {
                let outcome = match catch_unwind_operation(operation) {
                    Ok(result) => map_operation_result(result),
                    Err(_panic) => TaskOutcome::Failed(TaskError::Panic),
                };
                if task_cancellation.is_cancelled() {
                    TaskOutcome::Cancelled
                } else {
                    outcome
                }
            };
            publish_outcome(sender, outcome);
        };
        let scheduled = self.runtime.provider.spawn_cpu(Box::new(worker))?;

        Ok(AppTask::new(receiver, scheduled, cancellation))
    }
}

/// A cancellable future resolving to one explicit terminal outcome.
#[must_use = "application tasks must be awaited, cancelled, or intentionally dropped"]
pub struct AppTask<T> {
    receiver: oneshot::Receiver<TaskOutcome<T>>,
    scheduled: Option<ScheduledTask>,
    cancellation: CancellationToken,
}

impl<T> AppTask<T> {
    fn new(
        receiver: oneshot::Receiver<TaskOutcome<T>>,
        scheduled: ScheduledTask,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            receiver,
            scheduled: Some(scheduled),
            cancellation,
        }
    }

    /// Requests cooperative cancellation and aborts the scheduled future when
    /// the provider supports it.
    pub fn cancel(&self) {
        self.cancellation.cancel();
        if let Some(scheduled) = &self.scheduled {
            scheduled.abort();
        }
    }
}

impl<T> Future for AppTask<T> {
    type Output = TaskOutcome<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Ready(Ok(outcome)) => {
                self.scheduled = None;
                Poll::Ready(outcome)
            }
            Poll::Ready(Err(_closed)) => {
                self.scheduled = None;
                Poll::Ready(TaskOutcome::Failed(TaskError::Join))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for AppTask<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn map_operation_result<T, E>(result: Result<T, E>) -> TaskOutcome<T>
where
    E: Error + Send + Sync + 'static,
{
    match result {
        Ok(value) => TaskOutcome::Completed(value),
        Err(error) => TaskOutcome::Failed(TaskError::Operation(Box::new(error))),
    }
}

fn catch_unwind_operation<F, T, E>(
    operation: F,
) -> Result<Result<T, E>, Box<dyn std::any::Any + Send>>
where
    F: FnOnce() -> Result<T, E>,
{
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
}

fn publish_outcome<T>(sender: oneshot::Sender<TaskOutcome<T>>, outcome: TaskOutcome<T>) {
    if let Err(discarded_outcome) = sender.send(outcome) {
        drop(discarded_outcome);
    }
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, time::Duration};

    use super::{ApplicationRuntime, RuntimeConfig, TaskError, TaskOutcome};

    fn block_on_task<T>(task: super::AppTask<T>) -> super::TaskOutcome<T> {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime")
            .block_on(task)
    }

    #[test]
    fn io_task_completes_with_explicit_outcome() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let scope = runtime.application_scope();
        let task = scope
            .spawn_io(async { Ok::<_, Infallible>(42) })
            .expect("spawn");

        let outcome = block_on_task(task);
        assert!(matches!(outcome, TaskOutcome::Completed(42)));
        assert!(
            runtime
                .shutdown(Duration::from_secs(1))
                .expect("shutdown")
                .drained()
        );
    }

    #[test]
    fn cancelling_scope_produces_cancelled_terminal_state() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let scope = runtime.application_scope();
        let task = scope
            .spawn_io(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok::<_, Infallible>(())
            })
            .expect("spawn");

        scope.cancel();
        let outcome = block_on_task(task);
        assert!(outcome.is_cancelled());
        assert!(
            runtime
                .shutdown(Duration::from_secs(1))
                .expect("shutdown")
                .drained()
        );
    }

    #[test]
    fn shutdown_rejects_new_tasks() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let scope = runtime.application_scope();
        runtime.request_shutdown();

        let result = scope.spawn_io(async { Ok::<_, Infallible>(()) });
        assert!(result.is_err());
        assert!(
            runtime
                .shutdown(Duration::from_secs(1))
                .expect("shutdown")
                .drained()
        );
    }

    #[test]
    fn blocking_and_cpu_tasks_use_their_distinct_domains() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let scope = runtime.application_scope();
        let blocking = scope
            .spawn_blocking(|| Ok::<_, Infallible>(7))
            .expect("blocking task");
        let cpu = scope
            .spawn_cpu(|| Ok::<_, Infallible>(11))
            .expect("cpu task");

        let blocking_outcome = block_on_task(blocking);
        let cpu_outcome = block_on_task(cpu);
        assert!(matches!(blocking_outcome, TaskOutcome::Completed(7)));
        assert!(matches!(cpu_outcome, TaskOutcome::Completed(11)));
        assert!(
            runtime
                .shutdown(Duration::from_secs(1))
                .expect("shutdown")
                .drained()
        );
    }

    #[test]
    fn panics_are_reported_as_a_distinct_terminal_state() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let scope = runtime.application_scope();
        let task = scope
            .spawn_blocking(|| -> Result<(), Infallible> {
                panic!("test panic");
            })
            .expect("spawn");

        let outcome = block_on_task(task);
        assert!(matches!(outcome, TaskOutcome::Failed(TaskError::Panic)));
        assert!(
            runtime
                .shutdown(Duration::from_secs(1))
                .expect("shutdown")
                .drained()
        );
    }

    #[test]
    fn blocking_permit_remains_held_until_cancelled_worker_exits() {
        use std::sync::mpsc;

        let config = RuntimeConfig {
            io_worker_threads: std::num::NonZeroUsize::new(1).expect("non-zero"),
            max_blocking_tasks: std::num::NonZeroUsize::new(1).expect("non-zero"),
            cpu_worker_threads: std::num::NonZeroUsize::new(1).expect("non-zero"),
        };
        let runtime = ApplicationRuntime::new(config).expect("runtime");
        let scope = runtime.application_scope();
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let first = scope
            .spawn_blocking(move || {
                started_sender.send(()).expect("started receiver");
                release_receiver.recv().expect("release signal");
                Ok::<_, Infallible>(())
            })
            .expect("first task");
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking operation started");
        first.cancel();

        let second = scope
            .spawn_blocking(|| Ok::<_, Infallible>(7))
            .expect("second task");
        let test_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        let second_outcome = test_runtime.block_on(async move {
            tokio::pin!(second);
            let pending = tokio::time::timeout(Duration::from_millis(50), &mut second).await;
            assert!(pending.is_err(), "second task bypassed the blocking permit");
            release_sender.send(()).expect("release receiver");
            second.await
        });
        assert!(matches!(second_outcome, TaskOutcome::Completed(7)));
        assert!(
            runtime
                .shutdown(Duration::from_secs(1))
                .expect("shutdown")
                .drained()
        );
    }
}
