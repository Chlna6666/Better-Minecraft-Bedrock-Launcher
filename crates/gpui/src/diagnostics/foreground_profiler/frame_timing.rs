use super::{FramePipelineSample, with_state};
use std::time::{Duration, Instant};

/// Minimum work budget used to classify a measured frame as late (120Hz).
pub const FRAME_BUDGET_FLOOR: Duration = Duration::from_nanos(8_333_333);
/// Work budget when the display refresh interval is unavailable (60Hz).
pub const UNKNOWN_REFRESH_FRAME_BUDGET: Duration = Duration::from_nanos(16_666_667);

impl FramePipelineSample {
    /// One display refresh, bounded below by the 120Hz measurement floor.
    pub fn frame_budget(&self) -> Duration {
        self.refresh_interval
            .filter(|interval| !interval.is_zero())
            .map_or(UNKNOWN_REFRESH_FRAME_BUDGET, |interval| {
                interval.max(FRAME_BUDGET_FLOOR)
            })
    }

    /// Whether measured work exceeded the frame budget; false when work is unknown.
    pub fn is_late(&self) -> bool {
        self.work.is_some_and(|work| work > self.frame_budget())
    }

    /// Missed refreshes against the real display interval, independent of the budget floor.
    pub fn missed_refreshes(&self) -> Option<u64> {
        let interval = self
            .refresh_interval
            .filter(|interval| !interval.is_zero())?;
        let refreshes = self.work?.as_nanos().div_ceil(interval.as_nanos()).max(1);
        Some(u64::try_from(refreshes - 1).unwrap_or(u64::MAX))
    }
}

pub(crate) fn record_frame_signal(window_id: u64, at: Instant, refresh_interval: Option<Duration>) {
    with_state(|state| {
        let window = state.windows.entry(window_id).or_default();
        window.signal_at = Some(at);
        window.refresh_interval = refresh_interval.filter(|interval| !interval.is_zero());
        if let Some(pending) = window.pending_frame.as_mut() {
            pending.signal_at = window.signal_at;
            pending.refresh_interval = window.refresh_interval;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::super::{
        ForegroundIntervalBoundary, ForegroundIntervalSnapshot, ForegroundWorkKind,
        ForegroundWorkSample, HangIncident, HangTrigger,
    };
    use super::*;

    fn frame(start: Instant, interval: Option<Duration>, work: Duration) -> FramePipelineSample {
        FramePipelineSample {
            window_id: 1,
            presented_at: start + work + Duration::from_secs(1),
            submitted_at: Some(start + work),
            refresh_interval: interval,
            work: Some(work),
            dirty_to_request: None,
            request_to_draw: None,
            draw: None,
            draw_to_submit: None,
            submit: None,
            submit_to_present: None,
            dirty_to_present: None,
            request_to_present: None,
        }
    }

    #[test]
    fn frame_budgets_follow_refresh_rate_with_a_measurement_floor() {
        let start = Instant::now();
        for hz in [30., 60., 120., 240., 360.] {
            let interval = Duration::from_secs_f64(1. / hz);
            let sample = frame(start, Some(interval), Duration::from_millis(6));
            assert_eq!(sample.frame_budget(), interval.max(FRAME_BUDGET_FLOOR));
            assert!(!sample.is_late());
            assert_eq!(
                sample.missed_refreshes(),
                Some(if hz > 120. {
                    1 + u64::from(hz > 240.)
                } else {
                    0
                })
            );
        }
        assert_eq!(
            frame(start, None, Duration::ZERO).frame_budget(),
            UNKNOWN_REFRESH_FRAME_BUDGET
        );
        assert_eq!(
            frame(start, Some(Duration::ZERO), Duration::ZERO).missed_refreshes(),
            None
        );
    }

    #[test]
    fn slow_submission_is_excluded_from_late_frame_detection() {
        let start = Instant::now();
        let sample = frame(
            start,
            Some(Duration::from_millis(16)),
            Duration::from_millis(5),
        );
        let snapshot = ForegroundIntervalSnapshot {
            started_at: start,
            boundary: ForegroundIntervalBoundary::Presented(sample),
            events: Vec::new(),
            small_poll_count: 0,
            small_poll_total: Duration::ZERO,
            dropped_events: 0,
        };
        assert!(
            HangIncident::detect(snapshot, Duration::from_secs(2), Duration::from_millis(16))
                .is_none()
        );
    }

    #[test]
    fn late_frame_contributors_only_include_work_before_submission() {
        let start = Instant::now();
        let sample = frame(
            start,
            Some(Duration::from_millis(10)),
            Duration::from_millis(12),
        );
        let event = |at, duration| ForegroundWorkSample {
            kind: ForegroundWorkKind::Input {
                kind: "test",
                window_id: 1,
            },
            started_at: start + Duration::from_millis(at),
            ended_at: start + Duration::from_millis(at + duration),
        };
        let snapshot = ForegroundIntervalSnapshot {
            started_at: start,
            boundary: ForegroundIntervalBoundary::Presented(sample),
            events: vec![event(1, 4), event(13, 20)],
            small_poll_count: 0,
            small_poll_total: Duration::ZERO,
            dropped_events: 0,
        };
        let incident =
            HangIncident::detect(snapshot, Duration::from_secs(2), Duration::from_millis(16))
                .unwrap();
        assert_eq!(incident.trigger, HangTrigger::LateFrame);
        assert_eq!(incident.contributors.len(), 1);
    }
}
