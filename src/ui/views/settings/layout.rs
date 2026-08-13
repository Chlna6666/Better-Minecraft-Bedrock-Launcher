use crate::ui::components::adaptive::{AdaptiveSizeClass, WindowMetrics};
use gpui::{Pixels, px};

#[derive(Clone, Copy, Debug)]
pub(super) struct SettingsLayout {
    pub(super) page_inset_x: Pixels,
    pub(super) page_inset_top: Pixels,
    pub(super) page_inset_bottom: Pixels,
    pub(super) page_padding: Pixels,
    pub(super) content_gap: Pixels,
    pub(super) content_max_width: Pixels,
    pub(super) plugin_max_width: Pixels,
    pub(super) scroll_tabs: bool,
    pub(super) stack_plugins: bool,
    pub(super) plugin_list_height: Pixels,
}

impl SettingsLayout {
    pub(super) fn new(window_width: Pixels, window_height: Pixels) -> Self {
        let metrics = WindowMetrics::new(window_width, window_height);
        let width_px = window_width / px(1.0);
        let height_px = window_height / px(1.0);

        let (page_inset_x, page_padding, content_gap) = match metrics.width_class {
            AdaptiveSizeClass::Compact => (px(8.0), px(10.0), px(8.0)),
            AdaptiveSizeClass::Regular => (px(14.0), px(12.0), px(10.0)),
            AdaptiveSizeClass::Spacious => (px(22.0), px(14.0), px(12.0)),
        };
        let dense_height = height_px < 720.0;
        let page_inset_top = if dense_height { px(78.0) } else { px(92.0) };
        let page_inset_bottom = if dense_height { px(10.0) } else { px(20.0) };

        let content_max_width = if width_px >= 1500.0 {
            px(1180.0)
        } else if width_px >= 1180.0 {
            px(1080.0)
        } else {
            px(960.0)
        };
        let plugin_max_width = if width_px >= 1500.0 {
            px(1380.0)
        } else if width_px >= 1180.0 {
            px(1240.0)
        } else {
            px(1040.0)
        };

        Self {
            page_inset_x,
            page_inset_top,
            page_inset_bottom,
            page_padding,
            content_gap,
            content_max_width,
            plugin_max_width,
            scroll_tabs: width_px < 820.0,
            stack_plugins: width_px < 980.0,
            plugin_list_height: if width_px < 640.0 {
                px(152.0)
            } else {
                px(176.0)
            },
        }
    }
}
