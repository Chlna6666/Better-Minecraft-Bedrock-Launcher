// Re-export the existing viewport implementation, but make the canvas paint bounds stable across
// fractional wheel-zoom steps. Both lifecycle.rs and canvas_stable.rs resolve this module, so the
// bounds used to decide whether a snapshot is stale are exactly the same as the bounds stored in
// the snapshot.
pub(super) use super::viewport_base::*;

const CANVAS_PAINT_PAGE_TILES: i32 = 32;
const CANVAS_PAINT_GUARD_TILES: i32 = 8;

pub(super) fn paint_tile_bounds_for_viewport(
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    radius: i32,
) -> Option<TileBounds> {
    let bounds = super::viewport_base::paint_tile_bounds_for_viewport(
        viewport,
        layout,
        radius.saturating_add(CANVAS_PAINT_GUARD_TILES),
    )?;

    let align_min = |value: i32| {
        value
            .div_euclid(CANVAS_PAINT_PAGE_TILES)
            .saturating_mul(CANVAS_PAINT_PAGE_TILES)
    };
    let align_max = |value: i32| {
        value
            .div_euclid(CANVAS_PAINT_PAGE_TILES)
            .saturating_add(1)
            .saturating_mul(CANVAS_PAINT_PAGE_TILES)
            .saturating_sub(1)
    };

    Some(TileBounds {
        min_x: align_min(bounds.min_x),
        max_x: align_max(bounds.max_x),
        min_z: align_min(bounds.min_z),
        max_z: align_max(bounds.max_z),
    })
}
