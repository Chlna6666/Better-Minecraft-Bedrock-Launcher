use std::time::{Duration, Instant};

/// One cadence shared by native size messages and DWM wakes for this window.
#[derive(Clone, Copy, Default)]
pub(super) struct Cadence {
    pub(super) configured_interval: Option<Duration>,
    last_started_at: Option<Instant>,
}

impl Cadence {
    pub(super) fn reset(&mut self) {
        self.last_started_at = None;
    }

    pub(super) fn is_due(self, now: Instant, display_interval: Duration) -> bool {
        let interval = self
            .configured_interval
            .unwrap_or_default()
            .max(display_interval);
        self.last_started_at
            .is_none_or(|last| now.saturating_duration_since(last) >= interval)
    }

    pub(super) fn accept(&mut self, now: Instant, display_interval: Duration) -> bool {
        if !self.is_due(now, display_interval) {
            return false;
        }
        self.last_started_at = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_resize_and_vsync_share_one_cadence_with_immediate_first_frame() {
        let now = Instant::now();
        let interval = Duration::from_millis(16);
        let mut cadence = Cadence::default();
        assert!(cadence.accept(now, interval));
        // A size event followed by a DWM wake must not generate a second frame.
        assert!(!cadence.accept(now + Duration::from_millis(1), interval));
        assert!(!cadence.accept(now + Duration::from_millis(15), interval));
        assert!(cadence.accept(now + interval, interval));

        // A new native gesture starts immediately, preserving an explicit FPS limit.
        cadence.configured_interval = Some(Duration::from_millis(33));
        cadence.reset();
        assert!(cadence.accept(now + Duration::from_millis(17), interval));
        assert!(!cadence.accept(now + Duration::from_millis(33), interval));
        assert!(cadence.accept(now + Duration::from_millis(50), interval));
    }
}
