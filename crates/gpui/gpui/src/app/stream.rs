use std::{future::poll_fn, task::Poll};

/// Yields exactly once so a ready stream cannot monopolize the foreground executor.
pub(super) async fn yield_to_foreground_executor() {
    let mut yielded = false;
    poll_fn(move |cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}
