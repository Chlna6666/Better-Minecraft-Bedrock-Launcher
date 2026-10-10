//! Bounded GPU-owner observations, independent of the latest-frame counters.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    sync::OnceLock,
    time::{Duration, Instant},
};

mod cpu;
#[cfg(test)]
mod tests;

const HISTORY_CAPACITY: usize = 1_024;
thread_local! {
    static BLOCKING_WAIT: Cell<Option<Duration>> = const { Cell::new(None) };
}

/// Command executed by the single Nova GPU owner, including non-drawing controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuOwnerJobKind {
    /// Consume a newly committed immutable scene.
    Draw,
    /// Consume a native presentation tick.
    Tick,
    /// Retry or advance retained presentation on the owner.
    Continue,
    /// Accept a new drawable size.
    Resize,
    /// Change transparency, cadence, memory policy or another renderer control.
    Control,
}

/// Result of an owner command; execution alone does not establish GPU submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuOwnerJobOutcome {
    /// A command that did not attempt presentation.
    Control,
    /// The renderer accepted a GPU submission; not physical scanout or GPU completion.
    Submitted,
    /// No submission, including readiness/cadence deferral and empty presentation work.
    Deferred,
    /// A presentation attempt returned an error.
    Failed,
}

/// One completed command. Times use monotonic profiling clocks, not animation samples.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpuOwnerJobSample {
    /// Internal owner proxy identity, distinct from GPUI's WindowId.
    pub owner_id: u64,
    /// GPUI WindowId as u64, bound when the first scene packet arrives.
    pub window_id: Option<u64>,
    /// Per-owner execution sequence, including controls and unsuccessful attempts.
    /// This is a job ID, not a UI FrameId: retained ticks can reuse one committed scene.
    pub job_id: u64,
    /// Command category.
    pub kind: GpuOwnerJobKind,
    /// Whether presentation submitted, deferred, failed, or was not attempted.
    pub outcome: GpuOwnerJobOutcome,
    /// Execution start relative to the shared owner observation origin, in microseconds.
    /// All windows use this origin, allowing cross-window service intervals to be aligned.
    pub started_at_us: u64,
    /// Execution completion relative to the same shared observation origin, in microseconds.
    pub completed_at_us: u64,
    /// Latest producer enqueue to execution start, including producer queue lock contention.
    /// None for autonomous work that never entered the command queue.
    pub queue_wait_us: Option<u64>,
    /// Earliest replaced request's enqueue to execution start. Preserves backlog age under
    /// latest-wins coalescing; None for autonomous work.
    pub pending_age_us: Option<u64>,
    /// Number of older commands merged into this executed request.
    pub coalesced_count: u64,
    /// Owner execution wall time, including backend calls, waits, reports and preemption.
    /// This is neither CPU execution time nor GPU pass duration.
    pub owner_job_duration_us: u64,
    /// Wall time in explicitly instrumented Nova wait_submission calls during this job.
    /// Excludes readiness callbacks, caller handshakes and unobserved driver-internal waits.
    pub owner_blocking_wait_us: u64,
    /// Autonomous scheduling deadline to execution start. The deadline is an earliest eligible
    /// cadence time, not a physical display deadline; None for queued commands.
    pub schedule_lateness_us: Option<u64>,
}

/// Nearest-rank percentiles computed over the bounded history, in microseconds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GpuOwnerDurationDistribution {
    /// Number of applicable samples; zero means no observations.
    pub sample_count: usize,
    /// Median, or zero when sample_count is zero.
    pub p50_us: u64,
    /// 95th percentile.
    pub p95_us: u64,
    /// 99th percentile.
    pub p99_us: u64,
    /// Largest retained observation.
    pub max_us: u64,
}

/// Lifetime service totals and recent latency distributions for one live owner proxy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpuOwnerWindowMetrics {
    /// Internal owner identity; use this key to query raw samples.
    pub owner_id: u64,
    /// Actual WindowId once the first packet binds it; None during initialization.
    pub window_id: Option<u64>,
    /// Completed commands since registration, including controls, failures and deferrals.
    pub completed_jobs: u64,
    /// Lifetime wall time servicing this window, including known waits.
    pub owner_per_window_service_time_us: u64,
    /// Lifetime explicitly instrumented submission waits, a subset of service time.
    pub owner_blocking_wait_time_us: u64,
    /// Latest-enqueue latency distribution; autonomous work is excluded.
    pub queue_wait: GpuOwnerDurationDistribution,
    /// Earliest coalesced request latency distribution.
    pub pending_age: GpuOwnerDurationDistribution,
    /// Execution wall time distribution over all retained commands.
    pub owner_job_duration: GpuOwnerDurationDistribution,
    /// Latest completed command's age; idle windows have stale observations by design.
    pub sample_age_ms: Option<u64>,
    /// Unavailable until a backend provides an actual presentation deadline. A cadence
    /// eligibility timestamp must not be interpreted as a screen display deadline.
    pub presentation_deadline_miss_count: Option<u64>,
}

/// Cached observations of completed GPU-owner work. No owner wake or graphics query is issued.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpuOwnerMetricsSnapshot {
    /// Observation period from the first owner job to this snapshot.
    pub elapsed_us: u64,
    /// Completed global dispatches, including renderer initialization/preparation/destruction.
    pub completed_jobs: u64,
    /// Completed dispatch wall time; excludes command-channel idle waits.
    pub owner_service_time_us: u64,
    /// Completed dispatch wall time / observation period. Includes blocking and preemption.
    pub owner_busy_ratio: f64,
    /// OS thread CPU time / observation period. Windows GetThreadTimes and Linux/FreeBSD
    /// CLOCK_THREAD_CPUTIME_ID are supported;
    /// None on unsupported platforms or query failure. OS accounting can be coarse.
    /// Captured after completed jobs; ongoing work is not included until completion.
    pub owner_cpu_busy_ratio: Option<f64>,
    /// Explicit submission waits in completed jobs, including non-window startup/shutdown work.
    pub owner_blocking_wait_time_us: u64,
    /// Age of the last completed global dispatch; no timer samples idle owners.
    pub sample_age_ms: Option<u64>,
    /// Live owner proxies. Destruction removes their histories and totals.
    pub windows: Vec<GpuOwnerWindowMetrics>,
}

#[derive(Clone)]
struct WindowMetrics {
    timeline_origin: Instant,
    window_id: Option<u64>,
    completed_jobs: u64,
    service_time: Duration,
    blocking_wait: Duration,
    completed_at: Option<Instant>,
    samples: VecDeque<GpuOwnerJobSample>,
}

#[derive(Clone, Default)]
struct Store {
    started_at: Option<Instant>,
    completed_at: Option<Instant>,
    completed_jobs: u64,
    service_time: Duration,
    blocking_wait: Duration,
    cpu_origin: Option<Duration>,
    cpu_time: Option<Duration>,
    windows: BTreeMap<u64, WindowMetrics>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(Mutex::default)
}

pub(crate) fn register(owner_id: u64) {
    let mut metrics = store().lock();
    let timeline_origin = metrics.started_at.unwrap_or_else(Instant::now);
    metrics.windows.insert(
        owner_id,
        WindowMetrics {
            timeline_origin,
            window_id: None,
            completed_jobs: 0,
            service_time: Duration::ZERO,
            blocking_wait: Duration::ZERO,
            completed_at: None,
            samples: VecDeque::with_capacity(HISTORY_CAPACITY),
        },
    );
}

pub(crate) fn unregister(owner_id: u64) {
    store().lock().windows.remove(&owner_id);
}

pub(crate) fn bind_window(owner_id: u64, window_id: u64) {
    if let Some(window) = store().lock().windows.get_mut(&owner_id) {
        window.window_id = Some(window_id);
    }
}

pub(crate) fn record_blocking_wait(duration: Duration) {
    BLOCKING_WAIT.with(|wait| {
        if let Some(previous) = wait.get() {
            wait.set(Some(previous.saturating_add(duration)));
        }
    });
}

pub(crate) fn blocking_wait() -> Duration {
    BLOCKING_WAIT.with(|wait| wait.get().unwrap_or_default())
}

/// The outer dispatch boundary includes unbound renderer creation and destruction work.
pub(crate) struct Dispatch {
    started_at: Instant,
}

impl Dispatch {
    pub(crate) fn start() -> Self {
        let started_at = Instant::now();
        {
            let mut metrics = store().lock();
            if metrics.started_at.is_none() {
                metrics.started_at = Some(started_at);
                metrics.cpu_origin = cpu::thread_time();
            }
        }
        BLOCKING_WAIT.with(|wait| wait.set(Some(Duration::ZERO)));
        Self { started_at }
    }
}

impl Drop for Dispatch {
    fn drop(&mut self) {
        let completed_at = Instant::now();
        let cpu_time = cpu::thread_time();
        let wait = BLOCKING_WAIT.with(|wait| wait.replace(None).unwrap_or_default());
        let mut metrics = store().lock();
        metrics.completed_jobs = metrics.completed_jobs.saturating_add(1);
        metrics.service_time += completed_at.saturating_duration_since(self.started_at);
        metrics.blocking_wait += wait;
        metrics.completed_at = Some(completed_at);
        metrics.cpu_time = cpu_time
            .zip(metrics.cpu_origin)
            .map(|(now, origin)| now.saturating_sub(origin));
    }
}

pub(crate) fn record_job(
    mut sample: GpuOwnerJobSample,
    started_at: Instant,
    completed_at: Instant,
) {
    if let Some(window) = store().lock().windows.get_mut(&sample.owner_id) {
        window.record(&mut sample, started_at, completed_at);
    }
}

impl WindowMetrics {
    fn record(
        &mut self,
        sample: &mut GpuOwnerJobSample,
        started_at: Instant,
        completed_at: Instant,
    ) {
        self.completed_jobs = self.completed_jobs.saturating_add(1);
        self.service_time += completed_at.saturating_duration_since(started_at);
        self.blocking_wait += Duration::from_micros(sample.owner_blocking_wait_us);
        self.completed_at = Some(completed_at);
        sample.window_id = self.window_id;
        sample.job_id = self.completed_jobs;
        sample.started_at_us = micros(started_at.saturating_duration_since(self.timeline_origin));
        sample.completed_at_us =
            micros(completed_at.saturating_duration_since(self.timeline_origin));
        sample.owner_job_duration_us = micros(completed_at.saturating_duration_since(started_at));
        if self.samples.len() == HISTORY_CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(sample.clone());
    }

    fn snapshot(&self, owner_id: u64, now: Instant) -> GpuOwnerWindowMetrics {
        GpuOwnerWindowMetrics {
            owner_id,
            window_id: self.window_id,
            completed_jobs: self.completed_jobs,
            owner_per_window_service_time_us: micros(self.service_time),
            owner_blocking_wait_time_us: micros(self.blocking_wait),
            queue_wait: distribution(
                self.samples
                    .iter()
                    .filter_map(|sample| sample.queue_wait_us),
            ),
            pending_age: distribution(
                self.samples
                    .iter()
                    .filter_map(|sample| sample.pending_age_us),
            ),
            owner_job_duration: distribution(
                self.samples
                    .iter()
                    .map(|sample| sample.owner_job_duration_us),
            ),
            sample_age_ms: self
                .completed_at
                .map(|time| millis(now.saturating_duration_since(time))),
            presentation_deadline_miss_count: None,
        }
    }
}

/// Returns lifetime owner totals and recent per-window distributions. Sorting happens after
/// releasing the observation lock. Histories contain at most 1,024 completed jobs per live
/// owner; global totals include initialization and destroyed windows. This is a diagnostic
/// read, not a frame clock or evidence of physical scanout. Unsupported CPU timing is None.
pub fn gpu_owner_metrics_snapshot() -> GpuOwnerMetricsSnapshot {
    let metrics = store().lock().clone();
    let now = Instant::now();
    let elapsed = metrics
        .started_at
        .map_or(Duration::ZERO, |time| now.saturating_duration_since(time));
    let ratio = |duration: Duration| {
        if elapsed.is_zero() {
            0.0
        } else {
            (duration.as_secs_f64() / elapsed.as_secs_f64()).clamp(0.0, 1.0)
        }
    };
    GpuOwnerMetricsSnapshot {
        elapsed_us: micros(elapsed),
        completed_jobs: metrics.completed_jobs,
        owner_service_time_us: micros(metrics.service_time),
        owner_busy_ratio: ratio(metrics.service_time),
        owner_cpu_busy_ratio: metrics.cpu_time.map(ratio),
        owner_blocking_wait_time_us: micros(metrics.blocking_wait),
        sample_age_ms: metrics
            .completed_at
            .map(|time| millis(now.saturating_duration_since(time))),
        windows: metrics
            .windows
            .iter()
            .map(|(id, window)| window.snapshot(*id, now))
            .collect(),
    }
}

/// Copies retained completed jobs with job_id greater than after_job_id for one owner proxy.
/// A gap can mean history eviction; use the first returned ID to detect it. Destroyed or
/// unknown owners return an empty vector. Querying neither wakes nor waits for the GPU owner.
pub fn gpu_owner_samples_since(owner_id: u64, after_job_id: u64) -> Vec<GpuOwnerJobSample> {
    store()
        .lock()
        .windows
        .get(&owner_id)
        .map_or_else(Vec::new, |window| {
            window
                .samples
                .iter()
                .filter(|sample| sample.job_id > after_job_id)
                .cloned()
                .collect()
        })
}

fn distribution(values: impl Iterator<Item = u64>) -> GpuOwnerDurationDistribution {
    let mut values: Vec<_> = values.collect();
    values.sort_unstable();
    if values.is_empty() {
        return GpuOwnerDurationDistribution::default();
    }
    let rank = |percent: usize| values[(values.len() * percent).div_ceil(100) - 1];
    GpuOwnerDurationDistribution {
        sample_count: values.len(),
        p50_us: rank(50),
        p95_us: rank(95),
        p99_us: rank(99),
        max_us: values[values.len() - 1],
    }
}

pub(crate) fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
