use async_task::Runnable;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "freebsd"))]
use collections::VecDeque;
use std::time::{Duration, Instant};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "freebsd"))]
use std::{
    mem,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_FOREGROUND_TASKS_PER_DRAIN: usize = 64;
const MAX_FOREGROUND_TASK_DRAIN_DURATION: Duration = Duration::from_millis(2);

struct ForegroundTaskBudget {
    started_at: Instant,
    task_count: usize,
}

impl ForegroundTaskBudget {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            task_count: 0,
        }
    }

    fn can_run_next(&self) -> bool {
        self.task_count < MAX_FOREGROUND_TASKS_PER_DRAIN
            && (self.task_count == 0
                || self.started_at.elapsed() < MAX_FOREGROUND_TASK_DRAIN_DURATION)
    }

    fn did_run_task(&mut self) {
        self.task_count += 1;
    }
}

pub(crate) fn drain_foreground_tasks(
    mut next_runnable: impl FnMut() -> Option<Runnable>,
    mut has_pending_runnables: impl FnMut() -> bool,
) -> bool {
    let mut budget = ForegroundTaskBudget::new();
    while budget.can_run_next() {
        let Some(runnable) = next_runnable() else {
            return false;
        };
        runnable.run();
        budget.did_run_task();
    }

    has_pending_runnables()
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "freebsd"))]
pub(crate) struct ForegroundTaskQueue {
    queue: Mutex<VecDeque<Runnable>>,
    wakeup_pending: AtomicBool,
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "freebsd"))]
impl ForegroundTaskQueue {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            wakeup_pending: AtomicBool::new(false),
        }
    }

    pub(crate) fn push(&self, runnable: Runnable) -> bool {
        self.queue.lock().unwrap().push_back(runnable);
        !self.wakeup_pending.swap(true, Ordering::AcqRel)
    }

    pub(crate) fn drain(&self) -> bool {
        self.wakeup_pending.store(false, Ordering::Release);
        let needs_wakeup = drain_foreground_tasks(|| self.pop_front(), || self.has_pending());
        needs_wakeup && !self.wakeup_pending.swap(true, Ordering::AcqRel)
    }

    fn pop_front(&self) -> Option<Runnable> {
        self.queue.lock().unwrap().pop_front()
    }

    fn has_pending(&self) -> bool {
        !self.queue.lock().unwrap().is_empty()
    }

    pub(crate) fn forget_pending(&self) {
        let pending = {
            let mut queue = self.queue.lock().unwrap();
            mem::take(&mut *queue)
        };
        for runnable in pending {
            mem::forget(runnable);
        }
        self.wakeup_pending.store(false, Ordering::Release);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "freebsd"))]
impl Drop for ForegroundTaskQueue {
    fn drop(&mut self) {
        let queue = match self.queue.get_mut() {
            Ok(queue) => queue,
            Err(error) => error.into_inner(),
        };
        for runnable in queue.drain(..) {
            mem::forget(runnable);
        }
    }
}
