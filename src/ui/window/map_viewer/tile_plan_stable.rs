pub(super) use super::tile_plan_legacy::*;

use super::model::{
    DRAG_RETAIN_RADIUS, DragState, MapViewport, RETAIN_RADIUS, ViewportTilePlan,
};
use super::tile_render::map_viewer_prefetch_radius;
use super::viewport::{
    TileBounds, canvas_tile_image_budget, squared_distance_to_tile_bounds,
    visible_tile_bounds_for_viewport,
};
use bedrock_render::RenderLayout;

// This is only a BMCBL scheduling group. The physical render/cache unit remains one
// bedrock-render tile (8x8 chunks, 128x128 blocks).
const PROGRESSIVE_TILE_CLUSTER_SPAN: i32 = 4;

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
        .map(|bounds| progressive_visible_tile_coords(bounds, center))
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
            .map(|bounds| {
                super::tile_plan_legacy::tile_coords_for_bounds(
                    bounds,
                    prefetch_radius,
                    center,
                    canvas_budget,
                )
            })
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
        // Only an actual pointer drag should use the 48-tile interaction admission slice.
        // Wheel zoom must register the complete visible plan so quickly changing zoom levels
        // cannot skip regions that are never queued before the next camera update.
        is_interacting: actively_dragging,
        prefetch_radius,
    }
}

fn progressive_visible_tile_coords(bounds: TileBounds, center: (i32, i32)) -> Vec<(i32, i32)> {
    if bounds.min_x > bounds.max_x || bounds.min_z > bounds.max_z {
        return Vec::new();
    }

    let span = PROGRESSIVE_TILE_CLUSTER_SPAN.max(1);
    let cluster_bounds = TileBounds {
        min_x: bounds.min_x.div_euclid(span),
        max_x: bounds.max_x.div_euclid(span),
        min_z: bounds.min_z.div_euclid(span),
        max_z: bounds.max_z.div_euclid(span),
    };
    let center_cluster = (center.0.div_euclid(span), center.1.div_euclid(span));

    let mut clusters = Vec::with_capacity(
        usize::try_from(
            cluster_bounds
                .max_x
                .saturating_sub(cluster_bounds.min_x)
                .saturating_add(1),
        )
        .unwrap_or(0)
        .saturating_mul(
            usize::try_from(
                cluster_bounds
                    .max_z
                    .saturating_sub(cluster_bounds.min_z)
                    .saturating_add(1),
            )
            .unwrap_or(0),
        ),
    );
    for cluster_z in cluster_bounds.min_z..=cluster_bounds.max_z {
        for cluster_x in cluster_bounds.min_x..=cluster_bounds.max_x {
            clusters.push((cluster_x, cluster_z));
        }
    }
    clusters.sort_by_key(|&(cluster_x, cluster_z)| {
        let dx = i64::from(cluster_x) - i64::from(center_cluster.0);
        let dz = i64::from(cluster_z) - i64::from(center_cluster.1);
        (
            dx.abs().max(dz.abs()),
            dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)),
            cluster_z,
            cluster_x,
        )
    });

    let mut coords = Vec::with_capacity(super::viewport::tile_bounds_count(bounds));
    for (cluster_x, cluster_z) in clusters {
        let cluster_min_x = cluster_x.saturating_mul(span).max(bounds.min_x);
        let cluster_max_x = cluster_x
            .saturating_mul(span)
            .saturating_add(span.saturating_sub(1))
            .min(bounds.max_x);
        let cluster_min_z = cluster_z.saturating_mul(span).max(bounds.min_z);
        let cluster_max_z = cluster_z
            .saturating_mul(span)
            .saturating_add(span.saturating_sub(1))
            .min(bounds.max_z);

        let mut cluster_tiles = Vec::with_capacity((span * span) as usize);
        for z in cluster_min_z..=cluster_max_z {
            for x in cluster_min_x..=cluster_max_x {
                cluster_tiles.push((x, z));
            }
        }
        cluster_tiles.sort_by_key(|&(x, z)| {
            (
                squared_distance_to_tile_bounds(
                    x,
                    z,
                    TileBounds {
                        min_x: center.0,
                        max_x: center.0,
                        min_z: center.1,
                        max_z: center.1,
                    },
                ),
                z,
                x,
            )
        });
        coords.extend(cluster_tiles);
    }
    coords
}
