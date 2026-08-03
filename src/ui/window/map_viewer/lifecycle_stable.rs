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
const TILE_MANIFEST_PROBE_BATCH_TILES: usize = 1;

// Preserve the compact 4x4 scheduling order produced by tile_plan_stable. The legacy probe
// helper re-sorts coordinates into long square-ring segments, which makes low-zoom progress
// appear as separated horizontal and vertical stripes.
fn select_manifest_probe_tiles(
    visible_tiles: &[(i32, i32)],
    prefetch_tiles: &[(i32, i32)],
    _center: (i32, i32),
    scanned_tiles: &std::collections::BTreeSet<(i32, i32)>,
) -> Vec<(i32, i32)> {
    let mut selected = Vec::with_capacity(TILE_MANIFEST_PROBE_BATCH_TILES);
    let mut seen = std::collections::BTreeSet::new();
    for coord in visible_tiles
        .iter()
        .chain(prefetch_tiles.iter())
        .copied()
    {
        if selected.len() >= TILE_MANIFEST_PROBE_BATCH_TILES {
            break;
        }
        if scanned_tiles.contains(&coord) || !seen.insert(coord) {
            continue;
        }
        selected.push(coord);
    }
    selected
}

// Keep the frontend paint snapshot on coarse tile pages instead of rebuilding it for every
// fractional wheel step. The snapshot still contains original 128x128 RenderImage tiles; this
// only stabilizes which retained Arc handles are submitted to GPUI. A small guard margin keeps
// the previous and next zoom views overlapping while the final visible plan catches up.
const CANVAS_PAINT_PAGE_TILES: i32 = 32;
const CANVAS_PAINT_GUARD_TILES: i32 = 8;

fn paint_tile_bounds_for_viewport(
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    radius: i32,
) -> Option<super::viewport::TileBounds> {
    let bounds = super::viewport::paint_tile_bounds_for_viewport(
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
    Some(super::viewport::TileBounds {
        min_x: align_min(bounds.min_x),
        min_z: align_min(bounds.min_z),
        max_x: align_max(bounds.max_x),
        max_z: align_max(bounds.max_z),
    })
}

fn screen_image_bounds(
    _bounds: gpui::Bounds<gpui::Pixels>,
    _viewport: super::model::MapViewport,
    _image: &super::canvas::ScreenPaintImage,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    None
}

include!("lifecycle.rs");
