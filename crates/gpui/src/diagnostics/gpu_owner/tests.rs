use super::*;

fn window(now: Instant, window_id: Option<u64>) -> WindowMetrics {
    WindowMetrics {
        timeline_origin: now,
        window_id,
        completed_jobs: 0,
        service_time: Duration::ZERO,
        blocking_wait: Duration::ZERO,
        completed_at: None,
        samples: VecDeque::new(),
    }
}

fn sample(owner_id: u64, wait: Option<u64>, outcome: GpuOwnerJobOutcome) -> GpuOwnerJobSample {
    GpuOwnerJobSample {
        owner_id,
        window_id: None,
        job_id: 0,
        kind: GpuOwnerJobKind::Draw,
        outcome,
        started_at_us: 0,
        completed_at_us: 0,
        queue_wait_us: wait,
        pending_age_us: wait,
        coalesced_count: 0,
        owner_job_duration_us: 100,
        owner_blocking_wait_us: 25,
        schedule_lateness_us: None,
    }
}

#[test]
fn distributions_use_nearest_rank_and_exclude_unqueued_work() {
    let now = Instant::now();
    let mut metrics = window(now, Some(90));
    for wait in (1..=100).map(Some).chain([None]) {
        metrics.record(
            &mut sample(1, wait, GpuOwnerJobOutcome::Deferred),
            now,
            now + Duration::from_micros(100),
        );
    }
    let snapshot = metrics.snapshot(1, now + Duration::from_millis(1));
    assert_eq!(
        snapshot.queue_wait,
        GpuOwnerDurationDistribution {
            sample_count: 100,
            p50_us: 50,
            p95_us: 95,
            p99_us: 99,
            max_us: 100,
        }
    );
    assert_eq!(snapshot.owner_job_duration.sample_count, 101);
    assert_eq!(snapshot.owner_per_window_service_time_us, 10_100);
    assert_eq!(snapshot.owner_blocking_wait_time_us, 2_525);
    assert_eq!(snapshot.presentation_deadline_miss_count, None);
}

#[test]
fn history_eviction_keeps_lifetime_service_and_execution_ids() {
    let now = Instant::now();
    let mut metrics = window(now, None);
    for index in 0..HISTORY_CAPACITY + 7 {
        let started = now + Duration::from_micros(index as u64 * 100);
        metrics.record(
            &mut sample(1, Some(index as u64), GpuOwnerJobOutcome::Failed),
            started,
            started + Duration::from_micros(100),
        );
    }
    assert_eq!(metrics.samples.len(), HISTORY_CAPACITY);
    assert_eq!(metrics.samples.front().unwrap().job_id, 8);
    assert_eq!(
        metrics.samples.back().unwrap().job_id,
        (HISTORY_CAPACITY + 7) as u64
    );
    assert_eq!(metrics.completed_jobs, (HISTORY_CAPACITY + 7) as u64);
    assert_eq!(
        micros(metrics.service_time),
        (HISTORY_CAPACITY + 7) as u64 * 100
    );
}

#[test]
fn window_ids_do_not_alias_owner_ids_or_other_windows() {
    let now = Instant::now();
    let mut windows = BTreeMap::from([(1, window(now, Some(800))), (2, window(now, Some(900)))]);
    windows.get_mut(&1).unwrap().record(
        &mut sample(1, Some(10), GpuOwnerJobOutcome::Submitted),
        now,
        now + Duration::from_micros(100),
    );
    windows.get_mut(&2).unwrap().record(
        &mut sample(2, Some(30), GpuOwnerJobOutcome::Deferred),
        now,
        now + Duration::from_micros(300),
    );
    assert_eq!(windows[&1].samples[0].window_id, Some(800));
    assert_eq!(windows[&2].samples[0].window_id, Some(900));
    assert_eq!(
        windows[&1].samples[0].outcome,
        GpuOwnerJobOutcome::Submitted
    );
    assert_eq!(windows[&2].samples[0].outcome, GpuOwnerJobOutcome::Deferred);
    assert_eq!(windows[&1].snapshot(1, now).queue_wait.p99_us, 10);
    assert_eq!(windows[&2].snapshot(2, now).queue_wait.p99_us, 30);
}

#[test]
fn submission_waits_only_accumulate_inside_the_owner_dispatch() {
    assert_eq!(BLOCKING_WAIT.with(Cell::get), None);
    record_blocking_wait(Duration::from_millis(9));
    assert_eq!(BLOCKING_WAIT.with(Cell::get), None);
    BLOCKING_WAIT.with(|wait| wait.set(Some(Duration::ZERO)));
    record_blocking_wait(Duration::from_micros(30));
    record_blocking_wait(Duration::from_micros(70));
    assert_eq!(blocking_wait(), Duration::from_micros(100));
    BLOCKING_WAIT.with(|wait| wait.set(None));
}

#[test]
fn empty_window_reports_no_samples_or_display_deadline() {
    let now = Instant::now();
    let snapshot = window(now, None).snapshot(1, now);
    assert_eq!(snapshot.sample_age_ms, None);
    assert_eq!(snapshot.queue_wait.sample_count, 0);
    assert_eq!(snapshot.owner_job_duration.sample_count, 0);
    assert_eq!(snapshot.presentation_deadline_miss_count, None);
}

#[test]
fn destruction_removes_window_history_and_cursor_queries() {
    let id = u64::MAX;
    let now = Instant::now();
    register(id);
    bind_window(id, 5_000);
    record_job(
        sample(id, Some(10), GpuOwnerJobOutcome::Submitted),
        now,
        now + Duration::from_micros(100),
    );
    assert_eq!(gpu_owner_samples_since(id, 0).len(), 1);
    assert!(gpu_owner_samples_since(id, 1).is_empty());
    unregister(id);
    assert!(gpu_owner_samples_since(id, 0).is_empty());
    assert!(
        !gpu_owner_metrics_snapshot()
            .windows
            .iter()
            .any(|window| window.owner_id == id)
    );
}

#[test]
fn windows_share_the_dispatch_timeline_origin() {
    let first_id = u64::MAX - 1;
    let second_id = u64::MAX - 2;
    let _dispatch = Dispatch::start();
    register(first_id);
    register(second_id);
    {
        let metrics = store().lock();
        assert_eq!(
            Some(metrics.windows[&first_id].timeline_origin),
            metrics.started_at
        );
        assert_eq!(
            metrics.windows[&first_id].timeline_origin,
            metrics.windows[&second_id].timeline_origin
        );
    }
    unregister(first_id);
    unregister(second_id);
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
#[test]
fn supported_thread_cpu_clock_is_monotonic() {
    let first = cpu::thread_time().expect("thread CPU clock");
    let second = cpu::thread_time().expect("thread CPU clock");
    assert!(second >= first);
}
