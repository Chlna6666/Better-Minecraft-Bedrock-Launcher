use std::time::{Duration, Instant};

use super::TARGET_FRAME_GENERATION_BUDGET;

const HIGH_REFRESH_FRAME_BUDGET_HEADROOM: f32 = 0.85;
const HIGH_REFRESH_FRAME_INTERVAL: Duration = Duration::from_millis(8);
const MIN_DYNAMIC_FRAME_BUDGET: Duration = Duration::from_millis(2);
const FRAME_GENERATION_BUDGET_WARN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::window) struct WindowFrameThrottle {
    retry_after: Option<Instant>,
    retry_generation: u64,
    armed_retry_generation: Option<u64>,
    last_frame_started_at: Option<Instant>,
    estimated_frame_interval: Option<Duration>,
    last_generation_budget_warning_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FrameActivity {
    pub(super) dirty: bool,
    pub(super) pending_present: bool,
    pub(super) active: bool,
    pub(super) minimized: bool,
}

impl WindowFrameThrottle {
    pub(in crate::window) fn should_delay(self, now: Instant) -> bool {
        self.retry_after
            .is_some_and(|retry_after| now < retry_after)
    }

    pub(super) fn delay(&mut self, now: Instant, duration: Duration) {
        let retry_after = now + duration;
        if self.retry_after.is_none_or(|current| current < retry_after) {
            self.retry_after = Some(retry_after);
            self.retry_generation = self.retry_generation.saturating_add(1);
            self.armed_retry_generation = None;
        }
    }

    fn clear_if_ready(&mut self, now: Instant) {
        if self
            .retry_after
            .is_some_and(|retry_after| now >= retry_after)
        {
            self.retry_after = None;
            self.armed_retry_generation = None;
        }
    }

    pub(super) fn arm_retry_timer(&mut self) -> Option<(Instant, u64)> {
        let retry_after = self.retry_after?;
        if self.armed_retry_generation == Some(self.retry_generation) {
            return None;
        }
        self.armed_retry_generation = Some(self.retry_generation);
        Some((retry_after, self.retry_generation))
    }

    pub(super) fn retry_timer_fired(&mut self, generation: u64, now: Instant) -> bool {
        if self.retry_generation != generation {
            return false;
        }
        self.armed_retry_generation = None;
        self.clear_if_ready(now);
        !self.should_delay(now)
    }

    pub(in crate::window) fn clear_delay(&mut self) {
        self.retry_after = None;
        self.armed_retry_generation = None;
    }

    pub(super) fn record_frame_start(&mut self, now: Instant) {
        if let Some(previous) = self.last_frame_started_at {
            let interval = now.saturating_duration_since(previous);
            if (Duration::from_millis(1)..=Duration::from_millis(100)).contains(&interval) {
                self.estimated_frame_interval = Some(match self.estimated_frame_interval {
                    Some(current) => average_duration(current, interval),
                    None => interval,
                });
            }
        }
        self.last_frame_started_at = Some(now);
    }

    pub(super) fn frame_budget(self) -> Duration {
        let budget = self
            .estimated_frame_interval
            .filter(|interval| *interval < HIGH_REFRESH_FRAME_INTERVAL)
            .map(|interval| interval.mul_f32(HIGH_REFRESH_FRAME_BUDGET_HEADROOM))
            .unwrap_or(TARGET_FRAME_GENERATION_BUDGET);
        budget.clamp(MIN_DYNAMIC_FRAME_BUDGET, TARGET_FRAME_GENERATION_BUDGET)
    }

    pub(super) fn retry_delay(self) -> Duration {
        let frame_interval = self
            .estimated_frame_interval
            .unwrap_or(HIGH_REFRESH_FRAME_INTERVAL);
        frame_interval.clamp(TARGET_FRAME_GENERATION_BUDGET, HIGH_REFRESH_FRAME_INTERVAL)
    }

    pub(super) fn should_warn_generation_budget_miss(&mut self, now: Instant) -> bool {
        let should_warn = self.last_generation_budget_warning_at.is_none_or(|last| {
            now.saturating_duration_since(last) >= FRAME_GENERATION_BUDGET_WARN_INTERVAL
        });
        if should_warn {
            self.last_generation_budget_warning_at = Some(now);
        }
        should_warn
    }
}

fn average_duration(current: Duration, sample: Duration) -> Duration {
    let current_micros = current.as_micros();
    let sample_micros = sample.as_micros();
    let average_micros = (current_micros.saturating_mul(3) + sample_micros) / 4;
    Duration::from_micros(average_micros.min(u128::from(u64::MAX)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_budget_warnings_are_rate_limited() {
        let mut throttle = WindowFrameThrottle::default();
        let now = Instant::now();

        assert!(throttle.should_warn_generation_budget_miss(now));
        assert!(
            !throttle.should_warn_generation_budget_miss(
                now + FRAME_GENERATION_BUDGET_WARN_INTERVAL / 2
            )
        );
        assert!(
            throttle
                .should_warn_generation_budget_miss(now + FRAME_GENERATION_BUDGET_WARN_INTERVAL)
        );
    }

    #[test]
    fn retry_delay_uses_observed_high_refresh_interval() {
        let mut throttle = WindowFrameThrottle::default();
        let now = Instant::now();

        throttle.record_frame_start(now);
        throttle.record_frame_start(now + Duration::from_millis(8));

        assert_eq!(throttle.retry_delay(), Duration::from_millis(8));
    }

    #[test]
    fn retry_delay_is_capped_for_unknown_or_slow_refresh() {
        let mut throttle = WindowFrameThrottle::default();
        let now = Instant::now();

        assert_eq!(throttle.retry_delay(), HIGH_REFRESH_FRAME_INTERVAL);

        throttle.record_frame_start(now);
        throttle.record_frame_start(now + Duration::from_millis(33));

        assert_eq!(throttle.retry_delay(), HIGH_REFRESH_FRAME_INTERVAL);
    }
}
