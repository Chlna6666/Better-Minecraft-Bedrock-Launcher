use super::model::*;
use super::prelude::*;
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
    let tile_step = adjusted_grid_step(TILE_WORLD_BLOCKS, block_bounds.0, block_bounds.1, 140);
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
        let chunk_step = adjusted_grid_step(16, block_bounds.0, block_bounds.1, 280);
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

pub(super) fn draw_professional_overlay_canvas(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
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
        if let Some(slime_runs) = slime_runs {
            for run in &slime_runs.runs {
                paint_chunk_rect(
                    bounds,
                    viewport,
                    layout,
                    run.min_chunk_x,
                    run.min_chunk_z,
                    run.max_chunk_x,
                    run.max_chunk_z,
                    Hsla {
                        a: 0.22,
                        ..rgb(0x43d17a).into()
                    },
                    None,
                    window,
                );
            }
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
                        a: 0.12,
                        ..rgb(0x5aa7ff).into()
                    },
                    Some(Hsla {
                        a: 0.75,
                        ..rgb(0x5aa7ff).into()
                    }),
                    window,
                );
            }
        }
        let aggregate_markers = range.block_screen_size * 16.0 < 24.0;
        if overlays.entities {
            if aggregate_markers {
                for marker in &overlay_paint.entity_chunk_markers {
                    paint_chunk_marker(
                        bounds,
                        viewport,
                        layout,
                        *marker,
                        rgb(0xff6b6b).into(),
                        window,
                    );
                }
            } else {
                for point in &overlay_paint.entity_points {
                    paint_point_marker(
                        bounds,
                        viewport,
                        layout,
                        point.block_x,
                        point.block_z,
                        rgb(0xff6b6b).into(),
                        window,
                    );
                }
            }
        }
        if overlays.block_entities {
            if aggregate_markers {
                for marker in &overlay_paint.block_entity_chunk_markers {
                    paint_chunk_marker(
                        bounds,
                        viewport,
                        layout,
                        *marker,
                        rgb(0xc084fc).into(),
                        window,
                    );
                }
            } else {
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
    }

    if let Some(preview) = paste_preview {
        paint_paste_preview_images(bounds, viewport, layout, paste_preview_images, window);
        paint_paste_preview_outline(bounds, viewport, layout, preview, window);
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
    for image in images {
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
            continue;
        }
        let image_bounds = Bounds {
            origin: point(px(left.floor()), px(top.floor())),
            size: size(px((right - left).ceil()), px((bottom - top).ceil())),
        };
        if let Err(error) = window.paint_image(
            image_bounds,
            Corners::all(px(0.0)),
            image.image.clone(),
            0,
            false,
        ) {
            tracing::debug!(?error, "failed to paint paste preview chunk image");
        }
    }
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
        let mut builder = PathBuilder::stroke(px(2.0));
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
    let size_px = px(7.0);
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
    let marker_size = (6.0 + marker.count.min(9) as f32).min(15.0);
    let size_px = px(marker_size);
    window.paint_quad(
        fill(
            Bounds {
                origin: point(px(x) - size_px / 2.0, px(y) - size_px / 2.0),
                size: size(size_px, size_px),
            },
            Hsla { a: 0.82, ..color },
        )
        .corner_radii(size_px / 2.0),
    );
}
