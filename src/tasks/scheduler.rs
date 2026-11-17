//! Priority-aware dispatch gates for slot-limited task pools.
//!
//! Tokio's `Semaphore` is a fair FIFO queue with no way to reorder waiters, so
//! `Prioritize` cannot work on top of it. Each pool (downloads, archive/install)
//! instead owns a [`PriorityGate`]: workers still spawn immediately and keep
//! their abort/telemetry plumbing, but the first thing they await is a gate
//! grant. The gate picks the highest-priority waiter (FIFO within a priority,
//! with aging so low priorities cannot starve) whenever a slot frees up.
//!
//! Queue-level controls live here too: pause queue, resume queue and
//! set-concurrency all mutate gate state instead of a semaphore.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::task_manager::{is_cancelled, is_paused};

/// Scheduling priority. Lower bucket index wins; `Critical` is reserved for
/// host-critical work and is never produced by aging.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Critical,
    High,
    #[default]
    Normal,
    Background,
}

impl TaskPriority {
    pub const fn bucket(self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Background => 3,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Normal => "normal",
            Self::Background => "background",
        }
    }
}

/// Slot pools that dispatch through priority gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskPool {
    Download,
    Archive,
}

impl TaskPool {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Download => "download",
            Self::Archive => "archive",
        }
    }
}

const DEFAULT_DOWNLOAD_CONCURRENCY: usize = 2;
const DEFAULT_ARCHIVE_CONCURRENCY: usize = 2;

/// A queued waiter is promoted one priority level per interval spent waiting,
/// down to `High` — background work cannot starve, but also never reaches
/// `Critical`.
const AGING_INTERVAL: Duration = Duration::from_secs(30);

struct Waiter {
    task_id: String,
    priority: TaskPriority,
    enqueued_at: Instant,
    seq: u64,
    grant: oneshot::Sender<bool>,
}

struct GateState {
    waiters: Vec<Waiter>,
    active: usize,
    paused: bool,
    next_seq: u64,
}

/// A granted dispatch slot. Dropping it releases the slot and dispatches the
/// next waiter. A "cancelled" grant does not consume capacity: the worker wakes
/// only to observe the cancel flag, finish the task and exit.
pub struct GatePermit {
    gate: Arc<PriorityGate>,
    counted: bool,
}

impl Drop for GatePermit {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().unwrap();
        if self.counted {
            state.active = state.active.saturating_sub(1);
        }
        PriorityGate::dispatch_locked(&mut state, self.gate.capacity.load(Ordering::Relaxed));
    }
}

pub struct PriorityGate {
    capacity: AtomicUsize,
    state: Mutex<GateState>,
}

impl PriorityGate {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity: AtomicUsize::new(capacity.max(1)),
            state: Mutex::new(GateState {
                waiters: Vec::new(),
                active: 0,
                paused: false,
                next_seq: 0,
            }),
        })
    }

    /// Changes the dispatch capacity. Raising it dispatches waiting tasks
    /// immediately; lowering it lets running tasks finish before new ones start.
    pub fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity.max(1), Ordering::Relaxed);
        let mut state = self.state.lock().unwrap();
        Self::dispatch_locked(&mut state, capacity.max(1));
    }

    pub fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Stops granting new slots. Running tasks are unaffected.
    pub fn set_paused(&self, paused: bool) {
        let mut state = self.state.lock().unwrap();
        state.paused = paused;
        if !paused {
            Self::dispatch_locked(&mut state, self.capacity.load(Ordering::Relaxed));
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().unwrap().paused
    }

    pub fn waiting_count(&self) -> usize {
        self.state.lock().unwrap().waiters.len()
    }

    /// Bumps a waiting task's priority. Returns false when the task is not in
    /// this gate's queue (already dispatched or never submitted here).
    pub fn prioritize(&self, task_id: &str, priority: TaskPriority) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(waiter) = state.waiters.iter_mut().find(|w| w.task_id == task_id) else {
            return false;
        };
        waiter.priority = priority;
        Self::dispatch_locked(&mut state, self.capacity.load(Ordering::Relaxed));
        true
    }

    /// Re-runs dispatch so a waiter that was just cancelled can be granted
    /// (and finish) without waiting for an unrelated slot release.
    pub fn notify_cancelled(&self, task_id: &str) {
        let mut state = self.state.lock().unwrap();
        if state.waiters.iter().any(|waiter| waiter.task_id == task_id) {
            Self::dispatch_locked(&mut state, self.capacity.load(Ordering::Relaxed));
        }
    }

    /// Re-runs dispatch after a pause was lifted so the previously skipped
    /// waiter can be granted again.
    pub fn notify_resumed(&self, task_id: &str) {
        let mut state = self.state.lock().unwrap();
        if state.waiters.iter().any(|waiter| waiter.task_id == task_id) {
            Self::dispatch_locked(&mut state, self.capacity.load(Ordering::Relaxed));
        }
    }

    /// Waits for a dispatch slot. Returns `None` when the gate is closed
    /// (currently never; kept for symmetry with `Semaphore::acquire_owned`).
    pub async fn acquire(
        self: &Arc<Self>,
        task_id: &str,
        priority: TaskPriority,
    ) -> Option<GatePermit> {
        let (grant, wait) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            let seq = state.next_seq.wrapping_add(1);
            state.next_seq = seq;
            state.waiters.push(Waiter {
                task_id: task_id.to_string(),
                priority,
                enqueued_at: Instant::now(),
                seq,
                grant,
            });
            Self::dispatch_locked(&mut state, self.capacity.load(Ordering::Relaxed));
        }
        match wait.await {
            Ok(counted) => Some(GatePermit {
                gate: Arc::clone(self),
                counted,
            }),
            Err(_) => None,
        }
    }

    fn dispatch_locked(state: &mut GateState, capacity: usize) {
        loop {
            // Cancelled waiters are granted immediately and without consuming a
            // slot: their worker only needs to wake up, observe the cancel flag,
            // finish the task and exit. This keeps cooperative cancellation of
            // queued tasks from waiting on unrelated work.
            if let Some(index) = state
                .waiters
                .iter()
                .position(|waiter| is_cancelled(&waiter.task_id))
            {
                let waiter = state.waiters.remove(index);
                // A dropped receiver means the worker was aborted: nothing to
                // grant, keep going.
                let _ = waiter.grant.send(false);
                continue;
            }

            if state.paused || state.active >= capacity {
                return;
            }
            // Paused waiters keep their queue position but must not consume a
            // slot: a paused download should not block other queued work.
            let Some(index) = best_waiter_index(state) else {
                return;
            };
            let waiter = state.waiters.remove(index);
            match waiter.grant.send(true) {
                Ok(()) => state.active += 1,
                // Receiver already dropped (aborted task): do not consume a
                // slot, dispatch the next waiter instead.
                Err(_) => continue,
            }
        }
    }
}

fn effective_bucket(waiter: &Waiter, now: Instant) -> u8 {
    let base = waiter.priority.bucket();
    if base == 0 {
        return 0;
    }
    let steps = (now.saturating_duration_since(waiter.enqueued_at).as_secs()
        / AGING_INTERVAL.as_secs()) as u8;
    // Aging floor is `High`: long-waiting background work outranks fresh
    // normal work, but never claims `Critical`.
    base.saturating_sub(steps).max(1)
}

fn best_waiter_index(state: &GateState) -> Option<usize> {
    let now = Instant::now();
    state
        .waiters
        .iter()
        .enumerate()
        .filter(|(_, waiter)| !is_paused(&waiter.task_id))
        .min_by_key(|(_, waiter)| (effective_bucket(waiter, now), waiter.seq))
        .map(|(index, _)| index)
}

static DOWNLOAD_GATE: OnceLock<Arc<PriorityGate>> = OnceLock::new();
static ARCHIVE_GATE: OnceLock<Arc<PriorityGate>> = OnceLock::new();

fn gate(pool: TaskPool) -> &'static Arc<PriorityGate> {
    match pool {
        TaskPool::Download => {
            DOWNLOAD_GATE.get_or_init(|| PriorityGate::new(DEFAULT_DOWNLOAD_CONCURRENCY))
        }
        TaskPool::Archive => {
            ARCHIVE_GATE.get_or_init(|| PriorityGate::new(DEFAULT_ARCHIVE_CONCURRENCY))
        }
    }
}

/// Waits for a slot in `pool`. See [`PriorityGate::acquire`].
pub async fn acquire(pool: TaskPool, task_id: &str, priority: TaskPriority) -> Option<GatePermit> {
    gate(pool).acquire(task_id, priority).await
}

/// Bumps the priority of a queued task in whichever pool holds it. Returns
/// false when the task is not currently waiting in any pool.
pub fn prioritize(task_id: &str, priority: TaskPriority) -> bool {
    let mut found = false;
    for pool in [TaskPool::Download, TaskPool::Archive] {
        found |= gate(pool).prioritize(task_id, priority);
    }
    found
}

/// Wakes both gates so a newly cancelled waiter can be granted (and finish)
/// without waiting for an unrelated slot release.
pub fn notify_cancelled(task_id: &str) {
    for pool in [TaskPool::Download, TaskPool::Archive] {
        gate(pool).notify_cancelled(task_id);
    }
}

/// Re-runs dispatch after a pause was lifted, so a queued task that was
/// skipped while paused gets its slot back.
pub fn notify_resumed(task_id: &str) {
    for pool in [TaskPool::Download, TaskPool::Archive] {
        gate(pool).notify_resumed(task_id);
    }
}

/// Pauses or resumes dispatch for one pool. Running tasks keep running.
pub fn set_pool_paused(pool: TaskPool, paused: bool) {
    gate(pool).set_paused(paused);
}

pub fn is_pool_paused(pool: TaskPool) -> bool {
    gate(pool).is_paused()
}

/// Changes the concurrency limit of one pool.
pub fn set_pool_concurrency(pool: TaskPool, concurrency: usize) {
    gate(pool).set_capacity(concurrency);
}

pub fn pool_concurrency(pool: TaskPool) -> usize {
    gate(pool).capacity()
}

pub fn waiting_count(pool: TaskPool) -> usize {
    gate(pool).waiting_count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn unique_task_id(tag: &str) -> String {
        format!(
            "gate-test-{tag}-{}",
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn waiter(task_id: &str, priority: TaskPriority, seq: u64) -> Waiter {
        let (grant, _) = oneshot::channel();
        Waiter {
            task_id: task_id.to_string(),
            priority,
            enqueued_at: Instant::now(),
            seq,
            grant,
        }
    }

    #[tokio::test]
    async fn higher_priority_waiter_is_granted_first() {
        let gate = PriorityGate::new(1);
        let first = gate
            .acquire("gate-test-holder", TaskPriority::Normal)
            .await
            .expect("granted");

        let (bg_id, high_id) = (unique_task_id("bg"), unique_task_id("high"));
        let bg_gate = Arc::clone(&gate);
        let bg_id_task = bg_id.clone();
        // The JoinHandle keeps the granted permit alive until awaited, so a
        // finished waiter still holds its slot.
        let bg =
            tokio::spawn(
                async move { bg_gate.acquire(&bg_id_task, TaskPriority::Background).await },
            );
        // Ensure the background waiter is enqueued before the high one.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let high_gate = Arc::clone(&gate);
        let high_id_task = high_id.clone();
        let high =
            tokio::spawn(async move { high_gate.acquire(&high_id_task, TaskPriority::High).await });
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Freeing the single slot must grant High before Background.
        drop(first);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(high.is_finished(), "high priority waiter should be granted");
        assert!(!bg.is_finished(), "background waiter must still wait");

        let high_permit = high.await.expect("high join").expect("high granted");
        drop(high_permit);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(bg.is_finished(), "background waiter proceeds after release");
        let _ = bg.await;
    }

    #[tokio::test]
    async fn paused_queue_holds_new_grants_until_resumed() {
        let gate = PriorityGate::new(2);
        gate.set_paused(true);
        let gate_task = Arc::clone(&gate);
        let waiter_id = unique_task_id("paused");
        let waiting = tokio::spawn(async move {
            gate_task
                .acquire(&waiter_id, TaskPriority::Normal)
                .await
                .is_some()
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        gate.set_paused(false);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(waiting.is_finished());
        assert!(waiting.await.unwrap());
    }

    #[tokio::test]
    async fn raising_concurrency_dispatches_waiting_tasks() {
        let gate = PriorityGate::new(1);
        let holder = gate
            .acquire(&unique_task_id("holder"), TaskPriority::Normal)
            .await
            .expect("granted");
        let mut waiters = Vec::new();
        for _ in 0..2 {
            let gate_task = Arc::clone(&gate);
            let id = unique_task_id("raise");
            waiters.push(tokio::spawn(async move {
                gate_task.acquire(&id, TaskPriority::Normal).await.is_some()
            }));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(waiters.iter().all(|waiter| !waiter.is_finished()));
        gate.set_capacity(3);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(waiters.iter().all(|waiter| waiter.is_finished()));
        drop(holder);
        for waiter in waiters {
            assert!(waiter.await.unwrap());
        }
    }

    #[tokio::test]
    async fn prioritize_moves_waiter_ahead() {
        let gate = PriorityGate::new(1);
        let holder = gate
            .acquire(&unique_task_id("holder"), TaskPriority::Normal)
            .await
            .expect("granted");

        let normal_id = unique_task_id("normal");
        let gate_task = Arc::clone(&gate);
        let normal_task_id = normal_id.clone();
        let normal = tokio::spawn(async move {
            gate_task
                .acquire(&normal_task_id, TaskPriority::Normal)
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let bg_id = unique_task_id("bg");
        let gate_task = Arc::clone(&gate);
        let bg_task_id = bg_id.clone();
        let background = tokio::spawn(async move {
            gate_task
                .acquire(&bg_task_id, TaskPriority::Background)
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(gate.prioritize(&bg_id, TaskPriority::High));
        drop(holder);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            background.is_finished(),
            "prioritized waiter should be granted before normal"
        );
        assert!(!normal.is_finished());
        let background_permit = background
            .await
            .expect("background join")
            .expect("background granted");
        drop(background_permit);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(normal.is_finished());
        let _ = normal.await;
    }

    #[test]
    fn aging_promotes_long_waiting_waiter_but_never_to_critical() {
        let now = Instant::now();
        let mut background = waiter("aging-bg", TaskPriority::Background, 1);
        background.enqueued_at = now - AGING_INTERVAL * 3;
        assert_eq!(
            effective_bucket(&background, now),
            1,
            "background ages to high"
        );

        let mut normal = waiter("aging-normal", TaskPriority::Normal, 2);
        normal.enqueued_at = now - AGING_INTERVAL * 10;
        assert_eq!(effective_bucket(&normal, now), 1, "aging floor is high");

        let critical = waiter("aging-critical", TaskPriority::Critical, 3);
        assert_eq!(effective_bucket(&critical, now), 0);
    }

    #[tokio::test]
    async fn cancelled_waiter_is_granted_without_consuming_capacity() {
        use crate::tasks::stage::TaskStage;
        use crate::tasks::task_manager::{
            TaskFinishStatus, TaskOptions, cancel_task, create_task, finish_task,
            register_cooperative_cancel, remove_task,
        };

        let task_id = unique_task_id("cancelled");
        create_task(TaskOptions::new("排队取消测试", TaskStage::RUNNING).id(task_id.clone()))
            .expect("unique id");
        register_cooperative_cancel(task_id.clone());

        let gate = PriorityGate::new(1);
        let holder = gate
            .acquire(&unique_task_id("holder"), TaskPriority::Normal)
            .await
            .expect("granted");

        let gate_task = Arc::clone(&gate);
        let waiter_task_id = task_id.clone();
        let waiting = tokio::spawn(async move {
            gate_task
                .acquire(&waiter_task_id, TaskPriority::Normal)
                .await
                .is_some()
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());

        // Cancel while queued: the waiter must be woken even though the slot is
        // still held, and the grant must not count against capacity. Production
        // wiring does this from `cancel_task`; this gate is local to the test,
        // so notify it directly.
        cancel_task(&task_id);
        gate.notify_cancelled(&task_id);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(waiting.is_finished(), "cancelled waiter should wake");

        finish_task(&task_id, TaskFinishStatus::Cancelled, None);
        drop(holder);
        assert!(remove_task(&task_id));
        assert!(waiting.await.unwrap());
    }
}
