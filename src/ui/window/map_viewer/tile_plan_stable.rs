pub(super) use super::tile_plan_legacy::*;

use super::model::{DRAG_RETAIN_RADIUS, DragState, MapViewport, RETAIN_RADIUS};
use super::tile_render::map_viewer_prefetch_radius;
use super::viewport::{
    RetainedTileFilter, TileBounds, canvas_tile_image_budget, retained_tile_filter_for_visible_bounds,
    tile_coords_for_bounds, tile_coords_for_visible_bounds, visible_tile_bounds_for_viewport,
};
use bedrock_render::RenderLayout;
use std::collections::BTreeSet;

pub(super) struct ViewportTilePlanOptions {
    pub(super) viewport: MapViewport,
    pub(super) layout: RenderLayout,
    pub(super) is_interacting: bool,
    pub(super) drag: Option<DragState>,
}

pub(super) fn build_viewport_tile_plan(options: ViewportTilePlanOptions) -> ViewportTilePlan {
    let center = options.viewport.center_tile(options.layout);
    let visible_bounds = visible_tile_bounds_for_viewport(options.viewport, options.layout, center);
    let visible = visible_bounds
        .map(|bounds| tile_coords_for_visible_bounds(bounds, center))
        .unwrap_or_default();

    let actively_dragging = options.drag.is_some();
    let retain_radius = if actively_dragging {
        DRAG_RETAIN_RADIUS
    } else {
        RETAIN_RADIUS
    };
    let canvas_budget = canvas_tile_image_budget(options.viewport, options.layout);
    let retain_filter = visible_bounds.map(|bounds| {
        retained_tile_filter_for_visible_bounds(bounds, center, retain_radius, canvas_budget)
    });

    let prefetch_radius = if actively_dragging {
        0
    } else {
        map_viewer_prefetch_radius()
    };
    let mut prefetch = if prefetch_radius > 0 {
        visible_bounds
            .map(|bounds| tile_coords_for_bounds(bounds, prefetch_radius, center, canvas_budget))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    if prefetch_radius > 0
        && let (Some(visible_bounds), Some(drag)) = (visible_bounds, options.drag)
    {
        prefetch.extend(projected_drag_prefetch_tiles(
            options.viewport,
            options.layout,
            visible_bounds,
            center,
            prefetch_radius,
            drag,
        ));
        let mut seen = BTreeSet::new();
        prefetch.retain(|coord| seen.insert(*coord));
    }

    ViewportTilePlan {
        visible,
        visible_bounds,
        prefetch,
        retain_filter,
        center,
        is_interacting: actively_dragging,
        prefetch_radius,
    }
}

fn projected_drag_prefetch_tiles(
    viewport: MapViewport,
    layout: RenderLayout,
    visible_bounds: TileBounds,
    center: (i32, i32),
    prefetch_radius: i32,
    drag: DragState,
) -> Vec<(i32, i32)> {
    let drag_bias = drag.last_movement_x.abs().max(drag.last_movement_y.abs());
    if drag_bias <= 0.0 {
        return Vec::new();
    }
    let mut projected_viewport = viewport;
    let projected_shift = drag_bias.max(32.0);
    projected_viewport.offset_x += drag.last_movement_x.signum() * projected_shift;
    projected_viewport.offset_y += drag.last_movement_y.signum() * projected_shift;
    visible_tile_bounds_for_viewport(projected_viewport, layout, center)
        .map(|projected_bounds| {
            let expanded = projected_bounds.expand(prefetch_radius);
            super::tile_plan_legacy::tile_coords_for_bounds(
                visible_bounds,
                prefetch_radius,
                center,
                super::viewport::tile_bounds_count(expanded).max(1),
            )
        })
        .unwrap_or_default()
}
