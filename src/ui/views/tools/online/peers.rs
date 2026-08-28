use crate::ui::components::icon::themed_icon;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::{
    OnlinePeerEntry, OnlinePeerRole, OnlinePlayerEntry, ToolsPageState,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::actions;
use super::widgets::subtle_button;

/// 渲染“房间成员”卡片（置顶房主，快速可见）
pub(super) fn render_room_members_card(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
) -> Div {
    let disabled = state.online_operation.is_busy() || !state.easytier_running;
    crate::ui::components::page_shell::glass_card(colors)
        .w_full()
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(13.))
        .child(render_room_members_header(colors, i18n, state, disabled))
        .child(render_room_members_list(colors, i18n, state))
}

fn render_room_members_header(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
    disabled: bool,
) -> Div {
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
                .child(themed_icon(lucide_icons::icon_users(), 17.0, colors.accent))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(t!("Online.room_members")),
                )
                .child(
                    div()
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla {
                            a: 0.12,
                            ..colors.accent
                        })
                        .px(px(8.))
                        .py(px(2.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.accent)
                        .child(t!("Online.people_count", count = state.players.len())),
                ),
        )
        .child(
            subtle_button(
                colors,
                "online-players-refresh",
                if state.peers_loading {
                    t!("Online.refreshing")
                } else {
                    t!("Online.refresh")
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

fn render_room_members_list(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
) -> impl IntoElement {
    // 排序：房主 (is_room_host == true) 强制置顶展示
    let mut sorted_players = state.players.clone();
    sorted_players.sort_by_key(|player| !player.is_room_host);

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(7.))
        .when(state.peers_loading && sorted_players.is_empty(), |this| {
            this.child(empty_row(colors, t!("Online.syncing_members")))
        })
        .when(sorted_players.is_empty() && !state.peers_loading, |this| {
            this.child(empty_row(
                colors,
                if state.easytier_running {
                    t!("Online.waiting_members")
                } else {
                    t!("Online.members_after_join")
                },
            ))
        })
        .when(!sorted_players.is_empty(), |this| {
            this.children(
                sorted_players
                    .into_iter()
                    .enumerate()
                    .map(|(index, player)| render_player_row(colors, i18n, index, &player)),
            )
        })
}

/// 渲染“网络节点”卡片（底部显示，可点击展开/收起）
pub(super) fn render_network_nodes_card(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
) -> Div {
    let disabled = state.online_operation.is_busy() || !state.easytier_running;
    let expanded = state.network_nodes_expanded;

    crate::ui::components::page_shell::glass_card(colors)
        .w_full()
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(13.))
        .child(render_network_nodes_header(
            colors, i18n, state, disabled, expanded,
        ))
        .when(expanded, |this| {
            this.child(render_peer_list(colors, i18n, state))
        })
}

fn render_network_nodes_header(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
    disabled: bool,
    expanded: bool,
) -> Stateful<Div> {
    let peer_count = state.peers.len();
    div()
        .id("online-network-nodes-header")
        .w_full()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.update_global(|state: &mut ToolsPageState, _cx| {
                state.network_nodes_expanded = !state.network_nodes_expanded;
            });
        })
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
                    colors.text_secondary,
                ))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(t!("Online.network_nodes")),
                )
                .child(
                    div()
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla {
                            a: 0.12,
                            ..colors.settings_field_bg
                        })
                        .px(px(8.))
                        .py(px(2.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_secondary)
                        .child(t!("Online.nodes_count", count = peer_count)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when(expanded && !disabled, |this| {
                    this.child(
                        subtle_button(
                            colors,
                            "online-peers-refresh",
                            if state.peers_loading {
                                t!("Online.refreshing")
                            } else {
                                t!("Online.refresh")
                            },
                            lucide_icons::icon_refresh_cw(),
                            disabled,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            |_event, _window, cx| {
                                actions::refresh_peers(cx);
                            },
                        ),
                    )
                })
                .child(themed_icon(
                    if expanded {
                        lucide_icons::icon_chevron_up()
                    } else {
                        lucide_icons::icon_chevron_down()
                    },
                    16.0,
                    colors.text_muted,
                )),
        )
}

fn render_collapsed_hint(colors: &ThemeColors, i18n: &I18n, state: &ToolsPageState) -> Div {
    let peer_count = state.peers.len();
    crate::ui::components::page_shell::inner_well(colors)
        .w_full()
        .px(px(12.))
        .py(px(9.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(if state.easytier_running {
                    if peer_count > 0 {
                        t!("Online.network_linked", count = peer_count)
                    } else {
                        t!("Online.network_ready")
                    }
                } else {
                    t!("Online.connect_for_nodes")
                }),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.accent)
                .child(t!("Online.expand_nodes")),
        )
}

fn render_peer_list(colors: &ThemeColors, i18n: &I18n, state: &ToolsPageState) -> impl IntoElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.))
        .when(state.peers_loading && state.peers.is_empty(), |this| {
            this.child(empty_row(colors, t!("Online.syncing_nodes")))
        })
        .when(state.peers.is_empty() && !state.peers_loading, |this| {
            this.child(empty_row(
                colors,
                if state.easytier_running {
                    t!("Online.nodes_after_route")
                } else {
                    t!("Online.nodes_after_join")
                },
            ))
        })
        .when(!state.peers.is_empty(), |this| {
            this.children(render_peer_groups(colors, i18n, &state.peers))
        })
}

fn render_peer_groups(colors: &ThemeColors, i18n: &I18n, peers: &[OnlinePeerEntry]) -> Vec<Div> {
    [
        (
            OnlinePeerRole::Server,
            crate::i18n_key!("Online.role_server"),
        ),
        (OnlinePeerRole::User, crate::i18n_key!("Online.role_user")),
        (OnlinePeerRole::Relay, crate::i18n_key!("Online.role_relay")),
        (
            OnlinePeerRole::Unknown,
            crate::i18n_key!("Online.role_unknown"),
        ),
    ]
    .into_iter()
    .filter_map(|(role, title)| {
        let peers: Vec<_> = peers
            .iter()
            .enumerate()
            .filter(|(_, peer)| peer.role == role)
            .collect();
        (!peers.is_empty()).then(|| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(i18n.t_key(title)),
                )
                .children(
                    peers
                        .into_iter()
                        .map(|(index, peer)| render_peer_row(colors, i18n, index, peer)),
                )
        })
    })
    .collect()
}

fn render_peer_row(
    colors: &ThemeColors,
    i18n: &I18n,
    index: usize,
    peer: &OnlinePeerEntry,
) -> Stateful<Div> {
    crate::ui::components::page_shell::inner_well(colors)
        .id(("online-peer", index))
        .w_full()
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_primary)
                        .truncate()
                        .child(peer.hostname.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .truncate()
                        .child(connection_detail(i18n, peer)),
                ),
        )
        .child(
            div()
                .flex_none()
                .max_w(px(210.))
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .truncate()
                .child(peer_address(i18n, peer)),
        )
}

fn render_player_row(
    colors: &ThemeColors,
    i18n: &I18n,
    index: usize,
    player: &OnlinePlayerEntry,
) -> Stateful<Div> {
    let is_host = player.is_room_host;
    crate::ui::components::page_shell::inner_well(colors)
        .id(("online-player", index))
        .w_full()
        .when(is_host, |this| {
            this.border_color(Hsla {
                a: 0.24,
                ..colors.accent
            })
            .bg(Hsla {
                a: 0.10,
                ..colors.accent
            })
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
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div().flex().items_center().gap(px(6.)).child(
                        div()
                            .text_size(px(13.))
                            .font_weight(if is_host {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(colors.text_primary)
                            .truncate()
                            .child(player.player_name.clone()),
                    ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .truncate()
                        .child(player.client_id.clone()),
                ),
        )
        .child(
            div()
                .flex_none()
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .px(px(7.))
                .py(px(3.))
                .bg(if is_host {
                    Hsla {
                        a: 0.18,
                        ..colors.accent
                    }
                } else {
                    Hsla {
                        a: 0.08,
                        ..colors.text_secondary
                    }
                })
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if is_host {
                    colors.accent
                } else {
                    colors.text_secondary
                })
                .child(if is_host {
                    t!("Online.host")
                } else {
                    t!("Online.player")
                }),
        )
}

fn connection_detail(i18n: &I18n, peer: &OnlinePeerEntry) -> SharedString {
    use crate::core::online::EasyTierConnectionKind;

    let mut details = vec![match peer.connection_kind {
        EasyTierConnectionKind::Local => t!("Online.peer_local").to_string(),
        EasyTierConnectionKind::Direct if peer.role == OnlinePeerRole::Relay => {
            t!("Online.peer_relay_connected").to_string()
        }
        EasyTierConnectionKind::Direct => t!("Online.peer_p2p").to_string(),
        EasyTierConnectionKind::Relayed => peer
            .via_hostname
            .as_ref()
            .map(|hostname| t!("Online.peer_via", hostname = hostname).to_string())
            .unwrap_or_else(|| t!("Online.peer_public_relay").to_string()),
        EasyTierConnectionKind::Unknown if peer.role == OnlinePeerRole::Relay => {
            t!("Online.public_relay").to_string()
        }
        EasyTierConnectionKind::Unknown => t!("Online.peer_syncing").to_string(),
    }];
    if let Some(protocol) = peer.protocol.as_ref() {
        details.push(protocol.to_string());
    }
    if let Some(latency_ms) = peer.latency_ms {
        details.push(format!("{latency_ms} ms"));
    }
    SharedString::from(details.join(" · "))
}

fn peer_address(i18n: &I18n, peer: &OnlinePeerEntry) -> SharedString {
    peer.ipv4
        .clone()
        .or_else(|| peer.remote_endpoint.clone())
        .unwrap_or_else(|| {
            SharedString::from(if peer.role == OnlinePeerRole::Relay {
                t!("Online.public_relay")
            } else {
                t!("Online.no_virtual_address")
            })
        })
}

fn empty_row(colors: &ThemeColors, text: impl Into<SharedString>) -> Div {
    crate::ui::components::page_shell::inner_well(colors)
        .w_full()
        .px(px(12.))
        .py(px(14.))
        .text_size(px(12.))
        .text_color(colors.text_muted)
        .child(text.into())
}
