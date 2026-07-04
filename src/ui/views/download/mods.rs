use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::DownloadPageState;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn render_mod_panel(colors: &ThemeColors, _state: &DownloadPageState) -> Div {
    // Icon container with gradient-like layered effect
    let icon_container = div()
        .w(px(80.))
        .h(px(80.))
        .rounded(px(20.))
        .bg(Hsla {
            a: 0.08,
            ..colors.accent
        })
        .border_1()
        .border_color(Hsla {
            a: 0.15,
            ..colors.accent
        })
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(lucide_icons::icon_layers())
                .size(px(36.))
                .text_color(Hsla {
                    a: 0.60,
                    ..colors.accent
                }),
        );

    let badge = div()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(999.))
        .bg(Hsla {
            a: 0.10,
            ..colors.text_secondary
        })
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors.text_muted)
        .child("即将推出");

    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(16.))
                .child(icon_container)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child("模组支持"),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(colors.text_muted)
                                .child("前面的区域以后再来探索吧！"),
                        ),
                )
                .child(badge),
        )
}
