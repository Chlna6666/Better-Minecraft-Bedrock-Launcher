use crate::ui::components::icon::themed_icon;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::{ToolsPageState, ToolsTab};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

struct ToolNavigationItem {
    id: &'static str,
    tab: ToolsTab,
    label: &'static str,
    description: &'static str,
    icon: &'static str,
}

struct NavigationPalette {
    accent: Hsla,
    background: Hsla,
    border: Hsla,
}

pub(super) fn render_sidebar(colors: &ThemeColors, active: ToolsTab) -> Div {
    let items = [ToolNavigationItem {
        id: "tools-online",
        tab: ToolsTab::Online,
        label: "联机大厅",
        description: "创建或加入 EasyTier 房间",
        icon: lucide_icons::icon_users(),
    }];

    crate::ui::components::page_shell::split_sidebar_panel(colors)
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div().w_full().flex().flex_col().gap(px(8.)).children(
                items
                    .into_iter()
                    .map(|item| render_navigation_item(colors, item, active)),
            ),
        )
}

fn render_navigation_item(
    colors: &ThemeColors,
    item: ToolNavigationItem,
    active: ToolsTab,
) -> Stateful<Div> {
    let selected = item.tab == active;
    let palette = navigation_palette(colors, selected);

    div()
        .id(item.id)
        .w_full()
        .rounded(px(15.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.background)
        .px(px(12.))
        .py(px(12.))
        .cursor_pointer()
        .when(!selected, |this| {
            this.hover(|style| style.bg(colors.surface_hover))
        })
        .flex()
        .items_center()
        .gap(px(10.))
        .child(render_navigation_icon(item.icon, palette.accent))
        .child(render_navigation_copy(colors, &item, selected))
        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
            cx.update_global(|state: &mut ToolsPageState, _cx| {
                state.tab = item.tab;
            });
        })
}

fn navigation_palette(colors: &ThemeColors, selected: bool) -> NavigationPalette {
    if selected {
        NavigationPalette {
            accent: colors.accent,
            background: Hsla {
                a: 0.13,
                ..colors.accent
            },
            border: Hsla {
                a: 0.30,
                ..colors.accent
            },
        }
    } else {
        NavigationPalette {
            accent: colors.text_secondary,
            background: Hsla {
                a: 0.40,
                ..colors.surface
            },
            border: Hsla {
                a: 0.12,
                ..colors.border
            },
        }
    }
}

fn render_navigation_icon(icon: &'static str, accent: Hsla) -> Div {
    div()
        .size(px(34.))
        .rounded(px(11.))
        .bg(Hsla { a: 0.12, ..accent })
        .flex()
        .items_center()
        .justify_center()
        .child(themed_icon(icon, 17.0, accent))
}

fn render_navigation_copy(colors: &ThemeColors, item: &ToolNavigationItem, selected: bool) -> Div {
    div()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(item.label),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(if selected {
                    colors.text_secondary
                } else {
                    colors.text_muted
                })
                .line_height(px(16.))
                .child(item.description),
        )
}
