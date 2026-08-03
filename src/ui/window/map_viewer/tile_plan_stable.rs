pub(super) use super::tile_plan_legacy::*;

use super::model::{DRAG_RETAIN_RADIUS, DragState, MapViewport, RETAIN_RADIUS};
use super::tile_render::map_viewer_prefetch_radius;
use super::viewport::{
    canvas_tile_image_budget, tile_coords_for_bounds, tile_coords_for_visible_bounds,
    visible_tile_bounds_for_viewport,
};
use bedrock_render::RenderLayout;

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
        super::tile_plan_legacy::retained_tile_filter_for_visible_bounds(
            bounds,
            center,
            retain_radius,
            canvas_budget,
        )
    });

    let prefetch_radius = if actively_dragging {
        0
    } else {
        map_viewer_prefetch_radius()
    };
    let prefetch = if prefetch_radius > 0 {
        visible_bounds
            .map(|bounds| tile_coords_for_bounds(bounds, prefetch_radius, center, canvas_budget))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

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
