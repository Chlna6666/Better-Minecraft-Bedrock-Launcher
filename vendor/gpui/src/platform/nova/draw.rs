use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NovaDrawStepMode {
    Present,
    BackdropSource,
}

pub(super) fn draw_steps_for_upload(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    blend_pipelines: NovaBlendPipelines,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_resource_set: ResourceSetId,
    mut sprite_resource_set: impl FnMut(AtlasTextureId) -> Option<ResourceSetId>,
    mut custom_mesh_3d_pipeline: impl FnMut(GpuMesh3dShaderId) -> Option<RenderPipelineId>,
    mut custom_mesh_3d_cache_entry: impl FnMut(GpuMesh3dId, u64) -> Option<NovaMeshCacheEntry>,
    underline_resource_set: ResourceSetId,
    backdrop_blur_resource_set: ResourceSetId,
    custom_mesh_3d_resource_set: ResourceSetId,
    custom_mesh_3d_indices_buffer: BufferId,
    mode: NovaDrawStepMode,
) -> Vec<RenderStepDescriptor> {
    let mut steps = Vec::new();
    draw_steps_for_upload_into(
        upload,
        pipelines,
        blend_pipelines,
        quad_resource_set,
        shadow_resource_set,
        path_resource_set,
        sprite_resource_set,
        custom_mesh_3d_pipeline,
        custom_mesh_3d_cache_entry,
        underline_resource_set,
        backdrop_blur_resource_set,
        custom_mesh_3d_resource_set,
        custom_mesh_3d_indices_buffer,
        mode,
        &mut steps,
    );
    steps
}

pub(super) fn draw_steps_for_upload_into(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    blend_pipelines: NovaBlendPipelines,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_resource_set: ResourceSetId,
    mut sprite_resource_set: impl FnMut(AtlasTextureId) -> Option<ResourceSetId>,
    mut custom_mesh_3d_pipeline: impl FnMut(GpuMesh3dShaderId) -> Option<RenderPipelineId>,
    mut custom_mesh_3d_cache_entry: impl FnMut(GpuMesh3dId, u64) -> Option<NovaMeshCacheEntry>,
    underline_resource_set: ResourceSetId,
    backdrop_blur_resource_set: ResourceSetId,
    custom_mesh_3d_resource_set: ResourceSetId,
    custom_mesh_3d_indices_buffer: BufferId,
    mode: NovaDrawStepMode,
    steps: &mut Vec<RenderStepDescriptor>,
) {
    steps.clear();
    steps.reserve(upload.batches.len().saturating_add(1));
    for batch in &upload.batches {
        if mode != NovaDrawStepMode::Present
            && matches!(batch, NovaUploadedBatch::BackdropBlurs { .. })
        {
            break;
        }
        match *batch {
            NovaUploadedBatch::SolidQuads { first, count } => {
                push_draw_step(
                    steps,
                    DrawStepDescriptor {
                        pipeline: blend_pipelines.solid_quads,
                        resource_sets: resource_set_list([quad_resource_set]),
                        vertex_count: 4,
                        first_vertex: 0,
                        instance_count: count,
                        first_instance: first,
                        scissor: None,
                    },
                );
            }
            NovaUploadedBatch::Quads { first, count } => {
                push_draw_step(
                    steps,
                    DrawStepDescriptor {
                        pipeline: blend_pipelines.quads,
                        resource_sets: resource_set_list([quad_resource_set]),
                        vertex_count: 4,
                        first_vertex: 0,
                        instance_count: count,
                        first_instance: first,
                        scissor: None,
                    },
                );
            }
            NovaUploadedBatch::Shadows { first, count } => {
                push_draw_step(
                    steps,
                    DrawStepDescriptor {
                        pipeline: blend_pipelines.shadows,
                        resource_sets: resource_set_list([shadow_resource_set]),
                        vertex_count: 4,
                        first_vertex: 0,
                        instance_count: count,
                        first_instance: first,
                        scissor: None,
                    },
                );
            }
            NovaUploadedBatch::PathRasterization { .. } => {}
            NovaUploadedBatch::Paths { first, count } => {
                push_draw_step(
                    steps,
                    DrawStepDescriptor {
                        pipeline: pipelines.paths,
                        resource_sets: resource_set_list([path_resource_set]),
                        vertex_count: 4,
                        first_vertex: 0,
                        instance_count: count,
                        first_instance: first,
                        scissor: None,
                    },
                );
            }
            NovaUploadedBatch::MonoSprites {
                texture_id,
                first,
                count,
            } => {
                if let Some(resource_set) = sprite_resource_set(texture_id) {
                    push_draw_step(
                        steps,
                        DrawStepDescriptor {
                            pipeline: blend_pipelines.mono_sprites,
                            resource_sets: resource_set_list([resource_set]),
                            vertex_count: 4,
                            first_vertex: 0,
                            instance_count: count,
                            first_instance: first,
                            scissor: None,
                        },
                    );
                }
            }
            NovaUploadedBatch::PolySprites {
                texture_id,
                first,
                count,
            } => {
                if let Some(resource_set) = sprite_resource_set(texture_id) {
                    push_draw_step(
                        steps,
                        DrawStepDescriptor {
                            pipeline: blend_pipelines.poly_sprites,
                            resource_sets: resource_set_list([resource_set]),
                            vertex_count: 4,
                            first_vertex: 0,
                            instance_count: count,
                            first_instance: first,
                            scissor: None,
                        },
                    );
                }
            }
            NovaUploadedBatch::Underlines { first, count } => {
                push_draw_step(
                    steps,
                    DrawStepDescriptor {
                        pipeline: blend_pipelines.underlines,
                        resource_sets: resource_set_list([underline_resource_set]),
                        vertex_count: 4,
                        first_vertex: 0,
                        instance_count: count,
                        first_instance: first,
                        scissor: None,
                    },
                );
            }
            NovaUploadedBatch::BackdropBlurs { first, count } => {
                if mode == NovaDrawStepMode::Present {
                    push_draw_step(
                        steps,
                        DrawStepDescriptor {
                            pipeline: blend_pipelines.backdrop_blurs,
                            resource_sets: resource_set_list([backdrop_blur_resource_set]),
                            vertex_count: 4,
                            first_vertex: 0,
                            instance_count: count,
                            first_instance: first,
                            scissor: None,
                        },
                    );
                }
            }
            NovaUploadedBatch::CustomMesh3d {
                mesh_id,
                generation,
                shader_id,
                range,
                first_parameter_index,
            } => {
                if mode == NovaDrawStepMode::Present {
                    let Some(mesh) = custom_mesh_3d_cache_entry(mesh_id, generation) else {
                        continue;
                    };
                    let Some(range_end) = range.start.checked_add(range.count) else {
                        continue;
                    };
                    if range.count == 0 || range_end > mesh.index_count || mesh.vertex_count == 0 {
                        continue;
                    } else {
                        let Some(first_index) = mesh.index_offset.checked_add(range.start) else {
                            continue;
                        };
                        let Ok(base_vertex) = i32::try_from(mesh.vertex_offset) else {
                            continue;
                        };
                        if let Some(pipeline) = custom_mesh_3d_pipeline(shader_id) {
                            steps.push(RenderStepDescriptor::DrawIndexed(
                                DrawIndexedStepDescriptor {
                                    pipeline,
                                    resource_sets: resource_set_list([custom_mesh_3d_resource_set]),
                                    index_buffer: IndexBufferBinding {
                                        buffer: custom_mesh_3d_indices_buffer,
                                        format: IndexFormat::Uint32,
                                        offset: 0,
                                    },
                                    index_count: range.count,
                                    first_index,
                                    base_vertex,
                                    instance_count: 1,
                                    first_instance: first_parameter_index,
                                    scissor: None,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }
    if steps.is_empty() {
        steps.push(RenderStepDescriptor::Draw(DrawStepDescriptor {
            pipeline: blend_pipelines.solid_quads,
            resource_sets: resource_set_list([quad_resource_set]),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 0,
            first_instance: 0,
            scissor: None,
        }));
    }
}

fn push_draw_step(steps: &mut Vec<RenderStepDescriptor>, step: DrawStepDescriptor) {
    if step.vertex_count == 0 || step.instance_count == 0 {
        return;
    }
    if let Some(RenderStepDescriptor::Draw(previous)) = steps.last_mut() {
        if draw_steps_can_merge(previous, &step) {
            if let Some(instance_count) = previous.instance_count.checked_add(step.instance_count) {
                previous.instance_count = instance_count;
                return;
            }
        }
    }

    steps.push(RenderStepDescriptor::Draw(step));
}

fn draw_steps_can_merge(previous: &DrawStepDescriptor, next: &DrawStepDescriptor) -> bool {
    previous.pipeline == next.pipeline
        && previous.resource_sets == next.resource_sets
        && previous.vertex_count == next.vertex_count
        && previous.first_vertex == next.first_vertex
        && previous.scissor == next.scissor
        && previous.first_instance.checked_add(previous.instance_count) == Some(next.first_instance)
}

pub(super) fn apply_scissor_to_steps(steps: &mut [RenderStepDescriptor], scissor: ScissorRect) {
    for step in steps {
        match step {
            RenderStepDescriptor::Draw(step) => step.scissor = Some(scissor),
            RenderStepDescriptor::DrawIndexed(step) => step.scissor = Some(scissor),
        }
    }
}

pub(super) fn partial_scissor_for_plan(
    render_plan: FrameRenderPlan<'_>,
    target_size: DrawableSize,
) -> Option<ScissorRect> {
    if render_plan.partial_present_mode != PartialPresentMode::Partial {
        return None;
    }

    let bounds = render_plan.dirty_region.union_bounds()?;
    let target_width = target_size.width;
    let target_height = target_size.height;
    let x = scaled_pixels_floor_u32(bounds.origin.x).min(target_width);
    let y = scaled_pixels_floor_u32(bounds.origin.y).min(target_height);
    let right = scaled_pixels_ceil_u32(bounds.right()).min(target_width);
    let bottom = scaled_pixels_ceil_u32(bounds.bottom()).min(target_height);
    let scissor = ScissorRect {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    };
    (!scissor.is_empty()).then_some(scissor)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped scaled-pixel bounds are converted to integer scissor coordinates"
)]
pub(super) fn scaled_pixels_floor_u32(value: crate::ScaledPixels) -> u32 {
    let value = f64::from(value).floor();
    if value <= 0.0 {
        0
    } else if value >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        value as u32
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped scaled-pixel bounds are converted to integer scissor coordinates"
)]
pub(super) fn scaled_pixels_ceil_u32(value: crate::ScaledPixels) -> u32 {
    let value = f64::from(value).ceil();
    if value <= 0.0 {
        0
    } else if value >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        value as u32
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct NovaBackdropBlurRenderPass {
    pub(super) target_texture_view: TextureViewId,
    pub(super) step: DrawStepDescriptor,
}

pub(super) fn backdrop_blur_render_passes_for_targets(
    pipelines: &NovaPipelines,
    targets: &NovaBackdropBlurTargets,
    levels: usize,
) -> Vec<NovaBackdropBlurRenderPass> {
    if targets.levels.is_empty() {
        return Vec::new();
    }
    let levels = levels.clamp(1, targets.levels.len());
    let mut passes = Vec::with_capacity(levels.saturating_mul(2).saturating_sub(1));
    for (level_index, level) in targets.levels.iter().take(levels).enumerate() {
        let resource_set = if level_index == 0 {
            targets.source_pass_resource_set
        } else {
            targets.levels[level_index - 1].pass_resource_set
        };
        passes.push(NovaBackdropBlurRenderPass {
            target_texture_view: level.texture_view,
            step: DrawStepDescriptor {
                pipeline: pipelines.backdrop_blur_downsample,
                resource_sets: resource_set_list([resource_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            },
        });
    }
    for target_index in (0..levels.saturating_sub(1)).rev() {
        passes.push(NovaBackdropBlurRenderPass {
            target_texture_view: targets.levels[target_index].texture_view,
            step: DrawStepDescriptor {
                pipeline: pipelines.backdrop_blur_upsample,
                resource_sets: resource_set_list([
                    targets.levels[target_index + 1].pass_resource_set
                ]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            },
        });
    }
    passes
}

pub(super) fn path_mask_draw_steps_for_upload(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    path_rasterization_resource_set: ResourceSetId,
) -> Vec<DrawStepDescriptor> {
    let mut steps = Vec::new();
    path_mask_draw_steps_for_upload_into(
        upload,
        pipelines,
        path_rasterization_resource_set,
        &mut steps,
    );
    steps
}

pub(super) fn path_mask_draw_steps_for_upload_into(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    path_rasterization_resource_set: ResourceSetId,
    steps: &mut Vec<DrawStepDescriptor>,
) {
    steps.clear();
    steps.reserve(upload.batches.len());
    for batch in &upload.batches {
        match *batch {
            NovaUploadedBatch::PathRasterization {
                first_vertex,
                vertex_count,
            } => push_path_mask_draw_step(
                steps,
                DrawStepDescriptor {
                    pipeline: pipelines.path_rasterization,
                    resource_sets: resource_set_list([path_rasterization_resource_set]),
                    vertex_count,
                    first_vertex,
                    instance_count: 1,
                    first_instance: 0,
                    scissor: None,
                },
            ),
            NovaUploadedBatch::SolidQuads { .. }
            | NovaUploadedBatch::Quads { .. }
            | NovaUploadedBatch::Shadows { .. }
            | NovaUploadedBatch::Paths { .. }
            | NovaUploadedBatch::MonoSprites { .. }
            | NovaUploadedBatch::PolySprites { .. }
            | NovaUploadedBatch::Underlines { .. }
            | NovaUploadedBatch::BackdropBlurs { .. }
            | NovaUploadedBatch::CustomMesh3d { .. } => {}
        }
    }
}

fn push_path_mask_draw_step(steps: &mut Vec<DrawStepDescriptor>, step: DrawStepDescriptor) {
    if step.vertex_count == 0 || step.instance_count == 0 {
        return;
    }
    if let Some(previous) = steps.last_mut() {
        if path_mask_draw_steps_can_merge(previous, &step) {
            if let Some(vertex_count) = previous.vertex_count.checked_add(step.vertex_count) {
                previous.vertex_count = vertex_count;
                return;
            }
        }
    }

    steps.push(step);
}

fn path_mask_draw_steps_can_merge(
    previous: &DrawStepDescriptor,
    next: &DrawStepDescriptor,
) -> bool {
    previous.pipeline == next.pipeline
        && previous.resource_sets == next.resource_sets
        && previous.instance_count == next.instance_count
        && previous.first_instance == next.first_instance
        && previous.scissor == next.scissor
        && previous.first_vertex.checked_add(previous.vertex_count) == Some(next.first_vertex)
}
