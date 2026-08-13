use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::layout::SettingsLayout;
use gpui::*;

pub(super) fn page_shell(
    content: impl IntoElement,
    colors: &ThemeColors,
    layout: &SettingsLayout,
) -> Div {
    div()
        .absolute()
        .left(layout.page_inset_x)
        .right(layout.page_inset_x)
        .top(layout.page_inset_top)
        .bottom(layout.page_inset_bottom)
        .min_w(px(0.))
        .min_h(px(0.))
        .rounded(px(crate::ui::theme::tokens::radius::LG))
        .overflow_hidden()
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.18,
            ..colors.bg
        })
        .child(
            div()
                .size_full()
                .min_w(px(0.))
                .min_h(px(0.))
                .p(layout.page_padding)
                .child(content),
        )
}
