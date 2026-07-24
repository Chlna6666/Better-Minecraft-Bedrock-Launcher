use async_task::Runnable;
use std::{
    future::Future,
    mem::ManuallyDrop,
    panic::Location,
    pin::Pin,
    task::{Context, Poll},
    thread::{self, ThreadId},
};

/// Variant of `async_task::spawn_local` that includes the source location of the spawn in panics.
///
/// Copy-modified from:
/// <https://github.com/smol-rs/async-task/blob/ca9dbe1db9c422fd765847fa91306e30a6bb58a9/src/runnable.rs#L405>
#[track_caller]
pub(super) fn spawn_local_with_source_location<Fut, S>(
    future: Fut,
    schedule: S,
) -> (Runnable<()>, async_task::Task<Fut::Output, ()>)
where
    Fut: Future + 'static,
    Fut::Output: 'static,
    S: async_task::Schedule<()> + Send + Sync + 'static,
{
    #[inline]
    fn thread_id() -> ThreadId {
        std::thread_local! {
            static ID: ThreadId = thread::current().id();
        }
        ID.try_with(|id| *id)
            .unwrap_or_else(|_| thread::current().id())
    }

    struct Checked<F> {
        id: ThreadId,
        inner: ManuallyDrop<F>,
        location: &'static Location<'static>,
    }

    impl<F> Drop for Checked<F> {
        fn drop(&mut self) {
            assert!(
                self.id == thread_id(),
                "local task dropped by a thread that didn't spawn it. Task spawned at {}",
                self.location
            );
            unsafe { ManuallyDrop::drop(&mut self.inner) };
        }
    }

    impl<F: Future> Future for Checked<F> {
        type Output = F::Output;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            assert!(
                self.id == thread_id(),
                "local task polled by a thread that didn't spawn it. Task spawned at {}",
                self.location
            );
            unsafe { self.map_unchecked_mut(|c| &mut *c.inner).poll(cx) }
        }
    }

    // Wrap the future into one that checks which thread it's on.
    let future = Checked {
        id: thread_id(),
        inner: ManuallyDrop::new(future),
        location: Location::caller(),
    };

    unsafe { async_task::spawn_unchecked(future, schedule) }
}
