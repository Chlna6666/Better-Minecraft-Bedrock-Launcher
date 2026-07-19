use crate::ui::components::icon::themed_icon;
use crate::ui::components::scroll::ScrollableElement as _;
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
pub(super) fn render_room_members_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
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
        .overflow_hidden()
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(13.))
        .child(render_room_members_header(colors, state, disabled))
        .child(render_room_members_list(colors, state))
}

fn render_room_members_header(colors: &ThemeColors, state: &ToolsPageState, disabled: bool) -> Div {
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
                        .child("房间成员"),
                )
                .child(
                    div()
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.12,
                            ..colors.accent
                        })
                        .px(px(8.))
                        .py(px(2.))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.accent)
                        .child(format!("{} 人", state.players.len())),
                ),
        )
        .child(
            subtle_button(
                colors,
                "online-players-refresh",
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

fn render_room_members_list(colors: &ThemeColors, state: &ToolsPageState) -> impl IntoElement {
    // 排序：房主 (is_room_host == true) 强制置顶展示
    let mut sorted_players = state.players.clone();
    sorted_players.sort_by_key(|player| !player.is_room_host);

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(7.))
        .when(state.peers_loading, |this| {
            this.child(empty_row(colors, "正在同步房间成员…"))
        })
        .when(sorted_players.is_empty() && !state.peers_loading, |this| {
            this.child(empty_row(
                colors,
                if state.easytier_running {
                    "房间建立成功，等待其他玩家加入"
                } else {
                    "加入或创建房间后可查看成员列表"
                },
            ))
        })
        .when(!sorted_players.is_empty(), |this| {
            this.children(
                sorted_players
                    .into_iter()
                    .enumerate()
                    .map(|(index, player)| render_player_row(colors, index, &player)),
            )
        })
}

/// 渲染“网络节点”卡片（底部显示，可点击展开/收起）
pub(super) fn render_network_nodes_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let disabled = state.online_operation.is_busy() || !state.easytier_running;
    let expanded = state.network_nodes_expanded;

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
        .overflow_hidden()
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(13.))
        .child(render_network_nodes_header(
            colors, state, disabled, expanded,
        ))
        .when(expanded, |this| this.child(render_peer_list(colors, state)))
}

fn render_network_nodes_header(
    colors: &ThemeColors,
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
                        .child("网络节点"),
                )
                .child(
                    div()
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.12,
                            ..colors.settings_field_bg
                        })
                        .px(px(8.))
                        .py(px(2.))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_secondary)
                        .child(format!("{} 节点", peer_count)),
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
                                "刷新中"
                            } else {
                                "刷新"
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

fn render_collapsed_hint(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let peer_count = state.peers.len();
    div()
        .w_full()
        .rounded(px(12.))
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
                        format!("已建立网络链路（共 {peer_count} 个节点），点击展开详情")
                    } else {
                        "网络节点就绪，点击展开明细".to_string()
                    }
                } else {
                    "连接房间后可展开查看局域网节点与中转信息".to_string()
                }),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(colors.accent)
                .child("点击展开"),
        )
}

fn render_peer_list(colors: &ThemeColors, state: &ToolsPageState) -> impl IntoElement {
    div()
        .w_full()
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
            this.children(render_peer_groups(colors, &state.peers))
        })
}

fn render_peer_groups(colors: &ThemeColors, peers: &[OnlinePeerEntry]) -> Vec<Div> {
    [
        (OnlinePeerRole::Server, "联机中心节点"),
        (OnlinePeerRole::User, "客户端网络节点"),
        (OnlinePeerRole::Relay, "公共中转节点"),
        (OnlinePeerRole::Unknown, "其他网络节点"),
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
                        .child(title),
                )
                .children(
                    peers
                        .into_iter()
                        .map(|(index, peer)| render_peer_row(colors, index, peer)),
                )
        })
    })
    .collect()
}

fn render_peer_row(colors: &ThemeColors, index: usize, peer: &OnlinePeerEntry) -> Stateful<Div> {
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
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(12.5))
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
                        .child(connection_detail(peer)),
                ),
        )
        .child(
            div()
                .flex_none()
                .max_w(px(210.))
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .truncate()
                .child(peer_address(peer)),
        )
}

fn render_player_row(
    colors: &ThemeColors,
    index: usize,
    player: &OnlinePlayerEntry,
) -> Stateful<Div> {
    let is_host = player.is_room_host;
    div()
        .id(("online-player", index))
        .w_full()
        .rounded(px(13.))
        .border_1()
        .border_color(if is_host {
            Hsla {
                a: 0.28,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.12,
                ..colors.border
            }
        })
        .bg(if is_host {
            Hsla {
                a: 0.12,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.46,
                ..colors.settings_field_bg
            }
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
                            .text_size(px(12.5))
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
                .rounded(px(8.))
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
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if is_host {
                    colors.accent
                } else {
                    colors.text_secondary
                })
                .child(if is_host { "房主" } else { "玩家" }),
        )
}

fn connection_detail(peer: &OnlinePeerEntry) -> SharedString {
    use crate::core::online::EasyTierConnectionKind;

    let mut details = vec![match peer.connection_kind {
        EasyTierConnectionKind::Local => "本机节点".to_string(),
        EasyTierConnectionKind::Direct if peer.role == OnlinePeerRole::Relay => {
            "中转入口已连接".to_string()
        }
        EasyTierConnectionKind::Direct => "P2P 直连".to_string(),
        EasyTierConnectionKind::Relayed => peer
            .via_hostname
            .as_ref()
            .map(|hostname| format!("经 {hostname} 中转"))
            .unwrap_or_else(|| "经公共节点中转".to_string()),
        EasyTierConnectionKind::Unknown if peer.role == OnlinePeerRole::Relay => {
            "公共中转节点".to_string()
        }
        EasyTierConnectionKind::Unknown => "连接信息同步中".to_string(),
    }];
    if let Some(protocol) = peer.protocol.as_ref() {
        details.push(protocol.to_string());
    }
    if let Some(latency_ms) = peer.latency_ms {
        details.push(format!("{latency_ms} ms"));
    }
    SharedString::from(details.join(" · "))
}

fn peer_address(peer: &OnlinePeerEntry) -> SharedString {
    peer.ipv4
        .clone()
        .or_else(|| peer.remote_endpoint.clone())
        .unwrap_or_else(|| {
            SharedString::from(if peer.role == OnlinePeerRole::Relay {
                "公共中转节点"
            } else {
                "无虚拟地址"
            })
        })
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
