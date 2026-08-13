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
    pub(super) plugin_compact: bool,
}

impl SettingsLayout {
    pub(super) fn new(window_width: Pixels, window_height: Pixels) -> Self {
        let metrics = WindowMetrics::new(window_width, window_height);
        let width_px = window_width / px(1.0);
        let height_px = window_height / px(1.0);

        let (page_inset_x, page_padding, content_gap) = match metrics.width_class {
            AdaptiveSizeClass::Compact => (px(8.0), px(9.0), px(8.0)),
            AdaptiveSizeClass::Regular => (px(12.0), px(10.0), px(9.0)),
            AdaptiveSizeClass::Spacious => (px(18.0), px(12.0), px(10.0)),
        };

        // 标题栏固定 60px。设置页只保留必要的呼吸空间，避免默认窗口下
        // 因固定 92px 顶部 inset 把插件工作区压缩到不可用高度。
        let page_inset_top = if height_px < 680.0 {
            px(66.0)
        } else if height_px < 820.0 {
            px(72.0)
        } else {
            px(82.0)
        };
        let page_inset_bottom = if height_px < 820.0 { px(8.0) } else { px(14.0) };

        let content_max_width = if width_px >= 1500.0 {
            px(1180.0)
        } else if width_px >= 1180.0 {
            px(1080.0)
        } else {
            px(960.0)
        };

        // 插件页是工作区，不应该套普通设置页的窄内容宽度；让容器吃满可用宽度，
        // 仅在大窗口上留出少量视觉边界。
        let available_width = (width_px
            - 2.0 * (page_inset_x / px(1.0))
            - 2.0 * (page_padding / px(1.0)))
            .max(320.0);
        let plugin_max_width = px(if width_px >= 1600.0 {
            available_width.min(1480.0)
        } else {
            available_width
        });

        // 默认窗口（约 1200px 宽）不再强行保留 250px 左栏 + 详情双栏。
        // 该尺寸下改为纵向 master-detail，保证详情头、操作区和正文都有可用宽度。
        let plugin_compact = width_px < 1280.0 || height_px < 690.0;

        Self {
            page_inset_x,
            page_inset_top,
            page_inset_bottom,
            page_padding,
            content_gap,
            content_max_width,
            plugin_max_width,
            scroll_tabs: width_px < 760.0,
            plugin_compact,
        }
    }
}
