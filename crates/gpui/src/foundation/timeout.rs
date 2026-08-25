use crate::{BackgroundExecutor, Task};
use std::{future::Future, pin::Pin, task, time::Duration};

/// Extensions for futures that can be bounded by a GPUI executor timer.
pub trait FutureExt {
    /// Requires a future to complete before the specified duration elapses.
    fn with_timeout(self, timeout: Duration, executor: &BackgroundExecutor) -> WithTimeout<Self>
    where
        Self: Sized;
}

impl<T: Future> FutureExt for T {
    fn with_timeout(self, timeout: Duration, executor: &BackgroundExecutor) -> WithTimeout<Self> {
        WithTimeout {
            future: self,
            timer: executor.timer(timeout),
        }
    }
}

#[pin_project::pin_project]
pub struct WithTimeout<T> {
    #[pin]
    future: T,
    #[pin]
    timer: Task<()>,
}

/// Error returned when the timeout elapses before a future resolves.
#[derive(Debug, thiserror::Error)]
#[error("Timed out before future resolved")]
pub struct Timeout;

impl<T: Future> Future for WithTimeout<T> {
    type Output = Result<T::Output, Timeout>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let this = self.project();
        if let task::Poll::Ready(output) = this.future.poll(cx) {
            task::Poll::Ready(Ok(output))
        } else if this.timer.poll(cx).is_ready() {
            task::Poll::Ready(Err(Timeout))
        } else {
            task::Poll::Pending
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
/// Runs a future on smol for no longer than the specified timeout.
pub async fn smol_timeout<F, T>(timeout: Duration, future: F) -> Result<T, ()>
where
    F: Future<Output = T>,
{
    let timer = async {
        smol::Timer::after(timeout).await;
        Err(())
    };
    let future = async move { Ok(future.await) };
    smol::future::FutureExt::race(timer, future).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestAppContext;

    #[gpui::test]
    async fn with_timeout_observes_deadline(cx: &mut TestAppContext) {
        Task::ready(())
            .with_timeout(Duration::from_secs(1), &cx.executor())
            .await
            .expect("completed task should resolve before timeout");

        let long_duration = Duration::from_secs(6000);
        let short_duration = Duration::from_secs(1);
        cx.executor()
            .timer(long_duration)
            .with_timeout(short_duration, &cx.executor())
            .await
            .expect_err("timeout should have triggered");

        let future = cx
            .executor()
            .timer(long_duration)
            .with_timeout(short_duration, &cx.executor());
        cx.executor().advance_clock(short_duration * 2);
        futures::FutureExt::now_or_never(future)
            .unwrap_or_else(|| panic!("timeout should have triggered"))
            .expect_err("timeout should have triggered");
    }
}
