use super::{
    SceneView, SceneViewId, gpu::MeshArena,
    resources::RendererResources, scene::SceneResources,
};
use anyhow::{anyhow, bail};
use crate::prepare::StructuralDrawDelta;
use gfx_core::{BackendKind, MemoryTrimLevel, RenderCommand, RenderPassId};
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
}

impl CachedCommands {
    fn append(
        &mut self,
        revision: u64,
        steps: &mut Vec<RenderCommand>,
        build: impl FnOnce(&mut Vec<RenderCommand>) -> gpui::Result<()>,
    ) -> gpui::Result<()> {
        if self.revision == Some(revision) {
            steps.extend_from_slice(&self.commands);
            return Ok(());
        }
        self.revision = None;
        self.generated_draw_count = None;
        self.commands.clear();
        let first = steps.len();
        build(steps)?;
        self.commands.extend_from_slice(&steps[first..]);
        self.revision = Some(revision);
        Ok(())
    }

    /// Reuse native commands belonging to survivor Sections and resolve only
    /// the newly inserted/changed ranges. The final pass still flattens into a
    /// RenderCommand slice, but does no survivor pipeline/mesh/BindGroup work.
    fn append_spliced(
        &mut self,
        revision: u64,
        draw_count: Option<usize>,
        delta: Option<&StructuralDrawDelta>,
        steps: &mut Vec<RenderCommand>,
        build: impl FnOnce(&mut Vec<RenderCommand>) -> gpui::Result<()>,
        mut build_range: impl FnMut(std::ops::Range<usize>, &mut Vec<RenderCommand>) -> gpui::Result<()>,
    ) -> gpui::Result<()> {
        if self.revision == Some(revision) {
            steps.extend_from_slice(&self.commands);
            return Ok(());
        }
        if let (Some(count), Some(delta), Some(previous)) =
            (draw_count, delta, self.revision)
            && previous.wrapping_add(1) == revision
            && self.generated_draw_count == Some(delta.previous_count)
            && let Some(next) = self.splice_generated(delta, count, &mut build_range)?
        {
            if trace_timings() {
                eprintln!(
                    "gpui3d native_command_delta previous={} current={} edits={} commands={}",
                    delta.previous_count, count, delta.edits.len(), next.len(),
                );
            }
            steps.extend_from_slice(&next);
            self.commands = next;
            self.revision = Some(revision);
            self.generated_draw_count = Some(count);
            return Ok(());
        }
        self.append(revision, steps, build)?;
        self.generated_draw_count = draw_count;
        Ok(())
    }

    /// Transactional sparse rebuild: validate all previous command indices and
    /// edit intervals before touching the published command cache.
    fn splice_generated(
        &self,
        delta: &StructuralDrawDelta,
        new_count: usize,
        build_range: &mut impl FnMut(std::ops::Range<usize>, &mut Vec<RenderCommand>) -> gpui::Result<()>,
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
        let mut next = Vec::with_capacity(self.commands.len().saturating_add(new_count.saturating_sub(delta.previous_count)));
        let mut position = 0usize;
        let mut old_cursor = 0usize;
        let mut new_cursor = 0usize;
        for edit in &delta.edits {
            if !copy_native_survivors(
                &self.commands, &mut position,
                old_cursor..edit.old.start, new_cursor, &mut next,
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
            &self.commands, &mut position,
            old_cursor..delta.previous_count, new_cursor, &mut next,
        ) || position != self.commands.len() {
            return Ok(None);
        }
        Ok(Some(next))
    }
}

fn generated_range_matches(commands: &[RenderCommand], range: std::ops::Range<usize>) -> bool {
    let mut last = None;
    commands.iter().all(|command| {
        let RenderCommand::DrawIndirect(draw) = command else { return false };
        let index = draw.offset / 16;
        if draw.offset % 16 != 0 || index < range.start as u64
            || index >= range.end as u64 || last.is_some_and(|old| old >= index)
        {
            return false;
        }
        last = Some(index);
        true
    })
}

fn copy_native_survivors(
    old: &[RenderCommand],
    position: &mut usize,
    old_range: std::ops::Range<usize>,
    new_start: usize,
    next: &mut Vec<RenderCommand>,
) -> bool {
    while let Some(command) = old.get(*position) {
        let RenderCommand::DrawIndirect(draw) = command else { return false };
        let logical = (draw.offset / 16) as usize;
        if logical >= old_range.end { break; }
        if logical < old_range.start { return false; }
        let Some(next_logical) = new_start.checked_add(logical - old_range.start)
            .and_then(|value| u64::try_from(value).ok())
            .and_then(|value| value.checked_mul(16)) else { return false };
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
pub(super) struct Renderer {
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
    frame_time: Option<Instant>,
    frame_index: u64,
    timing_trace: RendererTimingTrace,
}

impl Renderer {
    pub(super) fn new(
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
        let resources = RendererResources::new(device, &context)?;
        Ok(Self {
            backend: context.backend_kind(),
            render_pass: context.render_pass(),
            color_format: context.color_format(),
            target_extent: context.viewport(),
            resources,
            generated_arena: Rc::new(RefCell::new(
                MeshArena::generated_output(device.buffer_binding_limits()),
            )),
            template_arena: Rc::new(RefCell::new(
                MeshArena::generated_templates(device.buffer_binding_limits()),
            )),
            argument_arena: Rc::new(RefCell::new(
                MeshArena::generated_arguments(device.buffer_binding_limits()),
            )),
            scenes: HashMap::new(),
            command_cache: HashMap::new(),
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
        if self.render_pass == context.render_pass()
            && self.color_format == context.color_format()
            && self.target_extent == next_target_extent
        {
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
                .append(revision, &mut output, |steps| {
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

impl RendererExtensionRenderer for Renderer {
    fn needs_gpu_completion(&self) -> bool {
        self.generated_arena.borrow().has_retired_ranges()
            || self.template_arena.borrow().has_retired_ranges()
            || self.argument_arena.borrow().has_retired_ranges()
            || self.scenes.values().any(SceneResources::has_retired_arena_ranges)
    }

    fn gpu_work_completed(&mut self) {
        self.generated_arena.borrow_mut().reclaim_after_gpu_idle();
        self.template_arena.borrow_mut().reclaim_after_gpu_idle();
        self.argument_arena.borrow_mut().reclaim_after_gpu_idle();
        for scene in self.scenes.values_mut() {
            scene.reclaim_arena_after_gpu_idle();
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
            self.sync_pipeline(device, &context)?;
            self.begin_frame(context.frame_time());
            self.retire_idle_scenes(device)?;
        }
        if !self.scenes.contains_key(&scene_view.id) {
            self.scenes.insert(
                scene_view.id,
                SceneResources::new_shared(
                    device, context.backend_kind(), Rc::clone(&self.generated_arena),
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
        let prepare_started = trace_timings().then(Instant::now);
        {
            let _phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::ScenePrepare);
            scene.prepare(device, &mut self.resources, &scene_view, &context)?;
        }
        let commands_started = prepare_started.map(|started| (started.elapsed(), Instant::now()));
        let first_step = steps.len();
        {
            let _phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::CommandEncode);
            self.command_cache
                .entry(scene_view.id)
                .or_default()
                .append_spliced(
                    scene.command_revision,
                    scene.generated_singleton_command_count(),
                    scene.native_command_delta(),
                    steps,
                    |steps| scene.append_steps(&self.resources, scene_view.voxel_shader, steps),
                    |range, steps| {
                        scene.append_generated_singleton_range(&self.resources, steps, range)
                    },
                )?;
        }
        if let Some((prepare, started)) = commands_started {
            self.timing_trace.record(
                prepare,
                started.elapsed(),
                steps.len() - first_step,
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
        self.resources.destroy(device)
    }
}
