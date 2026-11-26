use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DrawMode {
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
    /// Raster a signed content-space tile from the shared packed streams.
    /// Admission is checked by encode_tile_draw_commands before encoding.
    TileContent {
        batch_start: usize,
        batch_end: usize,
        tile: gfx_core::RasterTile,
    },
}

impl DrawMode {
    fn batch_range(self, len: usize) -> std::ops::Range<usize> {
        match self {
            Self::Present => 0..len,
            Self::BackdropSegment { batch_start, batch_end }
            | Self::BlurContent { batch_start, batch_end }
            | Self::TileContent { batch_start, batch_end, .. } => {
                let end = batch_end.min(len);
                batch_start.min(end)..end
            }
        }
    }
}

#[cfg(test)]
pub(super) fn encode_draw_commands(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    blend_pipelines: BlendPipelines,
    quad_bind_group: BindGroupId,
    shadow_bind_group: BindGroupId,
    path_bind_group: BindGroupId,
    sprite_bind_group: impl FnMut(AtlasTextureId) -> Option<BindGroupId>,
    underline_bind_group: BindGroupId,
    backdrop_blur_bind_group: BindGroupId,
    mode: DrawMode,
) -> Vec<RenderCommand> {
    let mut steps = Vec::new();
    encode_draw_commands_into(
        upload,
        pipelines,
        blend_pipelines,
        quad_bind_group,
        shadow_bind_group,
        path_bind_group,
        sprite_bind_group,
        underline_bind_group,
        |_| Some(backdrop_blur_bind_group),
        mode,
        &mut steps,
    );
    steps
}

pub(super) fn encode_draw_commands_into(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    blend_pipelines: BlendPipelines,
    quad_bind_group: BindGroupId,
    shadow_bind_group: BindGroupId,
    path_bind_group: BindGroupId,
    sprite_bind_group: impl FnMut(AtlasTextureId) -> Option<BindGroupId>,
    underline_bind_group: BindGroupId,
    backdrop_blur_bind_group: impl FnMut(BackdropBlurConfig) -> Option<BindGroupId>,
    mode: DrawMode,
    steps: &mut Vec<RenderCommand>,
) {
    encode_clipped_draw_commands(
        upload,
        pipelines,
        blend_pipelines,
        quad_bind_group,
        shadow_bind_group,
        path_bind_group,
        sprite_bind_group,
        underline_bind_group,
        backdrop_blur_bind_group,
        mode,
        None,
        steps,
    );
}

/// For a retained offscreen region, skip wholly disjoint STATIC draw batches
/// before GPU submission. Animated/unknown batches remain conservative.
pub(super) fn encode_clipped_draw_commands(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    blend_pipelines: BlendPipelines,
    quad_bind_group: BindGroupId,
    shadow_bind_group: BindGroupId,
    path_bind_group: BindGroupId,
    mut sprite_bind_group: impl FnMut(AtlasTextureId) -> Option<BindGroupId>,
    underline_bind_group: BindGroupId,
    mut backdrop_blur_bind_group: impl FnMut(BackdropBlurConfig) -> Option<BindGroupId>,
    mode: DrawMode,
    source_scissor: Option<ScissorRect>,
    steps: &mut Vec<RenderCommand>,
) {
    steps.clear();
    // Root source batches execute in the independent offscreen pass. Final
    // presentation samples only its compositor descriptor, with no Filter scope markers.
    let mode = match (mode, upload.root_color) {
        (DrawMode::Present, Some(root)) => DrawMode::BackdropSegment {
            batch_start: root.content_end,
            batch_end: root.content_end + 1,
        },
        _ => mode,
    };
    let tile = match mode {
        DrawMode::TileContent { tile, .. } => Some(tile),
        _ => None,
    };
    let range = mode.batch_range(upload.batches.len());
    steps.reserve(range.len().saturating_add(1));
    let first = range.start;
    let mut blur_depth = 0usize;
    for (local_index, batch) in upload.batches[range].iter().enumerate() {
        let batch_index = first + local_index;
        if (source_scissor.is_some() || tile.is_some())
            && upload.batch_visual_bounds.get(batch_index).copied().flatten()
                .is_some_and(|bounds| {
                    source_scissor.is_some_and(|scissor| static_batch_misses_scissor(bounds, scissor))
                        || tile.is_some_and(|tile| static_batch_misses_tile(bounds, tile))
                })
        {
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
                        &mut backdrop_blur_bind_group,
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
            UploadedBatch::SolidQuads { first, count } => push_draw_command(
                steps,
                DrawCommand {
                    pipeline: blend_pipelines.solid_quads,
                    bind_groups: bind_group_list([quad_bind_group]),
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: count,
                    first_instance: first,
                    scissor: None,
                },
            ),
            UploadedBatch::Quads { first, count } => push_draw_command(
                steps,
                DrawCommand {
                    pipeline: blend_pipelines.quads,
                    bind_groups: bind_group_list([quad_bind_group]),
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: count,
                    first_instance: first,
                    scissor: None,
                },
            ),
            UploadedBatch::Shadows { first, count } => push_draw_command(
                steps,
                DrawCommand {
                    pipeline: blend_pipelines.shadows,
                    bind_groups: bind_group_list([shadow_bind_group]),
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: count,
                    first_instance: first,
                    scissor: None,
                },
            ),
            UploadedBatch::PathRasterization { .. } => {}
            UploadedBatch::Paths { first, count } => push_draw_command(
                steps,
                DrawCommand {
                    pipeline: pipelines.paths,
                    bind_groups: bind_group_list([path_bind_group]),
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
                if let Some(bind_group) = sprite_bind_group(texture_id) {
                    #[cfg(target_os = "windows")]
                    let pipeline = if texture_id.kind == AtlasTextureKind::Subpixel {
                        blend_pipelines.subpixel_sprites
                    } else {
                        blend_pipelines.mono_sprites
                    };
                    #[cfg(not(target_os = "windows"))]
                    let pipeline = blend_pipelines.mono_sprites;
                    push_draw_command(
                        steps,
                        DrawCommand {
                            pipeline,
                            bind_groups: bind_group_list([bind_group]),
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
                color_only,
            } => {
                if let Some(bind_group) = sprite_bind_group(texture_id) {
                    push_image_draw_command(
                        steps,
                        blend_pipelines,
                        DrawCommand {
                            pipeline: if color_only {
                                blend_pipelines.poly_sprites_color
                            } else {
                                blend_pipelines.poly_sprites
                            },
                            bind_groups: bind_group_list([bind_group]),
                            vertex_count: 4,
                            first_vertex: 0,
                            instance_count: count,
                            first_instance: first,
                            scissor: None,
                        },
                    );
                }
            }
            UploadedBatch::Underlines { first, count } => push_draw_command(
                steps,
                DrawCommand {
                    pipeline: blend_pipelines.underlines,
                    bind_groups: bind_group_list([underline_bind_group]),
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: count,
                    first_instance: first,
                    scissor: None,
                },
            ),
            UploadedBatch::BackdropBlurs { first, count } => {
                upload.for_each_backdrop_blur_run(first, count, |run| {
                    let Some(bind_group) = backdrop_blur_bind_group(run.config) else {
                        return;
                    };
                    push_draw_command(
                        steps,
                        DrawCommand {
                            pipeline: blend_pipelines.backdrop_blurs,
                            bind_groups: bind_group_list([bind_group]),
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
    if steps.is_empty() && source_scissor.is_none() && tile.is_none() {
        steps.push(RenderCommand::Draw(DrawCommand {
            pipeline: blend_pipelines.solid_quads,
            bind_groups: bind_group_list([quad_bind_group]),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 0,
            first_instance: 0,
            scissor: None,
        }));
    }
}

/// Tile inputs are encoded once in content coordinates. Bind groups point
/// to the same instance buffers as the enclosing frame, with tile globals.
/// Never silently produce a partial cache when a source is unsupported or
/// when an atlas binding is unavailable. Paths, LCD coverage and effects
/// stay on their ordinary overlay/rendering path, as in GPUI Fast.
#[allow(clippy::too_many_arguments)]
pub(super) fn encode_tile_draw_commands(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    tile: gfx_core::RasterTile,
    range: std::ops::Range<usize>,
    quad_bind_group: BindGroupId,
    shadow_bind_group: BindGroupId,
    underline_bind_group: BindGroupId,
    mut sprite_bind_group: impl FnMut(AtlasTextureId) -> Option<BindGroupId>,
    steps: &mut Vec<RenderCommand>,
) -> bool {
    steps.clear();
    let Some(batches) = upload.batches.get(range.clone()) else { return false; };
    let Some(bounds) = upload.batch_visual_bounds.get(range.clone()) else { return false; };
    // A finite static AABB is also proof that the batch contains no
    // renderer-owned geometry animation or unbounded sprite transform.
    if !batches.iter().zip(bounds).all(|(batch, bounds)| {
        bounds.is_some() && match *batch {
            UploadedBatch::SolidQuads { .. }
            | UploadedBatch::Quads { .. }
            | UploadedBatch::Shadows { .. }
            | UploadedBatch::Underlines { .. } => true,
            UploadedBatch::MonoSprites { texture_id, .. } => texture_id.kind != AtlasTextureKind::Subpixel,
            UploadedBatch::PolySprites { .. } => true,
            _ => false,
        }
    }) {
        return false;
    }
    let mut missing_binding = false;
    encode_clipped_draw_commands(
        upload, pipelines, pipelines.premultiplied,
        quad_bind_group, shadow_bind_group, quad_bind_group,
        |texture_id| {
            let binding = sprite_bind_group(texture_id);
            missing_binding |= binding.is_none();
            binding
        },
        underline_bind_group, |_| None,
        DrawMode::TileContent { batch_start: range.start, batch_end: range.end, tile },
        None, steps,
    );
    if missing_binding {
        steps.clear();
        return false;
    }
    true
}

fn static_batch_misses_tile(bounds: crate::Bounds<crate::ScaledPixels>, tile: gfx_core::RasterTile) -> bool {
    let left = f64::from(tile.origin.x);
    let top = f64::from(tile.origin.y);
    f64::from(bounds.right().0) <= left
        || f64::from(bounds.bottom().0) <= top
        || f64::from(bounds.origin.x.0) >= left + f64::from(tile.extent.width())
        || f64::from(bounds.origin.y.0) >= top + f64::from(tile.extent.height())
}

/// Bounds are in drawable/device pixels, as are the render-pass scissors.
fn static_batch_misses_scissor(
    bounds: crate::Bounds<crate::ScaledPixels>,
    scissor: ScissorRect,
) -> bool {
    let left = bounds.origin.x.0;
    let top = bounds.origin.y.0;
    let right = bounds.right().0;
    let bottom = bounds.bottom().0;
    if ![left, top, right, bottom].into_iter().all(f32::is_finite) {
        return false;
    }
    right <= scissor.x as f32
        || bottom <= scissor.y as f32
        || left >= scissor.x.saturating_add(scissor.width) as f32
        || top >= scissor.y.saturating_add(scissor.height) as f32
}

fn push_blur_composite_step(
    upload: &FrameUpload,
    pipeline: RenderPipelineId,
    backdrop_blur_bind_group: &mut impl FnMut(BackdropBlurConfig) -> Option<BindGroupId>,
    index: u32,
    steps: &mut Vec<RenderCommand>,
) {
    let Some(config) = upload.blur_config_at(index) else {
        return;
    };
    let Some(bind_group) = backdrop_blur_bind_group(config) else {
        return;
    };
    push_draw_command(
        steps,
        DrawCommand {
            pipeline,
            bind_groups: bind_group_list([bind_group]),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 1,
            first_instance: index,
            scissor: None,
        },
    );
}

fn push_draw_command(steps: &mut Vec<RenderCommand>, step: DrawCommand) {
    if step.vertex_count == 0 || step.instance_count == 0 {
        return;
    }
    if let Some(RenderCommand::Draw(previous)) = steps.last_mut()
        && commands_can_merge(previous, &step)
        && let Some(instance_count) = previous.instance_count.checked_add(step.instance_count)
    {
        previous.instance_count = instance_count;
        return;
    }
    steps.push(RenderCommand::Draw(step));
}

// Scissor/blur skipping can bring separate image batches together. Keep their
// original merge opportunity even when only one batch qualifies for specialization.
fn push_image_draw_command(
    steps: &mut Vec<RenderCommand>,
    pipelines: BlendPipelines,
    mut step: DrawCommand,
) {
    if let Some(RenderCommand::Draw(previous)) = steps.last_mut()
        && previous.pipeline != step.pipeline
        && (previous.pipeline == pipelines.poly_sprites
            || previous.pipeline == pipelines.poly_sprites_color)
    {
        let selected_pipeline = step.pipeline;
        step.pipeline = previous.pipeline;
        if commands_can_merge(previous, &step) {
            previous.pipeline = pipelines.poly_sprites;
            step.pipeline = pipelines.poly_sprites;
        } else {
            step.pipeline = selected_pipeline;
        }
    }
    push_draw_command(steps, step);
}

fn commands_can_merge(previous: &DrawCommand, next: &DrawCommand) -> bool {
    previous.pipeline == next.pipeline
        && previous.bind_groups == next.bind_groups
        && previous.vertex_count == next.vertex_count
        && previous.first_vertex == next.first_vertex
        && previous.scissor == next.scissor
        && previous.first_instance.checked_add(previous.instance_count) == Some(next.first_instance)
}

pub(super) fn packet_scissor(
    packet: &PresentationPacket,
    target_size: DrawableSize,
) -> Option<ScissorRect> {
    if packet.partial_present_mode != PartialPresentMode::Partial || packet.dirty_region.is_full() {
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
    pub(super) step: RenderCommand,
}

#[cfg(test)]
pub(super) fn encode_blur_target_passes(
    pipelines: &Pipelines,
    targets: &BackdropBlurTargets,
    frame_resource_index: usize,
    passes: &mut Vec<BackdropBlurRenderPass>,
) {
    let configs: Vec<_> = targets
        .variants
        .iter()
        .take(targets.active_variant_count)
        .map(|variant| variant.config)
        .collect();
    encode_blur_config_passes(pipelines, targets, frame_resource_index, &configs, passes);
}

pub(super) fn encode_blur_config_passes(
    pipelines: &Pipelines,
    targets: &BackdropBlurTargets,
    frame_resource_index: usize,
    configs: &[BackdropBlurConfig],
    passes: &mut Vec<BackdropBlurRenderPass>,
) {
    let Some(source_bind_group) = targets
        .source_pass_bind_groups
        .get(frame_resource_index)
        .copied()
    else {
        passes.clear();
        return;
    };
    encode_blur_source_passes(
        pipelines,
        targets,
        frame_resource_index,
        configs,
        source_bind_group,
        passes,
    );
}

pub(super) fn encode_blur_source_passes(
    pipelines: &Pipelines,
    targets: &BackdropBlurTargets,
    frame_resource_index: usize,
    configs: &[BackdropBlurConfig],
    source_bind_group: BindGroupId,
    passes: &mut Vec<BackdropBlurRenderPass>,
) {
    passes.clear();
    passes.reserve(configs.len().saturating_mul(2));

    for config in configs {
        let Some((variant_index, variant)) = targets
            .variants
            .iter()
            .take(targets.active_variant_count)
            .enumerate()
            .find(|(_, variant)| variant.config == *config)
        else {
            continue;
        };
        let [horizontal, vertical] = variant.levels.as_slice() else {
            continue;
        };
        let Some(horizontal_bind_group) = horizontal
            .pass_bind_groups
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
            step: RenderCommand::Draw(DrawCommand {
                pipeline: pipelines.backdrop_blur_downsample,
                bind_groups: bind_group_list([source_bind_group]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: pass_base,
                scissor: None,
            }),
        });
        passes.push(BackdropBlurRenderPass {
            target_texture_view: vertical.texture_view,
            step: RenderCommand::Draw(DrawCommand {
                pipeline: pipelines.backdrop_blur_upsample,
                bind_groups: bind_group_list([horizontal_bind_group]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: pass_base.saturating_add(1),
                scissor: None,
            }),
        });
    }
}

#[cfg(test)]
pub(super) fn encoded_path_commands(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    path_rasterization_bind_group: BindGroupId,
) -> Vec<RenderCommand> {
    let mut steps = Vec::new();
    encode_path_commands(
        upload,
        pipelines,
        path_rasterization_bind_group,
        &mut steps,
    );
    steps
}

pub(super) fn encode_path_commands(
    upload: &FrameUpload,
    pipelines: &Pipelines,
    path_rasterization_bind_group: BindGroupId,
    steps: &mut Vec<RenderCommand>,
) {
    steps.clear();
    steps.reserve(upload.batches.len());
    for batch in &upload.batches {
        match *batch {
            UploadedBatch::PathRasterization {
                first_vertex,
                vertex_count,
            } => push_path_command(
                steps,
                DrawCommand {
                    pipeline: pipelines.path_rasterization,
                    bind_groups: bind_group_list([path_rasterization_bind_group]),
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

fn push_path_command(steps: &mut Vec<RenderCommand>, step: DrawCommand) {
    if step.vertex_count == 0 || step.instance_count == 0 {
        return;
    }
    if let Some(RenderCommand::Draw(previous)) = steps.last_mut()
        && path_commands_can_merge(previous, &step)
        && let Some(vertex_count) = previous.vertex_count.checked_add(step.vertex_count)
    {
        previous.vertex_count = vertex_count;
        return;
    }
    steps.push(RenderCommand::Draw(step));
}

fn path_commands_can_merge(previous: &DrawCommand, next: &DrawCommand) -> bool {
    previous.pipeline == next.pipeline
        && previous.bind_groups == next.bind_groups
        && previous.instance_count == next.instance_count
        && previous.first_instance == next.first_instance
        && previous.scissor == next.scissor
        && previous.first_vertex.checked_add(previous.vertex_count) == Some(next.first_vertex)
}

#[cfg(test)]
mod static_batch_culling_tests {
    use super::*;

    #[test]
    fn tile_culling_preserves_signed_content_coordinates() {
        let tile = gfx_core::RasterTile {
            origin: gfx_core::SignedOrigin2d { x: -512, y: 65536 },
            extent: gfx_core::Extent2d::new(512, 512).unwrap(),
        };
        let rect = |x, y| crate::bounds(
            crate::point(crate::ScaledPixels(x), crate::ScaledPixels(y)),
            crate::size(crate::ScaledPixels(10.), crate::ScaledPixels(10.)),
        );
        assert!(!static_batch_misses_tile(rect(-500., 65540.), tile));
        assert!(static_batch_misses_tile(rect(0., 65540.), tile));
        assert!(static_batch_misses_tile(rect(-500., 65000.), tile));
    }

    #[test]
    fn fully_disjoint_static_quad_batch_can_be_skipped() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(40.0), crate::ScaledPixels(30.0)),
        );
        assert!(static_batch_misses_scissor(
            bounds,
            ScissorRect {
                x: 600,
                y: 400,
                width: 20,
                height: 20,
            }
        ));
        assert!(!static_batch_misses_scissor(
            bounds,
            ScissorRect {
                x: 20,
                y: 25,
                width: 20,
                height: 20,
            }
        ));
    }
}
