use crate::ui::components::icon::themed_icon;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::actions;
use super::widgets::subtle_button;

pub(super) fn render_peers_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let disabled = state.online_operation.is_busy() || !state.easytier_running;
    div()
        .w_full()
        .rounded(px(20.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.74,
            ..colors.surface
        })
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(13.))
        .child(render_peers_header(colors, state, disabled))
        .child(render_peer_list(colors, state))
}

fn render_peers_header(colors: &ThemeColors, state: &ToolsPageState, disabled: bool) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .child(themed_icon(
                    lucide_icons::icon_network(),
                    17.0,
                    colors.accent,
                ))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(format!("在线节点 · {}", state.peers.len())),
                ),
        )
        .child(
            subtle_button(
                colors,
                "online-peers-refresh",
                if state.peers_loading {
                    "刷新中"
                } else {
                    "刷新"
                },
                lucide_icons::icon_refresh_cw(),
                disabled,
            )
            .when(!disabled, |this| {
                this.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    actions::refresh_peers(cx);
                })
            }),
        )
}

fn render_peer_list(colors: &ThemeColors, state: &ToolsPageState) -> impl IntoElement {
    div()
        .w_full()
        .max_h(px(220.))
        .overflow_y_scrollbar()
        .scrollbar_width(px(0.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .when(state.peers_loading, |this| {
            this.child(empty_row(colors, "正在同步节点列表…"))
        })
        .when(state.peers.is_empty() && !state.peers_loading, |this| {
            this.child(empty_row(
                colors,
                if state.easytier_running {
                    "路由建立后，节点会显示在这里"
                } else {
                    "连接房间后显示在线节点"
                },
            ))
        })
        .when(!state.peers.is_empty(), |this| {
            this.children(
                state
                    .peers
                    .iter()
                    .enumerate()
                    .map(|(index, peer)| render_peer_row(colors, index, peer)),
            )
        })
}

fn render_peer_row(
    colors: &ThemeColors,
    index: usize,
    peer: &crate::ui::views::tools::state::OnlinePeerEntry,
) -> Stateful<Div> {
    div()
        .id(("online-peer", index))
        .w_full()
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.46,
            ..colors.settings_field_bg
        })
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .min_w(px(0.))
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_primary)
                .truncate()
                .child(peer.hostname.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(
                    peer.ipv4
                        .clone()
                        .unwrap_or_else(|| SharedString::from("等待地址")),
                ),
        )
}

fn empty_row(colors: &ThemeColors, text: &'static str) -> Div {
    div()
        .w_full()
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.32,
            ..colors.settings_field_bg
        })
        .px(px(12.))
        .py(px(14.))
        .text_size(px(12.))
        .text_color(colors.text_muted)
        .child(text)
}
