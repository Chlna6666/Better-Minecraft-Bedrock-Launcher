// The legacy lifecycle imports these values through `use super::model::*`. Local definitions
// intentionally shadow that glob import for the active stable map path.
//
// Wheel zoom must not be treated as a 120 ms paint freeze: bedrock-render can keep producing
// independent 8x8-chunk tiles while BMCBL incrementally publishes every ready batch.
const VIEWPORT_INTERACTION_IDLE_DELAY: std::time::Duration = std::time::Duration::ZERO;
const VIEWPORT_TILE_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
const INTERACTION_VISIBLE_TILE_FOREGROUND_WORK_LIMIT: usize = usize::MAX;

// One BMCBL seed can make bedrock-render return a much wider internal scan area. Submit one
// seed at a time and mark the complete returned coverage before selecting the next seed. This
// removes overlapping probe jobs without changing bedrock-render's physical tile/cache unit.

// Preserve the compact 4x4 scheduling order produced by tile_plan_stable. The legacy probe
// helper re-sorts coordinates into long square-ring segments, which makes low-zoom progress
// appear as separated horizontal and vertical stripes.

// The lifecycle and canvas builder must use byte-for-byte identical paint bounds. The previous
// stable wrapper expanded and aligned this value to 32-tile pages, while canvas_stable stored the
// unaligned viewport bounds in TilePaintSnapshot. Every sync therefore treated the snapshot as
// stale and rebuilt hundreds of macro pages per second, even when the camera had not moved.
fn paint_tile_bounds_for_viewport(
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    radius: i32,
) -> Option<super::viewport::TileBounds> {
    super::viewport::paint_tile_bounds_for_viewport(viewport, layout, radius)
}

fn screen_image_bounds(
    _bounds: gpui::Bounds<gpui::Pixels>,
    _viewport: super::model::MapViewport,
    _image: &super::canvas::ScreenPaintImage,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    None
}

include!("lifecycle.rs");
