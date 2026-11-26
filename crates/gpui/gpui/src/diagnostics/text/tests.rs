use super::*;

#[test]
fn wait_and_method_distributions_are_independent() {
    let mut metrics = Metrics::default();
    for index in 1..=100 {
        metrics.record(
            Duration::from_micros(index),
            Duration::from_micros(101 - index),
        );
    }
    let snapshot = metrics.snapshot(TextBackend::Cosmic, TextOperation::Shape);
    assert_eq!(snapshot.calls, 100);
    assert_eq!(snapshot.lock_wait_total_us, 5_050);
    assert_eq!(snapshot.method_total_us, 5_050);
    assert_eq!(snapshot.lock_wait.p50_us, 50);
    assert_eq!(snapshot.lock_wait.p95_us, 95);
    assert_eq!(snapshot.method.p99_us, 99);
}

#[test]
fn eviction_bounds_history_without_resetting_totals() {
    let mut metrics = Metrics::default();
    for index in 0..HISTORY_CAPACITY + 4 {
        metrics.record(
            Duration::from_micros(index as u64),
            Duration::from_micros(1),
        );
    }
    let snapshot = metrics.snapshot(TextBackend::Cosmic, TextOperation::Rasterize);
    assert_eq!(snapshot.calls, 260);
    assert_eq!(snapshot.lock_wait.sample_count, HISTORY_CAPACITY);
    assert_eq!(snapshot.method_total_us, 260);
    assert_eq!(metrics.samples.front().unwrap().0, 4);
    assert_eq!(snapshot.lock_wait.max_us, 259);
}

#[test]
fn empty_history_is_not_a_measured_zero() {
    let snapshot = Metrics::default().snapshot(TextBackend::Cosmic, TextOperation::RasterBounds);
    assert_eq!(snapshot.calls, 0);
    assert_eq!(snapshot.method.sample_count, 0);
    assert_eq!(snapshot.lock_wait.sample_count, 0);
}
