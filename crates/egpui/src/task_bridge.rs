//! High-frequency application task updates delivered to GPUI foreground state.

use std::{
    error::Error,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use futures_util::FutureExt;
use gpui::{App, AsyncApp, Task, Timer};
use thiserror::Error;
use tokio::sync::{mpsc, watch};

use crate::UiDispatchError;

const TASK_OPEN: u8 = 0;
const TASK_TERMINAL: u8 = 1;

type BoxTaskError = Box<dyn Error + Send + Sync>;

/// Stable category for a failure delivered to GPUI state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiTaskFailureKind {
    /// The application operation returned an error.
    Operation,
    /// The application operation panicked.
    Panic,
}

/// Owned task failure suitable for storage in a GPUI entity or global.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiTaskFailure {
    kind: UiTaskFailureKind,
    message: Arc<str>,
}

impl UiTaskFailure {
    /// Creates an operation failure.
    #[must_use]
    pub fn operation(message: impl Into<Arc<str>>) -> Self {
        Self {
            kind: UiTaskFailureKind::Operation,
            message: message.into(),
        }
    }

    /// Creates a panic failure without exposing a panic payload.
    #[must_use]
    pub fn panic() -> Self {
        Self {
            kind: UiTaskFailureKind::Panic,
            message: Arc::from("application task panicked"),
        }
    }

    /// Returns the stable failure category.
    #[must_use]
    pub const fn kind(&self) -> UiTaskFailureKind {
        self.kind
    }

    /// Returns the user-loggable failure detail.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// One explicit terminal state delivered by a reported application task.
#[derive(Debug)]
pub enum UiTaskTerminal<T> {
    /// The operation completed and transferred its output to GPUI.
    Completed(T),
    /// The producer future was cancelled or dropped before finishing.
    Cancelled,
    /// The operation failed or panicked.
    Failed(UiTaskFailure),
}

impl<T> UiTaskTerminal<T> {
    /// Returns whether this terminal state completed successfully.
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        matches!(self, Self::Completed(_))
    }

    /// Returns whether cancellation won the task race.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// Foreground update emitted by [`UiTaskBridge`].
#[derive(Debug)]
pub enum UiTaskUpdate<P, T> {
    /// Latest progress snapshot. Intermediate snapshots may be coalesced.
    Progress(P),
    /// Exactly one terminal update, after which the consumer stops.
    Terminal(UiTaskTerminal<T>),
}

/// Failure returned by [`UiTaskReporter::report`].
#[derive(Debug, Error)]
pub enum UiTaskExecutionError {
    /// The operation failed after its failure was delivered to GPUI.
    #[error("reported application task failed: {0}")]
    Operation(#[source] BoxTaskError),
    /// The operation failed and GPUI had already stopped consuming updates.
    #[error("reported application task failed and its terminal UI update was not delivered")]
    OperationAndTerminalDelivery {
        /// Application operation failure.
        #[source]
        source: BoxTaskError,
        /// Foreground delivery failure.
        delivery: UiDispatchError,
    },
    /// The operation panicked after its failure was delivered to GPUI.
    #[error("reported application task panicked")]
    Panic,
    /// The operation panicked and GPUI had already stopped consuming updates.
    #[error("reported application task panicked and its terminal UI update was not delivered")]
    PanicAndTerminalDelivery {
        /// Foreground delivery failure.
        delivery: UiDispatchError,
    },
    /// A successful operation could not deliver its terminal value to GPUI.
    #[error(transparent)]
    TerminalDelivery(#[from] UiDispatchError),
}

/// Cloneable latest-progress publisher for worker helpers.
#[derive(Clone)]
pub struct UiTaskProgress<P> {
    sender: watch::Sender<P>,
    state: Arc<AtomicU8>,
}

impl<P> UiTaskProgress<P> {
    /// Publishes a latest-wins progress snapshot without queue growth.
    ///
    /// # Errors
    ///
    /// Returns [`UiDispatchError::Closed`] once the task is terminal or its
    /// foreground consumer has stopped.
    pub fn publish(&self, progress: P) -> Result<(), UiDispatchError> {
        if self.state.load(Ordering::Acquire) != TASK_OPEN {
            return Err(UiDispatchError::Closed);
        }
        self.sender
            .send(progress)
            .map_err(|_| UiDispatchError::Closed)
    }

    /// Returns whether the task or foreground consumer has stopped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state.load(Ordering::Acquire) != TASK_OPEN || self.sender.is_closed()
    }
}

/// Unique producer that reports one application task to GPUI.
///
/// Dropping this value before [`finish`](Self::finish) closes the terminal
/// channel. The foreground consumer interprets that closure as cancellation,
/// so cancellation cannot leave a view permanently in a running state.
pub struct UiTaskReporter<P, T> {
    progress: UiTaskProgress<P>,
    terminal_sender: mpsc::Sender<UiTaskTerminal<T>>,
}

impl<P, T> UiTaskReporter<P, T>
where
    P: Clone + Send + Sync + 'static,
    T: Send + 'static,
{
    /// Returns a cloneable progress publisher for nested worker functions.
    #[must_use]
    pub fn progress(&self) -> UiTaskProgress<P> {
        self.progress.clone()
    }

    /// Delivers the unique terminal state and closes further progress.
    ///
    /// # Errors
    ///
    /// Returns [`UiDispatchError::Closed`] if the foreground consumer stopped.
    pub async fn finish(self, terminal: UiTaskTerminal<T>) -> Result<(), UiDispatchError> {
        self.progress.state.store(TASK_TERMINAL, Ordering::Release);
        self.terminal_sender
            .send(terminal)
            .await
            .map_err(|_| UiDispatchError::Closed)
    }

    /// Runs one operation, converting completion, error, panic, or cancellation
    /// into one foreground terminal update.
    ///
    /// The returned future should be scheduled through
    /// [`TaskScope::spawn_io`](crate::TaskScope::spawn_io). Dropping it invokes
    /// this reporter's cancellation-on-close behavior.
    ///
    /// # Errors
    ///
    /// Returns the operation, panic, or terminal-delivery failure after
    /// attempting to publish the corresponding foreground state.
    pub async fn report<F, E>(self, operation: F) -> Result<(), UiTaskExecutionError>
    where
        F: Future<Output = Result<T, E>> + Send,
        E: Error + Send + Sync + 'static,
    {
        match AssertUnwindSafe(operation).catch_unwind().await {
            Ok(Ok(value)) => self
                .finish(UiTaskTerminal::Completed(value))
                .await
                .map_err(UiTaskExecutionError::TerminalDelivery),
            Ok(Err(error)) => self.report_operation_error(error).await,
            Err(_panic) => self.report_panic().await,
        }
    }

    async fn report_operation_error<E>(self, error: E) -> Result<(), UiTaskExecutionError>
    where
        E: Error + Send + Sync + 'static,
    {
        let failure = UiTaskFailure::operation(error.to_string());
        let source = Box::new(error) as BoxTaskError;
        match self.finish(UiTaskTerminal::Failed(failure)).await {
            Ok(()) => Err(UiTaskExecutionError::Operation(source)),
            Err(delivery) => {
                Err(UiTaskExecutionError::OperationAndTerminalDelivery { source, delivery })
            }
        }
    }

    async fn report_panic(self) -> Result<(), UiTaskExecutionError> {
        match self
            .finish(UiTaskTerminal::Failed(UiTaskFailure::panic()))
            .await
        {
            Ok(()) => Err(UiTaskExecutionError::Panic),
            Err(delivery) => Err(UiTaskExecutionError::PanicAndTerminalDelivery { delivery }),
        }
    }
}

impl<P, T> Drop for UiTaskReporter<P, T> {
    fn drop(&mut self) {
        self.progress.state.store(TASK_TERMINAL, Ordering::Release);
    }
}

/// Installs one coalescing progress and lossless terminal consumer.
pub struct UiTaskBridge;

impl UiTaskBridge {
    /// Installs a GPUI foreground consumer and returns its unique reporter.
    ///
    /// Progress uses a `watch` channel, so a large download or extraction can
    /// publish frequently without building an unbounded queue. The terminal
    /// channel has dedicated capacity and is selected before progress, so a
    /// completed, failed, or cancelled task cannot be starved by progress.
    ///
    /// The returned GPUI task must be stored for the lifetime of the consumer.
    #[must_use]
    pub fn install<P, T>(
        initial_progress: P,
        cx: &mut App,
        mut apply: impl FnMut(UiTaskUpdate<P, T>, &mut AsyncApp) + 'static,
    ) -> (UiTaskReporter<P, T>, Task<()>)
    where
        P: Clone + Send + Sync + 'static,
        T: Send + 'static,
    {
        let (reporter, mut receiver) = task_channel(initial_progress);
        let task = cx.spawn(async move |cx| {
            let initial = receiver.progress.borrow_and_update().clone();
            apply(UiTaskUpdate::Progress(initial), cx);
            while let Some(update) = receiver.next_update().await {
                let terminal = matches!(update, UiTaskUpdate::Terminal(_));
                apply(update, cx);
                if terminal {
                    break;
                }
                Timer::after(Duration::ZERO).await;
            }
        });
        (reporter, task)
    }
}

struct UiTaskReceiver<P, T> {
    progress: watch::Receiver<P>,
    terminal: mpsc::Receiver<UiTaskTerminal<T>>,
    terminal_observed: bool,
}

impl<P, T> UiTaskReceiver<P, T>
where
    P: Clone,
{
    async fn next_update(&mut self) -> Option<UiTaskUpdate<P, T>> {
        if self.terminal_observed {
            return None;
        }
        tokio::select! {
            biased;
            terminal = self.terminal.recv() => {
                self.terminal_observed = true;
                Some(UiTaskUpdate::Terminal(
                    terminal.unwrap_or(UiTaskTerminal::Cancelled),
                ))
            }
            changed = self.progress.changed() => {
                if changed.is_err() {
                    return None;
                }
                Some(UiTaskUpdate::Progress(
                    self.progress.borrow_and_update().clone(),
                ))
            }
        }
    }
}

fn task_channel<P, T>(initial_progress: P) -> (UiTaskReporter<P, T>, UiTaskReceiver<P, T>) {
    let (progress_sender, progress) = watch::channel(initial_progress);
    let (terminal_sender, terminal) = mpsc::channel(1);
    let state = Arc::new(AtomicU8::new(TASK_OPEN));
    (
        UiTaskReporter {
            progress: UiTaskProgress {
                sender: progress_sender,
                state,
            },
            terminal_sender,
        },
        UiTaskReceiver {
            progress,
            terminal,
            terminal_observed: false,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{
        UiTaskExecutionError, UiTaskFailureKind, UiTaskTerminal, UiTaskUpdate, task_channel,
    };

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(future)
    }

    #[test]
    fn progress_is_coalesced_and_drop_emits_cancellation() {
        block_on(async {
            let (reporter, mut receiver) = task_channel::<u32, ()>(0);
            let progress = reporter.progress();
            for value in 1..=10_000 {
                progress.publish(value).expect("publish progress");
            }

            assert!(matches!(
                receiver.next_update().await,
                Some(UiTaskUpdate::Progress(10_000))
            ));
            drop(reporter);
            assert!(matches!(
                receiver.next_update().await,
                Some(UiTaskUpdate::Terminal(UiTaskTerminal::Cancelled))
            ));
            assert!(progress.is_closed());
        });
    }

    #[test]
    fn terminal_update_has_priority_over_pending_progress() {
        block_on(async {
            let (reporter, mut receiver) = task_channel::<u32, u32>(0);
            reporter.progress().publish(1).expect("publish progress");
            reporter
                .finish(UiTaskTerminal::Completed(42))
                .await
                .expect("finish");

            assert!(matches!(
                receiver.next_update().await,
                Some(UiTaskUpdate::Terminal(UiTaskTerminal::Completed(42)))
            ));
        });
    }

    #[test]
    fn report_preserves_operation_failure_and_ui_terminal_state() {
        block_on(async {
            let (reporter, mut receiver) = task_channel::<u32, ()>(0);
            let error = reporter
                .report(async { Err::<(), _>(io::Error::other("disk full")) })
                .await
                .expect_err("operation must fail");
            assert!(matches!(error, UiTaskExecutionError::Operation(_)));

            let Some(UiTaskUpdate::Terminal(UiTaskTerminal::Failed(failure))) =
                receiver.next_update().await
            else {
                panic!("expected failed terminal update");
            };
            assert_eq!(failure.kind(), UiTaskFailureKind::Operation);
            assert_eq!(failure.message(), "disk full");
        });
    }

    #[test]
    fn report_converts_panic_to_failure() {
        block_on(async {
            let (reporter, mut receiver) = task_channel::<u32, ()>(0);
            let error = reporter
                .report(async {
                    panic!("test panic");
                    #[allow(unreachable_code)]
                    Ok::<(), io::Error>(())
                })
                .await
                .expect_err("panic must fail");
            assert!(matches!(error, UiTaskExecutionError::Panic));

            let Some(UiTaskUpdate::Terminal(UiTaskTerminal::Failed(failure))) =
                receiver.next_update().await
            else {
                panic!("expected panic terminal update");
            };
            assert_eq!(failure.kind(), UiTaskFailureKind::Panic);
        });
    }
}
