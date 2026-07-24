use super::{
    scope::Scope,
    task::{AnyFuture, Task, TaskLabel},
};
use crate::PlatformDispatcher;
use std::{
    future::Future,
    mem,
    sync::Arc,
    task::Poll,
    time::{Duration, Instant},
};
use waker_fn::waker_fn;

#[cfg(any(test, feature = "test-support"))]
use rand::rngs::StdRng;
#[cfg(any(test, feature = "test-support"))]
use std::sync::atomic::Ordering::SeqCst;

/// A pointer to the executor that is currently running,
/// for spawning background tasks.
#[derive(Clone)]
pub struct BackgroundExecutor {
    #[doc(hidden)]
    pub dispatcher: Arc<dyn PlatformDispatcher>,
}

/// BackgroundExecutor lets you run things on background threads.
/// In production this is a thread pool with no ordering guarantees.
/// In tests this is simulated by running tasks one by one in a deterministic
/// (but arbitrary) order controlled by the `SEED` environment variable.
impl BackgroundExecutor {
    #[doc(hidden)]
    pub fn new(dispatcher: Arc<dyn PlatformDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Enqueues the given future to be run to completion on a background thread.
    pub fn spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.spawn_internal::<R>(Box::pin(future), None)
    }

    /// Enqueues the given future to be run to completion on a background thread.
    /// The given label can be used to control the priority of the task in tests.
    pub fn spawn_labeled<R>(
        &self,
        label: TaskLabel,
        future: impl Future<Output = R> + Send + 'static,
    ) -> Task<R>
    where
        R: Send + 'static,
    {
        self.spawn_internal::<R>(Box::pin(future), Some(label))
    }

    fn spawn_internal<R: Send + 'static>(
        &self,
        future: AnyFuture<R>,
        label: Option<TaskLabel>,
    ) -> Task<R> {
        let dispatcher = self.dispatcher.clone();
        let (runnable, task) =
            async_task::spawn(future, move |runnable| dispatcher.dispatch(runnable, label));
        runnable.schedule();
        Task::spawned(task)
    }

    /// Used by the test harness to run an async test in a synchronous fashion.
    #[cfg(any(test, feature = "test-support"))]
    #[track_caller]
    pub fn block_test<R>(&self, future: impl Future<Output = R>) -> R {
        if let Ok(value) = self.block_internal(false, future, None) {
            value
        } else {
            unreachable!()
        }
    }

    /// Block the current thread until the given future resolves.
    /// Consider using `block_with_timeout` instead.
    pub fn block<R>(&self, future: impl Future<Output = R>) -> R {
        if let Ok(value) = self.block_internal(true, future, None) {
            value
        } else {
            unreachable!()
        }
    }

    #[cfg(not(any(test, feature = "test-support")))]
    pub(crate) fn block_internal<Fut: Future>(
        &self,
        _background_only: bool,
        future: Fut,
        timeout: Option<Duration>,
    ) -> Result<Fut::Output, impl Future<Output = Fut::Output> + use<Fut>> {
        let mut future = Box::pin(future);
        if timeout == Some(Duration::ZERO) {
            return Err(future);
        }
        let deadline = timeout.map(|timeout| Instant::now() + timeout);

        let parker = parking::Parker::new();
        let unparker = parker.unparker();
        let waker = waker_fn(move || {
            unparker.unpark();
        });
        let mut cx = std::task::Context::from_waker(&waker);

        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(result) => return Ok(result),
                Poll::Pending => {
                    let timeout =
                        deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
                    if let Some(timeout) = timeout {
                        if !parker.park_timeout(timeout)
                            && deadline.is_some_and(|deadline| deadline < Instant::now())
                        {
                            return Err(future);
                        }
                    } else {
                        parker.park();
                    }
                }
            }
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[track_caller]
    pub(crate) fn block_internal<Fut: Future>(
        &self,
        background_only: bool,
        future: Fut,
        timeout: Option<Duration>,
    ) -> Result<Fut::Output, impl Future<Output = Fut::Output> + use<Fut>> {
        use parking::Parker;
        use std::sync::atomic::AtomicBool;

        let mut future = Box::pin(future);
        if timeout == Some(Duration::ZERO) {
            return Err(future);
        }
        let Some(dispatcher) = self.dispatcher.as_test() else {
            return Err(future);
        };

        let mut max_ticks = if timeout.is_some() {
            dispatcher.gen_block_on_ticks()
        } else {
            usize::MAX
        };

        let parker = Parker::new();
        let unparker = parker.unparker();

        let awoken = Arc::new(AtomicBool::new(false));
        let waker = waker_fn({
            let awoken = awoken.clone();
            let unparker = unparker.clone();
            move || {
                awoken.store(true, SeqCst);
                unparker.unpark();
            }
        });
        let mut cx = std::task::Context::from_waker(&waker);

        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(result) => return Ok(result),
                Poll::Pending => {
                    if max_ticks == 0 {
                        return Err(future);
                    }
                    max_ticks -= 1;

                    if !dispatcher.tick(background_only) {
                        if awoken.swap(false, SeqCst) {
                            continue;
                        }

                        if !dispatcher.parking_allowed() {
                            if dispatcher.advance_clock_to_next_delayed() {
                                continue;
                            }
                            let mut backtrace_message = String::new();
                            let mut waiting_message = String::new();
                            if let Some(backtrace) = dispatcher.waiting_backtrace() {
                                backtrace_message =
                                    format!("\nbacktrace of waiting future:\n{:?}", backtrace);
                            }
                            if let Some(waiting_hint) = dispatcher.waiting_hint() {
                                waiting_message = format!("\n  waiting on: {}\n", waiting_hint);
                            }
                            panic!(
                                "parked with nothing left to run{waiting_message}{backtrace_message}",
                            )
                        }
                        dispatcher.set_unparker(unparker.clone());
                        parker.park();
                    }
                }
            }
        }
    }

    /// Block the current thread until the given future resolves
    /// or `duration` has elapsed.
    pub fn block_with_timeout<Fut: Future>(
        &self,
        duration: Duration,
        future: Fut,
    ) -> Result<Fut::Output, impl Future<Output = Fut::Output> + use<Fut>> {
        self.block_internal(true, future, Some(duration))
    }

    /// Scoped lets you start a number of tasks and waits
    /// for all of them to complete before returning.
    pub async fn scoped<'scope, F>(&self, scheduler: F)
    where
        F: FnOnce(&mut Scope<'scope>),
    {
        let mut scope = Scope::new(self.clone());
        (scheduler)(&mut scope);
        let spawned = mem::take(&mut scope.futures)
            .into_iter()
            .map(|f| self.spawn(f))
            .collect::<Vec<_>>();
        for task in spawned {
            task.await;
        }
    }

    /// Get the current time.
    ///
    /// Calling this instead of `std::time::Instant::now` allows the use
    /// of fake timers in tests.
    pub fn now(&self) -> Instant {
        self.dispatcher.now()
    }

    /// Returns a task that will complete after the given duration.
    /// Depending on other concurrent tasks the elapsed duration may be longer
    /// than requested.
    pub fn timer(&self, duration: Duration) -> Task<()> {
        if duration.is_zero() {
            return Task::ready(());
        }
        let (runnable, task) = async_task::spawn(async move {}, {
            let dispatcher = self.dispatcher.clone();
            move |runnable| dispatcher.dispatch_after(duration, runnable)
        });
        runnable.schedule();
        Task::spawned(task)
    }

    /// in tests, start_waiting lets you indicate which task is waiting (for debugging only)
    #[cfg(any(test, feature = "test-support"))]
    pub fn start_waiting(&self) {
        self.dispatcher.as_test().unwrap().start_waiting();
    }

    /// in tests, removes the debugging data added by start_waiting
    #[cfg(any(test, feature = "test-support"))]
    pub fn finish_waiting(&self) {
        self.dispatcher.as_test().unwrap().finish_waiting();
    }

    /// in tests, run an arbitrary number of tasks (determined by the SEED environment variable)
    #[cfg(any(test, feature = "test-support"))]
    pub fn simulate_random_delay(&self) -> impl Future<Output = ()> + use<> {
        self.dispatcher.as_test().unwrap().simulate_random_delay()
    }

    /// in tests, indicate that a given task from `spawn_labeled` should run after everything else
    #[cfg(any(test, feature = "test-support"))]
    pub fn deprioritize(&self, task_label: TaskLabel) {
        self.dispatcher.as_test().unwrap().deprioritize(task_label)
    }

    /// in tests, move time forward. This does not run any tasks, but does make `timer`s ready.
    #[cfg(any(test, feature = "test-support"))]
    pub fn advance_clock(&self, duration: Duration) {
        self.dispatcher.as_test().unwrap().advance_clock(duration)
    }

    /// in tests, run one task.
    #[cfg(any(test, feature = "test-support"))]
    pub fn tick(&self) -> bool {
        self.dispatcher.as_test().unwrap().tick(false)
    }

    /// in tests, run all tasks that are ready to run. If after doing so
    /// the test still has outstanding tasks, this will panic. (See also [`Self::allow_parking`])
    #[cfg(any(test, feature = "test-support"))]
    pub fn run_until_parked(&self) {
        self.dispatcher.as_test().unwrap().run_until_parked()
    }

    /// in tests, prevents `run_until_parked` from panicking if there are outstanding tasks.
    /// This is useful when you are integrating other (non-GPUI) futures, like disk access, that
    /// do take real async time to run.
    #[cfg(any(test, feature = "test-support"))]
    pub fn allow_parking(&self) {
        self.dispatcher.as_test().unwrap().allow_parking();
    }

    /// undoes the effect of [`Self::allow_parking`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn forbid_parking(&self) {
        self.dispatcher.as_test().unwrap().forbid_parking();
    }

    /// adds detail to the "parked with nothing let to run" message.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_waiting_hint(&self, msg: Option<String>) {
        self.dispatcher.as_test().unwrap().set_waiting_hint(msg);
    }

    /// in tests, returns the rng used by the dispatcher and seeded by the `SEED` environment variable
    #[cfg(any(test, feature = "test-support"))]
    pub fn rng(&self) -> StdRng {
        self.dispatcher.as_test().unwrap().rng()
    }

    /// How many CPUs are available to the dispatcher.
    pub fn num_cpus(&self) -> usize {
        #[cfg(any(test, feature = "test-support"))]
        return 4;

        #[cfg(not(any(test, feature = "test-support")))]
        return num_cpus::get();
    }

    /// Whether we're on the main thread.
    pub fn is_main_thread(&self) -> bool {
        self.dispatcher.is_main_thread()
    }

    #[cfg(any(test, feature = "test-support"))]
    /// in tests, control the number of ticks that `block_with_timeout` will run before timing out.
    pub fn set_block_on_ticks(&self, range: std::ops::RangeInclusive<usize>) {
        self.dispatcher.as_test().unwrap().set_block_on_ticks(range);
    }
}
