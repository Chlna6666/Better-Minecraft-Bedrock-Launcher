//! Lock-free GPU-owner buffer upload feedback for streaming admission.
//!
//! These measurements span native ExtensionDevice::write_buffer(_batch)
//! calls on the GPU owner, including driver staging/submission. They are NOT
//! fence-completed device execution times. The stream manager only uses them
//! to forecast its next submission cost; missing samples retain the old
//! conservative byte budget.

use gfx_core::{BackendKind, GpuTimestampSample, GpuTimestampStage};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, atomic::{AtomicU64, AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

const MIB: u128 = 1024 * 1024;
const DEFAULT_NS_PER_MIB: u64 = 500_000;
const DEFAULT_NS_PER_WRITE: u64 = 20_000;
const MODEL_COUNT: usize = 6;

struct Model {
    samples: AtomicU64,
    bytes: AtomicU64,
    writes: AtomicU64,
    submit_ns: AtomicU64,
    ns_per_mib: AtomicU64,
    ns_per_write: AtomicU64,
    hardware_samples: AtomicU64,
    hardware_upload_ns: AtomicU64,
    hardware_ns_per_mib: AtomicU64,
}

impl Model {
    const fn new() -> Self {
        Self {
            samples: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            submit_ns: AtomicU64::new(0),
            ns_per_mib: AtomicU64::new(DEFAULT_NS_PER_MIB),
            ns_per_write: AtomicU64::new(DEFAULT_NS_PER_WRITE),
            hardware_samples: AtomicU64::new(0),
            hardware_upload_ns: AtomicU64::new(0),
            hardware_ns_per_mib: AtomicU64::new(0),
        }
    }
}

static MODELS: [Model; MODEL_COUNT] = [const { Model::new() }; MODEL_COUNT];
static LAST_BACKEND: AtomicUsize = AtomicUsize::new(MODEL_COUNT);

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

const fn backend_at(index: usize) -> BackendKind {
    match index {
        0 => BackendKind::Vulkan,
        1 => BackendKind::Dx11,
        2 => BackendKind::Dx12,
        3 => BackendKind::Metal,
        4 => BackendKind::OpenGl,
        _ => BackendKind::WebGl,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GpuUploadCostEstimate {
    pub backend: BackendKind,
    pub samples: u64,
    pub measured_bytes: u64,
    pub measured_writes: u64,
    pub measured_submit_us: u64,
    pub ns_per_mib: u64,
    pub ns_per_write: u64,
    /// Completed, batch-correlated native GPU upload samples. These are
    /// separate from the GPU-owner/CPU submission samples above.
    pub hardware_samples: u64,
    pub measured_hardware_upload_us: u64,
    pub hardware_ns_per_mib: u64,
}

impl GpuUploadCostEstimate {
    /// Predicted GPU-owner API submission time, not device completion time.
    /// The per-write term penalizes thousands of tiny, fragmented patches.
    pub fn predicted_submit_us(self, bytes: usize, writes: usize) -> u64 {
        let nanoseconds = ((bytes as u128)
            .saturating_mul(u128::from(self.ns_per_mib)) / MIB)
            .saturating_add((writes as u128).saturating_mul(u128::from(self.ns_per_write)));
        (nanoseconds.saturating_add(999) / 1000).min(u128::from(u64::MAX)) as u64
    }

    /// GPU execution estimate; None means not enough completed hardware probes
    /// to safely train on this adapter. Do not substitute host API timings.
    pub fn predicted_hardware_us(self, bytes: usize) -> Option<u64> {
        if self.hardware_samples < 8 || self.hardware_ns_per_mib == 0 {
            return None;
        }
        let nanos = (bytes as u128).saturating_mul(u128::from(self.hardware_ns_per_mib)) / MIB;
        Some((nanos.saturating_add(999) / 1000).min(u128::from(u64::MAX)) as u64)
    }
}

/// Last active 3D backend. Sampling is disabled until its first valid owner
/// upload; an empty or failed GPU write never trains the estimator.
pub fn gpu_upload_cost_estimate() -> Option<GpuUploadCostEstimate> {
    let index = LAST_BACKEND.load(Ordering::Relaxed);
    let model = MODELS.get(index)?;
    let samples = model.samples.load(Ordering::Relaxed);
    (samples != 0).then(|| GpuUploadCostEstimate {
        backend: backend_at(index),
        samples,
        measured_bytes: model.bytes.load(Ordering::Relaxed),
        measured_writes: model.writes.load(Ordering::Relaxed),
        measured_submit_us: model.submit_ns.load(Ordering::Relaxed) / 1_000,
        ns_per_mib: model.ns_per_mib.load(Ordering::Relaxed),
        ns_per_write: model.ns_per_write.load(Ordering::Relaxed),
        hardware_samples: model.hardware_samples.load(Ordering::Relaxed),
        measured_hardware_upload_us: model.hardware_upload_ns.load(Ordering::Relaxed) / 1000,
        hardware_ns_per_mib: model.hardware_ns_per_mib.load(Ordering::Relaxed),
    })
}

fn smooth(previous: u64, sample: u64) -> u64 {
    ((u128::from(previous).saturating_mul(7) + u128::from(sample)) / 8)
        .min(u128::from(u64::MAX)) as u64
}

pub(crate) fn observe_buffer_upload(
    backend: BackendKind,
    bytes: usize,
    writes: usize,
    elapsed: Duration,
) {
    if bytes == 0 || writes == 0 {
        return;
    }
    let index = backend_index(backend);
    let model = &MODELS[index];
    let elapsed_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX).max(1);
    let prior_fixed = model.ns_per_write.load(Ordering::Relaxed);
    let prior_rate = model.ns_per_mib.load(Ordering::Relaxed);
    let size = bytes as u128;
    let operations = writes as u128;

    // Learn the transfer slope from large batches and the syscall/staging
    // overhead from small batches; do not fit two noisy coefficients to one
    // measurement or let a tiny record dominate throughput estimation.
    if bytes >= 256 * 1024 {
        let adjusted = u128::from(elapsed_ns)
            .saturating_sub(u128::from(prior_fixed).saturating_mul(operations));
        let observed = ((adjusted.saturating_mul(MIB)) / size)
            .clamp(1_000, 200_000_000) as u64;
        model.ns_per_mib.store(smooth(prior_rate, observed), Ordering::Relaxed);
    } else {
        let estimated_data = size.saturating_mul(u128::from(prior_rate)) / MIB;
        let adjusted = u128::from(elapsed_ns).saturating_sub(estimated_data);
        let observed = (adjusted / operations).clamp(1_000, 20_000_000) as u64;
        model.ns_per_write.store(smooth(prior_fixed, observed), Ordering::Relaxed);
    }
    model.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    model.writes.fetch_add(writes as u64, Ordering::Relaxed);
    model.submit_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
    model.samples.fetch_add(1, Ordering::Relaxed);
    LAST_BACKEND.store(index, Ordering::Relaxed);
}

/// One submission group may contain several native buffer-copy calls.
    /// Accept a hardware rate ONLY after every expected tagged call has
    /// produced a completed timestamp. Missing/unsupported samples expire
    /// without corrupting the cost model.
#[derive(Clone, Copy)]
struct TimedBatch {
    bytes: u64,
    writes: u64,
    expected: u32,
    completed: u32,
    gpu_ns: u128,
    updated: Instant,
}

static PENDING_HARDWARE: OnceLock<Mutex<HashMap<(usize, u64), TimedBatch>>> = OnceLock::new();

fn pending_hardware() -> &'static Mutex<HashMap<(usize, u64), TimedBatch>> {
    PENDING_HARDWARE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register one successful profiled native buffer upload against its Section
/// batch. No GPU wait; this runs after the normal upload API returns.
pub(crate) fn observe_tagged_upload(
    backend: BackendKind,
    batch_id: u64,
    bytes: usize,
    writes: usize,
) {
    if batch_id == 0 || bytes == 0 || writes == 0 {
        return;
    }
    let Ok(mut pending) = pending_hardware().lock() else { return; };
    let now = Instant::now();
    if pending.len() >= 256 {
        pending.retain(|_, record| now.saturating_duration_since(record.updated) < Duration::from_secs(15));
        // Missing native probes or a device loss must not leak an unbounded
        // backlog of stale Section publications.
        if pending.len() >= 1024 {
            pending.clear();
        }
    }
    let entry = pending.entry((backend_index(backend), batch_id)).or_insert(TimedBatch {
        bytes: 0, writes: 0, expected: 0, completed: 0, gpu_ns: 0, updated: now,
    });
    entry.bytes = entry.bytes.saturating_add(bytes as u64);
    entry.writes = entry.writes.saturating_add(writes as u64);
    entry.expected = entry.expected.saturating_add(1);
    entry.updated = now;
}

/// Match fence-completed native hardware ticks to the original upload batch.
/// A partial batch cannot train the per-byte rate.
pub(crate) fn observe_completed_gpu_upload(backend: BackendKind, sample: GpuTimestampSample) {
    if sample.tag.stage != GpuTimestampStage::BufferUpload {
        return;
    }
    let Some(duration) = sample.duration() else { return; };
    let nanos = duration.as_nanos();
    if nanos == 0 { return; }
    let index = backend_index(backend);
    let Ok(mut pending) = pending_hardware().lock() else { return; };
    let key = (index, sample.tag.batch_id);
    let Some(record) = pending.get_mut(&key) else { return; };
    record.gpu_ns = record.gpu_ns.saturating_add(nanos);
    record.completed = record.completed.saturating_add(1);
    if record.completed != record.expected {
        return;
    }
    let record = pending.remove(&key).expect("completed hardware batch");
    drop(pending);
    // Tiny patch timings are often below query clock resolution and should
    // not dominate a per-MiB throughput estimate.
    if record.bytes < 64 * 1024 {
        return;
    }
    let Some(rate) = normalized_gpu_rate(record.gpu_ns, record.bytes) else { return; };
    let model = &MODELS[index];
    let previous_samples = model.hardware_samples.fetch_add(1, Ordering::Relaxed);
    let previous = model.hardware_ns_per_mib.load(Ordering::Relaxed);
    model.hardware_ns_per_mib.store(
        if previous_samples == 0 { rate } else { smooth(previous, rate) },
        Ordering::Relaxed,
    );
    model.hardware_upload_ns.fetch_add(
        u64::try_from(record.gpu_ns).unwrap_or(u64::MAX), Ordering::Relaxed,
    );
}

fn normalized_gpu_rate(nanos: u128, bytes: u64) -> Option<u64> {
    if bytes == 0 || nanos == 0 { return None; }
    Some((nanos.saturating_mul(MIB) / u128::from(bytes))
        .clamp(1_000, 200_000_000) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicted_upload_cost_includes_fragmentation() {
        let stats = GpuUploadCostEstimate {
            backend: BackendKind::Vulkan,
            samples: 10,
            measured_bytes: 0,
            measured_writes: 0,
            measured_submit_us: 0,
            ns_per_mib: 1_000_000,
            ns_per_write: 10_000,
            hardware_samples: 0,
            measured_hardware_upload_us: 0,
            hardware_ns_per_mib: 0,
        };
        assert_eq!(stats.predicted_submit_us(1024 * 1024, 4), 1040);
        assert_eq!(stats.predicted_submit_us(1024 * 1024, 100), 2000);
        assert_eq!(stats.predicted_submit_us(0, 0), 0);
    }

    #[test]
    fn hardware_cost_requires_completed_samples() {
        let mut cost = GpuUploadCostEstimate {
            backend: BackendKind::Vulkan, samples: 10, measured_bytes: 0,
            measured_writes: 0, measured_submit_us: 0,
            ns_per_mib: 1_000_000, ns_per_write: 20_000,
            hardware_samples: 7, measured_hardware_upload_us: 0,
            hardware_ns_per_mib: 2_000_000,
        };
        assert_eq!(cost.predicted_hardware_us(1024 * 1024), None);
        cost.hardware_samples = 8;
        assert_eq!(cost.predicted_hardware_us(1024 * 1024), Some(2000));
        assert_eq!(normalized_gpu_rate(1_000_000, 1024 * 1024), Some(1_000_000));
    }

    #[test]
    fn smoothing_never_exceeds_sample_range() {
        assert_eq!(smooth(80, 0), 70);
        assert_eq!(smooth(80, 160), 90);
    }
}
