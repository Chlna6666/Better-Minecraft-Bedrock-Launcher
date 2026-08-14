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
            a: 0.36,
            ..colors.border
        })
        // 设置页是承载信息的主工作表面，而不是漂浮玻璃层。
        // 保持近实色，只让自定义背景极轻微透出；导航和弹层再负责材质层级。
        .bg(Hsla {
            a: 0.96,
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
