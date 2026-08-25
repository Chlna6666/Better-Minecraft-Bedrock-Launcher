use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::store::shared_metrics;
use super::timing::duration_micros;

/// Aggregate diagnostics for bounded animated-image streams.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct AnimationMetricsSnapshot {
    /// Number of times a streaming animation reopened its frame iterator after a loop.
    pub loop_restarts: usize,
    /// Number of loop restarts that failed.
    pub loop_restart_failures: usize,
    /// Aggregate time spent reopening frame iterators.
    pub loop_restart_total_time: Option<Duration>,
    /// Longest observed frame-iterator restart.
    pub loop_restart_max_time: Option<Duration>,
    /// Number of worker advances delayed by bounded queue capacity.
    pub queue_backpressure_count: usize,
    /// Number of queued frames discarded because playback had already advanced past them.
    pub stale_frame_count: usize,
    /// Number of global worker-pool wakeups caused by releasing queued bytes.
    pub worker_pool_wake_count: usize,
}

pub(super) fn animation_metrics_snapshot() -> AnimationMetricsSnapshot {
    let metrics = shared_metrics();
    let loop_restart_total_micros = metrics
        .animation_loop_restart_total_micros
        .load(Ordering::Relaxed);
    let loop_restart_max_micros = metrics
        .animation_loop_restart_max_micros
        .load(Ordering::Relaxed);
    AnimationMetricsSnapshot {
        loop_restarts: metrics.animation_loop_restart_count.load(Ordering::Relaxed) as usize,
        loop_restart_failures: metrics
            .animation_loop_restart_failure_count
            .load(Ordering::Relaxed) as usize,
        loop_restart_total_time: (loop_restart_total_micros > 0)
            .then(|| Duration::from_micros(loop_restart_total_micros)),
        loop_restart_max_time: (loop_restart_max_micros > 0)
            .then(|| Duration::from_micros(loop_restart_max_micros)),
        queue_backpressure_count: metrics
            .animation_queue_backpressure_count
            .load(Ordering::Relaxed) as usize,
        stale_frame_count: metrics.animation_stale_frame_count.load(Ordering::Relaxed) as usize,
        worker_pool_wake_count: metrics
            .animation_worker_pool_wake_count
            .load(Ordering::Relaxed) as usize,
    }
}

pub(crate) fn record_animation_loop_restart(duration: Duration, succeeded: bool) {
    let metrics = shared_metrics();
    let micros = duration_micros(duration).max(1);
    metrics
        .animation_loop_restart_count
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .animation_loop_restart_total_micros
        .fetch_add(micros, Ordering::Relaxed);
    metrics
        .animation_loop_restart_max_micros
        .fetch_max(micros, Ordering::Relaxed);
    if !succeeded {
        metrics
            .animation_loop_restart_failure_count
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn record_animation_queue_backpressure() {
    shared_metrics()
        .animation_queue_backpressure_count
        .fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_animation_stale_frame_count(count: usize) {
    if count != 0 {
        shared_metrics()
            .animation_stale_frame_count
            .fetch_add(count as u64, Ordering::Relaxed);
    }
}

pub(crate) fn record_animation_worker_pool_wake() {
    shared_metrics()
        .animation_worker_pool_wake_count
        .fetch_add(1, Ordering::Relaxed);
}
