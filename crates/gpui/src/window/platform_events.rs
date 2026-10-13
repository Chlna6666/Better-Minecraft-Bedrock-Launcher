use super::*;

impl Window {
    fn invalidate_text_rasterization(&mut self) {
        // Platform shaping/layout is expressed in logical Pixels and its API does not depend on
        // the window DPI/scale factor. Moving between displays therefore invalidates glyph bitmap
        // bounds and atlas residency, but not the shaped line/layout identity. Keeping the line
        // cache avoids reshaping every visible string after a DPI transition; the measured-layout
        // fingerprint still includes scale factor and will re-run measurement where required.
        self.text_system.clear_raster_cache();
        self.sprite_atlas.clear_glyphs();
        #[cfg(target_os = "windows")]
        crate::platform::advance_text_rasterization_generation();
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
        let live_resize_active = self.platform_window.is_live_resizing();

        let text_rasterization_changed =
            self.scale_factor != scale_factor || self.display_id != display_id;

        if self.scale_factor == scale_factor
            && self.viewport_size == viewport_size
            && self.display_id == display_id
            && self.live_resize_active == live_resize_active
        {
            return;
        }

        self.scale_factor = scale_factor;
        self.viewport_size = viewport_size;
        self.display_id = display_id;
        self.live_resize_active = live_resize_active;
        self.mouse_position = self.platform_window.mouse_position();
        if text_rasterization_changed {
            self.invalidate_text_rasterization();
        }
        self.force_full_redraw.set(true);

        #[cfg(target_os = "windows")]
        if crate::diagnostics::gpu_owner::resize_trace_enabled() {
            log::info!(
                target: "gpui_resize_trace",
                "gpui_resize stage=viewport window_id={} logical_px={:.3}x{:.3} scale={:.5} physical_px={}x{}",
                self.handle.window_id().as_u64(),
                self.viewport_size.width.0,
                self.viewport_size.height.0,
                self.scale_factor,
                (self.viewport_size.width.0 * self.scale_factor).round() as u32,
                (self.viewport_size.height.0 * self.scale_factor).round() as u32
            );
        }

        // A change in the available root layout space is itself a dependency.
        // Relying only on explicit viewport_size()/bounds() reads misses wrapper
        // views such as BMCBL's RootView, which return div().size_full() without
        // ever querying the Window dimensions. Retained replay can then keep an
        // old-size subtree even after the GPU swapchain has grown, leaving black
        // client-area strips. Invalidate this window's root cache boundary on
        // actual geometry changes, not on unrelated input, hover, or native move.
        // Descendant cached views remain eligible for retained replay when their
        // own bounds and paint context are unchanged.
        let root_view_id = self.root.as_ref().map(AnyView::entity_id);
        if let Some(root_id) = root_view_id {
            self.invalidator.invalidate_view(root_id, cx);
        }
        if !live_resize_active {
            // Only image owners whose target size was deferred need a final
            // resample. Static siblings keep their retained scene and atlas data.
            for view_id in self.live_resize_image_views.drain() {
                if Some(view_id) != root_view_id {
                    self.invalidator.invalidate_view(view_id, cx);
                }
            }
        }
        let viewport_dependent_views = self
            .viewport_dependent_views
            .borrow()
            .iter()
            .copied()
            .collect::<SmallVec<[_; 8]>>();
        for view_id in viewport_dependent_views {
            if Some(view_id) != root_view_id {
                self.invalidator.invalidate_view(view_id, cx);
            }
        }
        self.redraw_without_view_cache_refresh();

        self.bounds_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    pub(crate) fn appearance_changed(&mut self, cx: &mut App) {
        let appearance = self.platform_window.appearance();
        let changed = self.appearance != appearance;
        self.appearance = appearance;
        if changed {
            self.refresh();
        }

        self.appearance_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }
}
