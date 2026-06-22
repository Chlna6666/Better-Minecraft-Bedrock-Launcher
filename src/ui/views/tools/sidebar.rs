use crate::ui::components::icon::themed_icon;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::{ToolsPageState, ToolsTab};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn render_sidebar(colors: &ThemeColors, active: ToolsTab) -> Div {
    let item = |id: &'static str,
                tab: ToolsTab,
                name: &'static str,
                desc: &'static str,
                icon: &'static str,
                active: ToolsTab| {
        let is_active = tab == active;
        let bg = if is_active {
            Hsla {
                a: 0.16,
                ..colors.accent
            }
        } else {
            colors.surface
        };
        let border = if is_active {
            colors.accent
        } else {
            colors.border
        };

        div()
            .id(id)
            .w_full()
            .rounded(px(14.))
            .border_1()
            .border_color(border)
            .bg(bg)
            .p(px(12.))
            .flex()
            .items_center()
            .gap(px(12.))
            .cursor_pointer()
            .child(themed_icon(icon, 18.0, colors.text_secondary))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_primary)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_secondary)
                            .child(desc),
                    ),
            )
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cx.update_global(|s: &mut ToolsPageState, cx| {
                    s.tab = tab;
                });
            })
    };

    div()
        .w(px(280.))
        .h_full()
        .rounded_xl()
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.70,
            ..colors.surface
        })
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_secondary)
                .child("工具列表"),
        )
        .child(item(
            "tools-online",
            ToolsTab::Online,
            "联机",
            "创建/加入大厅并查看房间状态",
            lucide_icons::icon_users(),
            active,
        ))
}
