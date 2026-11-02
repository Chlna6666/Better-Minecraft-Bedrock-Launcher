use super::super::{
    SceneView, SceneViewId,
    resources::{PassKind, RendererResources},
};
use super::{
    ScreenEffects,
    compositor::{Compositor, Inputs},
    scene::EffectScene,
};

use anyhow::Context as _;
use gfx_core::*;
use gpui::{
    ExtensionDevice, RendererExtension, RendererExtensionContext, RendererExtensionPass,
    RendererExtensionRenderer,
};
use std::{collections::HashMap, sync::Arc, time::Instant};
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub(in crate::scene_view) struct EffectsRenderer {
    last_frame: Option<Instant>,
    pass: RenderPassId,
    hdr: RendererResources,
    surface: RendererResources,
    albedo: RendererResources,
    shadow_resources: RendererResources,
    compositor: Compositor,
    scenes: HashMap<SceneViewId, EffectScene>,
    passes: Vec<RendererExtensionPass>,
    epoch: Instant,
    cloud: HashMap<crate::TextureAssetId, super::super::gpu::TextureResources>,
    cloud_fallback: Option<super::super::gpu::TextureResources>,
    completed: bool,
    target_extent: Extent2d,
}
impl EffectsRenderer {
    pub(in crate::scene_view) fn new(
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
        settings: ScreenEffects,
    ) -> gpui::Result<Self> {
        let caps = device.compute_capabilities();
        if !caps.indirect_draw || caps.max_workgroups.contains(&0) {
            anyhow::bail!("screen effects require ordered compute and indirect drawing");
        }
        let target_extent = scaled_extent(&context, settings)?;
        let mut cleanup = super::construction::Construction::default();
        let mut renderers = Vec::<RendererResources>::new();
        let mut compositor = None;
        let result = (|| {
            let pass = cleanup.record(
                device.create_render_pass(&RenderPassDescriptor {
                    label: Some("gpui-3d scene attachments".into()),
                    color_attachment: ColorAttachmentDescriptor {
                        format: Format::Rgba16Float,
                    },
                    depth_attachment: Some(DepthAttachmentDescriptor {
                        format: Format::Depth32Float,
                    }),
                })?,
                |d, id| {
                    let _ = d.destroy_render_pass(id);
                },
            );
            let local = context.offscreen(pass, Format::Rgba16Float, target_extent);
            for kind in [
                PassKind::Hdr,
                PassKind::Surface,
                PassKind::Albedo,
                PassKind::Shadow,
            ] {
                let context = if kind == PassKind::Shadow {
                    local.offscreen(pass, Format::Rgba16Float, Extent2d::new(1024, 1024)?)
                } else {
                    local.clone()
                };
                let resource =
                    RendererResources::new_pass(device, &context, renderers.first(), kind)?;
                renderers.push(resource);
            }
            compositor = Some(Compositor::new(device, &context)?);
            let fallback = crate::TextureAsset::linear_rgba8(1, 1, vec![0, 0, 0, 0])?;
            let cloud_fallback = super::super::gpu::create_texture(device, &fallback)?;
            let [hdr, surface, albedo, shadow_resources]: [RendererResources; 4] =
                std::mem::take(&mut renderers)
                    .try_into()
                    .unwrap_or_else(|_| unreachable!("four passes were constructed"));
            Ok(Self {
                last_frame: None,
                pass,
                hdr,
                surface,
                albedo,
                shadow_resources,
                compositor: compositor
                    .take()
                    .unwrap_or_else(|| unreachable!("compositor was constructed")),
                scenes: HashMap::new(),
                passes: Vec::new(),
                epoch: context.frame_time(),
                cloud: HashMap::new(),
                cloud_fallback: Some(cloud_fallback),
                completed: false,
                target_extent,
            })
        })();
        if result.is_err() {
            if let Some(mut compositor) = compositor {
                let _ = compositor.destroy(device);
            }
            for mut renderer in renderers.into_iter().rev() {
                let _ = renderer.destroy(device);
            }
        }
        cleanup.finish(device, result)
    }
    fn render_scene(
        &mut self,
        device: &mut dyn ExtensionDevice,
        view: &SceneView,
        context: &RendererExtensionContext,
        steps: &mut Vec<RenderCommand>,
    ) -> gpui::Result<()> {
        let settings = view
            .screen_effects
            .ok_or_else(|| anyhow::anyhow!("screen settings missing"))?;
        self.retire_inactive_scenes(
            device,
            view.id,
            context.frame_time(),
            settings.cloud_texture,
        )?;
        let extent = scaled_extent(context, settings)?;
        if !self.scenes.contains_key(&view.id) {
            self.scenes.insert(
                view.id,
                EffectScene::new(device, context.backend_kind(), extent)?,
            );
        }
        let completed = std::mem::take(&mut self.completed);
        for state in self.scenes.values_mut() {
            // Keep logical IDs until their plans have been submitted. The backend
            // then retires native resources behind its queue fence, including compute.
            let submitted = std::mem::take(&mut state.submitted_retirements);
            for retired in state.retired.drain(..submitted) {
                retired.destroy(device)?;
            }
            if completed {
                state.scene.reclaim_arena_after_gpu_idle();
            }
        }
        let state = self
            .scenes
            .get_mut(&view.id)
            .ok_or_else(|| anyhow::anyhow!("effect scene missing"))?;
        state.last_seen = context.frame_time();
        state.resize(device, context.backend_kind(), extent)?;
        let local = context.offscreen(self.pass, Format::Rgba16Float, extent);
        if self.target_extent != extent {
            self.hdr.replace_pipelines(device, &local)?;
            self.surface.replace_pipelines(device, &local)?;
            self.albedo.replace_pipelines(device, &local)?;
            self.target_extent = extent;
        }
        self.compositor.sync_post(device, &local)?;
        // Sun visibility is resolved once from the real material-aware shadow map below.
        state.capture_geometry(
            device,
            [&mut self.hdr, &mut self.surface, &mut self.albedo],
            view,
            &local,
            &mut self.passes,
        )?;
        let fallback = self
            .cloud_fallback
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("cloud fallback missing"))?
            .view;
        let cloud = if let Some(id) = settings.cloud_texture {
            if !self.cloud.contains_key(&id) {
                let asset = view.textures.get(&id).ok_or_else(|| {
                    anyhow::anyhow!("cloud texture {id:?} is missing from the scene texture table")
                })?;
                self.cloud
                    .insert(id, super::super::gpu::create_texture(device, asset)?);
            }
            self.cloud
                .get(&id)
                .map(|asset| asset.view)
                .unwrap_or(fallback)
        } else {
            fallback
        };
        state
            .shadow_pass(
                device,
                &self.shadow_resources,
                view,
                &local,
                &mut self.passes,
            )
            .context("material shadow pass")?;
        let light = state.scene.light_resources();
        state.write_frames(
            device,
            view,
            settings,
            &local,
            context,
            context
                .frame_time()
                .saturating_duration_since(self.epoch)
                .as_secs_f32(),
            light.2,
        )?;
        if state.tile_camera != Some(view.camera)
            || state
                .tile_scene
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &view.scene))
            || state.tile_transform != view.scene_transform
        {
            // A failed dispatch may already have cleared the old tile lists.
            state.tile_camera = None;
            state
                .tiles
                .update(device, state.frame, light)
                .context("screen light binning")?;
            state.tile_camera = Some(view.camera);
            state.tile_transform = view.scene_transform;
            state.tile_scene = Some(view.scene.clone());
        }
        let key = (light.0, settings.cloud_texture);
        if state.groups.is_none() || state.group_key != Some(key) {
            let inputs = |frame, color| Inputs {
                targets: &state.targets,
                uniform: frame,
                lights: (light.0, light.1),
                tiles: (state.tiles.buffer, state.tiles.bytes),
                shadow: state.shadow.view,
                cloud,
                color,
            };
            let a = self.compositor.bind(device, inputs(state.frame, None))?;
            let b = match self.compositor.bind(
                device,
                inputs(state.composite_frame, Some(state.targets.resolved.view)),
            ) {
                Ok(group) => group,
                Err(error) => {
                    let _ = device.destroy_bind_group(a);
                    return Err(error);
                }
            };
            if let Some(groups) = state.groups.replace([a, b]) {
                for group in groups {
                    device.destroy_bind_group(group)?;
                }
            }
            state.group_key = Some(key);
        }
        let groups = state
            .groups
            .ok_or_else(|| anyhow::anyhow!("screen bindings missing"))?;
        self.passes.push(RendererExtensionPass {
            target: state.targets.resolved.view,
            render_pass: self.compositor.post_pass,
            commands: vec![quad(self.compositor.post_pipeline, groups[0])],
            color_load_op: LoadOp::Clear(ClearColor::default()),
            depth_attachment: None,
        });
        steps.push(quad(self.compositor.pipeline(device, context)?, groups[1]));
        Ok(())
    }
}

impl EffectsRenderer {
    fn retire_inactive_scenes(
        &mut self,
        device: &mut dyn ExtensionDevice,
        current: SceneViewId,
        now: Instant,
        cloud: Option<crate::TextureAssetId>,
    ) -> gpui::Result<()> {
        if let Some(previous) = self.last_frame.replace(now)
            && previous != now
        {
            let expired = self
                .scenes
                .iter()
                .filter(|(id, state)| **id != current && state.last_seen != previous)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            for id in expired {
                if let Some(scene) = self.scenes.remove(&id) {
                    scene.destroy(device)?;
                }
            }
            let used = self
                .scenes
                .values()
                .filter_map(|scene| scene.group_key.and_then(|(_, cloud)| cloud))
                .chain(cloud)
                .collect::<std::collections::HashSet<_>>();
            let expired = self
                .cloud
                .keys()
                .filter(|id| !used.contains(id))
                .copied()
                .collect::<Vec<_>>();
            for id in expired {
                if let Some(texture) = self.cloud.remove(&id) {
                    super::super::gpu::destroy_texture(device, texture)?;
                }
            }
        }
        Ok(())
    }
}
pub(super) fn pass(
    render_pass: RenderPassId,
    target: TextureViewId,
    depth: TextureViewId,
    commands: Vec<RenderCommand>,
    clear: bool,
) -> RendererExtensionPass {
    RendererExtensionPass {
        target,
        render_pass,
        commands,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth,
            depth_load_op: if clear {
                LoadOp::Clear(1.)
            } else {
                LoadOp::Load
            },
        }),
    }
}
fn quad(pipeline: RenderPipelineId, group: BindGroupId) -> RenderCommand {
    RenderCommand::Draw(DrawCommand {
        pipeline,
        bind_groups: bind_group_list([group]),
        vertex_count: 3,
        first_vertex: 0,
        instance_count: 1,
        first_instance: 0,
        scissor: None,
    })
}
fn scaled_extent(
    context: &RendererExtensionContext,
    settings: ScreenEffects,
) -> gpui::Result<Extent2d> {
    let bounds = context.bounds();
    let width = f64::from(bounds.size.width) as f32;
    let height = f64::from(bounds.size.height) as f32;
    let scale = settings.resolution_scale
        * (4_194_304. / (width * height * settings.resolution_scale.powi(2)))
            .sqrt()
            .min(1.);
    Ok(Extent2d::new(
        (width * scale).ceil().max(1.) as u32,
        (height * scale).ceil().max(1.) as u32,
    )?)
}
pub(super) fn same_capture(a: &SceneView, b: &SceneView) -> bool {
    Arc::ptr_eq(&a.scene, &b.scene)
        && Arc::ptr_eq(&a.textures, &b.textures)
        && Arc::ptr_eq(&a.animation, &b.animation)
        && a.camera == b.camera
        && a.scene_transform == b.scene_transform
        && a.voxel_shading == b.voxel_shading
        && a.animation_time == b.animation_time
        && a.sampling == b.sampling
        && a.anisotropy_enabled == b.anisotropy_enabled
        && a.blend_edge_feather == b.blend_edge_feather
        && a.culling_bounds == b.culling_bounds
        && a.projection_region == b.projection_region
        && a.inset_max_pixels == b.inset_max_pixels
        && a.inset_fraction == b.inset_fraction
}

impl RendererExtensionRenderer for EffectsRenderer {
    fn render_passes_submitted(&mut self) {
        for state in self.scenes.values_mut() {
            state.submitted_retirements = state.retired.len();
            if std::mem::take(&mut state.queued_capture) {
                state.targets_valid = true;
            }
            if std::mem::take(&mut state.queued_shadow) {
                state.shadow_valid = true;
            }
        }
    }
    fn render(
        &mut self,
        input: &dyn RendererExtension,
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
        steps: &mut Vec<RenderCommand>,
    ) -> gpui::Result<()> {
        let view = input
            .downcast_ref::<SceneView>()
            .ok_or_else(|| anyhow::anyhow!("invalid effects input"))?;
        let result = self.render_scene(device, view, &context, steps);
        if result.is_err() {
            self.passes.clear();
            for state in self.scenes.values_mut() {
                state.queued_capture = false;
                state.queued_shadow = false;
            }
        }
        result
    }
    fn drain_render_passes(&mut self, passes: &mut Vec<RendererExtensionPass>) {
        passes.append(&mut self.passes);
    }
    fn needs_gpu_completion(&self) -> bool {
        self.scenes
            .values()
            .any(|scene| scene.scene.has_retired_arena_ranges())
    }
    fn gpu_work_completed(&mut self) {
        self.completed = true;
    }
    fn trim_memory(
        &mut self,
        device: &mut dyn ExtensionDevice,
        level: MemoryTrimLevel,
    ) -> gpui::Result<()> {
        if level == MemoryTrimLevel::Aggressive {
            for (_, scene) in self.scenes.drain() {
                scene.destroy(device)?;
            }
        }
        Ok(())
    }
    fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for (_, scene) in self.scenes.drain() {
            scene.destroy(device)?;
        }
        self.shadow_resources.destroy(device)?;
        self.albedo.destroy(device)?;
        self.surface.destroy(device)?;
        self.hdr.destroy(device)?;
        for (_, texture) in self.cloud.drain() {
            super::super::gpu::destroy_texture(device, texture)?;
        }
        if let Some(texture) = self.cloud_fallback.take() {
            super::super::gpu::destroy_texture(device, texture)?;
        }
        self.compositor.destroy(device)?;
        device.destroy_render_pass(self.pass)?;
        Ok(())
    }
}
