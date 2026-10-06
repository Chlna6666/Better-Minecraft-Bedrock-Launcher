use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use super::super::store::{WindowMetrics, shared_metrics};

/// Native VSync cadence and active-presentation outcomes for one window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowVSyncMetricsSnapshot {
    /// VSync events delivered to this window, including events with no pending frame request.
    pub wake_count: usize,
    /// Delivered events without a valid DWM QPC refresh-period sample, including background cadence.
    pub fallback_wake_count: usize,
    /// Median recent interval between delivered VSync events, in microseconds.
    pub wake_interval_p50_micros: usize,
    /// 95th percentile recent interval between delivered VSync events, in microseconds.
    pub wake_interval_p95_micros: usize,
    /// 99th percentile recent interval between delivered VSync events, in microseconds.
    pub wake_interval_p99_micros: usize,
    /// Maximum retained interval between delivered VSync events, in microseconds.
    pub wake_interval_max_micros: usize,
    /// Number of recent delivered VSync intervals retained.
    pub wake_interval_sample_count: usize,
    /// Median DWM-reported QPC refresh period, in microseconds.
    pub refresh_period_p50_micros: usize,
    /// 95th percentile DWM-reported QPC refresh period, in microseconds.
    pub refresh_period_p95_micros: usize,
    /// 99th percentile DWM-reported QPC refresh period, in microseconds.
    pub refresh_period_p99_micros: usize,
    /// Maximum retained DWM-reported QPC refresh period, in microseconds.
    pub refresh_period_max_micros: usize,
    /// Number of valid DWM QPC refresh-period samples retained.
    pub refresh_period_sample_count: usize,
    /// Median DWM-reported composition period derived from `rateCompose`, in microseconds.
    pub composition_period_p50_micros: usize,
    /// 95th percentile DWM-reported composition period derived from `rateCompose`, in microseconds.
    pub composition_period_p95_micros: usize,
    /// 99th percentile DWM-reported composition period derived from `rateCompose`, in microseconds.
    pub composition_period_p99_micros: usize,
    /// Maximum retained DWM-reported composition period, in microseconds.
    pub composition_period_max_micros: usize,
    /// Number of valid DWM composition-period samples retained.
    pub composition_period_sample_count: usize,
    /// Active presentation attempts made by the Windows native frame callback.
    pub active_presentation_attempt_count: usize,
    /// Active attempts rejected by the nonblocking backend readiness preflight.
    pub active_presentation_preflight_not_ready_count: usize,
    /// Active attempts that requested another native presentation frame after no submission.
    pub active_presentation_retry_count: usize,
}

pub(super) fn snapshot(metrics: &WindowMetrics) -> WindowVSyncMetricsSnapshot {
    let wake_interval = metrics.native_vsync_interval_samples.percentiles();
    let refresh_period = metrics.dwm_refresh_period_samples.percentiles();
    let composition_period = metrics.dwm_composition_period_samples.percentiles();

    WindowVSyncMetricsSnapshot {
        wake_count: metrics.native_vsync_wake_count as usize,
        fallback_wake_count: metrics.native_vsync_fallback_wake_count as usize,
        wake_interval_p50_micros: wake_interval.p50_micros as usize,
        wake_interval_p95_micros: wake_interval.p95_micros as usize,
        wake_interval_p99_micros: wake_interval.p99_micros as usize,
        wake_interval_max_micros: metrics.native_vsync_interval_samples.max_micros() as usize,
        wake_interval_sample_count: wake_interval.count,
        refresh_period_p50_micros: refresh_period.p50_micros as usize,
        refresh_period_p95_micros: refresh_period.p95_micros as usize,
        refresh_period_p99_micros: refresh_period.p99_micros as usize,
        refresh_period_max_micros: metrics.dwm_refresh_period_samples.max_micros() as usize,
        refresh_period_sample_count: refresh_period.count,
        composition_period_p50_micros: composition_period.p50_micros as usize,
        composition_period_p95_micros: composition_period.p95_micros as usize,
        composition_period_p99_micros: composition_period.p99_micros as usize,
        composition_period_max_micros: metrics.dwm_composition_period_samples.max_micros() as usize,
        composition_period_sample_count: composition_period.count,
        active_presentation_attempt_count: metrics.active_presentation_attempt_count as usize,
        active_presentation_preflight_not_ready_count: metrics
            .active_presentation_preflight_not_ready_count
            as usize,
        active_presentation_retry_count: metrics.active_presentation_retry_count as usize,
    }
}

pub(crate) fn record_window_native_vsync_wake(
    window_id: u64,
    reported_refresh_period: Option<Duration>,
    reported_composition_period: Option<Duration>,
    event_received_at: Instant,
) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.native_vsync_wake_count = metrics.native_vsync_wake_count.saturating_add(1);
        if let Some(previous_wake_at) = metrics.last_native_vsync_wake_at.replace(event_received_at)
        {
            metrics
                .native_vsync_interval_samples
                .record(event_received_at.saturating_duration_since(previous_wake_at));
        }
        if let Some(period) = reported_refresh_period {
            metrics.dwm_refresh_period_samples.record(period);
        } else {
            metrics.native_vsync_fallback_wake_count =
                metrics.native_vsync_fallback_wake_count.saturating_add(1);
        }
        if let Some(period) = reported_composition_period {
            metrics.dwm_composition_period_samples.record(period);
        }
    }
}

pub(crate) fn record_window_active_presentation_attempt(window_id: u64) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.active_presentation_attempt_count =
            metrics.active_presentation_attempt_count.saturating_add(1);
    }
}

/// Records an active frame rejected by `can_present_without_wait`, not every deferred draw cause.
pub(crate) fn record_window_active_presentation_preflight_not_ready(window_id: u64) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.active_presentation_preflight_not_ready_count = metrics
            .active_presentation_preflight_not_ready_count
            .saturating_add(1);
    }
}

pub(crate) fn record_window_active_presentation_retry(window_id: u64) {
    if let Ok(mut window_metrics) = shared_metrics().window_metrics.lock() {
        let metrics = window_metrics.entry(window_id).or_default();
        metrics.active_presentation_retry_count =
            metrics.active_presentation_retry_count.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        record_window_active_presentation_attempt,
        record_window_active_presentation_preflight_not_ready,
        record_window_active_presentation_retry, record_window_native_vsync_wake,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(u64::MAX);

    #[test]
    fn records_native_vsync_periods_and_active_presentation_outcomes() {
        let window_id = NEXT_WINDOW_ID.fetch_sub(1, Ordering::Relaxed);
        let first_wake_at = Instant::now();
        record_window_native_vsync_wake(
            window_id,
            Some(Duration::from_micros(4_167)),
            Some(Duration::from_micros(8_333)),
            first_wake_at,
        );
        record_window_native_vsync_wake(
            window_id,
            None,
            None,
            first_wake_at + Duration::from_micros(8_333),
        );
        record_window_active_presentation_attempt(window_id);
        record_window_active_presentation_preflight_not_ready(window_id);
        record_window_active_presentation_retry(window_id);

        let metrics = super::super::window_metrics_snapshot()
            .into_iter()
            .find(|metrics| metrics.window_id == window_id)
            .expect("the recorded window is present in the snapshot");
        assert_eq!(metrics.native_vsync.wake_count, 2);
        assert_eq!(metrics.native_vsync.fallback_wake_count, 1);
        assert_eq!(metrics.native_vsync.wake_interval_sample_count, 1);
        assert_eq!(metrics.native_vsync.wake_interval_p50_micros, 8_333);
        assert_eq!(metrics.native_vsync.refresh_period_p50_micros, 4_167);
        assert_eq!(metrics.native_vsync.composition_period_p50_micros, 8_333);
        assert_eq!(metrics.native_vsync.active_presentation_attempt_count, 1);
        assert_eq!(
            metrics
                .native_vsync
                .active_presentation_preflight_not_ready_count,
            1
        );
        assert_eq!(metrics.native_vsync.active_presentation_retry_count, 1);
    }
}
