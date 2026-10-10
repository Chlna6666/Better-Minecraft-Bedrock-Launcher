use super::*;
use crate::diagnostics::performance_metrics::store::WindowTimingSamples;

/// Per-window path-mask pixel-cache measurements for successfully submitted frames.
///
/// CPU timings measure the backend pass call, including any synchronous driver waits; they
/// are not GPU execution times. Skips do not contribute zero-duration timing samples.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPathMaskMetricsSnapshot {
    /// Submitted frames that rasterized a nonempty path mask.
    pub rendered_frames: u64,
    /// Submitted frames that reused resident mask pixels instead of rasterizing them.
    pub skipped_frames: u64,
    /// Last successful rasterization pass-call duration, in microseconds.
    pub pass_cpu_last_us: u64,
    /// Cumulative successful rasterization pass-call duration, in microseconds.
    pub pass_cpu_total_us: u64,
    /// Median CPU duration over the most recent 256 successful rasterizations.
    pub pass_cpu_p50_us: u64,
    /// 95th percentile CPU duration over the most recent 256 successful rasterizations.
    pub pass_cpu_p95_us: u64,
    /// 99th percentile CPU duration over the most recent 256 successful rasterizations.
    pub pass_cpu_p99_us: u64,
    /// Number of retained successful rasterization timing samples.
    pub pass_cpu_sample_count: usize,
}

#[derive(Clone, Default)]
pub(in crate::diagnostics::performance_metrics) struct Metrics {
    rendered: u64,
    skipped: u64,
    last_us: u64,
    total_us: u64,
    samples: WindowTimingSamples,
}

impl Metrics {
    fn record(&mut self, rendered: bool, elapsed: Duration) {
        if rendered {
            self.rendered = self.rendered.saturating_add(1);
            self.last_us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
            self.total_us = self.total_us.saturating_add(self.last_us);
            self.samples.record(elapsed);
        } else {
            self.skipped = self.skipped.saturating_add(1);
        }
    }

    pub(in crate::diagnostics::performance_metrics) fn snapshot(
        &self,
    ) -> WindowPathMaskMetricsSnapshot {
        let samples = self.samples.percentiles();
        WindowPathMaskMetricsSnapshot {
            rendered_frames: self.rendered,
            skipped_frames: self.skipped,
            pass_cpu_last_us: self.last_us,
            pass_cpu_total_us: self.total_us,
            pass_cpu_p50_us: samples.p50_micros,
            pass_cpu_p95_us: samples.p95_micros,
            pass_cpu_p99_us: samples.p99_micros,
            pass_cpu_sample_count: samples.count,
        }
    }
}

pub(crate) fn record_window_path_mask(window_id: u64, rendered: bool, elapsed: Duration) {
    if let Ok(mut windows) = shared_metrics().window_metrics.lock() {
        windows
            .entry(window_id)
            .or_default()
            .path_mask
            .record(rendered, elapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_frames_do_not_dilute_rasterization_cpu_percentiles() {
        let mut metrics = Metrics::default();
        metrics.record(true, Duration::from_micros(17));
        for _ in 0..1_000 {
            metrics.record(false, Duration::ZERO);
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.rendered_frames, 1);
        assert_eq!(snapshot.skipped_frames, 1_000);
        assert_eq!(snapshot.pass_cpu_p99_us, 17);
        assert_eq!(snapshot.pass_cpu_sample_count, 1);
    }
}
