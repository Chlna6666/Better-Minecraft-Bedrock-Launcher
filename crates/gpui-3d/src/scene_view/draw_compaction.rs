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
    source: BufferId,
    source_bytes: u64,
    records: u32,
    runs: Vec<Run>,
    gather: Vec<u32>,
}

fn plan(commands: &[RenderCommand], limit: u32) -> Option<Plan> {
    let max = MAX_RUN.min(limit as usize);
    if max < MIN_RUN || commands.len() < MIN_RUN { return None; }
    let mut source = None;
    let mut source_bytes = 0;
    let mut records = 0u32;
    let mut runs = Vec::new();
    let mut gather = Vec::new();
    let mut index = 0;
    while index < commands.len() {
        let RenderCommand::DrawIndirect(first) = &commands[index] else {
            index += 1;
            continue;
        };
        if first.offset % 16 != 0 || source.is_some_and(|id| id != first.arguments) {
            index += 1;
            continue;
        }
        let mut len = 1usize;
        while len < max {
            let Some(RenderCommand::DrawIndirect(next)) = commands.get(index + len) else { break; };
            if next.arguments != first.arguments || next.pipeline != first.pipeline
                || next.bind_groups != first.bind_groups || next.scissor != first.scissor
                || next.offset % 16 != 0
            { break; }
            len += 1;
        }
        if len >= MIN_RUN {
            let start = records;
            records = records.checked_add(len as u32)?;
            source = Some(first.arguments);
            for command in &commands[index..index + len] {
                let RenderCommand::DrawIndirect(draw) = command else { unreachable!() };
                let slot = u32::try_from(draw.offset / 16).ok()?;
                source_bytes = source_bytes.max((u64::from(slot) + 1) * 16);
                gather.push(slot);
            }
            runs.push(Run { command: index, source: start, count: len as u32, destination: start });
        }
        index += len;
    }
    Some(Plan { source: source?, source_bytes, records, runs, gather })
}

struct Scratch {
    buffers: [BufferId; 3],
    sizes: [u64; 3],
}

struct Prepared {
    commands: Vec<RenderCommand>,
    counts: Vec<IndirectCountBinding>,
    compute: [ComputeBufferBinding; 4],
    groups: u32,
}

// Strong Arc identity prevents address reuse and avoids hashing/scanning
// the immutable command list on a camera-only update. Each live view cache
// also retains the negative result for non-compaction-friendly scenes.
struct Cached {
    source: Arc<[RenderCommand]>,
    prepared: Option<Prepared>,
    cost: CostModel,
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
        if context.backend_kind() != BackendKind::Vulkan
            || std::env::var_os("GPUI_3D_DRAW_COMPACTION").as_deref() != Some(std::ffi::OsStr::new("1"))
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
        Some(Self { shader, views: HashMap::new(), probes: HashMap::new() })
    }

    pub(super) fn submit_batch(
        &mut self, device: &mut dyn ExtensionDevice, passes: &[NativeOffscreenPass],
        epochs: &[(SceneViewId, u64)],
    ) -> Result<Option<SubmissionId>> {
        let ordinary = |device: &mut dyn ExtensionDevice| crate::submit_native_offscreen_passes(
            passes, |passes, tags| device.submit_offscreen_graphics(passes, tags, OffscreenQueuePreference::OrderedGraphics),
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
            let decision = view.schedule(device, self.shader, pass)?;
            let next_probe = view.pending_id();
            if previous_probe != next_probe {
                if let Some(old) = previous_probe { self.probes.remove(&old); }
                if let Some(new) = next_probe { self.probes.insert(new, id); }
            }
            special |= decision.is_some();
            decisions.push(decision);
        }
        if !special { return ordinary(device); }
        // All compaction producers are on the ordered owner queue; each
        // view has distinct scratch through the entire graphics batch.
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
        device.submit_counted_offscreen_batch(&native, &counts, &tags)
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
        let old = self.scratch.replace(Scratch { buffers, sizes });
        if let Some(old) = old {
            for buffer in old.buffers { device.destroy_buffer(buffer)?; }
        }
        Ok(buffers)
    }

    fn prepare(&mut self, device: &mut dyn ExtensionDevice, pass: &NativeOffscreenPass) -> Result<Option<Prepared>> {
        // Vulkan's native DrawIndirect query currently measures indirect-only passes.
        if !pass.commands.iter().all(|command| matches!(command, RenderCommand::DrawIndirect(_) | RenderCommand::DrawIndexedIndirect(_))) {
            return Ok(None);
        }
        let Some(limit) = device.counted_offscreen_limit() else { return Ok(None); };
        let Some(plan) = plan(&pass.commands, limit) else { return Ok(None); };
        let group_count = u32::try_from(plan.runs.len()).map_err(|_| Error::InvalidInput("compaction group count overflow".into()))?;
        if group_count > device.compute_capabilities().max_workgroups[0] { return Ok(None); }
        let header_words = group_count.checked_mul(4)
            .ok_or_else(|| Error::InvalidInput("compaction metadata header overflow".into()))?;
        let total_words = header_words.checked_add(plan.records)
            .ok_or_else(|| Error::InvalidInput("compaction metadata size overflow".into()))?;
        let sizes = [u64::from(total_words) * 4, u64::from(plan.records) * 16, u64::from(group_count) * 4];
        let limits = device.buffer_binding_limits();
        if limits.validate_range(0, plan.source_bytes, false).is_err()
            || sizes.iter().any(|&size| limits.validate_range(0, size, false).is_err())
        { return Ok(None); }
        let [metadata, output, counts] = self.reserve(device, sizes)?;
        let mut bytes = Vec::with_capacity(total_words as usize * 4);
        for run in &plan.runs {
            for word in [header_words + run.source, run.count, run.destination, 0] {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        }
        for slot in plan.gather {
            bytes.extend_from_slice(&slot.to_le_bytes());
        }
        device.write_buffer(metadata, 0, &bytes)?;
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
            let RenderCommand::DrawIndirect(mut draw) = pass.commands[run.command].clone() else { unreachable!() };
            draw.arguments = output;
            draw.offset = u64::from(run.destination) * 16;
            bindings.push(IndirectCountBinding {
                command_index: commands.len(), count_buffer: counts,
                count_offset: index as u64 * 4, max_draw_count: run.count,
            });
            commands.push(RenderCommand::DrawIndirect(draw));
            cursor = run.command + run.count as usize;
        }
        commands.extend_from_slice(&pass.commands[cursor..]);
        Ok(Some(Prepared { commands, counts: bindings, compute, groups: group_count }))
    }

    fn schedule(&mut self, device: &mut dyn ExtensionDevice, shader: ShaderModuleId, pass: &NativeOffscreenPass) -> Result<Option<Decision>> {
        if !self.cached.as_ref().is_some_and(|cached| Arc::ptr_eq(&cached.source, &pass.commands)) {
            // Invalidate before preparing: a failed allocation or upload may
            // have replaced scratch storage referenced by the previous plan.
            self.cached = None;
            let prepared = self.prepare(device, pass)?;
            self.cached = Some(Cached { source: Arc::clone(&pass.commands), prepared, cost: CostModel::default() });
        }
        let Some(cached) = self.cached.as_mut() else { return Ok(None); };
        let Some(prepared) = cached.prepared.as_ref() else { return Ok(None); };
        let now = Instant::now();
        let (path, probe) = cached.cost.choose(now);
        let id = probe.then(|| cached.cost.begin(path, now));
        let draw_tag = id.map(|batch_id| GpuTimestampTag { batch_id, stage: GpuTimestampStage::DrawIndirect });
        if path == Path::Original {
            return Ok(draw_tag.map(|tag| Decision { compact: false, tag: Some(tag) }));
        }
        // Culling, compaction and counted drawing share the ordered owner queue.
        let dispatch = ComputeDispatch {
            shader, bindings: &prepared.compute, workgroups: [prepared.groups, 1, 1],
        };
        if let Some(batch_id) = id {
            device.dispatch_compute_batch_profiled(std::slice::from_ref(&dispatch),
                GpuTimestampTag { batch_id, stage: GpuTimestampStage::DrawCompaction })?;
        } else {
            device.dispatch_compute(&dispatch)?;
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
