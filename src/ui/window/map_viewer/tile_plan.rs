use super::model::*;
use super::prelude::*;
use super::tile_render::map_viewer_prefetch_radius;
use super::viewport::*;

pub(super) struct ViewportTilePlanOptions {
    pub(super) viewport: MapViewport,
    pub(super) layout: RenderLayout,
    pub(super) metadata_index_ready: bool,
    pub(super) is_interacting: bool,
    pub(super) drag: Option<DragState>,
}

pub(super) fn build_viewport_tile_plan(options: ViewportTilePlanOptions) -> ViewportTilePlan {
    let center = options.viewport.center_tile(options.layout);
    let visible_bounds = visible_tile_bounds_for_viewport(options.viewport, options.layout, center);
    let visible = visible_bounds
        .map(|bounds| tile_coords_for_visible_bounds(bounds, center))
        .unwrap_or_default();
    let retain_radius = if options.is_interacting {
        DRAG_RETAIN_RADIUS
    } else {
        RETAIN_RADIUS
    };
    let canvas_budget = canvas_tile_image_budget(options.viewport, options.layout);
    let retain_filter = visible_bounds.map(|bounds| {
        retained_tile_filter_for_visible_bounds(bounds, center, retain_radius, canvas_budget)
    });
    let prefetch_radius = if options.is_interacting {
        0
    } else {
        map_viewer_prefetch_radius()
    };
    let mut prefetch = if options.metadata_index_ready && prefetch_radius > 0 {
        visible_bounds
            .map(|bounds| tile_coords_for_bounds(bounds, prefetch_radius, center, canvas_budget))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if options.metadata_index_ready
        && prefetch_radius > 0
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
        prefetch.sort_unstable();
        prefetch.dedup();
    }
    ViewportTilePlan {
        visible,
        visible_bounds,
        prefetch,
        retain_filter,
        center,
        is_interacting: options.is_interacting,
        prefetch_radius,
    }
}

pub(super) fn retained_tile_filter_for_viewport(
    viewport: MapViewport,
    layout: RenderLayout,
    is_dragging: bool,
) -> Option<RetainedTileFilter> {
    let center = viewport.center_tile(layout);
    let visible = visible_tile_bounds_for_viewport(viewport, layout, center)?;
    let radius = if is_dragging {
        DRAG_RETAIN_RADIUS
    } else {
        RETAIN_RADIUS
    };
    Some(retained_tile_filter_for_visible_bounds(
        visible,
        center,
        radius,
        canvas_tile_image_budget(viewport, layout),
    ))
}

pub(super) fn retained_tile_filter_for_visible_bounds(
    visible: TileBounds,
    center: (i32, i32),
    radius: i32,
    max_tiles: usize,
) -> RetainedTileFilter {
    let mut retained = visible.expand(radius);
    clamp_tile_span(&mut retained.min_x, &mut retained.max_x, center.0);
    clamp_tile_span(&mut retained.min_z, &mut retained.max_z, center.1);
    if tile_bounds_count(retained) > max_tiles && tile_bounds_count(visible) <= max_tiles {
        retained = visible;
    } else {
        clamp_tile_count(&mut retained, center, max_tiles);
    }
    RetainedTileFilter::new(visible, retained, radius)
}

pub(super) fn tile_coords_for_visible_bounds(
    visible: TileBounds,
    center: (i32, i32),
) -> Vec<(i32, i32)> {
    collect_circular_tile_coords(visible, visible, 0, center)
}

pub(super) fn tile_coords_for_bounds(
    visible: TileBounds,
    radius: i32,
    center: (i32, i32),
    max_tiles: usize,
) -> Vec<(i32, i32)> {
    let mut expanded = visible.expand(radius);
    clamp_tile_span(&mut expanded.min_x, &mut expanded.max_x, center.0);
    clamp_tile_span(&mut expanded.min_z, &mut expanded.max_z, center.1);
    if tile_bounds_count(expanded) > max_tiles && tile_bounds_count(visible) <= max_tiles {
        expanded = visible;
    } else {
        clamp_tile_count(&mut expanded, center, max_tiles);
    }
    collect_circular_tile_coords(visible, expanded, radius, center)
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
            let mut expanded = projected_bounds.expand(prefetch_radius);
            clamp_tile_span(&mut expanded.min_x, &mut expanded.max_x, center.0);
            clamp_tile_span(&mut expanded.min_z, &mut expanded.max_z, center.1);
            collect_circular_tile_coords(visible_bounds, expanded, prefetch_radius, center)
        })
        .unwrap_or_default()
}
