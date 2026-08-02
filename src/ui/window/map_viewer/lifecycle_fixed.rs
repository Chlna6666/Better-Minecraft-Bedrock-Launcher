// Keep the previous complete viewport frame while progressive dirty rectangles arrive.
// The included lifecycle module uses this local helper only to decide whether the first
// progressive rectangle should discard the retained frame. Returning no candidate here
// makes progressive rectangles append over the transformed retained frame; the final
// complete frame still replaces the entire list atomically.
fn screen_image_bounds(
    _bounds: gpui::Bounds<gpui::Pixels>,
    _viewport: super::model::MapViewport,
    _image: &super::canvas::ScreenPaintImage,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    None
}

// A cancelled composite request keeps its physical permit until its worker unwinds.
// Reserve enough slots for the replacement generation so rapid zoom/drag changes cannot
// leave pending viewport work waiting on a permit whose completion callback was superseded.
fn map_concurrent_render_batches() -> usize {
    super::tile_render::map_concurrent_render_batches().max(4)
}

include!("lifecycle.rs");
