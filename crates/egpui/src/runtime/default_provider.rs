use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use futures_util::future::BoxFuture;
use rayon::{ThreadPool, ThreadPoolBuilder};
use tokio::{
    runtime::{Builder, Runtime},
    sync::Semaphore,
};
use tokio_util::task::TaskTracker;

use super::{RuntimeConfig, RuntimeError, RuntimeProvider, ScheduledTask, ShutdownReport};

struct ProviderState {
    accepting: bool,
    runtime: Option<Runtime>,
}

struct CpuActivity {
    active: std::sync::atomic::AtomicUsize,
    idle_gate: Mutex<()>,
    idle: Condvar,
}

impl CpuActivity {
    fn new() -> Self {
        Self {
            active: std::sync::atomic::AtomicUsize::new(0),
            idle_gate: Mutex::new(()),
            idle: Condvar::new(),
        }
    }

    fn start(self: &Arc<Self>) -> CpuActivityGuard {
        self.active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        CpuActivityGuard {
            activity: self.clone(),
        }
    }

    fn active(&self) -> usize {
        self.active.load(std::sync::atomic::Ordering::Acquire)
    }

    fn wait_until(&self, deadline: Instant) -> Result<(), RuntimeError> {
        let guard = self.idle_gate.lock().map_err(|_| RuntimeError::Poisoned)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (guard, _wait_result) = self
            .idle
            .wait_timeout_while(guard, remaining, |_| self.active() != 0)
            .map_err(|_| RuntimeError::Poisoned)?;
        drop(guard);
        Ok(())
    }
}

struct CpuActivityGuard {
    activity: Arc<CpuActivity>,
}

impl Drop for CpuActivityGuard {
    fn drop(&mut self) {
        if self
            .activity
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            self.activity.idle.notify_all();
        }
    }
}

/// Default execution provider backed by Tokio and Rayon.
pub struct DefaultRuntimeProvider {
    state: Mutex<ProviderState>,
    tracker: TaskTracker,
    blocking_slots: Arc<Semaphore>,
    cpu_pool: ThreadPool,
    cpu_activity: Arc<CpuActivity>,
}

impl DefaultRuntimeProvider {
    /// Builds the default physical execution domains.
    ///
    /// # Errors
    ///
    /// Returns an error when Tokio or Rayon cannot create its workers.
    pub fn new(config: RuntimeConfig) -> Result<Self, RuntimeError> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(config.io_worker_threads.get())
            .max_blocking_threads(config.max_blocking_tasks.get())
            .thread_name("egpui-io")
            .enable_all()
            .build()
            .map_err(RuntimeError::Build)?;
        let cpu_pool = ThreadPoolBuilder::new()
            .num_threads(config.cpu_worker_threads.get())
            .thread_name(|index| format!("egpui-cpu-{index}"))
            .build()
            .map_err(|error| RuntimeError::Build(std::io::Error::other(error)))?;

        Ok(Self {
            state: Mutex::new(ProviderState {
                accepting: true,
                runtime: Some(runtime),
            }),
            tracker: TaskTracker::new(),
            blocking_slots: Arc::new(Semaphore::new(config.max_blocking_tasks.get())),
            cpu_pool,
            cpu_activity: Arc::new(CpuActivity::new()),
        })
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, ProviderState>, RuntimeError> {
        self.state.lock().map_err(|_| RuntimeError::Poisoned)
    }
}

impl RuntimeProvider for DefaultRuntimeProvider {
    fn spawn_io(&self, future: BoxFuture<'static, ()>) -> Result<ScheduledTask, RuntimeError> {
        let state = self.state()?;
        if !state.accepting {
            return Err(RuntimeError::Closed);
        }
        let runtime = state.runtime.as_ref().ok_or(RuntimeError::Closed)?;
        let task = self.tracker.spawn_on(future, runtime.handle());
        drop(state);

        Ok(ScheduledTask::abortable(task.abort_handle()))
    }

    fn spawn_blocking(
        &self,
        operation: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<ScheduledTask, RuntimeError> {
        let state = self.state()?;
        if !state.accepting {
            return Err(RuntimeError::Closed);
        }
        let runtime = state.runtime.as_ref().ok_or(RuntimeError::Closed)?;
        let worker_handle = runtime.handle().clone();
        let blocking_slots = self.blocking_slots.clone();
        let future = async move {
            let permit = match blocking_slots.acquire_owned().await {
                Ok(permit) => permit,
                Err(_closed) => return,
            };
            let worker = worker_handle.spawn_blocking(move || {
                let _permit = permit;
                operation();
            });
            if worker.await.is_err() {
                return;
            }
        };
        let task = self.tracker.spawn_on(future, runtime.handle());
        drop(state);

        Ok(ScheduledTask::abortable(task.abort_handle()))
    }

    fn spawn_cpu(
        &self,
        operation: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<ScheduledTask, RuntimeError> {
        let state = self.state()?;
        if !state.accepting {
            return Err(RuntimeError::Closed);
        }
        if state.runtime.is_none() {
            return Err(RuntimeError::Closed);
        }
        let activity_guard = self.cpu_activity.start();
        self.cpu_pool.spawn(move || {
            let _activity_guard = activity_guard;
            operation();
        });
        drop(state);

        Ok(ScheduledTask::cooperative_only())
    }

    fn request_shutdown(&self) {
        if let Ok(mut state) = self.state() {
            state.accepting = false;
            self.tracker.close();
        }
    }

    fn shutdown(&self, timeout: Duration) -> Result<ShutdownReport, RuntimeError> {
        let started = Instant::now();
        let deadline = started.checked_add(timeout).unwrap_or(started);
        let runtime = {
            let mut state = self.state()?;
            state.accepting = false;
            self.tracker.close();
            state.runtime.take()
        };
        let Some(runtime) = runtime else {
            return Ok(ShutdownReport {
                async_tasks_drained: true,
                remaining_cpu_tasks: self.cpu_activity.active(),
                elapsed: started.elapsed(),
            });
        };

        let async_tasks_drained = runtime.block_on(async {
            tokio::time::timeout(timeout, self.tracker.wait())
                .await
                .is_ok()
        });
        self.cpu_activity.wait_until(deadline)?;
        runtime.shutdown_timeout(deadline.saturating_duration_since(Instant::now()));

        Ok(ShutdownReport {
            async_tasks_drained,
            remaining_cpu_tasks: self.cpu_activity.active(),
            elapsed: started.elapsed(),
        })
    }
}
