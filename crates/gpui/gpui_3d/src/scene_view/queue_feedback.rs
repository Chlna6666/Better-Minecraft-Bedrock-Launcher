//! Per-GPU-owner adaptive meshing queue admission.
//!
//! Hardware compute timestamps measure ONLY native GPU execution. An observed
//! SubmissionId fence latency also includes device queueing and the OWNER's
//! polling cadence: its excess over GPU execution is a conservative pressure
//! proxy, NEVER a fabricated hardware GPU Wait measurement. Graphics
//! DrawIndirect ticks and host frame cadence are kept distinct as well.
//! No CPU waits, global locks, shader policy or cross-device training.

use gfx_core::{GpuTimestampSample, GpuTimestampStage, SubmissionId};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const MIN_TRAINING_SAMPLES: u32 = 6;
const MAX_INFLIGHT_NATIVE: usize = 3;
const MAX_PENDING_TIMESTAMPS: usize = 128;
const MAX_PENDING_AGE: Duration = Duration::from_secs(10);
const MAX_UNOBSERVED_NATIVE_PROBES: u32 = 8;
const GRAPHICS_WINDOW: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComputePath {
    Ordered,
    Native,
}

/// Update a bounded 95th-percentile cache, once every 16 measurements.
/// Hot-path admission reads cached scalars; it never sorts a frame window.
fn record_graphics_window(
    window: &mut VecDeque<u64>,
    ns: u64,
    p95: &mut u64,
    count: u32,
) {
    if window.len() == GRAPHICS_WINDOW {
        window.pop_front();
    }
    window.push_back(ns);
    if count <= 16 || count.is_multiple_of(16) {
        let mut sorted = window.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable();
        let rank = sorted.len().saturating_mul(95).div_ceil(100).saturating_sub(1);
        *p95 = sorted[rank.min(sorted.len().saturating_sub(1))];
    }
}

#[derive(Clone, Copy, Default)]
struct Cost {
    samples: u32,
    /// Fixed-point: hardware nanoseconds per 1024 dispatched workgroups.
    ns_per_1024: u64,
}

impl Cost {
    fn observe(&mut self, workgroups: u64, gpu_ns: u64) {
        if workgroups == 0 || gpu_ns == 0 {
            return;
        }
        let sample = ((gpu_ns as u128).saturating_mul(1024)
            / u128::from(workgroups)).min(u128::from(u64::MAX)) as u64;
        if sample == 0 {
            return;
        }
        self.ns_per_1024 = if self.samples == 0 {
            sample
        } else {
            smooth(self.ns_per_1024, sample)
        };
        self.samples = self.samples.saturating_add(1);
    }

    fn predict(&self, workgroups: u64) -> Option<u64> {
        (self.samples >= MIN_TRAINING_SAMPLES).then(|| {
            (u128::from(self.ns_per_1024).saturating_mul(u128::from(workgroups))
                / 1024).min(u128::from(u64::MAX)) as u64
        })
    }
}

fn smooth(previous: u64, next: u64) -> u64 {
    ((u128::from(previous).saturating_mul(7) + u128::from(next)) / 8)
        .min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy)]
struct PendingGpu {
    path: ComputePath,
    workgroups: u64,
    created: Instant,
    /// Host-observed native completion, not native GPU semaphore wait time.
    observed_fence_ns: Option<u64>,
}

struct PendingFence {
    id: Option<u64>,
    issued_at: Instant,
}

/// Owned exclusively by one Sidecar GPU device. Multiple SceneViews on that
/// owner share one model; no samples leak between physical devices or drivers.
#[derive(Default)]
pub(super) struct QueueFeedback {
    ordered: Cost,
    native: Cost,
    /// Estimate of excess native fence latency over the native GPU
    /// execution interval. Includes polling jitter, so use as an UPPER
    /// pressure guard, not as the duration of a GPU semaphore Wait.
    observed_handoff_pressure_ns: u64,
    handoff_samples: u32,
    graphics_draw_ns: u64,
    graphics_draw_p95_ns: u64,
    graphics_window: VecDeque<u64>,
    graphics_samples: u32,
    frame_cadence_ns: u64,
    frame_p95_ns: u64,
    frame_window: VecDeque<u64>,
    frame_samples: u32,
    last_frame: Option<Instant>,
    decisions: u64,
    native_disabled: bool,
    /// Stop repeated expensive independent submissions if this queue family
    /// never yields valid hardware timestamp results (driver/family limits).
    unobserved_native_probes: u32,
    pending_gpu: HashMap<u64, PendingGpu>,
    in_flight: HashMap<SubmissionId, PendingFence>,
}

impl QueueFeedback {
    pub(super) fn observe_frame_cadence(&mut self, frame_time: Instant) {
        if self.last_frame == Some(frame_time) {
            return;
        }
        if let Some(previous) = self.last_frame
            && let Some(elapsed) = frame_time.checked_duration_since(previous)
            && elapsed >= Duration::from_millis(1)
            && elapsed <= Duration::from_millis(100)
        {
            let ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
            self.frame_cadence_ns = if self.frame_samples == 0 {
                ns
            } else {
                smooth(self.frame_cadence_ns, ns)
            };
            self.frame_samples = self.frame_samples.saturating_add(1);
            record_graphics_window(
                &mut self.frame_window, ns,
                &mut self.frame_p95_ns, self.frame_samples,
            );
        }
        self.last_frame = Some(frame_time);
    }

    /// Returns whether independent execution is worth attempting. Hardware
    /// samples, not HOST submit durations, train both GPU execution slopes.
    ///
    /// During warmup occasional bounded native probes are mandatory; otherwise
    /// a primary-only model could never learn whether asynchronous execution
    /// actually helps. An overloaded independent queue is never probed.
    pub(super) fn admit_native(
        &mut self,
        available: bool,
        has_gpu_timestamps: bool,
        workgroups: u64,
    ) -> bool {
        if !available || !has_gpu_timestamps || workgroups == 0
            || self.native_disabled
            || self.in_flight.len() >= MAX_INFLIGHT_NATIVE
        {
            return false;
        }
        self.decisions = self.decisions.saturating_add(1);
        let Some(ordered_ns) = self.ordered.predict(workgroups) else {
            return false;
        };
        // A native queue costs two GPU handoffs and an extra command/fence
        // submission. Don't probe work too short to overlap even the launch.
        let handoff_ns = self.observed_handoff_pressure_ns.max(350_000);
        if ordered_ns <= handoff_ns.saturating_mul(2) {
            return false;
        }
        let Some(native_ns) = self.native.predict(workgroups) else {
            return self.unobserved_native_probes < MAX_UNOBSERVED_NATIVE_PROBES
                && self.decisions.is_multiple_of(6);
        };
        if self.handoff_samples >= 4
            && self.observed_handoff_pressure_ns > ordered_ns.saturating_mul(2)
        {
            return false;
        }
        // Continue occasional ORDERED hardware probes even when independent
        // Compute wins. Otherwise primary execution rates become stale after
        // thermal throttling, workload changes or a driver power-state change.
        // 24 is divisible by the 4-batch tagged-sample cadence.
        if self.decisions.is_multiple_of(24) {
            return false;
        }
        // Draw time is GPU execution (but not the full frame). Cadence is
        // presentation scheduling, not a GPU metric. Together they only
        // indicate whether moving Compute off Graphics might be worthwhile.
        let graphics_pressure = self.graphics_samples >= 8
            && self.graphics_draw_p95_ns >= 2_000_000;
        let slow_present_cadence = self.frame_samples >= 16
            && self.frame_p95_ns >= 20_000_000;
        // A slow presentation interval alone can be caused by UI/CPU
        // scheduling or VSync; never treat it as proof of GPU congestion.
        // Only allow a slower native compute path when real DrawIndirect
        // GPU timestamps ALSO show graphics execution pressure.
        let can_overlap = graphics_pressure
            && (slow_present_cadence || self.graphics_draw_p95_ns >= 4_000_000)
            && native_ns.saturating_add(handoff_ns)
                < ordered_ns.saturating_mul(5) / 4;
        let genuinely_faster = native_ns.saturating_add(handoff_ns)
            < ordered_ns.saturating_mul(4) / 5;
        can_overlap || genuinely_faster
            || self.decisions.is_multiple_of(32) // bounded re-evaluation
    }

    /// A native encoder/queue submission failed after preflight. Disable
    /// further speculative async work on this device for this session.
    /// The current error still propagates and existing untracked Mesh Arena
    /// hazards remain quarantined until their native completion is proven.
    pub(super) fn native_failed(&mut self) {
        self.native_disabled = true;
    }

    /// Keep query/Resolve overhead bounded once enough real native samples
    /// have trained both paths. Cold-start always profiles the actual work.
    pub(super) fn should_tag(&self, supported: bool) -> bool {
        supported
            && (self.ordered.samples < MIN_TRAINING_SAMPLES
                || self.native.samples < MIN_TRAINING_SAMPLES
                || self.decisions.is_multiple_of(4))
    }

    pub(super) fn accepted(
        &mut self,
        tag: Option<u64>,
        path: ComputePath,
        workgroups: u64,
        submission: Option<SubmissionId>,
    ) {
        let now = Instant::now();
        if path == ComputePath::Native && tag.is_some() {
            self.unobserved_native_probes = self.unobserved_native_probes.saturating_add(1);
        }
        if let Some(tag) = tag {
            if self.pending_gpu.len() >= MAX_PENDING_TIMESTAMPS {
                self.pending_gpu.retain(|_, pending| {
                    now.saturating_duration_since(pending.created) < MAX_PENDING_AGE
                });
                // Never grow indefinitely on drivers without timestamp
                // support or after a lost device; discard training, not
                // actual lifetime tracking.
                if self.pending_gpu.len() >= MAX_PENDING_TIMESTAMPS {
                    self.pending_gpu.clear();
                }
            }
            self.pending_gpu.insert(tag, PendingGpu {
                path, workgroups, created: now, observed_fence_ns: None,
            });
        }
        if let Some(submission) = submission {
            self.in_flight.insert(submission, PendingFence {
                id: tag, issued_at: now,
            });
        }
    }

    pub(super) fn completed(&mut self, submission: SubmissionId) {
        let Some(fence) = self.in_flight.remove(&submission) else {
            return;
        };
        if let Some(tag) = fence.id
            && let Some(pending) = self.pending_gpu.get_mut(&tag)
        {
            pending.observed_fence_ns = Some(
                u64::try_from(fence.issued_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
            );
        }
    }

    pub(super) fn observe_gpu_sample(&mut self, sample: GpuTimestampSample) {
        let Some(duration) = sample.duration() else {
            return;
        };
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        match sample.tag.stage {
            GpuTimestampStage::GeneratedMesh => {
                let Some(pending) = self.pending_gpu.remove(&sample.tag.batch_id) else {
                    return;
                };
                match pending.path {
                    ComputePath::Ordered => self.ordered.observe(pending.workgroups, ns),
                    ComputePath::Native => {
                        self.unobserved_native_probes = 0;
                        self.native.observe(pending.workgroups, ns);
                        if let Some(fence_ns) = pending.observed_fence_ns {
                            let pressure = fence_ns.saturating_sub(ns);
                            self.observed_handoff_pressure_ns =
                                if self.handoff_samples == 0 { pressure } else {
                                    smooth(self.observed_handoff_pressure_ns, pressure)
                                };
                            self.handoff_samples = self.handoff_samples.saturating_add(1);
                        }
                    }
                }
            }
            GpuTimestampStage::DrawIndirect if ns > 0 => {
                self.graphics_draw_ns = if self.graphics_samples == 0 {
                    ns
                } else {
                    smooth(self.graphics_draw_ns, ns)
                };
                self.graphics_samples = self.graphics_samples.saturating_add(1);
                record_graphics_window(
                    &mut self.graphics_window, ns,
                    &mut self.graphics_draw_p95_ns, self.graphics_samples,
                );
            }
            _ => {}
        }
    }

    pub(super) fn debug_summary(&self) -> (u32, u32, u64, u64, u64) {
        (
            self.ordered.samples,
            self.native.samples,
            self.ordered.ns_per_1024,
            self.native.ns_per_1024,
            self.observed_handoff_pressure_ns,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx_core::{GpuTimestampStage, GpuTimestampTag};

    fn gpu(id: u64, micros: u64) -> GpuTimestampSample {
        GpuTimestampSample {
            tag: GpuTimestampTag {
                batch_id: id, stage: GpuTimestampStage::GeneratedMesh,
            },
            duration_ticks: micros, frequency_hz: 1_000_000,
        }
    }

    #[test]
    fn no_hardware_samples_cannot_fake_native_queue_admission() {
        let mut model = QueueFeedback::default();
        assert!(!model.admit_native(true, true, 10_000));
        assert!(!model.admit_native(true, false, 10_000));
        assert!(!model.admit_native(false, true, 10_000));
    }

    #[test]
    fn learns_separate_native_and_ordered_gpu_rates_with_bounded_exploration() {
        let mut model = QueueFeedback::default();
        for id in 1..=6 {
            model.accepted(Some(id), ComputePath::Ordered, 1024, None);
            model.observe_gpu_sample(gpu(id, 1_600));
        }
        assert_eq!(model.ordered.samples, 6);
        // Warmup uses periodic bounded probes, not permanent fixed workgroups.
        let mut admitted = false;
        for _ in 0..12 {
            admitted |= model.admit_native(true, true, 1024);
        }
        assert!(admitted);
        for id in 21..=26 {
            model.accepted(Some(id), ComputePath::Native, 1024, None);
            model.observe_gpu_sample(gpu(id, 500));
        }
        assert!(model.admit_native(true, true, 1024));
        assert!(!model.admit_native(true, true, 1));
    }

    #[test]
    fn independent_fence_pressure_disables_overloaded_queue_without_gpu_wait_claim() {
        let mut model = QueueFeedback::default();
        for id in 1..=6 {
            model.accepted(Some(id), ComputePath::Ordered, 1024, None);
            model.observe_gpu_sample(gpu(id, 1_800));
            model.accepted(Some(id + 20), ComputePath::Native, 1024, None);
            model.observe_gpu_sample(gpu(id + 20, 750));
        }
        for i in 0..MAX_INFLIGHT_NATIVE {
            model.in_flight.insert(
                SubmissionId::from_parts(i as u32 + 10, 1),
                PendingFence { id: None, issued_at: Instant::now() },
            );
        }
        assert!(!model.admit_native(true, true, 1024));
    }

    #[test]
    fn native_backend_failure_keeps_subsequent_mesh_work_ordered() {
        let mut model = QueueFeedback::default();
        for id in 1..=6 {
            model.accepted(Some(id), ComputePath::Ordered, 1024, None);
            model.observe_gpu_sample(gpu(id, 1_900));
        }
        model.native_failed();
        for _ in 0..64 {
            assert!(!model.admit_native(true, true, 1024));
        }
    }

    #[test]
    fn driver_without_native_timestamp_query_stops_probing() {
        let mut model = QueueFeedback::default();
        for id in 1..=6 {
            model.accepted(Some(id), ComputePath::Ordered, 1024, None);
            model.observe_gpu_sample(gpu(id, 2_000));
        }
        for id in 20..20 + MAX_UNOBSERVED_NATIVE_PROBES {
            model.accepted(Some(u64::from(id)), ComputePath::Native, 1024, None);
        }
        for _ in 0..64 {
            assert!(!model.admit_native(true, true, 1024));
        }
    }

    #[test]
    fn synthetic_gpu_timestamps_never_learn_from_host_fence_duration() {
        let mut model = QueueFeedback::default();
        model.accepted(Some(77), ComputePath::Native, 512, None);
        model.observe_gpu_sample(GpuTimestampSample {
            tag: GpuTimestampTag {
                batch_id: 77, stage: GpuTimestampStage::GeneratedMesh,
            },
            duration_ticks: 42, frequency_hz: 0,
        });
        assert_eq!(model.native.samples, 0);
        model.observe_gpu_sample(gpu(77, 640));
        assert_eq!(model.native.samples, 1);
        assert_eq!(model.observed_handoff_pressure_ns, 0);
    }
}
