use std::{
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Duration,
};

use futures_util::{Stream, StreamExt};
use gpui::{App, AsyncApp, Context, Task, Timer, WeakEntity};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};

type UiAction = Box<dyn FnOnce(&mut AsyncApp) + Send + 'static>;

/// Errors returned when a foreground bridge cannot accept a message.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum UiDispatchError {
    /// The bounded foreground queue currently has no capacity.
    #[error("the UI dispatch queue is full")]
    Full,
    /// The GPUI foreground consumer has stopped.
    #[error("the UI dispatch queue is closed")]
    Closed,
}

/// Configuration shared by background-to-GPUI bridges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiBridgeConfig {
    /// Maximum number of queued values before producers observe backpressure.
    pub capacity: NonZeroUsize,
    /// Maximum values applied before yielding to GPUI's foreground scheduler.
    pub maximum_batch_size: NonZeroUsize,
}

impl Default for UiBridgeConfig {
    fn default() -> Self {
        Self {
            capacity: NonZeroUsize::new(256).unwrap_or(NonZeroUsize::MIN),
            maximum_batch_size: NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// Failure while waiting for a foreground action to finish.
#[derive(Debug, Error)]
pub enum UiCallError {
    /// The foreground action could not be queued.
    #[error(transparent)]
    Dispatch(#[from] UiDispatchError),
    /// The foreground consumer stopped before returning a result.
    #[error("the UI foreground consumer stopped before returning a result")]
    ConsumerStopped,
    /// The foreground action panicked.
    #[error("the UI foreground action panicked")]
    Panicked,
}

/// Failure while applying an update to a GPUI entity.
#[derive(Debug, Error)]
pub enum UiEntityUpdateError {
    /// The foreground call could not finish.
    #[error(transparent)]
    Call(#[from] UiCallError),
    /// The weak entity no longer exists or GPUI rejected the update.
    #[error("failed to update GPUI entity: {0}")]
    Update(#[source] anyhow::Error),
}

/// Current bounded-queue state for diagnostics and overload reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiQueueState {
    /// Remaining queue capacity.
    pub remaining_capacity: usize,
    /// Configured queue capacity.
    pub maximum_capacity: usize,
    /// Whether the foreground consumer has stopped.
    pub closed: bool,
}

/// A sendable handle for scheduling an owned action on the GPUI foreground.
///
/// The action itself is transferred as `Send + 'static` data. `AsyncApp` is
/// supplied only after the action reaches GPUI's foreground executor.
#[derive(Clone)]
pub struct UiHandle {
    sender: mpsc::Sender<UiAction>,
}

impl UiHandle {
    pub(crate) fn channel(config: UiBridgeConfig) -> (Self, mpsc::Receiver<UiAction>) {
        let (sender, receiver) = mpsc::channel(config.capacity.get());
        (Self { sender }, receiver)
    }

    /// Waits for queue capacity and dispatches an action to GPUI.
    ///
    /// # Errors
    ///
    /// Returns [`UiDispatchError::Closed`] after the foreground consumer stops.
    pub async fn dispatch(
        &self,
        action: impl FnOnce(&mut AsyncApp) + Send + 'static,
    ) -> Result<(), UiDispatchError> {
        self.sender
            .send(Box::new(action))
            .await
            .map_err(|_| UiDispatchError::Closed)
    }

    /// Attempts to dispatch without waiting for queue capacity.
    ///
    /// # Errors
    ///
    /// Returns `Full` when backpressure is active or `Closed` after GPUI stops.
    pub fn try_dispatch(
        &self,
        action: impl FnOnce(&mut AsyncApp) + Send + 'static,
    ) -> Result<(), UiDispatchError> {
        self.sender
            .try_send(Box::new(action))
            .map_err(map_try_send_error)
    }

    /// Queues an action and waits until it has run on GPUI's foreground.
    ///
    /// This is intended for short state mutations whose result or entity
    /// lifetime failure must be observed by the background caller. It must not
    /// perform IO, blocking waits, decoding, or other application work.
    ///
    /// # Errors
    ///
    /// Returns an error when the queue closes, the consumer stops, or the
    /// action panics.
    pub async fn call<R>(
        &self,
        action: impl FnOnce(&mut AsyncApp) -> R + Send + 'static,
    ) -> Result<R, UiCallError>
    where
        R: Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.dispatch(move |cx| {
            let outcome = catch_unwind(AssertUnwindSafe(|| action(cx)))
                .map_err(|_panic| UiCallError::Panicked);
            if let Err(discarded) = sender.send(outcome) {
                drop(discarded);
            }
        })
        .await?;
        receiver.await.map_err(|_| UiCallError::ConsumerStopped)?
    }

    /// Applies a short mutation to a weak GPUI entity and observes failure.
    ///
    /// The update closure runs on GPUI's foreground and receives the entity
    /// [`Context`], so it can call `cx.notify()` after changing render state.
    ///
    /// # Errors
    ///
    /// Returns a queue/call error or the error returned when the weak entity
    /// can no longer be updated.
    pub async fn update_entity<T, R>(
        &self,
        entity: WeakEntity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R + Send + 'static,
    ) -> Result<R, UiEntityUpdateError>
    where
        T: 'static,
        R: Send + 'static,
    {
        self.call(move |cx| entity.update(cx, update))
            .await?
            .map_err(UiEntityUpdateError::Update)
    }

    /// Returns a non-blocking diagnostic snapshot of the bounded queue.
    #[must_use]
    pub fn queue_state(&self) -> UiQueueState {
        UiQueueState {
            remaining_capacity: self.sender.capacity(),
            maximum_capacity: self.sender.max_capacity(),
            closed: self.sender.is_closed(),
        }
    }
}

pub(crate) fn bind_ui_handle(
    mut receiver: mpsc::Receiver<UiAction>,
    maximum_batch_size: NonZeroUsize,
    cx: &mut App,
) -> Task<()> {
    cx.spawn(async move |cx| {
        while let Some(action) = receiver.recv().await {
            action(cx);
            for _ in 1..maximum_batch_size.get() {
                let Ok(action) = receiver.try_recv() else {
                    break;
                };
                action(cx);
            }
            Timer::after(Duration::ZERO).await;
        }
    })
}

/// A bounded, sendable stream of values consumed on the GPUI foreground.
#[derive(Clone)]
pub struct UiStreamBridge<T> {
    sender: mpsc::Sender<T>,
}

impl<T> UiStreamBridge<T>
where
    T: Send + 'static,
{
    /// Binds an existing stream to a GPUI foreground consumer.
    ///
    /// The returned task must be stored for as long as the stream should
    /// remain active.
    ///
    /// The stream is polled by GPUI's foreground executor. Therefore `stream`
    /// must be a non-blocking, channel-backed adapter whose producer already
    /// runs in an application-owned execution domain. Do not pass a stream
    /// that performs filesystem, network, decoding, or blocking work in `poll`.
    #[must_use]
    pub fn bind_stream<S>(
        cx: &mut App,
        stream: S,
        mut apply: impl FnMut(T, &mut AsyncApp) + 'static,
    ) -> Task<()>
    where
        S: Stream<Item = T> + 'static,
    {
        cx.spawn(async move |cx| {
            futures_util::pin_mut!(stream);
            while let Some(value) = stream.next().await {
                apply(value, cx);
            }
        })
    }

    /// Installs a foreground consumer and returns its bridge and owning task.
    ///
    /// The returned GPUI task must be stored for as long as the bridge should
    /// remain active.
    #[must_use]
    pub fn install(
        config: UiBridgeConfig,
        cx: &mut App,
        mut apply: impl FnMut(T, &mut AsyncApp) + 'static,
    ) -> (Self, Task<()>) {
        let (sender, mut receiver) = mpsc::channel(config.capacity.get());
        let task = cx.spawn(async move |cx| {
            while let Some(value) = receiver.recv().await {
                apply(value, cx);
                for _ in 1..config.maximum_batch_size.get() {
                    let Ok(value) = receiver.try_recv() else {
                        break;
                    };
                    apply(value, cx);
                }
                Timer::after(Duration::ZERO).await;
            }
        });
        (Self { sender }, task)
    }

    /// Waits for queue capacity and sends a value to the foreground consumer.
    ///
    /// # Errors
    ///
    /// Returns [`UiDispatchError::Closed`] after the consumer stops.
    pub async fn send(&self, value: T) -> Result<(), UiDispatchError> {
        self.sender
            .send(value)
            .await
            .map_err(|_| UiDispatchError::Closed)
    }

    /// Attempts to send without waiting for queue capacity.
    ///
    /// # Errors
    ///
    /// Returns `Full` when backpressure is active or `Closed` after the
    /// consumer stops.
    pub fn try_send(&self, value: T) -> Result<(), UiDispatchError> {
        self.sender.try_send(value).map_err(map_try_send_error)
    }

    /// Returns a non-blocking diagnostic snapshot of the event queue.
    #[must_use]
    pub fn queue_state(&self) -> UiQueueState {
        UiQueueState {
            remaining_capacity: self.sender.capacity(),
            maximum_capacity: self.sender.max_capacity(),
            closed: self.sender.is_closed(),
        }
    }
}

/// Coalescing bridge for high-frequency render snapshots.
///
/// Unlike [`UiStreamBridge`], intermediate values may be replaced before the
/// foreground observes them. Use this for progress, status, viewport, and
/// other "latest state wins" data. Use the stream bridge for ordered events
/// that must not be dropped.
#[derive(Clone)]
pub struct UiSnapshotBridge<T> {
    sender: watch::Sender<T>,
}

impl<T> UiSnapshotBridge<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Installs a foreground snapshot consumer.
    ///
    /// The initial value defines producer state but is not applied until a
    /// subsequent publication. The returned task owns the foreground
    /// subscription and must be stored with the consuming entity or global.
    #[must_use]
    pub fn install(
        initial: T,
        cx: &mut App,
        mut apply: impl FnMut(T, &mut AsyncApp) + 'static,
    ) -> (Self, Task<()>) {
        let (sender, mut receiver) = watch::channel(initial);
        let task = cx.spawn(async move |cx| {
            while receiver.changed().await.is_ok() {
                let value = receiver.borrow_and_update().clone();
                apply(value, cx);
                Timer::after(Duration::ZERO).await;
            }
        });
        (Self { sender }, task)
    }

    /// Publishes the newest render snapshot, replacing an unseen older value.
    ///
    /// # Errors
    ///
    /// Returns [`UiDispatchError::Closed`] after the foreground consumer stops.
    pub fn publish(&self, value: T) -> Result<(), UiDispatchError> {
        self.sender.send(value).map_err(|_| UiDispatchError::Closed)
    }

    /// Returns whether the foreground snapshot consumer has stopped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

fn map_try_send_error<T>(error: mpsc::error::TrySendError<T>) -> UiDispatchError {
    match error {
        mpsc::error::TrySendError::Full(_) => UiDispatchError::Full,
        mpsc::error::TrySendError::Closed(_) => UiDispatchError::Closed,
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::{UiBridgeConfig, UiDispatchError, UiHandle};

    #[test]
    fn bounded_ui_handle_reports_backpressure_and_closure() {
        let capacity = NonZeroUsize::new(1).expect("non-zero capacity");
        let (handle, receiver) = UiHandle::channel(UiBridgeConfig {
            capacity,
            maximum_batch_size: capacity,
        });

        handle.try_dispatch(|_| {}).expect("first action");
        assert_eq!(handle.try_dispatch(|_| {}), Err(UiDispatchError::Full));
        assert_eq!(handle.queue_state().remaining_capacity, 0);

        drop(receiver);
        assert_eq!(handle.try_dispatch(|_| {}), Err(UiDispatchError::Closed));
    }
}
