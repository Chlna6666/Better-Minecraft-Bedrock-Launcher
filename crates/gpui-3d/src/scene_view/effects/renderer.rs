use super::super::{
    SceneView, SceneViewId,
    resources::{PassKind, RendererResources},
};
use super::{
    ScreenEffects,
    compositor::{Compositor, Inputs},
    froxel::{FroxelGrid, FroxelInputs, FroxelProgram, FroxelVolume},
    scene::{EffectScene, SHADOW_MAP_SIZE},
};

use anyhow::Context as _;
use gfx_core::*;
use gpui::{
    ExtensionDevice, RendererExtension, RendererExtensionComputePass,
    RendererExtensionContext, RendererExtensionPass, RendererExtensionRenderer,
};
use std::{collections::HashMap, sync::Arc, time::Instant};
#[cfg(test)]
mod tests;

pub(in crate::scene_view) struct EffectsRenderer {
    last_frame: Option<Instant>,
    pass: RenderPassId,
    attributes_pass: RenderPassId,
    /// Single-sampled pass for the light-aligned shadow map.
    shadow_pass: RenderPassId,
    hdr: RendererResources,
    attributes: RendererResources,
    shadow_resources: RendererResources,
    compositor: Compositor,
    froxel_program: Option<FroxelProgram>,
    scenes: HashMap<SceneViewId, EffectScene>,
    passes: Vec<RendererExtensionPass>,
    // Mixed work is drained by GPUI's GPU owner between shadow raster and
    // the screen composite, never dispatched early from render_scene.
    compute_passes: Vec<RendererExtensionComputePass>,
    epoch: Instant,
    textures: Option<super::textures::Textures>,
    completed: bool,
    target_extent: Extent2d,
    msaa_sample_count: u32,
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
            // One unified capability query, plus the shader-side ability to read
            // multisampled depth. Both decide the geometry sample count; the kernel of
            // 4x MSAA is that every attachment of the pass shares it.
            let compositor_resources = Compositor::new(device, &context)?;
            let msaa_sample_count = if device.multisample_capabilities().max_sample_count >= 4
                && compositor_resources.supports_multisampled_depth()
            {
                4
            } else {
                1
            };
            let froxel_program =
                if compositor_resources.supports_froxel(msaa_sample_count > 1)
                    && device.texture_volume_capabilities().sampled
                    && device.texture_volume_capabilities().storage
                    && device.compute_bind_group_capabilities().storage_textures
                    && device.compute_bind_group_capabilities().sampled_textures
                {
                    match FroxelProgram::new(device, &context) {
                        Ok(program) => Some(cleanup.record(program, |d, program| {
                            let _ = program.destroy(d);
                        })),
                        Err(error) => {
                            eprintln!("gpui-3d: native Froxel disabled: {error}");
                            None
                        },
                    }
                } else { None };
            compositor = Some(compositor_resources);
            let pass = cleanup.record(
                device.create_render_pass(&RenderPassDescriptor {
                    label: Some("gpui-3d scene attachments".into()),
                    color_attachments: vec![ColorAttachmentDescriptor {
                        format: Format::Rgba16Float,
                    }],
                    depth_attachment: Some(DepthAttachmentDescriptor {
                        format: Format::Depth32Float,

                        stencil_format: None,
                    }),
                    sample_count: msaa_sample_count,
                    resolve_target: None,
                })?,
                |d, id| {
                    let _ = d.destroy_render_pass(id);
                },
            );
            // Capture both material attributes in one native MRT pass.
            let attributes_pass = cleanup.record(
                device.create_render_pass(&RenderPassDescriptor {
                    label: Some("gpui-3d surface+albedo MRT".into()),
                    color_attachments: vec![
                        ColorAttachmentDescriptor { format: Format::Rgba16Float },
                        ColorAttachmentDescriptor { format: Format::Rgba16Float },
                    ],
                    depth_attachment: Some(DepthAttachmentDescriptor {
                        format: Format::Depth32Float,
                        stencil_format: None,
                    }),
                    sample_count: msaa_sample_count,
                    resolve_target: None,
                })?,
                |d, id| { let _ = d.destroy_render_pass(id); },
            );
            let shadow_pass = cleanup.record(
                device.create_render_pass(&RenderPassDescriptor {
                    label: Some("gpui-3d shadow attachments".into()),
                    // The shadow material shader populates depth only; native
                    // backends currently require a color target. This unused
                    // attachment is R8 instead of 8-byte RGBA16F.
                    color_attachments: vec![ColorAttachmentDescriptor {
                        format: Format::R8Unorm,
                    }],
                    depth_attachment: Some(DepthAttachmentDescriptor {
                        format: Format::Depth32Float,
                        stencil_format: None,
                    }),
                    sample_count: 1,
                    resolve_target: None,
                })?,
                |d, id| {
                    let _ = d.destroy_render_pass(id);
                },
            );
            let local = context.offscreen(pass, Format::Rgba16Float, target_extent);
            for kind in [PassKind::Hdr, PassKind::AttributesMrt, PassKind::Shadow] {
                let context = match kind {
                    PassKind::Shadow => local.offscreen(
                        shadow_pass,
                        Format::R8Unorm,
                        Extent2d::new(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE)?,
                    ),
                    PassKind::AttributesMrt =>
                        local.offscreen(attributes_pass, Format::Rgba16Float, target_extent),
                    _ => local.clone(),
                };
                let resource =
                    RendererResources::new_pass(device, &context, renderers.first(), kind)?;
                renderers.push(resource);
            }
            let textures = super::textures::Textures::new(device)?;
            let [hdr, attributes, shadow_resources]: [RendererResources; 3] =
                std::mem::take(&mut renderers)
                    .try_into()
                    .unwrap_or_else(|_| unreachable!("three passes were constructed"));
            Ok(Self {
                last_frame: None,
                pass,
                attributes_pass,
                shadow_pass,
                hdr,
                attributes,
                shadow_resources,
                compositor: compositor
                    .take()
                    .unwrap_or_else(|| unreachable!("compositor was constructed")),
                froxel_program,
                scenes: HashMap::new(),
                passes: Vec::new(),
                compute_passes: Vec::new(),
                epoch: context.frame_time(),
                textures: Some(textures),
                completed: false,
                target_extent,
                msaa_sample_count,
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
            settings.texture_ids(),
        )?;
        let extent = scaled_extent(context, settings)?;
        if !self.scenes.contains_key(&view.id) {
            self.scenes.insert(
                view.id,
                EffectScene::new(
                    device,
                    context.backend_kind(),
                    extent,
                    self.msaa_sample_count,
                )?,
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
            self.attributes.replace_pipelines(
                device,
                &local.offscreen(self.attributes_pass, Format::Rgba16Float, extent),
            )?;
            self.target_extent = extent;
        }
        self.compositor.sync_post(device, &local)?;
        // Sun visibility is resolved once from the real material-aware shadow map below.
        state.capture_geometry(
            device,
            [&mut self.hdr, &mut self.attributes],
            view,
            &local,
            self.attributes_pass,
            &mut self.passes,
        )?;
        state.record_ssr_hiz(device, &local, settings, &mut self.passes);
        let [cloud, water, water_flow] = self
            .textures
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("effect textures missing"))?
            .views(device, view, settings)?;
        state
            .shadow_pass(
                device,
                &self.shadow_resources,
                view,
                &local,
                self.shadow_pass,
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
            state.ssr_hiz_levels(),
        )?;
        let tile_key = (view.camera, state.scene.light_revision());
        let tile_updated = light.2 != 0 && state.tile_key != Some(tile_key);
        if light.2 == 0 {
            state.tile_key = None;
        } else if tile_updated {
            // A failed dispatch may already have cleared the old tile lists.
            state.tile_key = None;
            state
                .tiles
                .update(device, state.frame, light)
                .context("screen light binning")?;
            state.tile_key = Some(tile_key);
        }
        // GPU-owner ordered sequence: native shadow/geometry producers have
        // already been queued as Graphics passes. Insert injection and
        // integration at the same interior boundary, immediately before the
        // final screen composite Graphics pass. Never dispatch from the UI
        // preparation phase (CSM textures are still unrendered there).
        let multisampled = self.msaa_sample_count > 1 && state.targets.msaa.is_some();
        let mut froxel_view = None;
        // Until cloud-material shadow injection uses the same cloud-volume
        // model as screen.wgsl, preserve the original cloud-occluded fog
        // shader whenever a cloud material is active.
        if settings.volumetric_fog && settings.fog_density > 0.0
            && !(settings.clouds && settings.cloud_texture.is_some())
            && !state.froxel_unavailable
            && self.compositor.supports_froxel(multisampled)
        {
            if let Some(program) = &self.froxel_program {
                if state.froxel.is_none() {
                    if let Some(grid) = FroxelGrid::plan(
                        extent, device.compute_capabilities(),
                        device.texture_volume_capabilities(),
                    ) {
                        match FroxelVolume::new(device, program, grid) {
                            Ok(volume) => state.froxel = Some(volume),
                            Err(error) => {
                                state.froxel_unavailable = true;
                                eprintln!("gpui-3d: Froxel allocation unavailable: {error}");
                            }
                        }
                    } else {
                        state.froxel_unavailable = true;
                    }
                }
                if let Some(volume) = state.froxel.as_mut() {
                    let inputs = FroxelInputs::new(
                        view, settings, (tile_key.1, light.2), state.shadow_revision,
                    );
                    let reusable = volume.is_reusable(inputs);
                    if !reusable {
                        let batch_id = crate::gpu_submission_trace::next_section_batch();
                        let profiled = device.gpu_work_timestamps_supported()
                            && crate::gpu_submission_trace::enabled();
                        let result = volume.queue(
                            device, program, state.frame, light,
                            (state.tiles.buffer, state.tiles.bytes),
                            std::array::from_fn(|cascade| state.shadow[cascade].view),
                            self.passes.len(), &mut self.compute_passes,
                            batch_id, profiled, inputs,
                        );
                        if let Err(error) = result {
                            volume.invalidate();
                            state.froxel_unavailable = true;
                            eprintln!("gpui-3d: Froxel compute disabled: {error}");
                        }
                    }
                    if reusable || !state.froxel_unavailable {
                        froxel_view = Some(volume.integrated.view);
                    }
                }
            }
        }
        let hiz_view = state.ssr_hiz.as_ref().map(|hiz| hiz.sampled_views()[0]);
        let key = (light.0, settings.texture_ids(), hiz_view, froxel_view);
        // The screen shader reads whatever depth the geometry passes wrote: the
        // multisampled copies when MSAA is on, matching its `_ms` shader variant.
        if state.groups.is_none() || state.group_key != Some(key) {
            let inputs = |frame, color| Inputs {
                targets: &state.targets,
                uniform: frame,
                lights: (light.0, light.1),
                tiles: (state.tiles.buffer, state.tiles.bytes),
                shadows: std::array::from_fn(|cascade| state.shadow[cascade].view),
                // When SSR is disabled or unsupported, bind the existing
                // near-cascade depth view; frame.work.z==0 guarantees no
                // depth-hierarchy sampling on the linear fallback path.
                ssr_hiz: state.ssr_hiz.as_ref().map_or(
                    [state.shadow[0].view; super::ssr_hiz::SSR_HIZ_LEVELS],
                    |hiz| hiz.sampled_views(),
                ),
                cloud,
                water,
                water_flow,
                color,
                froxel: froxel_view,
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
        // Validate the actual GPU allocation dependencies before enqueueing
        // the final graphics consumer. No CPU fence or host-side pixel work:
        // the compiled graph only verifies producer order and alias hazards.
        // Native mixed Graphics/Compute submission remains an explicit future
        // capability; do not pre-dispatch Froxel compute against queued CSM.
        let _transitions = state.validate_gpu_work_graph(
            settings.reflections,
            view.voxel_shading.shadow_strength > 0.0,
            tile_updated,
            froxel_view.is_some(),
            light.0,
        )?;
        self.passes.push(RendererExtensionPass {
            target: state.targets.resolved.view,
            color_attachments: Arc::from([]),
            gpu_work_tag: None,
            render_pass: self.compositor.post_pass,
            commands: vec![quad(
                self.compositor.post_pipeline(multisampled, froxel_view.is_some()), groups[0]
            )].into(),
            color_load_op: LoadOp::Clear(ClearColor::default()),
            depth_attachment: None,
            resolve_target: None,
        });
        steps.push(quad(
            self.compositor.pipeline(device, context, multisampled)?,
            groups[1],
        ));
        Ok(())
    }
}

impl EffectsRenderer {
    fn retire_inactive_scenes(
        &mut self,
        device: &mut dyn ExtensionDevice,
        current: SceneViewId,
        now: Instant,
        texture_ids: [Option<crate::TextureAssetId>; 3],
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
                .flat_map(|scene| {
                    scene
                        .group_key
                        .into_iter()
                        .flat_map(|(_, ids, _, _)| ids.into_iter().flatten())
                })
                .chain(texture_ids.into_iter().flatten())
                .collect::<std::collections::HashSet<_>>();
            if let Some(textures) = &mut self.textures {
                textures.retain(device, &used)?;
            }
        }
        Ok(())
    }
}
pub(super) fn pass(
    render_pass: RenderPassId,
    target: TextureViewId,
    depth: TextureViewId,
    resolve_target: Option<TextureViewId>,
    commands: Vec<RenderCommand>,
    clear: bool,
) -> RendererExtensionPass {
    RendererExtensionPass {
        target,
        color_attachments: Arc::from([]),
        gpu_work_tag: None,
        render_pass,
        commands: commands.into(),
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth,
            depth_load_op: if clear {
                LoadOp::Clear(1.)
            } else {
                LoadOp::Load
            },
            stencil_load_op: None,
        }),
        resolve_target,
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
        && a.fog == b.fog
        && a.culling_bounds == b.culling_bounds
        && a.hidden_nodes == b.hidden_nodes
        && a.projection_region == b.projection_region
        && a.inset_max_pixels == b.inset_max_pixels
        && a.inset_fraction == b.inset_fraction
        && a.screen_effects.is_some_and(|s| s.has_water_animation())
            == b.screen_effects.is_some_and(|s| s.has_water_animation())
}

impl RendererExtensionRenderer for EffectsRenderer {
    fn render_passes_submitted(&mut self) {
        for state in self.scenes.values_mut() {
            state.submitted_retirements = state.retired.len();
            if std::mem::take(&mut state.queued_capture) {
                state.targets_valid = true;
            }
            if let Some(volume) = state.froxel.as_mut() {
                volume.submitted();
            }
            if std::mem::take(&mut state.queued_ssr_hiz) {
                state.ssr_hiz_valid = true;
            }
            for (valid, queued) in state.shadow_valid.iter_mut()
                .zip(&mut state.queued_shadow)
            {
                if std::mem::take(queued) {
                    *valid = true;
                }
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
        let view = view.sample_camera(context.frame_time());
        let result = self.render_scene(device, &view, &context, steps);
        if result.is_err() {
            self.passes.clear();
            self.compute_passes.clear();
            for state in self.scenes.values_mut() {
                state.tile_key = None;
                if let Some(volume) = state.froxel.as_mut() {
                    volume.invalidate();
                }
                state.queued_capture = false;
                state.queued_ssr_hiz = false;
                state.ssr_hiz_valid = false;
                state.queued_shadow.fill(false);
            }
        }
        result
    }
    fn drain_render_passes(&mut self, passes: &mut Vec<RendererExtensionPass>) {
        passes.append(&mut self.passes);
    }
    fn drain_compute_passes(&mut self, compute: &mut Vec<RendererExtensionComputePass>) {
        compute.append(&mut self.compute_passes);
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
        self.attributes.destroy(device)?;
        self.hdr.destroy(device)?;
        if let Some(textures) = self.textures.take() {
            textures.destroy(device)?;
        }
        self.compositor.destroy(device)?;
        if let Some(program) = self.froxel_program.take() {
            program.destroy(device)?;
        }
        device.destroy_render_pass(self.attributes_pass)?;
        device.destroy_render_pass(self.shadow_pass)?;
        device.destroy_render_pass(self.pass)?;
        Ok(())
    }
}
