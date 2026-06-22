use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::online::append_online_log;
use super::online_widgets::pill_button;

pub(super) fn render_peers_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .rounded(px(18.))
        .bg(Hsla {
            a: 0.70,
            ..colors.surface
        })
        .border_1()
        .border_color(colors.border)
        .p(px(16.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("节点"),
                )
                .child(
                    pill_button(
                        colors,
                        "peers-refresh",
                        SharedString::from("刷新节点"),
                        lucide_icons::icon_refresh_cw(),
                    )
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.update_global(|state: &mut ToolsPageState, _cx| {
                            state.peers_loading = true;
                        });
                        append_online_log("刷新节点列表", cx);

                        cx.spawn(async move |cx| {
                            let result = crate::core::online::easytier_embedded_peers().await;
                            let _ = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                state.peers_loading = false;
                                match result {
                                    Ok(list) => {
                                        state.peers = list
                                            .into_iter()
                                            .map(|peer| {
                                                crate::ui::views::tools::state::OnlinePeerEntry {
                                                    hostname: SharedString::from(peer.hostname),
                                                    ipv4: peer.ipv4.map(SharedString::from),
                                                }
                                            })
                                            .collect();
                                    }
                                    Err(error) => {
                                        state.online_error = Some(SharedString::from(error));
                                    }
                                }
                            });
                        })
                        .detach();
                    }),
                ),
        )
        .child(
            div()
                .rounded(px(14.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.surface)
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .when(state.peers_loading, |this| {
                    this.child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_secondary)
                            .child("正在加载节点列表..."),
                    )
                })
                .when(state.peers.is_empty() && !state.peers_loading, |this| {
                    this.child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_secondary)
                            .child("当前没有可显示的节点。"),
                    )
                })
                .when(!state.peers.is_empty(), |this| {
                    this.child(
                        div()
                            .max_h(px(280.))
                            .overflow_y_scrollbar()
                            .scrollbar_width(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .children(state.peers.iter().map(|peer| {
                                let ipv4 =
                                    peer.ipv4.clone().unwrap_or_else(|| SharedString::from("-"));
                                div()
                                    .w_full()
                                    .rounded(px(12.))
                                    .border_1()
                                    .border_color(colors.border)
                                    .bg(Hsla {
                                        a: 0.70,
                                        ..colors.surface_hover
                                    })
                                    .px(px(12.))
                                    .py(px(10.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .text_size(px(13.))
                                            .text_color(colors.text_primary)
                                            .truncate()
                                            .child(peer.hostname.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(12.))
                                            .text_color(colors.text_secondary)
                                            .child(ipv4),
                                    )
                                    .into_any_element()
                            })),
                    )
                }),
        )
}
