use std::time::{Duration, Instant};

// At 240 Hz a native drag can deliver many WM_SIZE notifications between
// displays. Rebuilding the entire UI for every notification only increases
// GPU queue lag; cap layout commits at 120 Hz while coalescing the latest
// authoritative HWND geometry. Native sizing and compositor fallback remain
// asynchronous and are not delayed by this CPU cadence.
const MIN_LIVE_RESIZE_FRAME_INTERVAL: Duration = Duration::from_micros(8_333);

/// Paces native live-resize UI commits; the first frame starts immediately.
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
            .max(display_interval)
            .max(MIN_LIVE_RESIZE_FRAME_INTERVAL);
        self.last_started_at
            .is_none_or(|last| now.saturating_duration_since(last) >= interval)
    }

    pub(super) fn accept(&mut self, now: Instant, display_interval: Duration) -> bool {
        // WM_SIZE may change every millisecond. A newer extent replaces the
        // pending slot, but cannot bypass the UI commit cadence: doing so
        // rebuilds multiple obsolete Scenes in a single display interval.
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

    #[test]
    fn rapid_native_extents_are_coalesced_at_the_ui_cadence() {
        let now = Instant::now();
        let interval = Duration::from_millis(16);
        let mut cadence = Cadence::default();
        cadence.configured_interval = Some(Duration::from_millis(33));
        assert!(cadence.accept(now, interval));
        assert!(!cadence.accept(now + Duration::from_millis(1), interval));
        assert!(!cadence.accept(now + Duration::from_millis(32), interval));
        assert!(cadence.accept(now + Duration::from_millis(33), interval));
        // A new drag starts with a fresh frame even if the preceding modal loop
        // just committed at the same timestamp.
        cadence.reset();
        assert!(cadence.accept(now + Duration::from_millis(34), interval));
    }

    #[test]
    fn high_refresh_native_resize_has_a_bounded_ui_commit_rate() {
        let now = Instant::now();
        let mut cadence = Cadence::default();
        let refresh_240hz = Duration::from_micros(4_167);
        assert!(cadence.accept(now, refresh_240hz));
        assert!(!cadence.accept(now + refresh_240hz, refresh_240hz));
        assert!(cadence.accept(now + Duration::from_micros(8_333), refresh_240hz));
    }
}
