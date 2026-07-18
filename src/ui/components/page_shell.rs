use crate::ui::theme::colors::ThemeColors;
use gpui::{Div, Hsla, IntoElement, ParentElement, Pixels, Styled, div, px};

pub const PAGE_INSET_X: Pixels = px(22.);
pub const PAGE_INSET_TOP: Pixels = px(92.);
pub const PAGE_INSET_BOTTOM: Pixels = px(20.);
pub const SPLIT_PAGE_SIDEBAR_WIDTH: Pixels = px(280.);
pub const SPLIT_PAGE_GAP: Pixels = px(16.);

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
    panel_surface(colors)
        .w(SPLIT_PAGE_SIDEBAR_WIDTH)
        .h_full()
        .flex_none()
}

pub fn panel_surface(colors: &ThemeColors) -> Div {
    div()
        .rounded(px(18.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
}

pub fn split_content_panel(colors: &ThemeColors) -> Div {
    div()
        .flex_1()
        .h_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .relative()
        .overflow_hidden()
        .rounded(px(12.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
        .flex()
        .flex_col()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_pages_share_one_outer_inset() {
        assert_eq!(PAGE_INSET_X, px(22.));
        assert_eq!(PAGE_INSET_TOP, px(92.));
        assert_eq!(PAGE_INSET_BOTTOM, px(20.));
    }

    #[test]
    fn split_pages_share_manage_layout_metrics() {
        assert_eq!(SPLIT_PAGE_SIDEBAR_WIDTH, px(280.));
        assert_eq!(SPLIT_PAGE_GAP, px(16.));
    }
}
