//! Process-wide text execution observations, recorded after releasing font locks.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::OnceLock,
    time::Duration,
};

const HISTORY_CAPACITY: usize = 256;

/// Native text implementation that performed the observed operation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextBackend {
    /// Cosmic Text shaping and Swash glyph generation under its existing state lock.
    Cosmic,
    /// Windows DirectWrite. Only instrumented operations appear in snapshots.
    DirectWrite,
}

/// Text work classified separately so bounds preparation cannot masquerade as rasterization.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextOperation {
    /// Platform line shaping, after higher-level layout cache/singleflight decisions.
    Shape,
    /// Glyph bounds preparation, which may itself populate a raster cache.
    RasterBounds,
    /// Glyph pixel generation or copying from the platform's raster cache.
    Rasterize,
}

/// Nearest-rank wall-time distribution over recent calls, in microseconds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextDurationDistribution {
    /// Number of retained observations; zero means no measurements.
    pub sample_count: usize,
    /// Median wall time.
    pub p50_us: u64,
    /// 95th-percentile wall time.
    pub p95_us: u64,
    /// 99th-percentile wall time.
    pub p99_us: u64,
    /// Largest retained wall time.
    pub max_us: u64,
}

/// Lifetime totals and the most recent 256 calls for one backend/operation.
///
/// Includes calls returning errors. Method time excludes the observed lock acquisition
/// but can include preemption, fallback/font loading and cache work; it is not CPU time.
/// These process-wide observations have no WindowId and are not text-layout cache hit rates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextOperationMetricsSnapshot {
    /// Implementation actually used; absent backend rows mean no observations, not zero cost.
    pub backend: TextBackend,
    /// Native operation measured.
    pub operation: TextOperation,
    /// Completed measured calls, including error results.
    pub calls: u64,
    /// Cumulative acquisition wall time for the instrumented state lock.
    pub lock_wait_total_us: u64,
    /// Cumulative operation wall time after acquisition and before releasing that lock.
    pub method_total_us: u64,
    /// Recent acquisition wall times, not all locks inside the implementation.
    pub lock_wait: TextDurationDistribution,
    /// Recent method wall times, not GPU execution or isolated shaping/raster CPU time.
    pub method: TextDurationDistribution,
}

#[derive(Clone, Default)]
struct Metrics {
    calls: u64,
    lock_wait_total_us: u64,
    method_total_us: u64,
    samples: VecDeque<(u64, u64)>,
}

impl Metrics {
    fn record(&mut self, wait: Duration, method: Duration) {
        let wait = micros(wait);
        let method = micros(method);
        self.calls = self.calls.saturating_add(1);
        self.lock_wait_total_us = self.lock_wait_total_us.saturating_add(wait);
        self.method_total_us = self.method_total_us.saturating_add(method);
        if self.samples.capacity() == 0 {
            self.samples.reserve(HISTORY_CAPACITY);
        }
        if self.samples.len() == HISTORY_CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back((wait, method));
    }

    fn snapshot(
        &self,
        backend: TextBackend,
        operation: TextOperation,
    ) -> TextOperationMetricsSnapshot {
        TextOperationMetricsSnapshot {
            backend,
            operation,
            calls: self.calls,
            lock_wait_total_us: self.lock_wait_total_us,
            method_total_us: self.method_total_us,
            lock_wait: distribution(self.samples.iter().map(|sample| sample.0)),
            method: distribution(self.samples.iter().map(|sample| sample.1)),
        }
    }
}

type Store = BTreeMap<(TextBackend, TextOperation), Metrics>;

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(Mutex::default)
}

pub(crate) fn record(
    backend: TextBackend,
    operation: TextOperation,
    wait: Duration,
    method: Duration,
) {
    store()
        .lock()
        .entry((backend, operation))
        .or_default()
        .record(wait, method);
}

/// Returns only observed backend/operation rows. Percentiles use at most 256 recent calls;
/// lifetime totals continue beyond history eviction. Sorting is outside the recorder lock.
/// Reading does not wake workers, shape text, access fonts or query graphics resources.
pub fn text_metrics_snapshot() -> Vec<TextOperationMetricsSnapshot> {
    let metrics = store().lock().clone();
    metrics
        .into_iter()
        .map(|((backend, operation), metrics)| metrics.snapshot(backend, operation))
        .collect()
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn distribution(values: impl Iterator<Item = u64>) -> TextDurationDistribution {
    let mut values: Vec<_> = values.collect();
    values.sort_unstable();
    if values.is_empty() {
        return TextDurationDistribution::default();
    }
    let rank = |percent: usize| values[(values.len() * percent).div_ceil(100) - 1];
    TextDurationDistribution {
        sample_count: values.len(),
        p50_us: rank(50),
        p95_us: rank(95),
        p99_us: rank(99),
        max_us: values[values.len() - 1],
    }
}

#[cfg(test)]
mod tests;
