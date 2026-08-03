use super::*;

impl Window {
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
