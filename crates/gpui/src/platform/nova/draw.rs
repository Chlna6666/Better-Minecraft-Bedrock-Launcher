use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DrawStepMode {
    Present,
    /// Draw only batches in `[batch_start, batch_end)`. Backdrop groups use this mode to advance
    /// one shared scene-color target between draw-order barriers without rebuilding old prefixes.
    BackdropSegment {
        batch_start: usize,
        batch_end: usize,
    },
    /// Draw only an element-blur content range. Nested blur markers are interpreted so their
    /// composites remain in the parent source while their raw content is omitted.
    BlurContent {
        batch_start: usize,
        batch_end: usize,
    },
}

impl DrawStepMode {
    fn is_past_end(self, batch_index: usize) -> bool {
        match self {
            Self::Present => false,
            Self::BackdropSegment { batch_end, .. } | Self::BlurContent { batch_end, .. } => {
                batch_index >= batch_end
            }
        }
    }

    fn includes_batch(self, batch_index: usize) -> bool {
        match self {
            Self::Present => true,
            Self::BackdropSegment {
                batch_start,
                batch_end,
            }
            | Self::BlurContent {
                batch_start,
                batch_end,
            } => batch_index >= batch_start && batch_index < batch_end,
        }
    }
}

#[cfg(test)]
pub(super) fn draw_steps_for_upload(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    blend_pipelines: BlendPipelines,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_resource_set: ResourceSetId,
    sprite_resource_set: impl FnMut(AtlasTextureId) -> Option<ResourceSetId>,
    underline_resource_set: ResourceSetId,
    backdrop_blur_resource_set: ResourceSetId,
    mode: DrawStepMode,
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
        underline_resource_set,
        |_| Some(backdrop_blur_resource_set),
        mode,
        &mut steps,
    );
    steps
}

pub(super) fn draw_steps_for_upload_into(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    blend_pipelines: BlendPipelines,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_resource_set: ResourceSetId,
    mut sprite_resource_set: impl FnMut(AtlasTextureId) -> Option<ResourceSetId>,
    underline_resource_set: ResourceSetId,
    mut backdrop_blur_resource_set: impl FnMut(BackdropBlurConfig) -> Option<ResourceSetId>,
    mode: DrawStepMode,
    steps: &mut Vec<RenderStepDescriptor>,
) {
    steps.clear();
    steps.reserve(upload.batches.len().saturating_add(1));
    let mut blur_depth = 0usize;
    for (batch_index, batch) in upload.batches.iter().enumerate() {
        if mode.is_past_end(batch_index) {
            break;
        }
        if !mode.includes_batch(batch_index) {
            continue;
        }
        match *batch {
            UploadedBatch::BeginBlur { .. } => {
                blur_depth = blur_depth.saturating_add(1);
                continue;
            }
            UploadedBatch::EndBlur { .. } => {
                blur_depth = blur_depth.saturating_sub(1);
                continue;
            }
            UploadedBatch::CompositeBlur { index } => {
                if blur_depth == 0 {
                    push_blur_composite_step(
                        upload,
                        blend_pipelines.backdrop_blurs,
                        &mut backdrop_blur_resource_set,
                        index,
                        steps,
                    );
                }
                continue;
            }
            _ if blur_depth != 0 => continue,
            _ => {}
        }
        match *batch {
            UploadedBatch::SolidQuads { first, count } => push_draw_step(
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
            ),
            UploadedBatch::Quads { first, count } => push_draw_step(
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
            ),
            UploadedBatch::Shadows { first, count } => push_draw_step(
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
            ),
            UploadedBatch::PathRasterization { .. } => {}
            UploadedBatch::Paths { first, count } => push_draw_step(
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
            ),
            UploadedBatch::MonoSprites {
                texture_id,
                first,
                count,
            } => {
                if let Some(resource_set) = sprite_resource_set(texture_id) {
                    #[cfg(target_os = "windows")]
                    let pipeline = if texture_id.kind == AtlasTextureKind::Subpixel {
                        blend_pipelines.subpixel_sprites
                    } else {
                        blend_pipelines.mono_sprites
                    };
                    #[cfg(not(target_os = "windows"))]
                    let pipeline = blend_pipelines.mono_sprites;
                    push_draw_step(
                        steps,
                        DrawStepDescriptor {
                            pipeline,
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
            UploadedBatch::PolySprites {
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
            UploadedBatch::Underlines { first, count } => push_draw_step(
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
            ),
            UploadedBatch::BackdropBlurs { first, count } => {
                upload.for_each_backdrop_blur_run(first, count, |run| {
                    let Some(resource_set) = backdrop_blur_resource_set(run.config) else {
                        return;
                    };
                    push_draw_step(
                        steps,
                        DrawStepDescriptor {
                            pipeline: blend_pipelines.backdrop_blurs,
                            resource_sets: resource_set_list([resource_set]),
                            vertex_count: 4,
                            first_vertex: 0,
                            instance_count: run.count,
                            first_instance: run.first,
                            scissor: None,
                        },
                    );
                });
            }
            UploadedBatch::BeginBlur { .. }
            | UploadedBatch::EndBlur { .. }
            | UploadedBatch::CompositeBlur { .. } => unreachable!("blur markers handled above"),
            UploadedBatch::RendererExtensions { first, count } => {
                let start = first as usize;
                let end = start.saturating_add(count as usize);
                if let Some(extension_steps) = upload.renderer_extension_steps.get(start..end) {
                    for extension_steps in extension_steps {
                        steps.extend(extension_steps.iter().cloned());
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

fn push_blur_composite_step(
    upload: &FrameUpload,
    pipeline: RenderPipelineId,
    backdrop_blur_resource_set: &mut impl FnMut(BackdropBlurConfig) -> Option<ResourceSetId>,
    index: u32,
    steps: &mut Vec<RenderStepDescriptor>,
) {
    let Some(config) = upload.backdrop_blur_config_for_index(index) else {
        return;
    };
    let Some(resource_set) = backdrop_blur_resource_set(config) else {
        return;
    };
    push_draw_step(
        steps,
        DrawStepDescriptor {
            pipeline,
            resource_sets: resource_set_list([resource_set]),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 1,
            first_instance: index,
            scissor: None,
        },
    );
}

fn push_draw_step(steps: &mut Vec<RenderStepDescriptor>, step: DrawStepDescriptor) {
    if step.vertex_count == 0 || step.instance_count == 0 {
        return;
    }
    if let Some(RenderStepDescriptor::Draw(previous)) = steps.last_mut()
        && draw_steps_can_merge(previous, &step)
        && let Some(instance_count) = previous.instance_count.checked_add(step.instance_count)
    {
        previous.instance_count = instance_count;
        return;
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

pub(super) fn partial_scissor_for_packet(
    packet: &PresentationPacket,
    target_size: DrawableSize,
) -> Option<ScissorRect> {
    if packet.partial_present_mode != PartialPresentMode::Partial {
        return None;
    }
    let bounds = packet.dirty_region.union_bounds()?;
    let x = scaled_pixels_floor_u32(bounds.origin.x).min(target_size.width);
    let y = scaled_pixels_floor_u32(bounds.origin.y).min(target_size.height);
    let right = scaled_pixels_ceil_u32(bounds.right()).min(target_size.width);
    let bottom = scaled_pixels_ceil_u32(bounds.bottom()).min(target_size.height);
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
pub(super) struct BackdropBlurRenderPass {
    pub(super) target_texture_view: TextureViewId,
    pub(super) step: DrawStepDescriptor,
}

#[cfg(test)]
pub(super) fn backdrop_blur_render_passes_for_targets_into(
    pipelines: &Pipelines,
    targets: &BackdropBlurTargets,
    frame_resource_index: usize,
    passes: &mut Vec<BackdropBlurRenderPass>,
) {
    let configs: Vec<_> = targets
        .variants
        .iter()
        .map(|variant| variant.config)
        .collect();
    backdrop_blur_render_passes_for_configs_into(
        pipelines,
        targets,
        frame_resource_index,
        &configs,
        passes,
    );
}

pub(super) fn backdrop_blur_render_passes_for_configs_into(
    pipelines: &Pipelines,
    targets: &BackdropBlurTargets,
    frame_resource_index: usize,
    configs: &[BackdropBlurConfig],
    passes: &mut Vec<BackdropBlurRenderPass>,
) {
    let Some(source_resource_set) = targets
        .source_pass_resource_sets
        .get(frame_resource_index)
        .copied()
    else {
        passes.clear();
        return;
    };
    backdrop_blur_render_passes_for_configs_with_source_into(
        pipelines,
        targets,
        frame_resource_index,
        configs,
        source_resource_set,
        passes,
    );
}

pub(super) fn backdrop_blur_render_passes_for_configs_with_source_into(
    pipelines: &Pipelines,
    targets: &BackdropBlurTargets,
    frame_resource_index: usize,
    configs: &[BackdropBlurConfig],
    source_resource_set: ResourceSetId,
    passes: &mut Vec<BackdropBlurRenderPass>,
) {
    passes.clear();
    passes.reserve(configs.len().saturating_mul(2));

    for config in configs {
        let Some((variant_index, variant)) = targets
            .variants
            .iter()
            .enumerate()
            .find(|(_, variant)| variant.config == *config)
        else {
            continue;
        };
        let [horizontal, vertical] = variant.levels.as_slice() else {
            continue;
        };
        let Some(horizontal_resource_set) = horizontal
            .pass_resource_sets
            .get(frame_resource_index)
            .copied()
        else {
            continue;
        };
        let Ok(pass_base) = u32::try_from(variant_index.saturating_mul(2)) else {
            continue;
        };

        passes.push(BackdropBlurRenderPass {
            target_texture_view: horizontal.texture_view,
            step: DrawStepDescriptor {
                pipeline: pipelines.backdrop_blur_downsample,
                resource_sets: resource_set_list([source_resource_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: pass_base,
                scissor: None,
            },
        });
        passes.push(BackdropBlurRenderPass {
            target_texture_view: vertical.texture_view,
            step: DrawStepDescriptor {
                pipeline: pipelines.backdrop_blur_upsample,
                resource_sets: resource_set_list([horizontal_resource_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: pass_base.saturating_add(1),
                scissor: None,
            },
        });
    }
}

#[cfg(test)]
pub(super) fn path_mask_draw_steps_for_upload(
    upload: &FrameUpload,
    pipelines: &Pipelines,
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
    upload: &FrameUpload,
    pipelines: &Pipelines,
    path_rasterization_resource_set: ResourceSetId,
    steps: &mut Vec<DrawStepDescriptor>,
) {
    steps.clear();
    steps.reserve(upload.batches.len());
    for batch in &upload.batches {
        match *batch {
            UploadedBatch::PathRasterization {
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
            UploadedBatch::SolidQuads { .. }
            | UploadedBatch::Quads { .. }
            | UploadedBatch::Shadows { .. }
            | UploadedBatch::Paths { .. }
            | UploadedBatch::MonoSprites { .. }
            | UploadedBatch::PolySprites { .. }
            | UploadedBatch::Underlines { .. }
            | UploadedBatch::BackdropBlurs { .. }
            | UploadedBatch::BeginBlur { .. }
            | UploadedBatch::EndBlur { .. }
            | UploadedBatch::CompositeBlur { .. }
            | UploadedBatch::RendererExtensions { .. } => {}
        }
    }
}

fn push_path_mask_draw_step(steps: &mut Vec<DrawStepDescriptor>, step: DrawStepDescriptor) {
    if step.vertex_count == 0 || step.instance_count == 0 {
        return;
    }
    if let Some(previous) = steps.last_mut()
        && path_mask_draw_steps_can_merge(previous, &step)
        && let Some(vertex_count) = previous.vertex_count.checked_add(step.vertex_count)
    {
        previous.vertex_count = vertex_count;
        return;
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
