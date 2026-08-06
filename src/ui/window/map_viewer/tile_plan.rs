use super::model::*;
use super::prelude::*;
use super::tile_render::map_viewer_prefetch_radius;
use super::viewport::*;

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
        .map(|bounds| center_first_visible_tile_coords(bounds, center))
        .unwrap_or_default();

    let actively_dragging = options.drag.is_some();
    // Wheel zoom is also a camera interaction. Keep a wider retained border while it is
    // active, but do not classify it as a drag for the foreground registration limit.
    let retain_radius = if options.is_interacting {
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
        // Only an actual pointer drag should use the interaction admission path. Wheel zoom
        // registers the complete visible plan so quickly changing zoom levels cannot leave
        // coordinates that were never admitted to the render queue.
        is_interacting: actively_dragging,
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
    center_first_visible_tile_coords(visible, center)
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

fn center_first_visible_tile_coords(bounds: TileBounds, center: (i32, i32)) -> Vec<(i32, i32)> {
    if bounds.min_x > bounds.max_x || bounds.min_z > bounds.max_z {
        return Vec::new();
    }

    // Keep one globally ordered queue. The previous 4x4 cluster order was only locally
    // center-first: when the camera center was near a cluster edge, distant tiles in the
    // center cluster were submitted before adjacent tiles that were one step from the camera.
    // A wheel zoom could then cancel the batch and repeatedly skip those near-center holes.
    let mut coords = Vec::with_capacity(tile_bounds_count(bounds));
    for z in bounds.min_z..=bounds.max_z {
        for x in bounds.min_x..=bounds.max_x {
            coords.push((x, z));
        }
    }
    coords.sort_unstable_by_key(|&coord| tile_distance_sort_key(coord, center));
    coords
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn visible_tile_plan_is_globally_center_first_across_old_cluster_edges() {
        let bounds = TileBounds {
            min_x: -5,
            max_x: 6,
            min_z: -5,
            max_z: 6,
        };
        let center = (3, 3);
        let coords = center_first_visible_tile_coords(bounds, center);

        assert_eq!(coords.first().copied(), Some(center));
        assert_eq!(coords.len(), tile_bounds_count(bounds));
        assert_eq!(
            coords.iter().copied().collect::<BTreeSet<_>>().len(),
            coords.len()
        );
        assert!(coords.windows(2).all(|window| {
            tile_distance_sort_key(window[0], center) <= tile_distance_sort_key(window[1], center)
        }));
    }

    #[test]
    fn visible_tile_plan_handles_center_outside_bounds() {
        let bounds = TileBounds {
            min_x: 10,
            max_x: 12,
            min_z: 20,
            max_z: 22,
        };
        let center = (0, 0);
        let coords = center_first_visible_tile_coords(bounds, center);

        assert_eq!(coords.len(), 9);
        assert!(coords.windows(2).all(|window| {
            tile_distance_sort_key(window[0], center) <= tile_distance_sort_key(window[1], center)
        }));
    }
}
