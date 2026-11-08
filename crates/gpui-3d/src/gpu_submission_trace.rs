//! GPU-owner phase diagnostics: HOST API and command-encoding durations.
//!
//! These counters intentionally do not call them GPU execution times.
//! Direct3D/Vulkan command submission can return before the GPU executes.
//! Only completed frame GPU timestamps can establish hardware elapsed time.
//! Aggregation is lock-free and never waits for a device fence on the hot path.

use gfx_core::BackendKind;
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};
use std::time::Duration;

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    GeneratedInputUpload,
    GeneratedComputeIssue,
    IndirectMapUpload,
    IndirectMapComputeIssue,
    CullComputeIssue,
    DrawIndirectEncode,
    OffscreenEnqueue,
    DrawTableUpload,
    BindGroupSync,
}

static NEXT_SECTION_BATCH: AtomicU64 = AtomicU64::new(1);

/// One stable identity per published Section update, shared across Compute,
/// resident uploads and the native draw consumer for later correlation.
pub(crate) fn next_section_batch() -> u64 {
    NEXT_SECTION_BATCH.fetch_add(1, Ordering::Relaxed)
}

/// Completed GPU execution durations, never host API timings. Values are
/// emitted with batch_id so an individual Section publication can be traced
/// through every native submission without synchronizing the graphics owner.
pub(crate) fn report_gpu_samples(
    backend: BackendKind,
    samples: &[gfx_core::GpuTimestampSample],
) {
    if !enabled() { return; }
    for sample in samples {
        let Some(elapsed) = sample.duration() else { continue; };
        eprintln!(
            "gpui3d gpu_hardware_phase backend={:?} batch_id={} stage={:?} gpu_us={} gpu_ticks={} frequency_hz={} timing=completed_gpu_timestamp",
            backend, sample.tag.batch_id, sample.tag.stage,
            elapsed.as_micros(), sample.duration_ticks, sample.frequency_hz,
        );
    }
}

const PHASES: usize = 9;
const BACKENDS: usize = 6;
struct Accumulator {
    samples: AtomicU64,
    items: AtomicU64,
    bytes: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
}
impl Accumulator {
    const fn new() -> Self {
        Self {
            samples: AtomicU64::new(0),
            items: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
        }
    }
}
static COUNTERS: [Accumulator; PHASES * BACKENDS] =
    [const { Accumulator::new() }; PHASES * BACKENDS];

const fn backend_index(backend: BackendKind) -> usize {
    match backend {
        BackendKind::Vulkan => 0,
        BackendKind::Dx11 => 1,
        BackendKind::Dx12 => 2,
        BackendKind::Metal => 3,
        BackendKind::OpenGl => 4,
        BackendKind::WebGl => 5,
    }
}
const fn phase_name(index: usize) -> &'static str {
    match index {
        0 => "generated_input_upload",
        1 => "generated_compute_issue",
        2 => "indirect_map_upload",
        3 => "indirect_map_compute_issue",
        4 => "cull_compute_issue",
        5 => "draw_indirect_encode",
        6 => "offscreen_enqueue",
        7 => "draw_table_upload",
        _ => "bind_group_sync",
    }
}

/// Enabled in dev builds without environment variables, or explicitly in
/// release diagnostics; no per-dispatch clock reads in ordinary release.
pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        cfg!(debug_assertions) || std::env::var_os("GPUI_3D_TRACE_TIMINGS").is_some()
    })
}

/// Measures a successful API call or host command-build operation only.
pub(crate) fn observe(
    backend: BackendKind,
    phase: Phase,
    count: usize,
    bytes: usize,
    elapsed: Duration,
) {
    if !enabled() { return; }
    let entry = &COUNTERS[backend_index(backend) * PHASES + phase as usize];
    let micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
    entry.samples.fetch_add(1, Ordering::Relaxed);
    entry.items.fetch_add(count as u64, Ordering::Relaxed);
    entry.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    entry.total_us.fetch_add(micros, Ordering::Relaxed);
    entry.max_us.fetch_max(micros, Ordering::Relaxed);
}

/// Called periodically by the GPU-owner renderer. Samples are attributed to
/// the correct API backend; a second window on that backend shares a total.
pub(crate) fn report(backend: BackendKind) {
    if !enabled() { return; }
    for index in 0..PHASES {
        let entry = &COUNTERS[backend_index(backend) * PHASES + index];
        let samples = entry.samples.swap(0, Ordering::Relaxed);
        if samples == 0 { continue; }
        let items = entry.items.swap(0, Ordering::Relaxed);
        let bytes = entry.bytes.swap(0, Ordering::Relaxed);
        let us = entry.total_us.swap(0, Ordering::Relaxed);
        let max = entry.max_us.swap(0, Ordering::Relaxed);
        eprintln!(
            "gpui3d gpu_owner_phase backend={:?} phase={} samples={} items={} bytes={} host_avg_us={} host_max_us={} timing=host_api_not_gpu_execution",
            backend, phase_name(index), samples, items, bytes, us / samples, max,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_labels_are_distinct_and_complete() {
        let mut names = std::collections::HashSet::new();
        for index in 0..PHASES {
            assert!(names.insert(phase_name(index)));
        }
        assert_eq!(names.len(), PHASES);
    }
    #[test]
    fn counters_are_isolated_by_backend() {
        assert_ne!(backend_index(BackendKind::Dx11), backend_index(BackendKind::Vulkan));
        assert_ne!(backend_index(BackendKind::Dx12), backend_index(BackendKind::OpenGl));
    }
}
