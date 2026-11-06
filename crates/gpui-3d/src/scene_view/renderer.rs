use super::{SceneView, SceneViewId, resources::RendererResources, scene::SceneResources};
use anyhow::{anyhow, bail};
use gfx_core::{BackendKind, MemoryTrimLevel, RenderCommand, RenderPassId};
use gpui::ExtensionDevice;
use gpui::{RendererExtension, RendererExtensionContext, RendererExtensionRenderer};
use std::{
    collections::HashMap,
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
        self.commands.clear();
        let first = steps.len();
        build(steps)?;
        self.commands.extend_from_slice(&steps[first..]);
        self.revision = Some(revision);
        Ok(())
    }
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
        self.scenes
            .values()
            .any(SceneResources::has_retired_arena_ranges)
    }

    fn gpu_work_completed(&mut self) {
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
                SceneResources::new(device, context.backend_kind())?,
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
                .append(scene.command_revision, steps, |steps| {
                    scene.append_steps(&self.resources, scene_view.voxel_shader, steps)
                })?;
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
