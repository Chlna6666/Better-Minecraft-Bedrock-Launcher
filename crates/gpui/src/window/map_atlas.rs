use super::*;

impl Window {
    /// Force the map image layer through a real paint pass without discarding any resident image.
    ///
    /// Large map snapshots use a per-frame upload budget. A normal refresh may replay a cached
    /// absolute subtree and therefore never revisit deferred images. This method invalidates the
    /// retained view cache while keeping the atlas, image tile cache and frame bitmaps intact.
    pub fn refresh_map_image_uploads(&mut self) {
        self.force_full_redraw.set(true);
        self.force_view_cache_refresh = true;
        self.refresh();
    }

    /// Rebuild the current window atlas after many short-lived map viewport images were replaced.
    /// The caller must invoke this only while the map camera and compositor are idle.
    pub fn rebuild_map_image_atlas(&mut self) {
        self.animated_image_slots.clear();
        self.image_paint_tile_cache.clear();
        self.rendered_frame.release_image_element_bitmaps();
        self.next_frame.release_image_element_bitmaps();
        self.force_full_redraw.set(true);
        self.force_view_cache_refresh = true;
        self.platform_window
            .trim_gpui_memory(GpuiMemoryTrimLevel::Aggressive);
        self.refresh();
    }
}
