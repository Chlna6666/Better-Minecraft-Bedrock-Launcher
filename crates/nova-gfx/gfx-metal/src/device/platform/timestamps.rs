//! Bounded native GPU timestamp pairs. A tagged batch retains its query
//! resources until the owning command buffer reaches a terminal state.
use super::*;
use gfx_core::{GpuTimestampSample, GpuTimestampTag};
use objc2_foundation::NSRange;
use objc2_metal::{MTLCommonCounterSetTimestamp, MTLCounterSet, MTLCounterSampleBuffer,
    MTLCounterSampleBufferDescriptor, MTLCounterSamplingPoint, MTLComputePassDescriptor,
    MTLComputeCommandEncoder};

const MAX_PENDING: usize = 8;
const MAX_RANGES: usize = 128;

#[derive(Clone, Copy)]
pub(super) struct TaggedRange {
    pub first: usize,
    pub last: usize,
    pub tag: GpuTimestampTag,
}

struct QueryStorage {
    samples: Retained<ProtocolObject<dyn MTLCounterSampleBuffer>>,
    results: Retained<ProtocolObject<dyn MTLBuffer>>,
    capacity: usize,
}

pub(super) struct Probe {
    storage: QueryStorage,
    ranges: Vec<TaggedRange>,
    stage_boundary: bool,
    calibration: (u64, u64),
}

pub(super) struct WorkTimestamps {
    counter_set: Option<Retained<ProtocolObject<dyn MTLCounterSet>>>,
    stage_boundary: bool,
    dispatch_boundary: bool,
    draw_boundary: bool,
    pending: Vec<(Probe, Retained<ProtocolObject<dyn MTLCommandBuffer>>)>,
    free: Vec<QueryStorage>,
}

#[repr(C)]
struct Timebase { numer: u32, denom: u32 }
unsafe extern "C" { fn mach_timebase_info(info: *mut Timebase) -> i32; }

fn clock_pair(device: &ProtocolObject<dyn MTLDevice>) -> (u64, u64) {
    let (mut cpu, mut gpu) = (0u64, 0u64);
    // SAFETY: both outputs are live and distinct.
    unsafe { device.sampleTimestamps_gpuTimestamp(NonNull::from(&mut cpu), NonNull::from(&mut gpu)); }
    (cpu, gpu)
}

fn calibrated_frequency(start: (u64, u64), end: (u64, u64)) -> Option<u64> {
    let cpu = end.0.checked_sub(start.0)?;
    let gpu = end.1.checked_sub(start.1)?;
    if cpu == 0 || gpu == 0 { return None; }
    let mut scale = Timebase { numer: 0, denom: 0 };
    // SAFETY: Mach writes the local timebase structure.
    if unsafe { mach_timebase_info(&mut scale) } != 0 || scale.numer == 0 || scale.denom == 0 { return None; }
    let frequency = u128::from(gpu) * 1_000_000_000 * u128::from(scale.denom)
        / (u128::from(cpu) * u128::from(scale.numer));
    u64::try_from(frequency).ok().filter(|v| *v != 0)
}

impl WorkTimestamps {
    pub(super) fn new(device: &ProtocolObject<dyn MTLDevice>) -> Self {
        let stage_boundary = device.supportsCounterSampling(MTLCounterSamplingPoint::AtStageBoundary);
        let dispatch_boundary = device.supportsCounterSampling(MTLCounterSamplingPoint::AtDispatchBoundary);
        let draw_boundary = device.supportsCounterSampling(MTLCounterSamplingPoint::AtDrawBoundary);
        let counter_set = if stage_boundary || dispatch_boundary || draw_boundary {
            device.counterSets().and_then(|sets| {
                (0..sets.count()).find_map(|index| {
                    let set = sets.objectAtIndex(index);
                    // SAFETY: Metal exports this constant for its timestamp set.
                    (&*set.name() == unsafe { MTLCommonCounterSetTimestamp }).then_some(set)
                })
            })
        } else { None };
        Self { counter_set, stage_boundary, dispatch_boundary, draw_boundary, pending: Vec::new(), free: Vec::new() }
    }
    pub(super) fn supported(&self) -> bool { self.counter_set.is_some() }

    pub(super) fn acquire_compute(&mut self, device: &ProtocolObject<dyn MTLDevice>, ranges: &[TaggedRange]) -> Option<Probe> {
        if !self.stage_boundary && !self.dispatch_boundary { return None; }
        self.acquire(device, ranges)
    }

    pub(super) fn acquire_render(&mut self, device: &ProtocolObject<dyn MTLDevice>, ranges: &[TaggedRange]) -> Option<Probe> {
        if !self.stage_boundary && !self.draw_boundary { return None; }
        self.acquire(device, ranges)
    }

    fn acquire(&mut self, device: &ProtocolObject<dyn MTLDevice>, ranges: &[TaggedRange]) -> Option<Probe> {
        if ranges.is_empty() || ranges.len() > MAX_RANGES || self.pending.len() >= MAX_PENDING { return None; }
        let set = self.counter_set.as_ref()?;
        let count = ranges.len() * 2;
        let storage = if let Some(index) = self.free.iter().position(|item| item.capacity >= count) {
            self.free.swap_remove(index)
        } else {
            let capacity = count.next_power_of_two();
            let desc = MTLCounterSampleBufferDescriptor::new();
            desc.setCounterSet(Some(set));
            desc.setStorageMode(MTLStorageMode::Private);
            // SAFETY: bounded positive capacity.
            unsafe { desc.setSampleCount(capacity); }
            let samples = device.newCounterSampleBufferWithDescriptor_error(&desc).ok()?;
            let results = device.newBufferWithLength_options(capacity * 8, MTLResourceOptions::StorageModeShared)?;
            QueryStorage { samples, results, capacity }
        };
        // Invalidate old data before reuse, so a driver returning no sample
        // cannot accidentally publish a previous frame's result.
        unsafe { std::ptr::write_bytes(storage.results.contents().as_ptr().cast::<u8>(), 0, count * 8); }
        Some(Probe { storage, ranges: ranges.to_vec(), stage_boundary: self.stage_boundary, calibration: clock_pair(device) })
    }

    pub(super) fn submitted(&mut self, probe: Probe, commands: Retained<ProtocolObject<dyn MTLCommandBuffer>>) {
        self.pending.push((probe, commands));
    }

    pub(super) fn drain(&mut self, device: &ProtocolObject<dyn MTLDevice>, output: &mut Vec<GpuTimestampSample>) {
        let mut index = 0;
        while index < self.pending.len() {
            let status = self.pending[index].1.status();
            if !matches!(status, MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error) {
                index += 1;
                continue;
            }
            let (probe, _) = self.pending.swap_remove(index);
            if status == MTLCommandBufferStatus::Completed {
                if let Some(frequency_hz) = calibrated_frequency(probe.calibration, clock_pair(device)) {
                    // SAFETY: the command buffer completed, the shared
                    // resolve allocation contains two u64s per tagged range.
                    let values = unsafe { std::slice::from_raw_parts(probe.storage.results.contents().as_ptr().cast::<u64>(), probe.ranges.len() * 2) };
                    for (range, pair) in probe.ranges.iter().zip(values.chunks_exact(2)) {
                        if pair[0] != 0 && pair[0] != u64::MAX && pair[1] != u64::MAX && pair[1] > pair[0] {
                            output.push(GpuTimestampSample { tag: range.tag, duration_ticks: pair[1] - pair[0], frequency_hz });
                        }
                    }
                }
            }
            if self.free.len() < MAX_PENDING { self.free.push(probe.storage); }
        }
    }
}

impl Probe {
    fn index(&self, dispatch: usize, begin: bool) -> Option<usize> {
        self.ranges.iter().position(|range| if begin { range.first == dispatch } else { range.last == dispatch })
            .map(|index| index * 2 + usize::from(!begin))
    }

    pub(super) fn compute_encoder(&self, commands: &ProtocolObject<dyn MTLCommandBuffer>, index: usize)
        -> Option<Retained<ProtocolObject<dyn MTLComputeCommandEncoder>>> {
        let (start, end) = (self.index(index, true), self.index(index, false));
        if !self.stage_boundary || (start.is_none() && end.is_none()) { return commands.computeCommandEncoder(); }
        let desc = MTLComputePassDescriptor::new();
        // SAFETY: attachment zero exists; indices are bounded by capacity
        // or use Metal's documented don't-sample sentinel.
        unsafe {
            let attachment = desc.sampleBufferAttachments().objectAtIndexedSubscript(0);
            attachment.setSampleBuffer(Some(&self.storage.samples));
            attachment.setStartOfEncoderSampleIndex(start.unwrap_or(usize::MAX));
            attachment.setEndOfEncoderSampleIndex(end.unwrap_or(usize::MAX));
        }
        commands.computeCommandEncoderWithDescriptor(&desc)
    }

    pub(super) fn sample_dispatch(&self, encoder: &ProtocolObject<dyn MTLComputeCommandEncoder>, dispatch: usize, begin: bool) {
        if self.stage_boundary { return; }
        if let Some(index) = self.index(dispatch, begin) {
            // SAFETY: device supports dispatch sampling and index is valid.
            unsafe { encoder.sampleCountersInBuffer_atSampleIndex_withBarrier(&self.storage.samples, index, true); }
        }
    }

    pub(super) fn configure_render(&self, descriptor: &MTLRenderPassDescriptor, pass: usize) {
        if !self.stage_boundary { return; }
        let (start, end) = (self.index(pass, true), self.index(pass, false));
        if start.is_none() && end.is_none() { return; }
        // SAFETY: attachment zero exists; all sample indices are within the
        // retained allocation or use the native don't-sample sentinel.
        unsafe {
            let attachment = descriptor.sampleBufferAttachments().objectAtIndexedSubscript(0);
            attachment.setSampleBuffer(Some(&self.storage.samples));
            attachment.setStartOfVertexSampleIndex(start.unwrap_or(usize::MAX));
            attachment.setEndOfVertexSampleIndex(usize::MAX);
            attachment.setStartOfFragmentSampleIndex(usize::MAX);
            attachment.setEndOfFragmentSampleIndex(end.unwrap_or(usize::MAX));
        }
    }

    pub(super) fn sample_render(&self, encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, pass: usize, begin: bool) {
        if self.stage_boundary { return; }
        if let Some(index) = self.index(pass, begin) {
            // SAFETY: acquire_render checked draw-boundary support.
            unsafe { encoder.sampleCountersInBuffer_atSampleIndex_withBarrier(&self.storage.samples, index, true); }
        }
    }

    pub(super) fn resolve(&self, commands: &ProtocolObject<dyn MTLCommandBuffer>) -> Result<()> {
        let blit = commands.blitCommandEncoder().ok_or_else(|| Error::Backend("Metal timestamp resolve encoder unavailable".into()))?;
        // SAFETY: native timestamp records are u64, range and destination
        // fit their retained allocations, destination offset is aligned.
        unsafe { blit.resolveCounters_inRange_destinationBuffer_destinationOffset(
            &self.storage.samples, NSRange { location: 0, length: self.ranges.len() * 2 }, &self.storage.results, 0); }
        blit.endEncoding();
        Ok(())
    }
}
