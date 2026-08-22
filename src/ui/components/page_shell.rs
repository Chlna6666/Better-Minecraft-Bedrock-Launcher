//! 页面外壳与统一表面体系。
//!
//! 新版 UI 的设计语言只允许四层表面，全部从这里取：
//! - [`page_panel`]：页面级容器（圆角 LG20、边框 α0.20、panel_bg α0.90、环境阴影）
//! - [`glass_card`]：页内卡片（圆角 MD16、边框 α0.22、surface α0.72、软阴影）
//! - [`inner_well`]：卡片内井面/列表容器（圆角 SM12、field_bg α0.45、边框 α0.12）
//! - 控件层由 `components::button` 等提供（圆角 SM12、按压缩放反馈）
//!
//! 圆角/间距/字号一律使用 `crate::ui::theme::tokens`。

use crate::ui::theme::colors::ThemeColors;
use crate::ui::theme::tokens::{font, radius};
use gpui::{
    BoxShadow, Div, Hsla, IntoElement, ParentElement, Pixels, SharedString, Styled, div,
    linear_color_stop, linear_gradient, point, px, rgb,
};

pub const PAGE_INSET_X: Pixels = px(22.);
// 主窗口标题栏固定为 60px。页面只保留 12px 的透明布局间距，避免旧的 32px
// 顶部空带在高 DPI 下被放大成明显的横向色带；这里不创建任何额外背景或模糊层。
pub const PAGE_INSET_TOP: Pixels = px(72.);
pub const PAGE_INSET_BOTTOM: Pixels = px(20.);
pub const SPLIT_PAGE_SIDEBAR_WIDTH: Pixels = px(280.);
pub const SPLIT_PAGE_GAP: Pixels = px(16.);

/// 页面级容器阴影（唯一的环境阴影配方）。
pub fn panel_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Hsla {
            a: 0.14,
            ..rgb(0x000000).into()
        },
        blur_radius: px(32.),
        spread_radius: px(-10.),
        offset: point(px(0.), px(16.)),
    }]
}

/// 卡片阴影（唯一的卡片投影配方）。
pub fn card_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Hsla {
            a: 0.10,
            ..rgb(0x000000).into()
        },
        blur_radius: px(16.),
        spread_radius: px(-6.),
        offset: point(px(0.), px(6.)),
    }]
}

pub fn page_frame(content: impl IntoElement) -> Div {
    div()
        .absolute()
        .left(PAGE_INSET_X)
        .right(PAGE_INSET_X)
        .top(PAGE_INSET_TOP)
        .bottom(PAGE_INSET_BOTTOM)
        .flex()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(div().flex_1().min_h(px(0.)).min_w(px(0.)).child(content))
}

pub fn split_page(sidebar: impl IntoElement, content: impl IntoElement) -> Div {
    div()
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex()
        .gap(SPLIT_PAGE_GAP)
        .child(sidebar)
        .child(content)
}

pub fn split_sidebar_panel(colors: &ThemeColors) -> Div {
    page_panel(colors)
        .w(SPLIT_PAGE_SIDEBAR_WIDTH)
        .h_full()
        .flex_none()
}

/// 页面级玻璃容器：所有路由页的最外层表面统一用它。
/// 自带顶部主色渐变与高光细线，保证各页“开屏质感”一致。
pub fn page_panel(colors: &ThemeColors) -> Div {
    div()
        .relative()
        .rounded(px(radius::LG))
        .overflow_hidden()
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.90,
            ..colors.settings_panel_bg
        })
        .shadow(panel_shadow())
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(radius::LG))
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(
                        Hsla {
                            a: 0.12,
                            ..colors.accent
                        },
                        0.0,
                    ),
                    linear_color_stop(
                        Hsla {
                            a: 0.02,
                            ..colors.surface
                        },
                        1.0,
                    ),
                )),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(radius::LG))
                .right(px(radius::LG))
                .h(px(1.))
                .bg(Hsla {
                    a: 0.16,
                    ..colors.border
                }),
        )
}

/// 兼容旧名：等价于 [`page_panel`]。
pub fn panel_surface(colors: &ThemeColors) -> Div {
    page_panel(colors)
}

pub fn split_content_panel(colors: &ThemeColors) -> Div {
    page_panel(colors)
        .flex_1()
        .h_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .relative()
        .flex()
        .flex_col()
}

/// 页内卡片：设置卡、房间卡、任务卡等一律用它作基底。
pub fn glass_card(colors: &ThemeColors) -> Div {
    div()
        .rounded(px(radius::MD))
        .overflow_hidden()
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .shadow(card_shadow())
}

/// 卡片内井面：列表容器、日志区、输入组等低一层的表面。
pub fn inner_well(colors: &ThemeColors) -> Div {
    div()
        .rounded(px(radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.45,
            ..colors.settings_field_bg
        })
}

/// 页面/分区大标题：全应用统一 20px BOLD。
pub fn page_title(colors: &ThemeColors, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(font::TITLE))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(colors.text_primary)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_pages_share_one_outer_inset() {
        assert_eq!(PAGE_INSET_X, px(22.));
        assert_eq!(PAGE_INSET_TOP, px(72.));
        assert_eq!(PAGE_INSET_BOTTOM, px(20.));
    }

    #[test]
    fn split_pages_share_manage_layout_metrics() {
        assert_eq!(SPLIT_PAGE_SIDEBAR_WIDTH, px(280.));
        assert_eq!(SPLIT_PAGE_GAP, px(16.));
    }
}
