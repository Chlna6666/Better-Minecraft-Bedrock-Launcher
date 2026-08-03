// The legacy lifecycle imports these values through `use super::model::*`. Local definitions
// intentionally shadow that glob import for the active stable map path.
//
// Wheel zoom must not be treated as a 120 ms paint freeze: bedrock-render can keep producing
// independent 8x8-chunk tiles while BMCBL incrementally publishes every ready batch.
const VIEWPORT_INTERACTION_IDLE_DELAY: std::time::Duration = std::time::Duration::ZERO;
const VIEWPORT_TILE_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
const INTERACTION_VISIBLE_TILE_FOREGROUND_WORK_LIMIT: usize = usize::MAX;

// Preserve the compact 4x4 scheduling order produced by tile_plan_stable. The legacy probe
// helper re-sorts coordinates into long square-ring segments, which makes low-zoom progress
// appear as separated horizontal and vertical stripes.
fn select_manifest_probe_tiles(
    visible_tiles: &[(i32, i32)],
    prefetch_tiles: &[(i32, i32)],
    _center: (i32, i32),
    scanned_tiles: &std::collections::BTreeSet<(i32, i32)>,
) -> Vec<(i32, i32)> {
    let mut selected = Vec::with_capacity(super::model::TILE_MANIFEST_PROBE_BATCH_TILES);
    let mut seen = std::collections::BTreeSet::new();
    for coord in visible_tiles
        .iter()
        .chain(prefetch_tiles.iter())
        .copied()
    {
        if selected.len() >= super::model::TILE_MANIFEST_PROBE_BATCH_TILES {
            break;
        }
        if scanned_tiles.contains(&coord) || !seen.insert(coord) {
            continue;
        }
        selected.push(coord);
    }
    selected
}

fn screen_image_bounds(
    _bounds: gpui::Bounds<gpui::Pixels>,
    _viewport: super::model::MapViewport,
    _image: &super::canvas::ScreenPaintImage,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    None
}

include!("lifecycle.rs");
