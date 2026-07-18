use super::model::*;
use super::prelude::*;
use super::tile_state::MapRenderRange;
use super::viewport::*;

pub(super) fn draw_map_canvas(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    overlays: OverlayOptions,
    colors: ThemeColors,
    window: &mut Window,
) {
    let block_bounds = viewport_block_bounds(viewport, layout);
    let tile_step = grid_step_for_block_bounds(TILE_WORLD_BLOCKS, block_bounds, 140);
    draw_grid_lines(
        bounds,
        viewport,
        layout,
        tile_step,
        Hsla {
            a: 0.20,
            ..colors.text_primary
        },
        px(1.25),
        window,
    );

    let chunk_pixels =
        16.0 * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32 * viewport.scale;
    if overlays.dense_grid || chunk_pixels >= 18.0 {
        let chunk_step = grid_step_for_block_bounds(16, block_bounds, 280);
        draw_grid_lines(
            bounds,
            viewport,
            layout,
            chunk_step,
            Hsla {
                a: 0.16,
                ..colors.accent
            },
            px(1.0),
            window,
        );
    }
    if overlays.axis {
        draw_axes(bounds, viewport, layout, window);
    }
    if overlays.ruler {
        draw_ruler(bounds, viewport, layout, colors, window);
    }
}

pub(super) fn grid_step_for_block_bounds(
    base_step: i32,
    block_bounds: (i32, i32, i32, i32),
    max_lines: i32,
) -> i32 {
    adjusted_grid_step(base_step, block_bounds.0, block_bounds.2, max_lines).max(
        adjusted_grid_step(base_step, block_bounds.1, block_bounds.3, max_lines),
    )
}

pub(super) fn draw_professional_overlay_canvas(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    dimension: Dimension,
    overlays: OverlayOptions,
    overlay_paint: Option<&ProfessionalOverlayPaintCache>,
    slime_runs: Option<&SlimeOverlayRunCache>,
    selection: Option<ChunkSelection>,
    paste_preview: Option<&PastePreview>,
    paste_preview_images: &[PastePreviewImage],
    highlighted_window: Option<&SlimeChunkWindow>,
    colors: ThemeColors,
    window: &mut Window,
) {
    let Some(range) = region_render_range_for_viewport(viewport, layout) else {
        return;
    };
    if overlays.slime_chunks {
        let cached_bounds = slime_runs.and_then(|cache| {
            if cache.bounds.dimension != dimension || cache.runs.is_empty() {
                return None;
            }
            paint_cached_slime_runs(bounds, viewport, layout, range, cache, window)
        });
        if dimension == Dimension::Overworld {
            paint_slime_grid_overlay(bounds, viewport, layout, range, cached_bounds, window);
        }
    }

    if let Some(overlay_paint) = overlay_paint {
        if overlays.hardcoded_spawn_areas {
            for rect in &overlay_paint.hardcoded_spawn_rects {
                paint_block_rect(
                    bounds,
                    viewport,
                    layout,
                    rect.min_block_x,
                    rect.min_block_z,
                    rect.max_block_x,
                    rect.max_block_z,
                    Hsla {
                        a: 0.20,
                        ..rgb(0xf2b84b).into()
                    },
                    Some(Hsla {
                        a: 0.70,
                        ..rgb(0xf2b84b).into()
                    }),
                    window,
                );
            }
        }
        if overlays.villages {
            for rect in &overlay_paint.village_rects {
                paint_chunk_rect(
                    bounds,
                    viewport,
                    layout,
                    rect.min_chunk_x,
                    rect.min_chunk_z,
                    rect.max_chunk_x,
                    rect.max_chunk_z,
                    Hsla {
                        a: 0.08,
                        ..rgb(0x2f9bff).into()
                    },
                    Some(Hsla {
                        a: 0.88,
                        ..rgb(0x2f9bff).into()
                    }),
                    window,
                );
            }
        }
        if overlays.entities {
            let mut avatar_requests = Vec::new();
            const MAX_ENTITY_AVATAR_REQUESTS: usize = 2_048;
            let canvas_width = bounds.size.width / px(1.0);
            let canvas_height = bounds.size.height / px(1.0);
            for point in &overlay_paint.entity_points {
                paint_point_marker(
                    bounds,
                    viewport,
                    layout,
                    point.block_x,
                    point.block_z,
                    rgb(0xf97316).into(),
                    window,
                );
                if avatar_requests.len() >= MAX_ENTITY_AVATAR_REQUESTS {
                    continue;
                }
                let screen_x = overlay_marker_screen_x(bounds, viewport, layout, point.block_x);
                let screen_y = overlay_marker_screen_y(bounds, viewport, layout, point.block_z);
                if screen_x < -52.0
                    || screen_y < -52.0
                    || screen_x > canvas_width + 52.0
                    || screen_y > canvas_height + 52.0
                {
                    continue;
                }
                if let Some(image) = point
                    .identifier
                    .as_ref()
                    .and_then(|id| overlay_paint.entity_avatars.get(id))
                {
                    avatar_requests.push(entity_avatar_request(
                        bounds,
                        viewport,
                        layout,
                        point.block_x,
                        point.block_z,
                        image,
                    ));
                }
            }
            match window.paint_images_budgeted(avatar_requests, 32) {
                Ok(progress) if progress.deferred_requests > 0 => {
                    window.request_animation_frame();
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(?error, "failed to paint entity avatars");
                }
            }
        }
        if overlays.block_entities {
            for point in &overlay_paint.block_entity_points {
                paint_point_marker(
                    bounds,
                    viewport,
                    layout,
                    point.block_x,
                    point.block_z,
                    rgb(0xc084fc).into(),
                    window,
                );
            }
        }
        if overlays.pending_ticks {
            for marker in &overlay_paint.pending_tick_chunk_markers {
                paint_chunk_marker(
                    bounds,
                    viewport,
                    layout,
                    *marker,
                    rgb(0xfbbf24).into(),
                    window,
                );
            }
        }
    }

    if let Some(selection) = selection {
        let selection_bounds = selection.bounds();
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            selection_bounds.min_chunk_x,
            selection_bounds.min_chunk_z,
            selection_bounds.max_chunk_x,
            selection_bounds.max_chunk_z,
            Hsla {
                a: 0.10,
                ..colors.accent
            },
            Some(Hsla {
                a: 0.92,
                ..colors.accent
            }),
            window,
        );
        paint_selection_resize_handles(bounds, viewport, layout, selection_bounds, colors, window);
    }

    if let Some(preview) = paste_preview {
        paint_paste_preview_images(bounds, viewport, layout, paste_preview_images, window);
        paint_pending_paste_chunks(bounds, viewport, layout, preview, window);
        if !preview.is_writing() {
            paint_paste_preview_outline(bounds, viewport, layout, preview, window);
        }
    }

    if let Some(window_candidate) = highlighted_window {
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            window_candidate.min_chunk_x,
            window_candidate.min_chunk_z,
            window_candidate.max_chunk_x,
            window_candidate.max_chunk_z,
            Hsla {
                a: 0.14,
                ..rgb(0x9ef01a).into()
            },
            Some(Hsla {
                a: 0.95,
                ..rgb(0x9ef01a).into()
            }),
            window,
        );
    }
}

fn paint_selection_resize_handles(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    selection: SlimeChunkBounds,
    colors: ThemeColors,
    window: &mut Window,
) {
    let left = screen_x_for_block(
        bounds,
        viewport,
        layout,
        selection.min_chunk_x.saturating_mul(16),
    );
    let right = screen_x_for_block(
        bounds,
        viewport,
        layout,
        selection.max_chunk_x.saturating_add(1).saturating_mul(16),
    );
    let top = screen_y_for_block(
        bounds,
        viewport,
        layout,
        selection.min_chunk_z.saturating_mul(16),
    );
    let bottom = screen_y_for_block(
        bounds,
        viewport,
        layout,
        selection.max_chunk_z.saturating_add(1).saturating_mul(16),
    );
    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    for (x, y) in [
        (left, top),
        (center_x, top),
        (right, top),
        (right, center_y),
        (right, bottom),
        (center_x, bottom),
        (left, bottom),
        (left, center_y),
    ] {
        let outer = Bounds {
            origin: point(px(x - 4.5), px(y - 4.5)),
            size: size(px(9.0), px(9.0)),
        };
        let inner = Bounds {
            origin: point(px(x - 3.0), px(y - 3.0)),
            size: size(px(6.0), px(6.0)),
        };
        window.paint_quad(fill(outer, colors.surface));
        window.paint_quad(fill(inner, colors.accent));
    }
}

fn paint_pending_paste_chunks(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    preview: &PastePreview,
    window: &mut Window,
) {
    let Some(progress) = preview.write_progress else {
        return;
    };
    if progress.awaiting_tile_refresh {
        return;
    }
    for chunk in preview.targets.iter().skip(progress.completed) {
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            chunk.x,
            chunk.z,
            chunk.x,
            chunk.z,
            Hsla {
                a: 0.58,
                ..rgb(0x64748b).into()
            },
            Some(Hsla {
                a: 0.72,
                ..rgb(0x94a3b8).into()
            }),
            window,
        );
    }
}

fn paint_cached_slime_runs(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    range: MapRenderRange,
    cache: &SlimeOverlayRunCache,
    window: &mut Window,
) -> Option<(i32, i32, i32, i32)> {
    let min_chunk_x = cache.bounds.min_chunk_x.max(range.min_chunk_x);
    let max_chunk_x = cache.bounds.max_chunk_x.min(range.max_chunk_x);
    let min_chunk_z = cache.bounds.min_chunk_z.max(range.min_chunk_z);
    let max_chunk_z = cache.bounds.max_chunk_z.min(range.max_chunk_z);
    if min_chunk_x > max_chunk_x || min_chunk_z > max_chunk_z {
        return None;
    }
    for run in &cache.runs {
        let run_min_x = run.min_chunk_x.max(min_chunk_x);
        let run_max_x = run.max_chunk_x.min(max_chunk_x);
        let run_min_z = run.min_chunk_z.max(min_chunk_z);
        let run_max_z = run.max_chunk_z.min(max_chunk_z);
        if run_min_x > run_max_x || run_min_z > run_max_z {
            continue;
        }
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            run_min_x,
            run_min_z,
            run_max_x,
            run_max_z,
            Hsla {
                a: 0.22,
                ..rgb(0x43d17a).into()
            },
            None,
            window,
        );
    }
    Some((min_chunk_x, min_chunk_z, max_chunk_x, max_chunk_z))
}

fn paint_slime_grid_overlay(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    range: MapRenderRange,
    covered_bounds: Option<(i32, i32, i32, i32)>,
    window: &mut Window,
) {
    let block_bounds = viewport_block_bounds(viewport, layout);
    let chunk_step = grid_step_for_block_bounds(16, block_bounds, 280).max(1);
    let mut chunk_z = range.min_chunk_z;
    while chunk_z <= range.max_chunk_z {
        let max_chunk_z = chunk_z
            .saturating_add(chunk_step.saturating_sub(1))
            .min(range.max_chunk_z);
        let mut chunk_x = range.min_chunk_x;
        while chunk_x <= range.max_chunk_x {
            let max_chunk_x = chunk_x
                .saturating_add(chunk_step.saturating_sub(1))
                .min(range.max_chunk_x);
            let sample = ChunkPos {
                x: chunk_x.saturating_add(max_chunk_x.saturating_sub(chunk_x) / 2),
                z: chunk_z.saturating_add(max_chunk_z.saturating_sub(chunk_z) / 2),
                dimension: Dimension::Overworld,
            };
            let sample_is_cached = covered_bounds.is_some_and(|(min_x, min_z, max_x, max_z)| {
                sample.x >= min_x && sample.x <= max_x && sample.z >= min_z && sample.z <= max_z
            });
            if !sample_is_cached && is_slime_chunk(sample) {
                paint_chunk_rect(
                    bounds,
                    viewport,
                    layout,
                    sample.x,
                    sample.z,
                    sample.x,
                    sample.z,
                    Hsla {
                        a: 0.22,
                        ..rgb(0x43d17a).into()
                    },
                    None,
                    window,
                );
            }
            if chunk_x == range.max_chunk_x {
                break;
            }
            chunk_x = max_chunk_x.saturating_add(1);
        }
        if chunk_z == range.max_chunk_z {
            break;
        }
        chunk_z = max_chunk_z.saturating_add(1);
    }
}

fn paint_paste_preview_outline(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    preview: &PastePreview,
    window: &mut Window,
) {
    let Some(min_x) = preview.targets.iter().map(|chunk| chunk.x).min() else {
        return;
    };
    let Some(max_x) = preview.targets.iter().map(|chunk| chunk.x).max() else {
        return;
    };
    let Some(min_z) = preview.targets.iter().map(|chunk| chunk.z).min() else {
        return;
    };
    let Some(max_z) = preview.targets.iter().map(|chunk| chunk.z).max() else {
        return;
    };
    paint_chunk_rect(
        bounds,
        viewport,
        layout,
        min_x,
        min_z,
        max_x,
        max_z,
        Hsla {
            a: 0.0,
            ..rgb(0xf59e0b).into()
        },
        Some(Hsla {
            a: 0.95,
            ..rgb(0xf59e0b).into()
        }),
        window,
    );
    paint_chunk_rect(
        bounds,
        viewport,
        layout,
        preview.target_anchor.x,
        preview.target_anchor.z,
        preview.target_anchor.x,
        preview.target_anchor.z,
        Hsla {
            a: 0.0,
            ..rgb(0x22c55e).into()
        },
        Some(Hsla {
            a: 0.95,
            ..rgb(0x22c55e).into()
        }),
        window,
    );
}

fn paint_paste_preview_images(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    images: &[PastePreviewImage],
    window: &mut Window,
) {
    let requests = images.iter().filter_map(|image| {
        let left = screen_x_for_block(bounds, viewport, layout, image.target.x.saturating_mul(16));
        let top = screen_y_for_block(bounds, viewport, layout, image.target.z.saturating_mul(16));
        let right = screen_x_for_block(
            bounds,
            viewport,
            layout,
            image.target.x.saturating_add(1).saturating_mul(16),
        );
        let bottom = screen_y_for_block(
            bounds,
            viewport,
            layout,
            image.target.z.saturating_add(1).saturating_mul(16),
        );
        if right <= left || bottom <= top {
            return None;
        }
        let image_bounds = Bounds {
            origin: point(px(left.floor()), px(top.floor())),
            size: size(px((right - left).ceil()), px((bottom - top).ceil())),
        };
        Some(ImagePaintRequest::new(image_bounds, image.image.as_ref()))
    });
    if let Err(error) = window.paint_images(requests) {
        tracing::debug!(?error, "failed to paint paste preview chunk images");
    }
}

fn entity_avatar_request<'a>(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
    block_z: f32,
    image: &'a RenderImage,
) -> ImagePaintRequest<'a> {
    let x = overlay_marker_screen_x(bounds, viewport, layout, block_x);
    let y = overlay_marker_screen_y(bounds, viewport, layout, block_z);
    let size_px = px(overlay_icon_size_px(viewport, layout));
    ImagePaintRequest::new(
        Bounds {
            origin: point(px(x) - size_px / 2.0, px(y) - size_px / 2.0),
            size: size(size_px, size_px),
        },
        image,
    )
}

pub(super) fn overlay_icon_size_px(viewport: MapViewport, layout: RenderLayout) -> f32 {
    let chunk_screen_size =
        16.0 * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32 * viewport.scale;
    if !chunk_screen_size.is_finite() {
        return 12.0;
    }
    (chunk_screen_size * 0.35).clamp(16.0, 52.0)
}

pub(super) fn paint_chunk_rect(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    min_chunk_x: i32,
    min_chunk_z: i32,
    max_chunk_x: i32,
    max_chunk_z: i32,
    fill_color: Hsla,
    stroke_color: Option<Hsla>,
    window: &mut Window,
) {
    paint_block_rect(
        bounds,
        viewport,
        layout,
        min_chunk_x.saturating_mul(16),
        min_chunk_z.saturating_mul(16),
        max_chunk_x.saturating_add(1).saturating_mul(16),
        max_chunk_z.saturating_add(1).saturating_mul(16),
        fill_color,
        stroke_color,
        window,
    );
}

pub(super) fn paint_block_rect(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    min_block_x: i32,
    min_block_z: i32,
    max_block_x: i32,
    max_block_z: i32,
    fill_color: Hsla,
    stroke_color: Option<Hsla>,
    window: &mut Window,
) {
    let left = screen_x_for_block(bounds, viewport, layout, min_block_x);
    let top = screen_y_for_block(bounds, viewport, layout, min_block_z);
    let right = screen_x_for_block(bounds, viewport, layout, max_block_x);
    let bottom = screen_y_for_block(bounds, viewport, layout, max_block_z);
    if right <= left || bottom <= top {
        return;
    }
    let rect = Bounds {
        origin: point(px(left.floor()), px(top.floor())),
        size: size(px((right - left).ceil()), px((bottom - top).ceil())),
    };
    if fill_color.a > 0.0 {
        window.paint_quad(fill(rect, fill_color));
    }
    if let Some(stroke_color) = stroke_color {
        let stroke_width = (right - left).min(bottom - top).clamp(0.5, 2.0);
        let mut builder = PathBuilder::stroke(px(stroke_width));
        builder.move_to(rect.origin);
        builder.line_to(point(rect.right(), rect.top()));
        builder.line_to(point(rect.right(), rect.bottom()));
        builder.line_to(point(rect.left(), rect.bottom()));
        builder.line_to(rect.origin);
        if let Ok(path) = builder.build() {
            window.paint_path(path, stroke_color);
        }
    }
}

pub(super) fn paint_point_marker(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
    block_z: f32,
    color: Hsla,
    window: &mut Window,
) {
    let x = overlay_marker_screen_x(bounds, viewport, layout, block_x);
    let y = overlay_marker_screen_y(bounds, viewport, layout, block_z);
    let size_px = px((overlay_icon_size_px(viewport, layout) * 0.36).clamp(6.0, 18.0));
    window.paint_quad(
        fill(
            Bounds {
                origin: point(px(x) - size_px / 2.0, px(y) - size_px / 2.0),
                size: size(size_px, size_px),
            },
            Hsla { a: 0.88, ..color },
        )
        .corner_radii(px(3.5)),
    );
}

fn overlay_marker_screen_x(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
) -> f32 {
    bounds.left() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_x
                    + block_x * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_x
                    + (block_x - range.min_chunk_x as f32 * 16.0) * range.block_screen_size
            },
        )
}

fn overlay_marker_screen_y(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_z: f32,
) -> f32 {
    bounds.top() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_y
                    + block_z * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_y
                    + (block_z - range.min_chunk_z as f32 * 16.0) * range.block_screen_size
            },
        )
}

pub(super) fn paint_chunk_marker(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    marker: ChunkOverlayMarker,
    color: Hsla,
    window: &mut Window,
) {
    let block_x = marker.chunk_x as f32 * 16.0 + 8.0;
    let block_z = marker.chunk_z as f32 * 16.0 + 8.0;
    let x = bounds.left() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_x
                    + block_x * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_x
                    + (block_x - range.min_chunk_x as f32 * 16.0) * range.block_screen_size
            },
        );
    let y = bounds.top() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_y
                    + block_z * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_y
                    + (block_z - range.min_chunk_z as f32 * 16.0) * range.block_screen_size
            },
        );
    let marker_size = (overlay_icon_size_px(viewport, layout) * 0.55 + marker.count.min(9) as f32)
        .clamp(7.0, 30.0);
    let size_px = px(marker_size);
    window.paint_quad(fill(
        Bounds {
            origin: point(
                px(x) - size_px / 2.0 - px(1.0),
                px(y) - size_px / 2.0 - px(1.0),
            ),
            size: size(size_px + px(2.0), size_px + px(2.0)),
        },
        rgb(0x000000),
    ));
    window.paint_quad(fill(
        Bounds {
            origin: point(px(x) - size_px / 2.0, px(y) - size_px / 2.0),
            size: size(size_px, size_px),
        },
        Hsla { a: 0.82, ..color },
    ));
}
