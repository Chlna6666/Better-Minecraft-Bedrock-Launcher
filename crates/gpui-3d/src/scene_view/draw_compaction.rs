//! Opt-in ordered GPU argument compaction for long identical-state runs.
//! Preserves exact command ordering (including blending) and first_instance.
//! No readback, per-frame native allocation, or CPU fence waits.
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext};
use crate::NativeOffscreenPass;
use std::{collections::{HashMap, HashSet}, sync::Arc, time::{Duration, Instant}};
use super::SceneViewId;

const MAX_RUN: usize = 256;
const MIN_RUN: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    command: usize,
    // Index into the immutable gather list, not into the argument buffer.
    source: u32,
    count: u32,
    destination: u32,
}

struct Plan {
    indexed: bool,
    source: BufferId,
    source_bytes: u64,
    records: u32,
    runs: Vec<Run>,
    gather: Vec<u32>,
}

fn indirect_record(command: &RenderCommand, indexed: bool) -> Option<(BufferId, u64, Option<IndexBufferBinding>)> {
    match command {
        RenderCommand::DrawIndirect(draw) if !indexed => Some((draw.arguments, draw.offset, None)),
        RenderCommand::DrawIndexedIndirect(draw) if indexed => Some((draw.arguments, draw.offset, Some(draw.index_buffer))),
        _ => None,
    }
}

fn plan(commands: &[RenderCommand], limit: u32) -> Option<Plan> {
    let max = MAX_RUN.min(limit as usize);
    if max < MIN_RUN || commands.len() < MIN_RUN { return None; }
    // One source ABI per plan. Mixed passes retain their original indexed
    // commands when the first command selects non-indexed compaction.
    let indexed = matches!(commands.first(), Some(RenderCommand::DrawIndexedIndirect(_)));
    let stride = if indexed { 20 } else { 16 };
    let mut source = None;
    let mut source_bytes = 0;
    let mut records = 0u32;
    let mut runs = Vec::new();
    let mut gather = Vec::new();
    let mut index = 0;
    while index < commands.len() {
        let first = &commands[index];
        let Some((arguments, offset, index_buffer)) = indirect_record(first, indexed) else {
            index += 1;
            continue;
        };
        if offset % stride != 0 || source.is_some_and(|id| id != arguments) {
            index += 1;
            continue;
        }
        let mut len = 1usize;
        while len < max {
            let Some(next) = commands.get(index + len) else { break; };
            let Some((next_arguments, next_offset, next_index_buffer)) = indirect_record(next, indexed) else { break; };
            if next_arguments != arguments || next_index_buffer != index_buffer
                || next.pipeline() != first.pipeline() || next.bind_groups() != first.bind_groups()
                || next.scissor() != first.scissor() || next_offset % stride != 0
            { break; }
            len += 1;
        }
        if len >= MIN_RUN {
            let start = records;
            records = records.checked_add(len as u32)?;
            source = Some(arguments);
            for command in &commands[index..index + len] {
                let (_, offset, _) = indirect_record(command, indexed)?;
                let slot = u32::try_from(offset / stride).ok()?;
                // Shader record addressing uses u32 words, not byte offsets.
                if (u64::from(slot) + 1) * (stride / 4) > u64::from(u32::MAX) { return None; }
                source_bytes = source_bytes.max((u64::from(slot) + 1) * stride);
                gather.push(slot);
            }
            runs.push(Run { command: index, source: start, count: len as u32, destination: start });
        }
        index += len;
    }
    Some(Plan { indexed, source: source?, source_bytes, records, runs, gather })
}

struct Scratch {
    buffers: [BufferId; 3],
    sizes: [u64; 3],
    // Exact bytes committed to this allocation, never to a prior generation.
    metadata: Vec<u8>,
}

fn metadata_ranges(previous: &[u8], next: &[u8]) -> Vec<std::ops::Range<usize>> {
    super::uploads::changed_ranges(previous, next, 4, 16, 4)
}

// Gather patches arrive in command order, which is also the order of their
// metadata words. Coalesce adjacent changes without copying the unchanged tail.
// Validate all offsets before publishing any writes.
fn metadata_patch_runs(
    metadata: &[u8], patches: &[(usize, u32)],
) -> Option<Vec<(usize, Vec<u8>)>> {
    let mut runs: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut previous_end = 0;
    for &(word, slot) in patches {
        let offset = word.checked_mul(4)?;
        let end = offset.checked_add(4)?;
        let old = metadata.get(offset..end)?;
        if offset < previous_end { return None; }
        previous_end = end;
        let bytes = slot.to_le_bytes();
        if old == bytes.as_slice() { continue; }
        if let Some((start, data)) = runs.last_mut() {
            if start.checked_add(data.len()) == Some(offset) {
                data.extend_from_slice(&bytes);
                continue;
            }
        }
        runs.push((offset, bytes.to_vec()));
    }
    Some(runs)
}

impl Scratch {
    fn patch_metadata(&mut self, device: &mut dyn ExtensionDevice, patches: &[(usize, u32)]) -> Result<bool> {
        let Some(runs) = metadata_patch_runs(&self.metadata, patches) else { return Ok(false); };
        if runs.is_empty() { return Ok(true); }
        let writes: Vec<_> = runs.iter().map(|(offset, data)| BufferWrite {
            descriptor: BufferWriteDescriptor { buffer: self.buffers[0], offset: *offset as u64 },
            data: data.as_slice(),
        }).collect();
        if let Err(error) = device.write_buffer_batch(&writes) {
            // Partial acceptance invalidates the complete CPU shadow.
            self.metadata.clear();
            return Err(error);
        }
        for (offset, data) in runs {
            self.metadata[offset..offset + data.len()].copy_from_slice(&data);
        }
        Ok(true)
    }

    fn upload_metadata(&mut self, device: &mut dyn ExtensionDevice, next: Vec<u8>) -> Result<()> {
        let ranges = metadata_ranges(&self.metadata, &next);
        if !ranges.is_empty() {
            let writes: Vec<_> = ranges.into_iter().map(|range| BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: self.buffers[0],
                    offset: range.start as u64,
                },
                data: &next[range],
            }).collect();
            if let Err(error) = device.write_buffer_batch(&writes) {
                // A backend may have accepted a prefix of the batch. Forget
                // the proof rather than comparing against stale CPU bytes.
                self.metadata.clear();
                return Err(error);
            }
        }
        self.metadata = next;
        Ok(())
    }
}

struct Prepared {
    indexed: bool,
    commands: Vec<RenderCommand>,
    counts: Vec<IndirectCountBinding>,
    compute: [ComputeBufferBinding; 4],
    groups: u32,
    runs: Vec<Run>,
}

// Strong Arc identity prevents address reuse and avoids hashing/scanning
// the immutable command list on a camera-only update. Each live view cache
// also retains the negative result for non-compaction-friendly scenes.
struct Cached {
    source: Arc<[RenderCommand]>,
    prepared: Option<Prepared>,
    cost: CostModel,
}

/// A uniform set-one replacement preserves every grouping equivalence
/// relation and indirect argument address. Check the full native identity
/// instead of trusting matching draw counts, a hash, or a queued GPU wait.
/// No allocations or GPU metadata uploads are needed for this comparison.
fn sampled_snapshot_replacement(
    previous: &[RenderCommand], next: &[RenderCommand],
) -> Option<(BindGroupId, BindGroupId)> {
    if previous.len() != next.len() { return None; }
    let [_, old] = previous.first()?.bind_groups() else { return None; };
    let [_, new] = next.first()?.bind_groups() else { return None; };
    if old == new { return None; }
    for (a, b) in previous.iter().zip(next) {
        let [geometry_a, table_a] = a.bind_groups() else { return None; };
        let [geometry_b, table_b] = b.bind_groups() else { return None; };
        if geometry_a != geometry_b || table_a != old || table_b != new
            || a.pipeline() != b.pipeline() || a.scissor() != b.scissor()
        { return None; }
        match (a, b) {
            (RenderCommand::DrawIndirect(a), RenderCommand::DrawIndirect(b))
                if a.arguments == b.arguments && a.offset == b.offset => {}
            (RenderCommand::DrawIndexedIndirect(a), RenderCommand::DrawIndexedIndirect(b))
                if a.arguments == b.arguments && a.offset == b.offset
                    && a.index_buffer == b.index_buffer => {}
            _ => return None,
        }
    }
    Some((*old, *new))
}

/// Preserve the existing run boundaries when only logical argument offsets
/// move inside compacted runs. Uncompacted draws must remain byte-equivalent
/// in native identity. A changed source buffer or out-of-range offset needs
/// a full plan, never a guessed binding range.
fn gather_patches(
    previous: &[RenderCommand], next: &[RenderCommand], runs: &[Run],
    indexed: bool, source_bytes: u64,
) -> Option<Vec<(usize, u32)>> {
    if previous.len() != next.len() { return None; }
    let stride = if indexed { 20 } else { 16 };
    let mut patches = Vec::new();
    let mut run_index = 0;
    for (index, (a, b)) in previous.iter().zip(next).enumerate() {
        if a.pipeline() != b.pipeline() || a.bind_groups() != b.bind_groups()
            || a.scissor() != b.scissor() { return None; }
        let (buffer_a, offset_a, index_a) = indirect_record(a, indexed)?;
        let (buffer_b, offset_b, index_b) = indirect_record(b, indexed)?;
        if buffer_a != buffer_b || index_a != index_b { return None; }
        while runs.get(run_index).is_some_and(|run| index >= run.command + run.count as usize) {
            run_index += 1;
        }
        if offset_a == offset_b { continue; }
        let run = runs.get(run_index).filter(|run| index >= run.command)?;
        if offset_b % stride != 0 || offset_b.checked_add(stride)? > source_bytes {
            return None;
        }
        let slot = u32::try_from(offset_b / stride).ok()?;
        if (u64::from(slot) + 1) * (stride / 4) > u64::from(u32::MAX) { return None; }
        let word = runs.len().checked_mul(4)?
            .checked_add(run.source as usize)?.checked_add(index - run.command)?;
        patches.push((word, slot));
    }
    Some(patches)
}

impl Cached {
    fn try_patch_gather(
        &mut self, device: &mut dyn ExtensionDevice, scratch: &mut Scratch,
        next: &Arc<[RenderCommand]>,
    ) -> Result<bool> {
        let Some(prepared) = &self.prepared else { return Ok(false); };
        let Some(patches) = gather_patches(
            &self.source, next, &prepared.runs, prepared.indexed, prepared.compute[0].buffer.size,
        ) else { return Ok(false); };
        if !patches.is_empty() {
            if !scratch.patch_metadata(device, &patches)? { return Ok(false); }
            // The gather population changed, so do not compare timings
            // measured for the old set of indirect arguments.
            self.cost = CostModel::default();
        }
        self.source = Arc::clone(next);
        Ok(true)
    }

    fn try_rebind_sampled_snapshot(&mut self, next: &Arc<[RenderCommand]>) -> bool {
        let Some((old, new)) = sampled_snapshot_replacement(&self.source, next) else { return false; };
        if let Some(prepared) = self.prepared.as_mut() {
            super::renderer::rebind_sampled_commands(&mut prepared.commands, old, new);
        }
        self.source = Arc::clone(next);
        // Keep the gather list and measured history. An outstanding probe
        // spans the old snapshot, so do not pair it with new draw timings.
        self.cost.pending = None;
        true
    }
}

const MIN_SAMPLES: u32 = 6;
const PROBE_INTERVAL: u32 = 32;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Estimate {
    ns: u64,
    samples: u32,
}
impl Estimate {
    fn observe(&mut self, ns: u64) {
        self.ns = if self.samples == 0 { ns } else {
            ((u128::from(self.ns) * 7 + u128::from(ns)) / 8) as u64
        };
        self.samples = self.samples.saturating_add(1);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Path { Original, Compact }

struct Probe {
    id: u64,
    path: Path,
    started: Instant,
    compute: Option<u64>,
    draw: Option<u64>,
}

#[derive(Default)]
struct CostModel {
    original: Estimate,
    compact: Estimate,
    preferred_compact: bool,
    frames: u32,
    pending: Option<Probe>,
    missing: u8,
}

impl CostModel {
    fn choose(&mut self, now: Instant) -> (Path, bool) {
        if self.pending.as_ref().is_some_and(|p| now.saturating_duration_since(p.started) >= PROBE_TIMEOUT) {
            self.pending = None;
            self.missing = self.missing.saturating_add(1);
            self.preferred_compact = false;
        }
        self.frames = self.frames.wrapping_add(1);
        let trained = self.original.samples >= MIN_SAMPLES && self.compact.samples >= MIN_SAMPLES;
        let preferred = if trained && self.preferred_compact && self.missing == 0 { Path::Compact } else { Path::Original };
        if self.pending.is_some() { return (preferred, false); }
        if self.missing >= 3 && !self.frames.is_multiple_of(PROBE_INTERVAL * 4) {
            return (Path::Original, false);
        }
        if !trained {
            // Alternate completed samples from the exact same immutable
            // command snapshot; never compare different draw populations.
            return (if self.original.samples <= self.compact.samples { Path::Original } else { Path::Compact }, true);
        }
        if self.frames.is_multiple_of(PROBE_INTERVAL) {
            return (if preferred == Path::Original { Path::Compact } else { Path::Original }, true);
        }
        if self.frames % PROBE_INTERVAL == PROBE_INTERVAL / 2 {
            return (preferred, true);
        }
        (preferred, false)
    }

    fn begin(&mut self, path: Path, now: Instant) -> u64 {
        let id = crate::gpu_submission_trace::next_section_batch();
        self.pending = Some(Probe { id, path, started: now, compute: None, draw: None });
        id
    }

    fn observe(&mut self, sample: GpuTimestampSample) {
        let Some(probe) = self.pending.as_mut() else { return; };
        if probe.id != sample.tag.batch_id { return; }
        let Some(duration) = sample.duration() else { return; };
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        if ns == 0 { return; }
        match sample.tag.stage {
            GpuTimestampStage::DrawIndirect => probe.draw = Some(ns),
            GpuTimestampStage::DrawCompaction if probe.path == Path::Compact => probe.compute = Some(ns),
            _ => return,
        }
        let Some(draw) = probe.draw else { return; };
        match probe.path {
            Path::Original => self.original.observe(draw),
            Path::Compact => {
                let Some(compute) = probe.compute else { return; };
                self.compact.observe(draw.saturating_add(compute));
            }
        }
        self.pending = None;
        self.missing = 0;
        if self.original.samples >= MIN_SAMPLES && self.compact.samples >= MIN_SAMPLES {
            // Enter only with >=15% measured savings; exit below 5%.
            // Hysteresis keeps a narrow/noisy advantage from oscillating.
            let threshold = if self.preferred_compact { 95 } else { 85 };
            self.preferred_compact = u128::from(self.compact.ns) * 100
                < u128::from(self.original.ns) * threshold;
        }
    }
}

#[derive(Default)]
struct ViewCompactor {
    scratch: Option<Scratch>,
    cached: Option<Cached>,
}

pub(super) struct Compactor {
    shader: ShaderModuleId,
    indexed_shader: Option<ShaderModuleId>,
    argument_stride: u32,
    views: HashMap<SceneViewId, ViewCompactor>,
    probes: HashMap<u64, SceneViewId>,
}

#[derive(Clone, Copy)]
struct Decision {
    compact: bool,
    tag: Option<GpuTimestampTag>,
}

impl Compactor {
    pub(super) fn new(device: &mut dyn ExtensionDevice, context: &RendererExtensionContext) -> Option<Self> {
        // Unvalidated driver cost: preserve default behavior until benchmarked.
        let argument_stride = match context.backend_kind() {
            BackendKind::Vulkan | BackendKind::OpenGl | BackendKind::Metal => 16,
            BackendKind::Dx12 => 32,
            _ => return None,
        };
        if std::env::var_os("GPUI_3D_DRAW_COMPACTION").as_deref() != Some(std::ffi::OsStr::new("1"))
            || device.counted_offscreen_limit().is_none()
            || !device.gpu_work_timestamps_supported()
        { return None; }
        let caps = device.compute_capabilities();
        if caps.max_invocations_per_workgroup < 64 || caps.max_workgroup_size[0] < 64
            || caps.max_workgroup_shared_memory_bytes < 256
        { return None; }
        let shader = super::resources::embedded_shader(context, ShaderStage::Compute, "cs_compact_draws")
            .ok().and_then(|binary| device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d draw compaction".into()), binary,
            }).ok())?;
        let indexed_shader = super::resources::embedded_shader(context, ShaderStage::Compute, "cs_compact_indexed_draws")
            .ok().and_then(|binary| device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d indexed draw compaction".into()), binary,
            }).ok());
        Some(Self { shader, indexed_shader, argument_stride, views: HashMap::new(), probes: HashMap::new() })
    }

    pub(super) fn submit_batch(
        &mut self, device: &mut dyn ExtensionDevice, passes: &[NativeOffscreenPass],
        epochs: &[(SceneViewId, u64)],
    ) -> Result<Option<SubmissionId>> {
        let ordinary = |device: &mut dyn ExtensionDevice| crate::submit_native_offscreen_passes(
            passes, |passes, tags, tables| {
                if tables.iter().any(Option::is_some) {
                    device.submit_sampled_offscreen_batch(passes, tables, &[], tags)
                } else {
                    device.submit_offscreen_graphics(passes, tags, OffscreenQueuePreference::OrderedGraphics)
                }
            },
        );
        // Duplicate views could overwrite their own output before the first draw.
        if passes.len() != epochs.len() || epochs.iter().map(|(id, _)| *id).collect::<HashSet<_>>().len() != epochs.len() {
            return ordinary(device);
        }
        let mut decisions = Vec::with_capacity(passes.len());
        let mut special = false;
        for (pass, &(id, _)) in passes.iter().zip(epochs) {
            if pass.gpu_work_tag.is_some() {
                decisions.push(None);
                continue;
            }
            let view = self.views.entry(id).or_default();
            let previous_probe = view.pending_id();
            let decision = view.schedule(device, self.argument_stride, self.indexed_shader.is_some(), pass)?;
            let next_probe = view.pending_id();
            if previous_probe != next_probe {
                if let Some(old) = previous_probe { self.probes.remove(&old); }
                if let Some(new) = next_probe { self.probes.insert(new, id); }
            }
            special |= decision.is_some();
            decisions.push(decision);
        }
        if !special { return ordinary(device); }
        // All producers execute on the ordered primary queue. Keep an
        // independent timestamp tag per dispatch without splitting the batch.
        let mut dispatches = Vec::new();
        let mut compute_tags = Vec::new();
        for (&(id, _), decision) in epochs.iter().zip(&decisions) {
            let Some(choice) = (*decision).filter(|choice| choice.compact) else { continue; };
            let prepared = self.views[&id].cached.as_ref()
                .and_then(|cache| cache.prepared.as_ref()).expect("scheduled compaction plan");
            dispatches.push(ComputeDispatch {
                shader: if prepared.indexed { self.indexed_shader.expect("indexed compaction shader") } else { self.shader },
                bindings: &prepared.compute, workgroups: [prepared.groups, 1, 1],
            });
            compute_tags.push(choice.tag.map(|tag| GpuTimestampTag {
                batch_id: tag.batch_id, stage: GpuTimestampStage::DrawCompaction,
            }));
        }
        if !dispatches.is_empty() {
            device.dispatch_compute_batch_tagged(&dispatches, &compute_tags)?;
        }

        let mut native = Vec::with_capacity(passes.len());
        let mut tags = Vec::with_capacity(passes.len());
        let mut counts: Vec<&[IndirectCountBinding]> = Vec::with_capacity(passes.len());
        for ((pass, &(id, _)), decision) in passes.iter().zip(epochs).zip(decisions) {
            let mut descriptor = pass.commands();
            let mut table: &[IndirectCountBinding] = &[];
            if decision.is_some_and(|choice| choice.compact) {
                let prepared = self.views.get(&id).and_then(|view| view.cached.as_ref())
                    .and_then(|cache| cache.prepared.as_ref()).expect("scheduled compaction plan");
                descriptor.commands = &prepared.commands;
                table = &prepared.counts;
            }
            native.push(descriptor);
            counts.push(table);
            tags.push(decision.and_then(|choice| choice.tag).or(pass.gpu_work_tag));
        }
        if passes.iter().any(|pass| pass.sampled_table.is_some()) {
            let tables = passes.iter().map(|pass| pass.sampled_table).collect::<Vec<_>>();
            device.submit_sampled_offscreen_batch(&native, &tables, &counts, &tags)
        } else {
            device.submit_counted_offscreen_batch(&native, &counts, &tags)
        }
    }

    pub(super) fn observe(&mut self, sample: GpuTimestampSample) {
        let Some(&id) = self.probes.get(&sample.tag.batch_id) else { return; };
        if let Some(view) = self.views.get_mut(&id) {
            if let Some(cached) = view.cached.as_mut() {
                cached.cost.observe(sample);
            }
            if view.pending_id().is_some() { return; }
        }
        self.probes.remove(&sample.tag.batch_id);
    }

    pub(super) fn invalidate(&mut self) {
        self.probes.clear();
        for view in self.views.values_mut() { view.cached = None; }
    }

    pub(super) fn remove(&mut self, device: &mut dyn ExtensionDevice, id: SceneViewId) -> Result<()> {
        if let Some(view) = self.views.remove(&id) {
            if let Some(probe) = view.pending_id() { self.probes.remove(&probe); }
            view.destroy(device)?;
        }
        Ok(())
    }

    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> Result<()> {
        for view in self.views.into_values() { view.destroy(device)?; }
        if let Some(shader) = self.indexed_shader { device.destroy_shader_module(shader)?; }
        device.destroy_shader_module(self.shader)
    }
}

impl ViewCompactor {
    fn pending_id(&self) -> Option<u64> {
        self.cached.as_ref()?.cost.pending.as_ref().map(|probe| probe.id)
    }

    fn reserve(&mut self, device: &mut dyn ExtensionDevice, sizes: [u64; 3]) -> Result<[BufferId; 3]> {
        if let Some(scratch) = &self.scratch {
            if scratch.sizes.iter().zip(sizes).all(|(&capacity, size)| capacity >= size) {
                return Ok(scratch.buffers);
            }
        }
        let sizes = sizes.map(|size| size.next_power_of_two());
        let usages = [
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
            BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
            BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
        ];
        let mut allocated = Vec::new();
        for (size, usage) in sizes.into_iter().zip(usages) {
            match device.create_buffer(&BufferDescriptor {
                label: Some("gpui-3d compact indirect scratch".into()),
                size, usage, memory_location: MemoryLocation::GpuOnly,
            }) {
                Ok(buffer) => allocated.push(buffer),
                Err(error) => {
                    for buffer in allocated { let _ = device.destroy_buffer(buffer); }
                    return Err(error);
                }
            }
        }
        let buffers: [BufferId; 3] = allocated.try_into().expect("three compaction buffers");
        let old = self.scratch.replace(Scratch { buffers, sizes, metadata: Vec::new() });
        if let Some(old) = old {
            for buffer in old.buffers { device.destroy_buffer(buffer)?; }
        }
        Ok(buffers)
    }

    fn prepare(&mut self, device: &mut dyn ExtensionDevice, argument_stride: u32, indexed_supported: bool, pass: &NativeOffscreenPass) -> Result<Option<Prepared>> {
        // Native DrawIndirect probes measure indirect-only passes.
        if !pass.commands.iter().all(|command| matches!(command, RenderCommand::DrawIndirect(_) | RenderCommand::DrawIndexedIndirect(_))) {
            return Ok(None);
        }
        // Runs compare the complete IndexBufferBinding (including offset).
        // GL translates the compacted relative firstIndex on the GPU; its
        // rebase cost is included in the normal draw probe on both paths.
        let Some(limit) = device.counted_offscreen_limit() else { return Ok(None); };
        let Some(plan) = plan(&pass.commands, limit) else { return Ok(None); };
        if plan.indexed && !indexed_supported { return Ok(None); }
        let argument_stride = if plan.indexed && argument_stride == 16 { 20 } else { argument_stride };
        if u64::from(plan.records) * u64::from(argument_stride / 4) > u64::from(u32::MAX) { return Ok(None); }
        let group_count = u32::try_from(plan.runs.len()).map_err(|_| Error::InvalidInput("compaction group count overflow".into()))?;
        if group_count > device.compute_capabilities().max_workgroups[0] { return Ok(None); }
        let header_words = group_count.checked_mul(4)
            .ok_or_else(|| Error::InvalidInput("compaction metadata header overflow".into()))?;
        let total_words = header_words.checked_add(plan.records)
            .ok_or_else(|| Error::InvalidInput("compaction metadata size overflow".into()))?;
        let sizes = [u64::from(total_words) * 4, u64::from(plan.records) * u64::from(argument_stride), u64::from(group_count) * 4];
        let limits = device.buffer_binding_limits();
        if limits.validate_range(0, plan.source_bytes, false).is_err()
            || sizes.iter().any(|&size| limits.validate_range(0, size, false).is_err())
        { return Ok(None); }
        let [metadata, output, counts] = self.reserve(device, sizes)?;
        let mut bytes = Vec::with_capacity(total_words as usize * 4);
        for run in &plan.runs {
            for word in [header_words + run.source, run.count, run.destination, argument_stride / 16] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        for slot in plan.gather {
            bytes.extend_from_slice(&slot.to_le_bytes());
        }
        self.scratch.as_mut().expect("reserved compaction scratch").upload_metadata(device, bytes)?;
        let binding = |binding, buffer, size, access| ComputeBufferBinding {
            binding, buffer: BufferBinding { buffer, offset: 0, size, stride: None }, access,
        };
        let compute = [
            binding(0, plan.source, plan.source_bytes, ComputeBufferAccess::ReadOnly),
            binding(1, metadata, sizes[0], ComputeBufferAccess::ReadOnly),
            binding(2, output, sizes[1], ComputeBufferAccess::ReadWrite),
            binding(3, counts, sizes[2], ComputeBufferAccess::ReadWrite),
        ];
        let mut commands = Vec::with_capacity(pass.commands.len() - plan.records as usize + plan.runs.len());
        let mut bindings = Vec::with_capacity(plan.runs.len());
        let mut cursor = 0;
        for (index, run) in plan.runs.iter().enumerate() {
            commands.extend_from_slice(&pass.commands[cursor..run.command]);
            let mut command = pass.commands[run.command].clone();
            let offset = u64::from(run.destination) * u64::from(argument_stride);
            match &mut command {
                RenderCommand::DrawIndirect(draw) => { draw.arguments = output; draw.offset = offset; }
                RenderCommand::DrawIndexedIndirect(draw) => { draw.arguments = output; draw.offset = offset; }
                _ => unreachable!("planned indirect draw"),
            }
            bindings.push(IndirectCountBinding {
                command_index: commands.len(), count_buffer: counts,
                count_offset: index as u64 * 4, max_draw_count: run.count, argument_stride,
            });
            commands.push(command);
            cursor = run.command + run.count as usize;
        }
        commands.extend_from_slice(&pass.commands[cursor..]);
        Ok(Some(Prepared { indexed: plan.indexed, commands, counts: bindings, compute, groups: group_count, runs: plan.runs }))
    }

    fn schedule(&mut self, device: &mut dyn ExtensionDevice, argument_stride: u32, indexed_supported: bool, pass: &NativeOffscreenPass) -> Result<Option<Decision>> {
        let unchanged = self.cached.as_ref().is_some_and(|cached| Arc::ptr_eq(&cached.source, &pass.commands));
        let rebound = !unchanged && self.cached.as_mut().is_some_and(|cached| cached.try_rebind_sampled_snapshot(&pass.commands));
        let patched = if !unchanged && !rebound {
            match (&mut self.cached, &mut self.scratch) {
                (Some(cached), Some(scratch)) => match cached.try_patch_gather(device, scratch, &pass.commands) {
                    Ok(patched) => patched,
                    Err(error) => {
                        self.cached = None;
                        return Err(error);
                    }
                },
                _ => false,
            }
        } else { false };
        if !unchanged && !rebound && !patched {
            // Invalidate before preparing: a failed allocation or upload may
            // have replaced scratch storage referenced by the previous plan.
            self.cached = None;
            let prepared = self.prepare(device, argument_stride, indexed_supported, pass)?;
            self.cached = Some(Cached { source: Arc::clone(&pass.commands), prepared, cost: CostModel::default() });
        }
        let Some(cached) = self.cached.as_mut() else { return Ok(None); };
        if cached.prepared.is_none() { return Ok(None); }
        let now = Instant::now();
        let (path, probe) = cached.cost.choose(now);
        let id = probe.then(|| cached.cost.begin(path, now));
        let draw_tag = id.map(|batch_id| GpuTimestampTag { batch_id, stage: GpuTimestampStage::DrawIndirect });
        if path == Path::Original {
            return Ok(draw_tag.map(|tag| Decision { compact: false, tag: Some(tag) }));
        }
        Ok(Some(Decision { compact: true, tag: draw_tag }))
    }

    fn destroy(self, device: &mut dyn ExtensionDevice) -> Result<()> {
        if let Some(scratch) = self.scratch {
            for buffer in scratch.buffers { device.destroy_buffer(buffer)?; }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(offset: u64) -> RenderCommand {
        RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline: RenderPipelineId::from_parts(1, 1), bind_groups: bind_group_list([]),
            arguments: BufferId::from_parts(1, 1), offset, scissor: None,
        })
    }

    fn indexed_draw(offset: u64, index_offset: u64) -> RenderCommand {
        RenderCommand::DrawIndexedIndirect(DrawIndexedIndirectCommand {
            pipeline: RenderPipelineId::from_parts(1, 1),
            bind_groups: bind_group_list([]),
            arguments: BufferId::from_parts(1, 1),
            offset,
            index_buffer: IndexBufferBinding {
                buffer: BufferId::from_parts(2, 1),
                format: IndexFormat::Uint32,
                offset: index_offset,
            },
            scissor: None,
        })
    }

    #[test]
    fn sampled_replacement_preserves_compaction_only_for_exact_native_topology() {
        let geometry = BindGroupId::from_parts(1, 1);
        let old = BindGroupId::from_parts(2, 1);
        let new = BindGroupId::from_parts(3, 1);
        let mut previous: Vec<_> = (0..256).rev().map(|i| draw(i * 16)).collect();
        for command in &mut previous {
            let RenderCommand::DrawIndirect(draw) = command else { unreachable!() };
            draw.bind_groups = bind_group_list([geometry, old]);
        }
        let mut next = previous.clone();
        super::super::renderer::rebind_sampled_commands(&mut next, old, new);
        assert_eq!(sampled_snapshot_replacement(&previous, &next), Some((old, new)));
        let a = plan(&previous, 256).unwrap();
        let b = plan(&next, 256).unwrap();
        assert_eq!(a.runs, b.runs);
        assert_eq!(a.gather, b.gather);

        let mut cached = Cached { source: Arc::from(previous.as_slice()), prepared: None, cost: CostModel::default() };
        cached.cost.original.observe(123);
        let published: Arc<[RenderCommand]> = Arc::from(next.as_slice());
        assert!(cached.try_rebind_sampled_snapshot(&published));
        assert!(Arc::ptr_eq(&cached.source, &published));
        assert_eq!(cached.cost.original.ns, 123);
        let RenderCommand::DrawIndirect(draw) = &mut next[0] else { unreachable!() };
        draw.offset += 16;
        assert!(sampled_snapshot_replacement(&previous, &next).is_none());
        next[0] = published[0].clone();
        let RenderCommand::DrawIndirect(draw) = &mut next[1] else { unreachable!() };
        draw.bind_groups[0] = BindGroupId::from_parts(9, 1);
        assert!(sampled_snapshot_replacement(&previous, &next).is_none());
    }

    #[test]
    fn gather_offsets_patch_without_rebuilding_run_boundaries() {
        let old: Vec<_> = (0..256).rev().map(|i| draw(i * 16)).collect();
        let planned = plan(&old, 256).unwrap();
        let mut next = old.clone();
        next[17] = draw(32);
        next[200] = draw(64);
        assert_eq!(gather_patches(&old, &next, &planned.runs, false, planned.source_bytes),
            Some(vec![(4 + 17, 2), (4 + 200, 4)]));
        next[200] = draw(planned.source_bytes);
        assert!(gather_patches(&old, &next, &planned.runs, false, planned.source_bytes).is_none());
        next[200] = old[200].clone();
        next[17].set_scissor(Some(ScissorRect { x: 0, y: 0, width: 1, height: 1 }));
        assert!(gather_patches(&old, &next, &planned.runs, false, planned.source_bytes).is_none());
        assert!(gather_patches(&old, &old[..255], &planned.runs, false, planned.source_bytes).is_none());
    }

    #[test]
    fn sparse_gather_patches_coalesce_without_copying_unmodified_metadata() {
        let metadata = vec![0u8; 512];
        let runs = metadata_patch_runs(&metadata, &[(4, 7), (5, 9), (6, 0), (100, 3)]).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, 16);
        assert_eq!(runs[0].1, [7, 0, 0, 0, 9, 0, 0, 0]);
        assert_eq!(runs[1], (400, 3u32.to_le_bytes().to_vec()));
        assert!(metadata_patch_runs(&metadata, &[(128, 1)]).is_none());
        assert!(metadata_patch_runs(&metadata, &[(5, 1), (4, 2)]).is_none());
        assert!(metadata_patch_runs(&metadata, &[(5, 1), (5, 2)]).is_none());
        assert!(metadata_patch_runs(&metadata, &[(usize::MAX, 1)]).is_none());
        assert!(metadata_patch_runs(&metadata, &[(6, 0)]).unwrap().is_empty());
    }

    #[test]
    fn metadata_uploads_only_changed_words_and_handles_resize() {
        let original = vec![0u8; 512];
        let mut next = original.clone();
        next[16..20].copy_from_slice(&1u32.to_le_bytes());
        next[400..404].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(metadata_ranges(&original, &next), [16..20, 400..404]);
        assert!(metadata_ranges(&next, &next).is_empty());
        assert!(metadata_ranges(&next, &next[..128]).is_empty());
        assert_eq!(metadata_ranges(&next[..128], &next), [128..512]);
        assert_eq!(metadata_ranges(&[], &next), [0..512]);
        // Metadata headers may shift the entire gather tail. Simulate a
        // sparse patch and prove the GPU-visible prefix matches the full build.
        let mut resident = original;
        for range in metadata_ranges(&resident, &next) {
            resident[range.clone()].copy_from_slice(&next[range]);
        }
        assert_eq!(resident, next);
    }

    #[test]
    fn indexed_runs_preserve_gather_order_and_index_binding() {
        let mut commands: Vec<_> = (0..128).rev().map(|i| indexed_draw(i * 20, 0)).collect();
        commands.extend((200..328).map(|i| indexed_draw(i * 20, 4)));
        let plan = plan(&commands, 256).unwrap();
        assert!(plan.indexed);
        assert_eq!(plan.runs.len(), 2);
        assert_eq!(plan.runs[1].command, 128);
        assert_eq!(plan.records, 256);
        assert_eq!(plan.source_bytes, 328 * 20);
        assert_eq!(plan.gather[0], 127);
        assert_eq!(plan.gather[127], 0);
        assert_eq!(plan.gather[128], 200);
    }

    #[test]
    fn gathers_noncontiguous_runs_without_reordering() {
        let mut commands: Vec<_> = (0..300).map(|i| draw(i * 16)).collect();
        commands.extend((400..560).map(|i| draw(i * 16)));
        let plan = plan(&commands, 256).unwrap();
        assert_eq!(plan.runs, [
            Run { command: 0, source: 0, count: 256, destination: 0 },
            Run { command: 256, source: 256, count: 204, destination: 256 },
        ]);
        assert_eq!(plan.records, 460);
        assert_eq!(&plan.gather[298..303], &[298, 299, 400, 401, 402]);
        assert_eq!(plan.source_bytes, 560 * 16);
    }

    #[test]
    fn preserves_reverse_order_and_rejects_small_runs() {
        let commands: Vec<_> = (0..128).rev().map(|i| draw(i * 16)).collect();
        let gathered = plan(&commands, 256).unwrap();
        assert_eq!(gathered.gather, (0..128).rev().collect::<Vec<u32>>());
        assert!(plan(&commands[..127], 256).is_none());
        assert!(plan(&commands, 64).is_none());
    }

    fn sample(id: u64, stage: GpuTimestampStage, ns: u64) -> GpuTimestampSample {
        GpuTimestampSample { tag: GpuTimestampTag { batch_id: id, stage }, duration_ticks: ns, frequency_hz: 1_000_000_000 }
    }

    #[test]
    fn cost_model_requires_both_native_stages_and_hysteresis() {
        let mut model = CostModel::default();
        let now = Instant::now();
        for _ in 0..MIN_SAMPLES {
            let id = model.begin(Path::Original, now);
            model.observe(sample(id, GpuTimestampStage::DrawIndirect, 1000));
            let id = model.begin(Path::Compact, now);
            model.observe(sample(id, GpuTimestampStage::DrawIndirect, 600));
            assert!(model.pending.is_some());
            model.observe(sample(id, GpuTimestampStage::DrawCompaction, 100));
        }
        assert!(model.preferred_compact);
        let id = model.begin(Path::Compact, now);
        model.observe(sample(id + 1, GpuTimestampStage::DrawIndirect, 9999));
        assert!(model.pending.as_ref().unwrap().draw.is_none());
        model.choose(now + PROBE_TIMEOUT);
        assert!(!model.preferred_compact);
    }

    #[test]
    fn never_merges_distinct_scissors() {
        let mut commands: Vec<_> = (0..256).map(|i| draw(i * 16)).collect();
        for command in &mut commands[128..] {
            command.set_scissor(Some(ScissorRect { x: 1, y: 2, width: 3, height: 4 }));
        }
        let gathered = plan(&commands, 256).unwrap();
        assert_eq!(gathered.runs.len(), 2);
        assert_eq!(gathered.runs[1].command, 128);
    }
}
