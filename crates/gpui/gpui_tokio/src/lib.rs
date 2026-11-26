use std::future::Future;

use gpui::{App, AppContext, AsyncApp, Context, Global, Task};
use gpui_util::defer;

pub use tokio::task::JoinError;

/// Initializes the Tokio wrapper using a new Tokio runtime with two worker threads.
pub fn init(cx: &mut App) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    cx.set_global(GlobalTokio {
        owned_runtime: Some(runtime),
        handle,
    });
    Ok(())
}

/// Initializes the Tokio wrapper using an existing Tokio runtime handle.
pub fn init_from_handle(cx: &mut App, handle: tokio::runtime::Handle) {
    cx.set_global(GlobalTokio {
        owned_runtime: None,
        handle,
    });
}

struct GlobalTokio {
    owned_runtime: Option<tokio::runtime::Runtime>,
    handle: tokio::runtime::Handle,
}

impl Global for GlobalTokio {}

impl Drop for GlobalTokio {
    fn drop(&mut self) {
        if let Some(runtime) = self.owned_runtime.take() {
            runtime.shutdown_background();
        }
    }
}

pub struct Tokio;

#[doc(hidden)]
pub trait TokioContext {
    fn tokio_handle(&self) -> anyhow::Result<tokio::runtime::Handle>;

    fn tokio_background_spawn<R>(
        &self,
        future: impl Future<Output = R> + Send + 'static,
    ) -> Task<R>
    where
        R: Send + 'static;
}

impl TokioContext for App {
    fn tokio_handle(&self) -> anyhow::Result<tokio::runtime::Handle> {
        self.try_global::<GlobalTokio>()
            .map(|tokio| tokio.handle.clone())
            .ok_or_else(|| anyhow::anyhow!("gpui_tokio is not initialized"))
    }

    fn tokio_background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.background_spawn(future)
    }
}

impl<T: 'static> TokioContext for Context<'_, T> {
    fn tokio_handle(&self) -> anyhow::Result<tokio::runtime::Handle> {
        (**self).tokio_handle()
    }

    fn tokio_background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.background_spawn(future)
    }
}

impl TokioContext for AsyncApp {
    fn tokio_handle(&self) -> anyhow::Result<tokio::runtime::Handle> {
        self.read_global(|tokio: &GlobalTokio, _cx| tokio.handle.clone())
    }

    fn tokio_background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.background_spawn(future)
    }
}

impl Tokio {
    /// Spawns a future on Tokio and returns its completion through a GPUI task.
    /// Dropping the GPUI task cancels the Tokio task.
    pub fn spawn<Fut, R>(cx: &App, future: Fut) -> Task<Result<R, JoinError>>
    where
        Fut: Future<Output = R> + Send + 'static,
        R: Send + 'static,
    {
        let join_handle = cx.global::<GlobalTokio>().handle.spawn(future);
        let abort_handle = join_handle.abort_handle();
        let cancel = defer(move || abort_handle.abort());
        cx.background_spawn(async move {
            let result = join_handle.await;
            drop(cancel);
            result
        })
    }

    /// Spawns a fallible future on Tokio and flattens task and operation failures.
    /// Dropping the GPUI task cancels the Tokio task.
    pub fn spawn_result<C, Fut, R>(cx: &C, future: Fut) -> Task<anyhow::Result<R>>
    where
        C: TokioContext,
        Fut: Future<Output = anyhow::Result<R>> + Send + 'static,
        R: Send + 'static,
    {
        let join_handle = match cx.tokio_handle() {
            Ok(handle) => handle.spawn(future),
            Err(error) => return Task::ready(Err(error)),
        };
        let abort_handle = join_handle.abort_handle();
        let cancel = defer(move || abort_handle.abort());
        cx.tokio_background_spawn(async move {
            let result = join_handle.await?;
            drop(cancel);
            result
        })
    }

    pub fn handle(cx: &App) -> tokio::runtime::Handle {
        cx.global::<GlobalTokio>().handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    use gpui::TestAppContext;

    use super::*;

    struct DropSignal(Arc<AtomicBool>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    fn wait_for_flag(flag: &AtomicBool, message: &str) {
        let started_at = Instant::now();
        while !flag.load(Ordering::Acquire) {
            assert!(started_at.elapsed() < Duration::from_secs(2), "{message}");
            std::thread::yield_now();
        }
    }

    #[gpui::test]
    async fn spawn_result_returns_tokio_output(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let task = cx.update(|cx| {
            init(cx).expect("Tokio runtime should initialize");
            Tokio::spawn_result(cx, async { Ok(42) })
        });

        assert_eq!(task.await.expect("Tokio task should complete"), 42);
        cx.executor().forbid_parking();
    }

    #[gpui::test]
    fn dropping_gpui_task_cancels_tokio_producer(cx: &mut TestAppContext) {
        let started = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let task = cx.update({
            let started = Arc::clone(&started);
            let dropped = Arc::clone(&dropped);
            move |cx| {
                init(cx).expect("Tokio runtime should initialize");
                Tokio::spawn(cx, async move {
                    let _drop_signal = DropSignal(dropped);
                    started.store(true, Ordering::Release);
                    std::future::pending::<()>().await;
                })
            }
        });

        wait_for_flag(&started, "Tokio producer did not start");
        drop(task);
        cx.run_until_parked();
        wait_for_flag(&dropped, "Tokio producer was not cancelled");
    }
}
