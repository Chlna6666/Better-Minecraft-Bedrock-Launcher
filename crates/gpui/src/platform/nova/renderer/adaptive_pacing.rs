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
    gpu_cost_ns: Option<u64>,
    gpu_samples: u8,
    new_gpu_samples: u8,
    empty_gpu_windows: u8,
    observed_refresh: Option<Duration>,
    nominal_refresh: Option<Duration>,
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
            gpu_cost_ns: None,
            gpu_samples: 0,
            new_gpu_samples: 0,
            empty_gpu_windows: 0,
            observed_refresh: None,
            nominal_refresh: None,
        }
    }
}

impl AdaptivePacing {
    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Nominal native period or same-swapchain DXGI synchronization estimate.
    /// Neither CPU Present duration nor GPU execution time is a display clock.
    pub(super) fn native_refresh_period(&self) -> Option<Duration> {
        self.observed_refresh
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

    pub(super) fn record_gpu_cost(&mut self, sample: gfx_core::FrameGpuTimestamps) {
        if !self.enabled || sample.frequency == 0 || sample.duration_ticks == 0 {
            return;
        }
        let ns = u128::from(sample.duration_ticks) * 1_000_000_000 / u128::from(sample.frequency);
        let Ok(ns) = u64::try_from(ns) else { return; };
        if ns == 0 || ns > 1_000_000_000 {
            return;
        }
        // Per-renderer EWMA, measured GPU main-pass duration only. This
        // is a lower bound, not total 3D/offscreen/queue or scanout latency.
        self.gpu_cost_ns = Some(self.gpu_cost_ns.map_or(ns, |old| (old * 7 + ns) / 8));
        self.gpu_samples = self.gpu_samples.saturating_add(1);
        self.new_gpu_samples = self.new_gpu_samples.saturating_add(1);
    }

    fn reset_evidence(&mut self) {
        self.gpu_cost_ns = None;
        self.gpu_samples = 0;
        self.new_gpu_samples = 0;
        self.empty_gpu_windows = 0;
        self.observed_refresh = None;
        self.nominal_refresh = None;
        self.last_frame = None;
        self.previous = None;
        self.frames = 0;
        self.pressure = 0;
        self.pressure_pending = false;
        self.quiet_windows = 0;
        self.missing_windows = 0;
    }

    /// Retry a failed optional native actuator on the next valid sample,
    /// rather than permanently diverging from the software admission limit.
    pub(super) fn native_limit_failed(&mut self) {
        self.sync_native = self.enabled;
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
        let prior_period = self.observed_refresh;
        let prior_nominal = self.nominal_refresh;
        self.nominal_refresh = feedback.and_then(nominal_refresh);
        // Unknown nominal period is valid (e.g. VRR). A same-domain timestamp
        // delta divided by the native progress counter is only an observed
        // cadence estimate. Never turn it into an independent display clock.
        let measured = previous.zip(feedback).and_then(|(old, new)| observed_period(old, new));
        let progressing_sample = previous.zip(feedback).is_some_and(|(old, new)| progressing(old, new));
        self.observed_refresh = if progressing_sample || previous.is_none() {
            self.nominal_refresh.or(measured)
        } else {
            None
        };
        let nominal_changed = previous.is_some() && feedback.is_some() && match (prior_nominal, self.nominal_refresh) {
            (Some(a), Some(b)) => a.max(b) - a.min(b) > a.max(b) / 4,
            (None, None) => false,
            _ => true,
        };
        let observed_variation = prior_period.zip(self.observed_refresh)
            .is_some_and(|(a, b)| a.max(b) - a.min(b) > a.max(b) / 4);
        // A VRR period change does not invalidate the hardware GPU duration.
        // It does invalidate a quiet-window decision to shrink admission.
        let period_changed = nominal_changed;
        if std::mem::take(&mut self.new_gpu_samples) == 0 {
            self.empty_gpu_windows = self.empty_gpu_windows.saturating_add(1);
            if self.empty_gpu_windows >= 2 {
                self.gpu_cost_ns = None;
                self.gpu_samples = 0;
            }
        } else {
            self.empty_gpu_windows = 0;
        }
        let pressure = std::mem::take(&mut self.pressure);
        self.frames = 0;
        let previous_limit = self.limit;
        let gpu_heavy = self.gpu_samples >= 4 && self.gpu_cost_ns.zip(self.native_refresh_period())
            .is_some_and(|(cost, period)| u128::from(cost) * 4 >= period.as_nanos() * 3);
        if period_changed {
            self.quiet_windows = 0;
            self.missing_windows = 0;
            self.limit = MAX_IN_FLIGHT_SUBMISSIONS;
            self.gpu_cost_ns = None;
            self.gpu_samples = 0;
        } else if progressing_sample {
            self.missing_windows = 0;
            if pressure >= PRESSURE_FRAMES || gpu_heavy || observed_variation {
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

fn nominal_refresh(sample: PresentationFeedback) -> Option<Duration> {
    match sample {
        PresentationFeedback::Vulkan { refresh_duration_ns, .. }
        | PresentationFeedback::Wayland { refresh_duration_ns, .. } =>
            refresh_duration_ns.filter(|ns| (1_000_000..=100_000_000).contains(ns)).map(Duration::from_nanos),
        _ => None,
    }
}

fn period_from_delta(old_ns: u64, new_ns: u64, old_sequence: u64, new_sequence: u64) -> Option<Duration> {
    let intervals = new_sequence.checked_sub(old_sequence).filter(|count| *count > 0)?;
    let ns = new_ns.checked_sub(old_ns)?.checked_div(intervals)?;
    (1_000_000..=100_000_000).contains(&ns).then(|| Duration::from_nanos(ns))
}

fn observed_period(old: PresentationFeedback, new: PresentationFeedback) -> Option<Duration> {
    match (old, new) {
        (
            PresentationFeedback::Vulkan { present_id: a, actual_present_ns: t0, .. },
            PresentationFeedback::Vulkan { present_id: b, actual_present_ns: t1, .. },
        ) => period_from_delta(t0, t1, u64::from(a), u64::from(b)),
        (
            PresentationFeedback::Wayland { clock_id: c0, actual_present_ns: t0, sequence: a, .. },
            PresentationFeedback::Wayland { clock_id: c1, actual_present_ns: t1, sequence: b, .. },
        ) if c0 == c1 => period_from_delta(t0, t1, a, b),
        #[cfg(target_os = "windows")]
        (old @ PresentationFeedback::Dxgi { .. }, new @ PresentationFeedback::Dxgi { .. }) =>
            dxgi_refresh_period(old, new, qpc_frequency()?),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn qpc_frequency() -> Option<u64> {
    static FREQUENCY: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut frequency = 0i64;
        // SAFETY: QPC writes only the valid local frequency output.
        unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency) }.ok()?;
        u64::try_from(frequency).ok().filter(|frequency| *frequency != 0)
    })
}

#[cfg(any(test, target_os = "windows"))]
fn dxgi_refresh_period(old: PresentationFeedback, new: PresentationFeedback, frequency: u64) -> Option<Duration> {
    let PresentationFeedback::Dxgi { sync_refresh_count: a, sync_qpc_ticks: t0, .. } = old else { return None; };
    let PresentationFeedback::Dxgi { sync_refresh_count: b, sync_qpc_ticks: t1, .. } = new else { return None; };
    let refreshes = b.checked_sub(a).filter(|n| *n > 0)?;
    let ticks = u64::try_from(t1.checked_sub(t0)?).ok().filter(|n| *n > 0)?;
    let ns = u128::from(ticks) * 1_000_000_000 / u128::from(frequency.checked_mul(u64::from(refreshes)).filter(|n| *n > 0)?);
    // Reject idle gaps, counter resets and implausible driver samples.
    // This remains an observed average, not a claim of fixed-rate scanout.
    (1_000_000..=100_000_000).contains(&ns).then(|| Duration::from_nanos(ns as u64))
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

#[cfg(test)]
mod cost_tests {
    use super::*;
    #[test]
    fn gpu_cost_is_converted_and_reset_on_surface_change() {
        let mut pacing = AdaptivePacing::default();
        pacing.set_enabled(true);
        pacing.record_gpu_cost(gfx_core::FrameGpuTimestamps { duration_ticks: 8, frequency: 1_000 });
        assert_eq!(pacing.gpu_cost_ns, Some(8_000_000));
        pacing.record_gpu_cost(gfx_core::FrameGpuTimestamps { duration_ticks: 16, frequency: 1_000 });
        assert_eq!(pacing.gpu_cost_ns, Some(9_000_000));
        pacing.record_gpu_cost(gfx_core::FrameGpuTimestamps { duration_ticks: 10, frequency: 0 });
        assert_eq!(pacing.gpu_samples, 2);
        pacing.surface_changed();
        assert_eq!(pacing.gpu_cost_ns, None);
    }
}

#[cfg(test)]
mod dxgi_period_tests {
    use super::*;
    fn sample(count: u32, qpc: i64) -> PresentationFeedback {
        PresentationFeedback::Dxgi { present_count: count, present_refresh_count: count, sync_refresh_count: count, sync_qpc_ticks: qpc }
    }
    #[test]
    fn uses_refresh_counter_not_number_of_rendered_frames() {
        assert_eq!(dxgi_refresh_period(sample(10, 100_000), sample(12, 116_000), 1_000_000), Some(Duration::from_millis(8)));
        assert_eq!(dxgi_refresh_period(sample(12, 116_000), sample(10, 100_000), 1_000_000), None);
        assert_eq!(dxgi_refresh_period(sample(10, 100_000), sample(10, 116_000), 1_000_000), None);
        assert_eq!(dxgi_refresh_period(sample(10, 100_000), sample(12, 116_000), 0), None);
    }
}

#[cfg(test)]
mod variable_period_tests {
    use super::*;
    fn sample(clock_id: u32, seq: u64, ns: u64) -> PresentationFeedback {
        PresentationFeedback::Wayland { clock_id, actual_present_ns: ns, sequence: seq, refresh_duration_ns: None }
    }
    #[test]
    fn estimates_variable_cadence_without_inventing_nominal_refresh() {
        let a = sample(1, 5, 100_000_000);
        let b = sample(1, 7, 116_000_000);
        let c = sample(1, 9, 148_000_000);
        assert_eq!(nominal_refresh(b), None);
        assert_eq!(observed_period(a, b), Some(Duration::from_millis(8)));
        assert_eq!(observed_period(b, c), Some(Duration::from_millis(16)));
        assert_eq!(observed_period(a, sample(2, 7, 116_000_000)), None);
        assert_eq!(observed_period(b, a), None);
        assert_eq!(observed_period(a, sample(1, 5, 116_000_000)), None);
        let mut pacing = AdaptivePacing::default();
        pacing.set_enabled(true);
        pacing.observe(Some(a));
        pacing.observe(Some(b));
        assert_eq!(pacing.native_refresh_period(), Some(Duration::from_millis(8)));
        pacing.observe(Some(c));
        assert_eq!(pacing.native_refresh_period(), Some(Duration::from_millis(16)));
        assert_eq!(pacing.limit(), MAX_IN_FLIGHT_SUBMISSIONS);
    }
    #[test]
    fn stale_gpu_evidence_expires_without_new_samples() {
        let mut pacing = AdaptivePacing::default();
        pacing.set_enabled(true);
        pacing.record_gpu_cost(gfx_core::FrameGpuTimestamps { duration_ticks: 10, frequency: 1_000 });
        pacing.observe(None);
        pacing.observe(None);
        assert!(pacing.gpu_cost_ns.is_some());
        pacing.observe(None);
        assert_eq!(pacing.gpu_cost_ns, None);
    }
}
