use serde::{Deserialize, Serialize};
use std::time::Instant;

use super::super::store::{WindowMetrics, shared_metrics};

/// Backend readiness wake counts and event-queue delay for one window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowBackendReadyMetricsSnapshot {
    /// Current backend-ready notifications accepted by the Windows native owner.
    pub wake_count: usize,
    /// Median time from the backend callback enqueue to native event delivery, in microseconds.
    pub queue_delay_p50_micros: usize,
    /// 95th percentile backend-ready callback queue delay, in microseconds.
    pub queue_delay_p95_micros: usize,
    /// Maximum retained backend-ready callback queue delay, in microseconds.
    pub queue_delay_max_micros: usize,
    /// Number of recent backend-ready callback queue-delay samples retained.
    pub queue_delay_sample_count: usize,
}

pub(super) fn snapshot(metrics: &WindowMetrics) -> WindowBackendReadyMetricsSnapshot {
    let queue_delay = metrics.backend_ready_queue_delay_samples.percentiles();
    WindowBackendReadyMetricsSnapshot {
        wake_count: metrics.backend_ready_wake_count as usize,
        queue_delay_p50_micros: queue_delay.p50_micros as usize,
        queue_delay_p95_micros: queue_delay.p95_micros as usize,
        queue_delay_max_micros: metrics.backend_ready_queue_delay_samples.max_micros() as usize,
        queue_delay_sample_count: queue_delay.count,
    }
}

pub(crate) fn record_window_backend_ready_wake(
    window_id: u64,
    enqueued_at: Instant,
    event_received_at: Instant,
) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.backend_ready_wake_count = metrics.backend_ready_wake_count.saturating_add(1);
        metrics
            .backend_ready_queue_delay_samples
            .record(event_received_at.saturating_duration_since(enqueued_at));
    }
}

#[cfg(test)]
mod tests {
    use super::record_window_backend_ready_wake;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(u64::MAX / 2);

    #[test]
    fn records_backend_ready_wakes_separately_from_dwm_wakes() {
        let window_id = NEXT_WINDOW_ID.fetch_sub(1, Ordering::Relaxed);
        let enqueued_at = Instant::now();
        record_window_backend_ready_wake(
            window_id,
            enqueued_at,
            enqueued_at + Duration::from_micros(137),
        );

        let metrics = super::super::window_metrics_snapshot()
            .into_iter()
            .find(|metrics| metrics.window_id == window_id)
            .expect("the recorded window is present in the snapshot");
        assert_eq!(metrics.backend_ready.wake_count, 1);
        assert_eq!(metrics.backend_ready.queue_delay_p50_micros, 137);
        assert_eq!(metrics.backend_ready.queue_delay_sample_count, 1);
        assert_eq!(metrics.native_vsync.wake_count, 0);
    }
}
