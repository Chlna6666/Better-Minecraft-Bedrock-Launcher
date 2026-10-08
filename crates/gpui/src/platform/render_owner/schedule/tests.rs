use super::*;

#[test]
fn idle_and_hidden_windows_have_no_deadline() {
    let now = Instant::now();
    let mut schedule = Schedule::default();
    schedule.set_interval(Some(Duration::from_millis(10)));
    schedule.after_frame(now, false, false, None, false);
    assert_eq!(schedule.deadline(), None);
    schedule.after_frame(now, true, false, None, false);
    assert_eq!(schedule.deadline(), Some(now + Duration::from_millis(10)));
    schedule.set_interval(None);
    schedule.after_frame(now, true, false, None, false);
    assert_eq!(schedule.deadline(), None);
}

#[test]
fn readiness_suspends_deadlines_without_disabling_cadence() {
    let now = Instant::now();
    let mut schedule = Schedule::default();
    schedule.set_interval(Some(Duration::from_millis(10)));
    schedule.after_frame(now, true, true, None, false);
    assert!(schedule.is_enabled());
    assert_eq!(schedule.deadline(), None);
    schedule.after_frame(now, true, false, None, false);
    assert_eq!(schedule.deadline(), Some(now + Duration::from_millis(10)));
}

#[test]
fn explicit_frame_limit_and_missed_deadlines_do_not_queue_old_samples() {
    let now = Instant::now();
    let mut schedule = Schedule::default();
    schedule.set_interval(Some(Duration::from_millis(10)));
    let limited = now + Duration::from_millis(100);
    schedule.after_frame(now, true, false, Some(limited), false);
    assert_eq!(schedule.deadline(), Some(limited));
    let resumed = now + Duration::from_millis(200);
    schedule.after_frame(resumed, true, false, Some(limited), false);
    assert_eq!(
        schedule.deadline(),
        Some(resumed + Duration::from_millis(10))
    );
}

#[test]
fn zero_interval_cannot_start_a_busy_loop() {
    let mut schedule = Schedule::default();
    schedule.set_interval(Some(Duration::ZERO));
    schedule.after_frame(Instant::now(), true, false, None, false);
    assert!(!schedule.is_enabled());
    assert_eq!(schedule.deadline(), None);
}

#[test]
fn submission_cost_does_not_accumulate_into_display_cadence() {
    let now = Instant::now();
    let interval = Duration::from_millis(10);
    let mut schedule = Schedule::default();
    schedule.set_interval(Some(interval));
    schedule.after_frame(now, true, false, None, false);
    schedule.after_frame(
        now + interval + Duration::from_millis(3),
        true,
        false,
        None,
        false,
    );
    assert_eq!(schedule.deadline(), Some(now + interval * 2));
}

#[test]
fn native_callbacks_wait_after_submission_and_respect_explicit_limits() {
    let now = Instant::now();
    let mut schedule = Schedule::default();
    schedule.set_native_callbacks();
    schedule.set_native_visible(true);
    assert!(schedule.is_enabled());
    schedule.after_frame(now, true, false, None, true);
    assert_eq!(schedule.deadline(), None);
    let limited = now + Duration::from_millis(20);
    schedule.after_frame(now, true, false, Some(limited), false);
    assert_eq!(schedule.deadline(), Some(limited));
    schedule.after_frame(limited, true, false, Some(limited), true);
    assert_eq!(schedule.deadline(), None);
    schedule.after_frame(now, true, true, Some(limited), false);
    assert_eq!(schedule.deadline(), None);
    schedule.after_frame(now, false, false, Some(limited), false);
    assert_eq!(schedule.deadline(), None);
}

#[test]
fn deferred_native_acquire_backs_off_and_stops_on_hide_or_completion() {
    let now = Instant::now();
    let mut schedule = Schedule::default();
    schedule.set_native_callbacks();
    schedule.set_native_visible(true);
    for delay in [1, 2, 4, 8, 16, 16] {
        schedule.after_frame(now, true, false, None, false);
        assert_eq!(
            schedule.deadline(),
            Some(now + Duration::from_millis(delay))
        );
    }
    schedule.after_frame(now, true, false, None, true);
    assert_eq!(schedule.deadline(), None);
    schedule.after_frame(now, true, false, None, false);
    assert_eq!(schedule.deadline(), Some(now + Duration::from_millis(1)));
    schedule.set_native_visible(false);
    schedule.after_frame(now, true, false, None, false);
    assert!(!schedule.is_enabled());
    assert_eq!(schedule.deadline(), None);
    schedule.set_native_visible(true);
    schedule.after_frame(now, false, false, None, false);
    assert_eq!(schedule.deadline(), None);
}
