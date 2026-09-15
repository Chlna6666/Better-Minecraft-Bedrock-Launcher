use super::lifecycle::RetainedInvalidationScope;
use super::*;

impl Window {
    /// Force the current retained map-image subtree through a real paint pass without discarding
    /// any resident image.
    ///
    /// Large map snapshots use a per-frame upload budget. A normal animation frame may replay a
    /// cached absolute subtree and therefore never revisit deferred images. When this is called
    /// from an element paint, capture that exact retained path and invalidate only its subtree on
    /// the following frame. Callers without retained provenance keep the conservative full-window
    /// fallback.
    pub fn refresh_map_image_uploads(&mut self) {
        if let (Some(retained_id), Some(view_id)) = (
            self.current_retained_element_id(),
            self.current_view_or_root(),
        ) {
            self.on_next_frame(move |window, _cx| {
                if window.invalidator.invalidate_retained_path_with_scope(
                    view_id,
                    Some(&retained_id),
                    RetainedInvalidationScope::InvalidateSubtree,
                ) {
                    window.schedule_interactive_animation_frame();
                }
            });
            return;
        }

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
