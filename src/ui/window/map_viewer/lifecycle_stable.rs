// The legacy lifecycle imports these values through `use super::model::*`. Local definitions
// intentionally shadow that glob import for the active stable map path.
//
// Wheel zoom must not be treated as a 120 ms paint freeze: bedrock-render can keep producing
// independent 8x8-chunk tiles while BMCBL incrementally publishes every ready batch.
const VIEWPORT_INTERACTION_IDLE_DELAY: std::time::Duration = std::time::Duration::ZERO;
const VIEWPORT_TILE_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
const INTERACTION_VISIBLE_TILE_FOREGROUND_WORK_LIMIT: usize = usize::MAX;

fn screen_image_bounds(
    _bounds: gpui::Bounds<gpui::Pixels>,
    _viewport: super::model::MapViewport,
    _image: &super::canvas::ScreenPaintImage,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    None
}

include!("lifecycle.rs");