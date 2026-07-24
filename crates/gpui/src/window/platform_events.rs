use super::*;

impl Window {
    pub(super) fn window_origin_changed(&mut self, cx: &mut App) {
        let scale_factor = self.platform_window.scale_factor();
        let viewport_size = self.platform_window.content_size();
        let display_id = self.platform_window.display().map(|display| display.id());

        if self.scale_factor == scale_factor && self.display_id == display_id {
            return;
        }

        if self.viewport_size != viewport_size {
            self.content_bounds_changed(cx);
            return;
        }

        self.scale_factor = scale_factor;
        self.display_id = display_id;

        self.refresh();

        self.bounds_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    pub(super) fn content_bounds_changed(&mut self, cx: &mut App) {
        let scale_factor = self.platform_window.scale_factor();
        let viewport_size = self.platform_window.content_size();
        let display_id = self.platform_window.display().map(|display| display.id());

        let text_rasterization_changed =
            self.scale_factor != scale_factor || self.display_id != display_id;

        if self.scale_factor == scale_factor
            && self.viewport_size == viewport_size
            && self.display_id == display_id
        {
            return;
        }

        self.scale_factor = scale_factor;
        self.viewport_size = viewport_size;
        self.display_id = display_id;
        if text_rasterization_changed {
            self.text_system.clear_layout_cache();
            self.text_system.clear_raster_cache();
            self.sprite_atlas.clear_glyphs();
        }
        self.force_full_redraw.set(true);

        self.refresh();

        self.bounds_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    pub(crate) fn appearance_changed(&mut self, cx: &mut App) {
        self.appearance = self.platform_window.appearance();
        self.refresh();

        self.appearance_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }
}
