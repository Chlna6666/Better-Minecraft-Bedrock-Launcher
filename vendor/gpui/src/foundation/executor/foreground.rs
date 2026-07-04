use super::{
    local_task::spawn_local_with_source_location,
    task::{AnyLocalFuture, Task},
};
use crate::PlatformDispatcher;
use std::{future::Future, marker::PhantomData, rc::Rc, sync::Arc};

/// A pointer to the executor that is currently running,
/// for spawning tasks on the main thread.
///
/// This is intentionally `!Send` via the `not_send` marker field. This is because
/// `ForegroundExecutor::spawn` does not require `Send` but checks at runtime that the future is
/// only polled from the same thread it was spawned from. These checks would fail when spawning
/// foreground tasks from from background threads.
#[derive(Clone)]
pub struct ForegroundExecutor {
    #[doc(hidden)]
    pub dispatcher: Arc<dyn PlatformDispatcher>,
    not_send: PhantomData<Rc<()>>,
}

/// ForegroundExecutor runs things on the main thread.
impl ForegroundExecutor {
    /// Creates a new ForegroundExecutor from the given PlatformDispatcher.
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        Self {
            dispatcher,
            not_send: PhantomData,
        }
    }

    /// Enqueues the given Task to run on the main thread at some point in the future.
    #[track_caller]
    pub fn spawn<R>(&self, future: impl Future<Output = R> + 'static) -> Task<R>
    where
        R: 'static,
    {
        let dispatcher = self.dispatcher.clone();

        #[track_caller]
        fn inner<R: 'static>(
            dispatcher: Arc<dyn PlatformDispatcher>,
            future: AnyLocalFuture<R>,
        ) -> Task<R> {
            let (runnable, task) = spawn_local_with_source_location(future, move |runnable| {
                dispatcher.dispatch_on_main_thread(runnable)
            });
            runnable.schedule();
            Task::spawned(task)
        }
        inner::<R>(dispatcher, Box::pin(future))
    }
}
