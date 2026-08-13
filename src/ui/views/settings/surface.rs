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
            a: 0.30,
            ..colors.border
        })
        // 设置页本身是主工作表面，不再把它做成近乎透明的玻璃层。
        // 背景只保留轻微透出，卡片则承担更高的不透明层级，避免多层浅色玻璃叠加。
        .bg(Hsla {
            a: 0.82,
            ..colors.settings_panel_bg
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
