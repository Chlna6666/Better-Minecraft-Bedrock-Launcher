use crate::ui::components::adaptive::{AdaptiveSizeClass, WindowMetrics};
use crate::ui::components::page_shell::{PAGE_INSET_BOTTOM, PAGE_INSET_TOP, PAGE_INSET_X};
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
    pub(super) plugin_compact: bool,
}

impl SettingsLayout {
    pub(super) fn new(window_width: Pixels, window_height: Pixels) -> Self {
        let metrics = WindowMetrics::new(window_width, window_height);
        let width_px = window_width / px(1.0);
        let height_px = window_height / px(1.0);

        let (page_padding, content_gap) = match metrics.width_class {
            AdaptiveSizeClass::Compact => (px(9.0), px(8.0)),
            AdaptiveSizeClass::Regular => (px(10.0), px(9.0)),
            AdaptiveSizeClass::Spacious => (px(12.0), px(10.0)),
        };

        let content_max_width = if width_px >= 1500.0 {
            px(1180.0)
        } else if width_px >= 1180.0 {
            px(1080.0)
        } else {
            px(960.0)
        };

        // 设置页仍可自适应内部间距，但页面级外框必须与其他路由共用同一组 inset。
        // 这样切换页面时背景面板不会上下跳动，也不会因设置页单独压缩高度而显得更大。
        let available_width =
            (width_px - 2.0 * (PAGE_INSET_X / px(1.0)) - 2.0 * (page_padding / px(1.0))).max(320.0);
        let plugin_max_width = px(if width_px >= 1600.0 {
            available_width.min(1480.0)
        } else {
            available_width
        });

        // 默认窗口（约 1200px 宽）不再强行保留 250px 左栏 + 详情双栏。
        // 该尺寸下改为纵向 master-detail，保证详情头、操作区和正文都有可用宽度。
        let plugin_compact = width_px < 1280.0 || height_px < 690.0;

        Self {
            page_inset_x: PAGE_INSET_X,
            page_inset_top: PAGE_INSET_TOP,
            page_inset_bottom: PAGE_INSET_BOTTOM,
            page_padding,
            content_gap,
            content_max_width,
            plugin_max_width,
            scroll_tabs: width_px < 760.0,
            plugin_compact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_page_reuses_route_page_insets() {
        let compact = SettingsLayout::new(px(720.0), px(620.0));
        let regular = SettingsLayout::new(px(1215.0), px(750.0));
        let spacious = SettingsLayout::new(px(1600.0), px(1000.0));

        for layout in [compact, regular, spacious] {
            assert_eq!(layout.page_inset_x, PAGE_INSET_X);
            assert_eq!(layout.page_inset_top, PAGE_INSET_TOP);
            assert_eq!(layout.page_inset_bottom, PAGE_INSET_BOTTOM);
        }
    }
}
