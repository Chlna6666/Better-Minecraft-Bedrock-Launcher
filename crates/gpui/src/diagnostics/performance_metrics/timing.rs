use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub(super) fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

pub(super) fn record_once_micros(metric: &AtomicU64, duration: Duration) {
    let micros = duration_micros(duration);
    let value = micros.max(1);
    if metric
        .compare_exchange(0, value, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        // Another first-frame writer won the race; preserving the first sample is intentional.
    }
}
