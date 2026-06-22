use crate::ui::components::icon::themed_icon;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::DownloadPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn render_mod_panel(colors: &ThemeColors, _state: &DownloadPageState) -> Div {
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
                .gap(px(14.))
                .child(
                    div()
                        .w(px(64.))
                        .h(px(64.))
                        .rounded(px(16.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.text_secondary
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(themed_icon(
                            lucide_icons::icon_box(),
                            28.0,
                            colors.text_secondary,
                        )),
                )
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(colors.text_secondary)
                        .child("前面的区域以后再来探索吧！"),
                ),
        )
}
