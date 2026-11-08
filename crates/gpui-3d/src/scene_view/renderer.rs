use super::{
    SceneView, SceneViewId, gpu::MeshArena, resources::RendererResources, scene::SceneResources,
    sidecar_surface::SidecarSurface,
};
use crate::prepare::StructuralDrawDelta;
use anyhow::{anyhow, bail};
use gfx_core::{BackendKind, BindGroupList, MemoryTrimLevel, RenderCommand, RenderPassId};
use gpui::ExtensionDevice;
use gpui::{RendererExtension, RendererExtensionContext, RendererExtensionRenderer};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SCENE_MEMORY_TRIM_IDLE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
enum OffscreenFrameKind {
    Captured,
    Reused,
    RetainedWhilePreparing,
}

#[derive(Default)]
struct OffscreenTrace {
    window_start: Option<Instant>,
    captured: u64,
    reused: u64,
    retained: u64,
}

impl OffscreenTrace {
    fn record(&mut self, kind: OffscreenFrameKind) {
        if !trace_timings() {
            return;
        }
        match kind {
            OffscreenFrameKind::Captured => self.captured += 1,
            OffscreenFrameKind::Reused => self.reused += 1,
            OffscreenFrameKind::RetainedWhilePreparing => self.retained += 1,
        }
        let now = Instant::now();
        let start = self.window_start.get_or_insert(now);
        if now.saturating_duration_since(*start) < Duration::from_secs(1) {
            return;
        }
        let total = self.captured + self.reused + self.retained;
        eprintln!(
            "gpui3d offscreen_sidecar frames={} capture={} reuse={} pending_hold={} capture_ratio={:.3}",
            total,
            self.captured,
            self.reused,
            self.retained,
            self.captured as f64 / total.max(1) as f64,
        );
        *self = Self {
            window_start: Some(now),
            ..Self::default()
        };
    }
}

#[derive(Default)]
struct RendererTimingTrace {
    last_report: Option<Instant>,
    samples: u64,
    prepare_total_us: u128,
    prepare_max_us: u128,
    commands_total_us: u128,
    commands_max_us: u128,
    draw_calls_max: usize,
    gpu_cull_batches_max: usize,
    gpu_resident_draws_max: usize,
}

#[derive(Default)]
struct CachedCommands {
    revision: Option<u64>,
    commands: Vec<RenderCommand>,
    /// Only this canonical layout may be remapped through logical Section deltas.
    generated_draw_count: Option<usize>,
    /// The same draws regrouped so consecutive commands share pipeline and
    /// bind groups. Backends coalesce each identical-state run into one
    /// native indirect call, so a 100K-command scene records as a handful of
    /// native draws. Rebuilt only when the logical list changes.
    grouped: Option<Vec<RenderCommand>>,
}

impl CachedCommands {
    fn publish(&self, steps: &mut Vec<RenderCommand>) {
        match &self.grouped {
            Some(grouped) => steps.extend_from_slice(grouped),
            None => steps.extend_from_slice(&self.commands),
        }
    }

    fn append(
        &mut self,
        revision: u64,
        blended_batch_start: Option<usize>,
        steps: &mut Vec<RenderCommand>,
        build: impl FnOnce(&mut Vec<RenderCommand>) -> gpui::Result<()>,
    ) -> gpui::Result<()> {
        if self.revision == Some(revision) {
            self.publish(steps);
            return Ok(());
        }
        self.revision = None;
        self.generated_draw_count = None;
        self.commands.clear();
        let first = steps.len();
        build(steps)?;
        self.commands.extend_from_slice(&steps[first..]);
        self.grouped = group_indirect_commands(&self.commands, blended_batch_start);
        self.revision = Some(revision);
        // Backends coalesce adjacent identical-state draws into one native
        // indirect call; emit the regrouped list instead of the raw build
        // order so that coalescing actually happens.
        steps.truncate(first);
        self.publish(steps);
        Ok(())
    }

    /// Reuse native commands belonging to survivor Sections and resolve only
    /// the newly inserted/changed ranges. The final pass still flattens into a
    /// RenderCommand slice, but does no survivor pipeline/mesh/BindGroup work.
    fn append_spliced(
        &mut self,
        revision: u64,
        draw_count: Option<usize>,
        blended_batch_start: Option<usize>,
        delta: Option<&StructuralDrawDelta>,
        steps: &mut Vec<RenderCommand>,
        build: impl FnOnce(&mut Vec<RenderCommand>) -> gpui::Result<()>,
        mut build_range: impl FnMut(std::ops::Range<usize>, &mut Vec<RenderCommand>) -> gpui::Result<()>,
    ) -> gpui::Result<()> {
        if self.revision == Some(revision) {
            self.publish(steps);
            return Ok(());
        }
        if let (Some(count), Some(delta), Some(previous)) = (draw_count, delta, self.revision)
            && previous.wrapping_add(1) == revision
            && self.generated_draw_count == Some(delta.previous_count)
            && let Some(next) = self.splice_generated(delta, count, &mut build_range)?
        {
            if trace_timings() {
                eprintln!(
                    "gpui3d native_command_delta previous={} current={} edits={} commands={}",
                    delta.previous_count,
                    count,
                    delta.edits.len(),
                    next.len(),
                );
            }
            self.commands = next;
            self.grouped = group_indirect_commands(&self.commands, blended_batch_start);
            self.revision = Some(revision);
            self.generated_draw_count = Some(count);
            self.publish(steps);
            return Ok(());
        }
        self.append(revision, blended_batch_start, steps, build)?;
        self.generated_draw_count = draw_count;
        Ok(())
    }

    /// Transactional sparse rebuild: validate all previous command indices and
    /// edit intervals before touching the published command cache.
    fn splice_generated(
        &self,
        delta: &StructuralDrawDelta,
        new_count: usize,
        build_range: &mut impl FnMut(
            std::ops::Range<usize>,
            &mut Vec<RenderCommand>,
        ) -> gpui::Result<()>,
    ) -> gpui::Result<Option<Vec<RenderCommand>>> {
        let mut previous_command = None;
        for command in &self.commands {
            let RenderCommand::DrawIndirect(draw) = command else {
                return Ok(None);
            };
            if draw.offset % 16 != 0 || draw.offset / 16 >= delta.previous_count as u64 {
                return Ok(None);
            }
            if previous_command.is_some_and(|old| old >= draw.offset) {
                return Ok(None);
            }
            previous_command = Some(draw.offset);
        }
        let mut old = 0;
        let mut new = 0;
        for edit in &delta.edits {
            if edit.old.start < old
                || edit.new.start < new
                || edit.old.end > delta.previous_count
                || edit.new.end > new_count
                || edit.old.start > edit.old.end
                || edit.new.start > edit.new.end
                || edit.old.start - old != edit.new.start - new
            {
                return Ok(None);
            }
            old = edit.old.end;
            new = edit.new.end;
        }
        if delta.previous_count - old != new_count - new {
            return Ok(None);
        }
        let mut next = Vec::with_capacity(
            self.commands
                .len()
                .saturating_add(new_count.saturating_sub(delta.previous_count)),
        );
        let mut position = 0usize;
        let mut old_cursor = 0usize;
        let mut new_cursor = 0usize;
        for edit in &delta.edits {
            if !copy_native_survivors(
                &self.commands,
                &mut position,
                old_cursor..edit.old.start,
                new_cursor,
                &mut next,
            ) {
                return Ok(None);
            }
            while let Some(RenderCommand::DrawIndirect(draw)) = self.commands.get(position) {
                if draw.offset / 16 >= edit.old.end as u64 {
                    break;
                }
                position += 1;
            }
            let from = next.len();
            build_range(edit.new.clone(), &mut next)?;
            if !generated_range_matches(&next[from..], edit.new.clone()) {
                return Ok(None);
            }
            old_cursor = edit.old.end;
            new_cursor = edit.new.end;
        }
        if !copy_native_survivors(
            &self.commands,
            &mut position,
            old_cursor..delta.previous_count,
            new_cursor,
            &mut next,
        ) || position != self.commands.len()
        {
            return Ok(None);
        }
        Ok(Some(next))
    }
}

fn generated_range_matches(commands: &[RenderCommand], range: std::ops::Range<usize>) -> bool {
    let mut last = None;
    commands.iter().all(|command| {
        let RenderCommand::DrawIndirect(draw) = command else {
            return false;
        };
        let index = draw.offset / 16;
        if draw.offset % 16 != 0
            || index < range.start as u64
            || index >= range.end as u64
            || last.is_some_and(|old| old >= index)
        {
            return false;
        }
        last = Some(index);
        true
    })
}

/// Regroups a canonical all-indirect command list so consecutive commands
/// share pipeline, bind groups and scissor. Backends record each
/// identical-state run as one native indirect call, so thousands of Section
/// draws collapse into one native call per distinct binding state.
///
/// Opaque draws are bucketed by pipeline and bind group only before the
/// first blended batch. Every subsequent command keeps its original order.
/// Non-indirect lists return `None` and are published unchanged.
fn group_indirect_commands(
    commands: &[RenderCommand],
    blended_batch_start: Option<usize>,
) -> Option<Vec<RenderCommand>> {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    if *DISABLED.get_or_init(|| std::env::var_os("GPUI_3D_NO_GROUP").is_some()) {
        return None;
    }
    if commands.is_empty()
        || !commands
            .iter()
            .all(|command| matches!(command, RenderCommand::DrawIndirect(_)))
    {
        return None;
    }
    // Only opaque commands can be globally reordered. Transparent draws
    // must keep their exact camera-sorted order even when Pipeline/BindGroup
    // differs: collecting them by state would change alpha composition and
    // could produce camera-dependent water/transparent flicker.
    let opaque_len = blended_batch_start.map_or(commands.len(), |first_blend| {
        commands.iter().position(|command| match command {
            RenderCommand::DrawIndirect(draw) => draw.offset / 16 >= first_blend as u64,
            _ => false,
        }).unwrap_or(commands.len())
    });
    if opaque_len <= 1 {
        return None;
    }
    let opaque_commands = &commands[..opaque_len];
    let mut pipeline_order = Vec::<gfx_core::RenderPipelineId>::new();
    let mut keys = Vec::<(gfx_core::RenderPipelineId, BindGroupList)>::new();
    let mut buckets =
        HashMap::<(gfx_core::RenderPipelineId, &BindGroupList), Vec<RenderCommand>>::default();
    for command in opaque_commands {
        let RenderCommand::DrawIndirect(draw) = command else {
            continue;
        };
        let pipeline = command.pipeline();
        if !pipeline_order.contains(&pipeline) {
            pipeline_order.push(pipeline);
        }
        let key = (pipeline, &draw.bind_groups);
        match buckets.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().push(command.clone());
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                keys.push((pipeline, draw.bind_groups.clone()));
                entry.insert(vec![command.clone()]);
            }
        }
    }
    if keys.len() >= opaque_commands.len() {
        // Opaque grouping cannot improve submission count. Leave all
        // commands in original order rather than touch blended draws.
        return None;
    }
    let mut grouped = Vec::with_capacity(commands.len());
    for pipeline in &pipeline_order {
        for key in keys.iter().filter(|(candidate, _)| candidate == pipeline) {
            let lookup = (key.0, &key.1);
            grouped.extend(buckets.remove(&lookup).unwrap_or_default());
        }
    }
    grouped.extend_from_slice(&commands[opaque_len..]);
    debug_assert_eq!(grouped.len(), commands.len());
    // Regrouping must be a permutation of the original draws: every GPU
    // indirect argument record is still executed exactly once. Pipeline and
    // bind-group identity are preserved by construction; offsets are the
    // stable per-draw address into the shared argument buffer.
    debug_assert!(
        {
            let mut before = commands
                .iter()
                .map(|command| match command {
                    RenderCommand::DrawIndirect(draw) => draw.offset,
                    _ => u64::MAX,
                })
                .collect::<Vec<_>>();
            let mut after = grouped
                .iter()
                .map(|command| match command {
                    RenderCommand::DrawIndirect(draw) => draw.offset,
                    _ => u64::MAX,
                })
                .collect::<Vec<_>>();
            before.sort_unstable();
            after.sort_unstable();
            before == after
        },
        "regrouped indirect commands must be a permutation of the input"
    );
    Some(grouped)
}

fn copy_native_survivors(
    old: &[RenderCommand],
    position: &mut usize,
    old_range: std::ops::Range<usize>,
    new_start: usize,
    next: &mut Vec<RenderCommand>,
) -> bool {
    while let Some(command) = old.get(*position) {
        let RenderCommand::DrawIndirect(draw) = command else {
            return false;
        };
        let logical = (draw.offset / 16) as usize;
        if logical >= old_range.end {
            break;
        }
        if logical < old_range.start {
            return false;
        }
        let Some(next_logical) = new_start
            .checked_add(logical - old_range.start)
            .and_then(|value| u64::try_from(value).ok())
            .and_then(|value| value.checked_mul(16))
        else {
            return false;
        };
        let mut copy = draw.clone();
        copy.offset = next_logical;
        next.push(RenderCommand::DrawIndirect(copy));
        *position += 1;
    }
    true
}

impl RendererTimingTrace {
    fn record(
        &mut self,
        prepare: Duration,
        commands: Duration,
        draw_calls: usize,
        gpu_culling: (usize, usize),
    ) {
        let now = Instant::now();
        self.samples += 1;
        self.prepare_total_us += prepare.as_micros();
        self.prepare_max_us = self.prepare_max_us.max(prepare.as_micros());
        self.commands_total_us += commands.as_micros();
        self.commands_max_us = self.commands_max_us.max(commands.as_micros());
        self.draw_calls_max = self.draw_calls_max.max(draw_calls);
        self.gpu_cull_batches_max = self.gpu_cull_batches_max.max(gpu_culling.0);
        self.gpu_resident_draws_max = self.gpu_resident_draws_max.max(gpu_culling.1);
        if self
            .last_report
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            return;
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        eprintln!(
            "gpui3d render ts={} samples={} prepare_avg_ms={:.2} prepare_max_ms={:.2} commands_avg_ms={:.2} commands_max_ms={:.2} draw_calls_max={} gpu_cull_batches_max={} gpu_resident_draws_max={}",
            timestamp,
            self.samples,
            self.prepare_total_us as f64 / self.samples as f64 / 1000.0,
            self.prepare_max_us as f64 / 1000.0,
            self.commands_total_us as f64 / self.samples as f64 / 1000.0,
            self.commands_max_us as f64 / 1000.0,
            self.draw_calls_max,
            self.gpu_cull_batches_max,
            self.gpu_resident_draws_max,
        );
        *self = Self {
            last_report: Some(now),
            ..Self::default()
        };
    }
}

pub(super) fn trace_timings() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("GPUI_3D_TRACE_TIMINGS").is_some())
}

fn should_trim_scene(level: MemoryTrimLevel, last_used_at: Instant, now: Instant) -> bool {
    match level {
        MemoryTrimLevel::Light => false,
        MemoryTrimLevel::Moderate => {
            now.saturating_duration_since(last_used_at) >= SCENE_MEMORY_TRIM_IDLE
        }
        MemoryTrimLevel::Aggressive => true,
    }
}
/// The GPUI-facing renderer is only a bridge for viewport context and
/// offscreen compositor attachment. All 3D GPU resources, frame cache and
/// submission planning belong to the sidecar backend below.
pub(super) struct Renderer {
    backend: SidecarGpuBackend,
}

/// 3D-owned GPU backend. Its native resources remain thread-affine, while
/// CPU Scene preparation is performed by the independent sidecar scheduler.
/// Keeping this boundary explicit lets the host's extension pass supply the
/// final attachment without owning MeshArena or Draw Slot lifetime.
/// Qt3D RenderJob handoff policy. The GPU owner never traverses a newly
/// replaced large Scene solely to fill a missing worker result. Previously
/// committed matching/compatible geometry may still be presented.
fn defer_unprepared_scene(
    worker_pending: bool,
    committed_matches: bool,
    retained_compatible: bool,
) -> bool {
    worker_pending && !committed_matches && !retained_compatible
}

struct SidecarGpuBackend {
    backend: BackendKind,
    render_pass: RenderPassId,
    color_format: gfx_core::Format,
    target_extent: gfx_core::Extent2d,
    resources: RendererResources,
    /// Resident writable face pages live on the renderer's GPU owner and are
    /// reused by all SceneViews that share this device.
    generated_arena: Rc<RefCell<MeshArena>>,
    template_arena: Rc<RefCell<MeshArena>>,
    argument_arena: Rc<RefCell<MeshArena>>,
    scenes: HashMap<SceneViewId, SceneResources>,
    command_cache: HashMap<SceneViewId, CachedCommands>,
    /// True 3D offscreen producer path. Each capture becomes a separate
    /// native submission ordered before GPUI samples its completed texture.
    /// Device calls remain on this native owner for DX11/GL correctness.
    offscreen: Option<SidecarSurface>,
    pending_passes: Vec<gpui::RendererExtensionPass>,
    /// Capture epochs enter this list when their producer passes are encoded;
    /// they are moved to submitted candidates only when GPUI drains those
    /// passes, and published only after the host accepts the GPU submission.
    pending_capture_epochs: Vec<(SceneViewId, u64)>,
    submitted_capture_epochs: Vec<(SceneViewId, u64)>,
    offscreen_trace: OffscreenTrace,
    frame_time: Option<Instant>,
    frame_index: u64,
    timing_trace: RendererTimingTrace,
}

impl SidecarGpuBackend {
    fn new(
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
    ) -> gpui::Result<Self> {
        if trace_timings() {
            // One-time GPU capability report; no driver queries per frame.
            // Distinguish physical array capacity from actually available
            // backend texture-array creation/upload/shader sampling.
            let arrays = device.texture_array_capabilities();
            let buffers = device.buffer_binding_limits();
            let compute = device.compute_capabilities();
            eprintln!(
                "gpui3d backend_caps backend={:?} array_max_dimension={} array_max_layers={} array_resources={} layer_uploads={} indexed_sampling={} storage_alignment={} storage_max_binding_bytes={} compute_max_group_invocations={} indirect_draw={}",
                context.backend_kind(),
                arrays.max_dimension_2d,
                arrays.max_array_layers,
                arrays.array_resources,
                arrays.layer_uploads,
                arrays.indexed_sampling,
                buffers.storage_offset_alignment,
                buffers.max_storage_binding_bytes,
                compute.max_invocations_per_workgroup,
                compute.indirect_draw,
            );
        }
        // Probe native offscreen 3D submission only when explicitly enabled.
        // It is deliberately opt-in until DX11/DX12/Vulkan/OpenGL timing and
        // output parity tests confirm the extra GPU texture pass is beneficial.
        let mut offscreen = if std::env::var_os("GPUI_3D_OFFSCREEN_SIDECAR").is_some() {
            match SidecarSurface::new(device, &context) {
                Ok(surface) => Some(surface),
                Err(error) => {
                    eprintln!("gpui3d offscreen_sidecar_unavailable backend={:?} error={error}", context.backend_kind());
                    None
                }
            }
        } else {
            None
        };
        let render_context = offscreen.as_ref()
            .map(|surface| surface.capture_context(&context))
            .unwrap_or_else(|| context.clone());
        let resources = match RendererResources::new(device, &render_context) {
            Ok(resources) => resources,
            Err(error) => {
                if let Some(surface) = offscreen.take() {
                    let _ = surface.destroy(device);
                }
                return Err(error);
            }
        };
        if offscreen.is_some() && trace_timings() {
            eprintln!("gpui3d offscreen_sidecar_enabled backend={:?}", context.backend_kind());
        }
        Ok(Self {
            backend: context.backend_kind(),
            render_pass: render_context.render_pass(),
            color_format: render_context.color_format(),
            target_extent: render_context.viewport(),
            resources,
            generated_arena: Rc::new(RefCell::new(MeshArena::generated_output(
                device.buffer_binding_limits(),
            ))),
            template_arena: Rc::new(RefCell::new(MeshArena::generated_templates(
                device.buffer_binding_limits(),
            ))),
            argument_arena: Rc::new(RefCell::new(MeshArena::generated_arguments(
                device.buffer_binding_limits(),
            ))),
            scenes: HashMap::new(),
            command_cache: HashMap::new(),
            offscreen,
            pending_passes: Vec::new(),
            pending_capture_epochs: Vec::new(),
            submitted_capture_epochs: Vec::new(),
            offscreen_trace: OffscreenTrace::default(),
            frame_time: None,
            frame_index: 0,
            timing_trace: RendererTimingTrace::default(),
        })
    }

    fn sync_pipeline(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        if self.backend != context.backend_kind() {
            bail!("GPUI 3D renderer backend changed during a window lifetime")
        }
        let next_target_extent = context.viewport();
        // Nova DX11/DX12/Vulkan/OpenGL/Metal pipeline constructors all
        // accept an extent but use dynamic native viewports and scissors.
        // Different cropped SceneView sizes therefore never require
        // rebuilding eighteen mesh/shader variants or invalidating every
        // other view's GPU Draw command cache.
        if self.render_pass == context.render_pass()
            && self.color_format == context.color_format()
        {
            self.target_extent = next_target_extent;
            return Ok(());
        }
        self.resources.replace_pipelines(device, context)?;
        self.command_cache.clear();
        self.render_pass = context.render_pass();
        self.color_format = context.color_format();
        self.target_extent = next_target_extent;
        Ok(())
    }

    fn begin_frame(&mut self, frame_time: Instant) {
        if self.frame_time != Some(frame_time) {
            self.frame_time = Some(frame_time);
            self.frame_index = self.frame_index.wrapping_add(1);
            // A prior submission may have failed. Never acknowledge its
            // drained passes during a subsequent, unrelated GUI frame.
            self.pending_capture_epochs.clear();
            self.submitted_capture_epochs.clear();
        }
    }

    fn retire_idle_scenes(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let stale = self
            .scenes
            .iter()
            .filter_map(|(id, scene)| {
                (self.frame_index.saturating_sub(scene.last_used_frame) > 120).then_some(*id)
            })
            .collect::<Vec<_>>();
        self.destroy_scenes(device, stale)
    }

    fn trim_scene_cache(
        &mut self,
        device: &mut dyn ExtensionDevice,
        level: MemoryTrimLevel,
    ) -> gpui::Result<()> {
        match level {
            MemoryTrimLevel::Light => Ok(()),
            MemoryTrimLevel::Moderate => {
                let now = Instant::now();
                let stale = self
                    .scenes
                    .iter()
                    .filter_map(|(id, scene)| {
                        should_trim_scene(level, scene.last_used_at, now).then_some(*id)
                    })
                    .collect();
                self.destroy_scenes(device, stale)
            }
            MemoryTrimLevel::Aggressive => {
                let stale = self.scenes.keys().copied().collect();
                self.destroy_scenes(device, stale)
            }
        }
    }

    fn destroy_scenes(
        &mut self,
        device: &mut dyn ExtensionDevice,
        ids: Vec<SceneViewId>,
    ) -> gpui::Result<()> {
        for id in ids {
            self.command_cache.remove(&id);
            if let Some(mut scene) = self.scenes.remove(&id) {
                scene.destroy(device)?;
            }
            if let Some(surface) = self.offscreen.as_mut() {
                surface.retire(device, id)?;
            }
            // CPU-sidecar mailboxes are now SceneView-owned Arcs. They
            // retire independently when the last producer snapshot drops.
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CachedCommands, SCENE_MEMORY_TRIM_IDLE, should_trim_scene};
    use gfx_core::{
        DrawCommand, MemoryTrimLevel, RenderCommand, RenderPipelineId, bind_group_list,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn stable_scene_reuses_commands_and_changed_scene_rebuilds() {
        let mut cache = CachedCommands::default();
        let mut builds = 0;
        let mut output = Vec::new();
        for revision in [1, 1, 2] {
            cache
                .append(revision, None, &mut output, |steps| {
                    builds += 1;
                    steps.push(RenderCommand::Draw(DrawCommand {
                        pipeline: RenderPipelineId::from_parts(revision as u32, 1),
                        bind_groups: bind_group_list([]),
                        vertex_count: 3,
                        first_vertex: 0,
                        instance_count: 1,
                        first_instance: 0,
                        scissor: None,
                    }));
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(builds, 2);
        assert_eq!(
            output
                .iter()
                .map(|step| step.pipeline())
                .collect::<Vec<_>>(),
            [
                RenderPipelineId::from_parts(1, 1),
                RenderPipelineId::from_parts(1, 1),
                RenderPipelineId::from_parts(2, 1),
            ]
        );
    }

    #[test]
    fn indirect_grouping_never_reorders_blended_suffix() {
        use gfx_core::{BufferId, DrawIndirectCommand};
        let pipeline = |id| RenderPipelineId::from_parts(id, 1);
        let draw = |offset, pipeline_id| {
            RenderCommand::DrawIndirect(DrawIndirectCommand {
                pipeline: pipeline(pipeline_id),
                bind_groups: bind_group_list([]),
                arguments: BufferId::from_parts(1, 1),
                offset,
                scissor: None,
            })
        };
        // Two opaque pipelines can be regrouped, but transparent A-B-A
        // must remain in camera-sorted order, not be changed to A-A-B.
        let input = vec![
            draw(0, 1), draw(16, 2), draw(32, 1),
            draw(48, 3), draw(64, 4), draw(80, 3),
        ];
        let grouped = super::group_indirect_commands(&input, Some(3))
            .expect("opaque batching should regroup three commands");
        let offsets = |commands: &[RenderCommand]| {
            commands.iter().map(|command| match command {
                RenderCommand::DrawIndirect(draw) => draw.offset,
                _ => unreachable!(),
            }).collect::<Vec<_>>()
        };
        assert_eq!(offsets(&grouped[..3]), vec![0, 32, 16]);
        assert_eq!(offsets(&grouped[3..]), vec![48, 64, 80]);
        // When every command is transparent, no global reorder is legal.
        assert!(super::group_indirect_commands(&input[3..], Some(0)).is_none());
    }

    #[test]
    fn initial_and_incompatible_scene_publications_wait_for_cpu_render_jobs() {
        use super::defer_unprepared_scene;
        assert!(defer_unprepared_scene(true, false, false));
        assert!(!defer_unprepared_scene(true, true, false));
        assert!(!defer_unprepared_scene(true, false, true));
        assert!(!defer_unprepared_scene(false, false, false));
    }

    #[test]
    fn memory_trim_preserves_recent_scenes_until_aggressive_trim() {
        let now = Instant::now();
        let recent = now - SCENE_MEMORY_TRIM_IDLE + Duration::from_millis(1);
        let idle = now - SCENE_MEMORY_TRIM_IDLE;

        assert!(!should_trim_scene(MemoryTrimLevel::Light, idle, now));
        assert!(!should_trim_scene(MemoryTrimLevel::Moderate, recent, now));
        assert!(should_trim_scene(MemoryTrimLevel::Moderate, idle, now));
        assert!(should_trim_scene(MemoryTrimLevel::Aggressive, recent, now));
    }
}

impl Renderer {
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
    ) -> gpui::Result<Self> {
        Ok(Self {
            backend: SidecarGpuBackend::new(device, context)?,
        })
    }
}

/// The GPUI extension never owns GPU resources or reimplements the 3D
/// submission algorithm: it forwards the current viewport and attachment
/// context into the renderer-owned sidecar and drains its completed commands.
/// The shared native device owner still serializes backend access; arbitrary
/// cross-thread native contexts are intentionally not exposed here.
impl RendererExtensionRenderer for Renderer {
    fn prefers_independent_graphics_queue(&self) -> bool {
        self.backend.prefers_independent_graphics_queue()
    }

    fn render_passes_submitted(&mut self) {
        self.backend.render_passes_submitted();
    }

    fn drain_render_passes(&mut self, passes: &mut Vec<gpui::RendererExtensionPass>) {
        self.backend.drain_render_passes(passes);
    }

    fn needs_gpu_completion(&self) -> bool {
        self.backend.needs_gpu_completion()
    }

    fn gpu_work_completed(&mut self) {
        self.backend.gpu_work_completed();
    }

    fn render(
        &mut self,
        extension: &dyn RendererExtension,
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
        steps: &mut Vec<RenderCommand>,
    ) -> gpui::Result<()> {
        self.backend.render(extension, device, context, steps)
    }

    fn trim_memory(
        &mut self,
        device: &mut dyn ExtensionDevice,
        level: MemoryTrimLevel,
    ) -> gpui::Result<()> {
        self.backend.trim_memory(device, level)
    }

    fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        self.backend.destroy(device)
    }
}

impl RendererExtensionRenderer for SidecarGpuBackend {
    fn prefers_independent_graphics_queue(&self) -> bool {
        self.offscreen.is_some()
    }

    fn drain_render_passes(&mut self, passes: &mut Vec<gpui::RendererExtensionPass>) {
        // GPUI's host renders these offscreen passes first, then samples the
        // per-SceneView color target at the original extension painter order.
        passes.append(&mut self.pending_passes);
        self.submitted_capture_epochs.append(&mut self.pending_capture_epochs);
    }

    fn render_passes_submitted(&mut self) {
        if let Some(surface) = self.offscreen.as_mut() {
            surface.render_passes_submitted(&self.submitted_capture_epochs);
        }
        self.submitted_capture_epochs.clear();
    }

    fn needs_gpu_completion(&self) -> bool {
        self.generated_arena.borrow().has_retired_ranges()
            || self.template_arena.borrow().has_retired_ranges()
            || self.argument_arena.borrow().has_retired_ranges()
            || self
                .scenes
                .values()
                .any(SceneResources::has_retired_arena_ranges)
            || self.offscreen.as_ref().is_some_and(SidecarSurface::has_retired_targets)
    }

    fn gpu_work_completed(&mut self) {
        self.generated_arena.borrow_mut().reclaim_after_gpu_idle();
        self.template_arena.borrow_mut().reclaim_after_gpu_idle();
        self.argument_arena.borrow_mut().reclaim_after_gpu_idle();
        for scene in self.scenes.values_mut() {
            scene.reclaim_arena_after_gpu_idle();
        }
        if let Some(surface) = self.offscreen.as_mut() {
            surface.gpu_work_completed();
        }
    }

    fn render(
        &mut self,
        extension: &dyn RendererExtension,
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
        steps: &mut Vec<RenderCommand>,
    ) -> gpui::Result<()> {
        let scene_view = extension
            .downcast_ref::<SceneView>()
            .ok_or_else(|| anyhow!("GPUI 3D renderer received an unexpected extension type"))?;
        let scene_view = scene_view.sample_camera(context.frame_time());
        {
            let _phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::ResourceSync);
            if let Some(surface) = self.offscreen.as_mut() {
                surface.sync(device, &context)?;
            }
            let render_context = self.offscreen.as_ref()
                .map(|surface| surface.capture_context(&context))
                .unwrap_or_else(|| context.clone());
            self.sync_pipeline(device, &render_context)?;
            self.begin_frame(context.frame_time());
            self.retire_idle_scenes(device)?;
        }
        // A Qt3D-like CPU RenderJob may still be preparing the initial
        // scene or an incompatible full replacement. Do not turn the shared
        // GPU owner into a second synchronous geometry traversal. The host
        // extension keeps its frame alive until the worker publishes.
        let worker_pending = super::sidecar::pending(&scene_view);
        let retained = if worker_pending {
            self.scenes.get(&scene_view.id)
                .and_then(|scene| scene.retained_sidecar_view(&scene_view))
        } else {
            None
        };
        let matching = self.scenes.get(&scene_view.id)
            .is_some_and(|scene| scene.matches_published_scene(&scene_view));
        if defer_unprepared_scene(worker_pending, matching, retained.is_some()) {
            // Do not force the GUI to blank while a newer, incompatible Scene
            // is prepared. Reuse the last *submitted* texture when possible;
            // this branch must not allocate/upload or block the GPU owner.
            if let Some(surface) = self.offscreen.as_ref()
                && surface.composite_existing(scene_view.id, &context, steps)
            {
                self.offscreen_trace.record(OffscreenFrameKind::RetainedWhilePreparing);
            }
            return Ok(());
        }
        if !self.scenes.contains_key(&scene_view.id) {
            self.scenes.insert(
                scene_view.id,
                SceneResources::new_shared(
                    device,
                    context.backend_kind(),
                    Rc::clone(&self.generated_arena),
                    Rc::clone(&self.template_arena),
                    Rc::clone(&self.argument_arena),
                )?,
            );
        }
        let scene = self
            .scenes
            .get_mut(&scene_view.id)
            .ok_or_else(|| anyhow!("GPUI 3D scene-view state was not initialized"))?;
        scene.last_used_frame = self.frame_index;
        scene.last_used_at = Instant::now();
        // A full unrelated Scene replacement is allowed to arrive as a
        // versioned CPU-sidecar job. Until it is prepared, continue showing
        // the last published GPU geometry with the current camera instead
        // of blocking every window's shared GPU submission owner.
        let prepared_view = retained.as_ref().unwrap_or(&scene_view);
        let render_context = self.offscreen.as_ref()
            .map(|surface| surface.capture_context(&context))
            .unwrap_or_else(|| context.clone());
        let prepare_started = trace_timings().then(Instant::now);
        {
            let _phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::ScenePrepare);
            scene.prepare(device, &mut self.resources, prepared_view, &render_context)?;
        }
        // Pure GUI redraw / unchanged 3D state: sample the previously
        // submitted color image directly. Skip the offscreen producer pass,
        // command-cache publishing, GPU culling dispatch and unnecessary
        // descriptor references. The SceneResources revision includes camera
        // and uniform updates, not merely native draw-command changes.
        if let Some(surface) = self.offscreen.as_ref()
            && surface.can_reuse(
                scene_view.id, &context, scene.capture_revision,
            )
        {
            surface.composite_existing(scene_view.id, &context, steps);
            self.offscreen_trace.record(OffscreenFrameKind::Reused);
            return Ok(());
        }
        let commands_started = prepare_started.map(|started| (started.elapsed(), Instant::now()));
        let mut capture_steps = Vec::new();
        let commands_count = {
            // The same Draw/Instance/Shader layout is used by both paths.
            // Only the final consumer changes: own offscreen pass vs GPUI
            // root render pass. No extra scene traversal or mesh copies.
            let output = if self.offscreen.is_some() {
                &mut capture_steps
            } else {
                &mut *steps
            };
            let first_step = output.len();
            let _phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::CommandEncode);
            self.command_cache
                .entry(scene_view.id)
                .or_default()
                .append_spliced(
                    scene.command_revision,
                    scene.generated_singleton_command_count(),
                    scene.first_blended_batch_index(),
                    scene.native_command_delta(),
                    output,
                    |output| scene.append_steps(&self.resources, scene_view.voxel_shader, output),
                    |range, output| {
                        scene.append_generated_singleton_range(&self.resources, output, range)
                    },
                )?;
            output.len() - first_step
        };
        if let Some(surface) = self.offscreen.as_mut() {
            surface.enqueue(
                device, scene_view.id, &context, scene.capture_revision,
                capture_steps, &mut self.pending_passes, steps,
            )?;
            self.pending_capture_epochs.push((scene_view.id, scene.capture_revision));
            self.offscreen_trace.record(OffscreenFrameKind::Captured);
        }
        if let Some((prepare, started)) = commands_started {
            self.timing_trace.record(
                prepare,
                started.elapsed(),
                commands_count,
                scene.gpu_culling_counts(),
            );
        }
        Ok(())
    }

    fn trim_memory(
        &mut self,
        device: &mut dyn ExtensionDevice,
        level: MemoryTrimLevel,
    ) -> gpui::Result<()> {
        self.trim_scene_cache(device, level)
    }

    fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let scenes = self.scenes.keys().copied().collect();
        self.destroy_scenes(device, scenes)?;
        self.resources.destroy(device)?;
        if let Some(surface) = self.offscreen.take() {
            surface.destroy(device)?;
        }
        Ok(())
    }
}
