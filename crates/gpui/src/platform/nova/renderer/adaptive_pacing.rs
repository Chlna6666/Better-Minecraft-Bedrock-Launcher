//! Per-window adaptive admission. Native feedback establishes progress;
//! exact GPU fence pressure controls the resource-slot budget.
use super::MAX_IN_FLIGHT_SUBMISSIONS;
use gfx_core::PresentationFeedback;
use std::time::{Duration, Instant};

const SAMPLE_FRAMES: u8 = 16;
const QUIET_WINDOWS: u8 = 4;
const PRESSURE_FRAMES: u8 = 4;
const IDLE_GAP: Duration = Duration::from_millis(250);

pub(super) struct AdaptivePacing {
    enabled: bool,
    limit: usize,
    last_frame: Option<Instant>,
    previous: Option<PresentationFeedback>,
    frames: u8,
    pressure: u8,
    pressure_pending: bool,
    quiet_windows: u8,
    missing_windows: u8,
    sync_native: bool,
}

impl Default for AdaptivePacing {
    fn default() -> Self {
        Self {
            enabled: false,
            limit: MAX_IN_FLIGHT_SUBMISSIONS,
            last_frame: None,
            previous: None,
            frames: 0,
            pressure: 0,
            pressure_pending: false,
            quiet_windows: 0,
            missing_windows: 0,
            sync_native: false,
        }
    }
}

impl AdaptivePacing {
    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    pub(super) fn mark_gpu_pressure(&mut self) {
        if self.enabled {
            // Multiple retries of the same pending frame count once.
            self.pressure_pending = true;
        }
    }

    fn reset_evidence(&mut self) {
        self.last_frame = None;
        self.previous = None;
        self.frames = 0;
        self.pressure = 0;
        self.pressure_pending = false;
        self.quiet_windows = 0;
        self.missing_windows = 0;
    }

    pub(super) fn surface_changed(&mut self) {
        self.reset_evidence();
        // Resizing can restore the backend default latency. Reapply only after
        // obtaining a new native sample, not during drawable replacement.
        self.sync_native = self.enabled && self.limit < MAX_IN_FLIGHT_SUBMISSIONS;
    }

    pub(super) fn presented(&mut self, now: Instant, older_gpu_work: bool, resizing: bool) -> bool {
        if !self.enabled {
            return false;
        }
        if resizing {
            self.reset_evidence();
            return false;
        }
        if self.last_frame.is_some_and(|last| now.saturating_duration_since(last) > IDLE_GAP) {
            self.reset_evidence();
        }
        self.last_frame = Some(now);
        if self.pressure_pending || older_gpu_work {
            self.pressure = self.pressure.saturating_add(1);
        }
        self.pressure_pending = false;
        self.frames = self.frames.saturating_add(1);
        self.frames >= SAMPLE_FRAMES
    }

    pub(super) fn observe(&mut self, feedback: Option<PresentationFeedback>) -> Option<usize> {
        let previous = self.previous;
        self.previous = feedback;
        let pressure = std::mem::take(&mut self.pressure);
        self.frames = 0;
        let previous_limit = self.limit;
        if previous.zip(feedback).is_some_and(|(old, new)| progressing(old, new)) {
            self.missing_windows = 0;
            if pressure >= PRESSURE_FRAMES {
                self.quiet_windows = 0;
                self.limit = MAX_IN_FLIGHT_SUBMISSIONS;
            } else if pressure == 0 {
                self.quiet_windows = self.quiet_windows.saturating_add(1);
                if self.quiet_windows >= QUIET_WINDOWS {
                    self.limit = 1;
                }
            } else {
                self.quiet_windows = 0;
            }
        } else {
            self.quiet_windows = 0;
            self.missing_windows = self.missing_windows.saturating_add(1);
            // Stale/disjoint/unsupported feedback must not trap a reduced budget.
            if self.missing_windows >= 2 {
                self.limit = MAX_IN_FLIGHT_SUBMISSIONS;
            }
        }
        let sync = previous_limit != self.limit || (self.sync_native && feedback.is_some());
        if sync {
            self.sync_native = false;
            Some(self.limit)
        } else {
            None
        }
    }
}

fn progressing(old: PresentationFeedback, new: PresentationFeedback) -> bool {
    match (old, new) {
        (
            PresentationFeedback::Dxgi { present_count: a, sync_qpc_ticks: t0, .. },
            PresentationFeedback::Dxgi { present_count: b, sync_qpc_ticks: t1, .. },
        ) => b > a && t1 > t0,
        (
            PresentationFeedback::Vulkan { present_id: a, actual_present_ns: t0, refresh_duration_ns: r0, .. },
            PresentationFeedback::Vulkan { present_id: b, actual_present_ns: t1, refresh_duration_ns: r1, .. },
        ) => b > a && t1 > t0 && refresh_compatible(r0, r1),
        (
            PresentationFeedback::Wayland { clock_id: c0, actual_present_ns: t0, refresh_duration_ns: r0, .. },
            PresentationFeedback::Wayland { clock_id: c1, actual_present_ns: t1, refresh_duration_ns: r1, .. },
        ) => c0 == c1 && t1 > t0 && refresh_compatible(r0, r1),
        // Never compare different native domains or across counter resets.
        _ => false,
    }
}

// Unknown refresh (including VRR) stays unknown. A large nominal-period
// transition invalidates the old evidence rather than inventing a fixed Hz.
fn refresh_compatible(old: Option<u64>, new: Option<u64>) -> bool {
    match (old.filter(|v| *v != 0), new.filter(|v| *v != 0)) {
        (Some(a), Some(b)) => a.abs_diff(b) <= a.max(b) / 4,
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: u32) -> PresentationFeedback {
        PresentationFeedback::Dxgi {
            present_count: n,
            present_refresh_count: n,
            sync_refresh_count: n,
            sync_qpc_ticks: i64::from(n),
        }
    }

    fn window(pacing: &mut AdaptivePacing, now: &mut Instant, n: Option<u32>, pressure: u8) -> Option<usize> {
        for i in 0..SAMPLE_FRAMES {
            *now += Duration::from_millis(8);
            if i < pressure {
                pacing.mark_gpu_pressure();
                pacing.mark_gpu_pressure();
            }
            assert_eq!(pacing.presented(*now, false, false), i == SAMPLE_FRAMES - 1);
        }
        pacing.observe(n.map(sample))
    }

    #[test]
    fn lowers_only_after_sustained_quiet_progress_and_recovers_on_pressure() {
        let mut pacing = AdaptivePacing::default();
        pacing.set_enabled(true);
        let mut now = Instant::now();
        assert_eq!(window(&mut pacing, &mut now, Some(1), 0), None);
        for n in 2..=4 {
            assert_eq!(window(&mut pacing, &mut now, Some(n), 0), None);
        }
        assert_eq!(window(&mut pacing, &mut now, Some(5), 0), Some(1));
        assert_eq!(window(&mut pacing, &mut now, Some(6), PRESSURE_FRAMES), Some(MAX_IN_FLIGHT_SUBMISSIONS));
    }

    #[test]
    fn stale_feedback_restores_default_and_resize_invalidates_samples() {
        let mut pacing = AdaptivePacing::default();
        pacing.set_enabled(true);
        let mut now = Instant::now();
        for n in 1..=5 {
            window(&mut pacing, &mut now, Some(n), 0);
        }
        assert_eq!(pacing.limit(), 1);
        pacing.surface_changed();
        assert_eq!(window(&mut pacing, &mut now, None, 0), None);
        assert_eq!(window(&mut pacing, &mut now, None, 0), Some(MAX_IN_FLIGHT_SUBMISSIONS));
        assert_eq!(pacing.limit(), MAX_IN_FLIGHT_SUBMISSIONS);
    }

    #[test]
    fn wayland_clock_and_refresh_changes_invalidate_evidence() {
        let sample = |clock_id, actual_present_ns, refresh_duration_ns| PresentationFeedback::Wayland {
            clock_id,
            actual_present_ns,
            refresh_duration_ns,
            sequence: 0,
        };
        assert!(progressing(sample(1, 100, Some(8_333_333)), sample(1, 200, Some(8_400_000))));
        assert!(!progressing(sample(1, 100, Some(8_333_333)), sample(1, 200, Some(16_666_666))));
        assert!(!progressing(sample(1, 100, None), sample(2, 200, None)));
        assert!(progressing(sample(1, 100, None), sample(1, 200, None)));
        assert!(!progressing(sample(1, 200, None), sample(1, 100, None)));
        assert!(!progressing(sample(1, 100, Some(8_333_333)), sample(1, 200, None)));
    }

    #[test]
    fn disabled_and_idle_windows_do_not_accumulate_evidence() {
        let mut pacing = AdaptivePacing::default();
        let mut now = Instant::now();
        for _ in 0..100 {
            assert!(!pacing.presented(now, false, false));
        }
        pacing.set_enabled(true);
        for _ in 0..100 {
            now += IDLE_GAP + Duration::from_millis(1);
            assert!(!pacing.presented(now, false, false));
        }
        assert_eq!(pacing.limit(), MAX_IN_FLIGHT_SUBMISSIONS);
    }
}
