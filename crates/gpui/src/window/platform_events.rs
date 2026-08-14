use super::*;

impl Window {
    fn invalidate_text_rasterization(&mut self) {
        self.text_system.clear_layout_cache();
        self.text_system.clear_raster_cache();
        self.sprite_atlas.clear_glyphs();
    }

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

        let text_rasterization_changed =
            self.scale_factor != scale_factor || self.display_id != display_id;

        self.scale_factor = scale_factor;
        self.display_id = display_id;

        // 窗口移动到另一显示器，或系统在窗口映射后修正 DPI 时，即使客户区尺寸
        // 没有变化，也必须丢弃旧 DPI/显示器生成的字体位图。否则旧 glyph atlas 会被
        // 新缩放比例继续采样，表现为普通窗口字体发虚，而一次最大化/Resize 后又恢复清晰。
        if text_rasterization_changed {
            self.invalidate_text_rasterization();
            self.force_full_redraw.set(true);
        }

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
            self.invalidate_text_rasterization();
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
