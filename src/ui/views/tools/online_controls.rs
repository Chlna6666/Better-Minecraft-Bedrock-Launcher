use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use tracing::warn;

use super::online::{append_online_log, online_state_text};
use super::online_widgets::pill_button;

pub(crate) fn persist_tools_online_settings(cx: &mut App) {
    let (bootstrap_peers, player_name, game_ports, disable_p2p, no_tun) =
        cx.read_global(|state: &ToolsPageState, _cx| {
            (
                state.bootstrap_peers.to_string(),
                state.player_name.to_string(),
                state.game_ports.to_string(),
                state.disable_p2p,
                state.no_tun,
            )
        });

    cx.spawn(async move |_cx| {
        let result = tokio::task::spawn_blocking(move || {
            crate::config::config::update_config(|config| {
                config.online.bootstrap_peers = bootstrap_peers;
                config.online.player_name = player_name;
                config.online.game_ports = game_ports;
                config.online.disable_p2p = disable_p2p;
                config.online.no_tun = no_tun;
            })
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!("persist online settings failed: {error}"),
            Err(error) => tracing::warn!("persist online settings task failed: {error}"),
        }
    })
    .detach();
}

pub(super) fn render_controls_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let nat_label = if state.nat_checking {
        SharedString::from("正在检测 NAT...")
    } else {
        SharedString::from("检查 NAT")
    };

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
        .gap(px(14.))
        .child(
            div()
                .w_full()
                .rounded(px(14.))
                .border_1()
                .border_color(colors.border)
                .bg(Hsla {
                    a: 0.72,
                    ..colors.surface
                })
                .px(px(16.))
                .py(px(12.))
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(colors.text_secondary)
                        .child(
                            "生成或加入联机后，EasyTier 需要几秒时间建立节点路由，请稍候片刻再进入游戏联机。",
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .child(status_pill(colors, state))
                .when_some(state.easytier_ipv4.clone(), |this, ip| {
                    this.child(info_pill(colors, SharedString::from(format!("虚拟 IP {ip}"))))
                })
                .when(state.nat_udp_type.is_some() || state.nat_tcp_type.is_some(), |this| {
                    let udp = state.nat_udp_type.unwrap_or(0);
                    let tcp = state.nat_tcp_type.unwrap_or(0);
                    this.child(info_pill(colors, SharedString::from(format!("NAT UDP {udp} / TCP {tcp}"))))
                }),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .child(
                    pill_button(
                        colors,
                        "online-settings",
                        SharedString::from("EasyTier 设置"),
                        lucide_icons::icon_settings(),
                    )
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.update_global(|state: &mut ToolsPageState, _cx| {
                            state.easytier_settings_open = true;
                        });
                    }),
                )
                .child(
                    pill_button(
                        colors,
                        "online-nat",
                        nat_label,
                        lucide_icons::icon_shield_check(),
                    )
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.update_global(|state: &mut ToolsPageState, _cx| {
                            state.nat_checking = true;
                            state.nat_error = None;
                        });
                        append_online_log("开始检测 NAT 类型", cx);

                        cx.spawn(async move |cx| {
                            let snapshot = crate::core::easytier::api::detect_nat_types().await;
                            if let Err(error) = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                state.nat_checking = false;
                                state.nat_udp_type = Some(snapshot.udp_nat_type);
                                state.nat_tcp_type = Some(snapshot.tcp_nat_type);
                            }) {
                                warn!("update_global failed: {error:?}");
                                return;
                            }

                            let _ = cx.update(|cx| {
                                append_online_log(
                                    format!(
                                        "NAT 检测完成: udp={}, tcp={}",
                                        snapshot.udp_nat_type, snapshot.tcp_nat_type
                                    ),
                                    cx,
                                );
                            });
                        })
                        .detach();
                    }),
                )
                .child(
                    pill_button(
                        colors,
                        "online-refresh",
                        SharedString::from("刷新状态"),
                        lucide_icons::icon_activity(),
                    )
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                        cx.update_global(|state: &mut ToolsPageState, _cx| {
                            state.online_loading = true;
                            state.online_error = None;
                            state.peers_loading = true;
                        });
                        append_online_log("刷新联机状态与节点列表", cx);

                        cx.spawn(async move |cx| {
                            let status_result = crate::core::online::easytier_embedded_status().await;
                            let peers_result = crate::core::online::easytier_embedded_peers().await;

                            let (running, hostname, ipv4, error) = match status_result {
                                Ok(Some(status)) => (
                                    true,
                                    SharedString::from(status.hostname),
                                    status.ipv4.map(SharedString::from),
                                    None,
                                ),
                                Ok(None) => (false, SharedString::from(""), None, None),
                                Err(error) => (
                                    false,
                                    SharedString::from(""),
                                    None,
                                    Some(SharedString::from(error)),
                                ),
                            };

                            let peers = match peers_result {
                                Ok(list) => list
                                    .into_iter()
                                    .map(|peer| crate::ui::views::tools::state::OnlinePeerEntry {
                                        hostname: SharedString::from(peer.hostname),
                                        ipv4: peer.ipv4.map(SharedString::from),
                                    })
                                    .collect::<Vec<_>>(),
                                Err(_) => Vec::new(),
                            };

                            let _ = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                state.easytier_running = running;
                                state.easytier_hostname = hostname;
                                state.easytier_ipv4 = ipv4;
                                state.online_loading = false;
                                state.online_error = error;
                                state.peers = peers;
                                state.peers_loading = false;
                            });
                        })
                        .detach();
                    }),
                )
                .when(state.easytier_running, |this| {
                    this.child(
                        pill_button(
                            colors,
                            "online-stop",
                            SharedString::from("停止"),
                            lucide_icons::icon_square(),
                        )
                        .border_color(Hsla {
                            a: 0.35,
                            ..colors.danger
                        })
                        .text_color(colors.danger)
                        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                            cx.update_global(|state: &mut ToolsPageState, _cx| {
                                state.online_loading = true;
                                state.online_error = None;
                            });
                            append_online_log("停止 EasyTier", cx);

                            cx.spawn(async move |cx| {
                                let result = crate::core::online::easytier_stop().await;
                                let error = result.err().map(SharedString::from);
                                let _ = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                    state.online_loading = false;
                                    state.easytier_running = false;
                                    state.easytier_hostname = SharedString::from("");
                                    state.easytier_ipv4 = None;
                                    state.active_room_code = SharedString::from("");
                                    state.active_network_name = SharedString::from("");
                                    state.host_room_code = SharedString::from("");
                                    state.peers.clear();
                                    state.peers_loading = false;
                                    state.online_error = error;
                                });
                            })
                            .detach();
                        }),
                    )
                }),
        )
}

fn status_pill(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let running = state.easytier_running && state.online_error.is_none();
    let accent = if running {
        colors.accent
    } else {
        colors.text_secondary
    };
    let background = Hsla { a: 0.12, ..accent };

    div()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(999.))
        .border_1()
        .border_color(Hsla { a: 0.18, ..accent })
        .bg(background)
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(accent)
                .child(online_state_text(state)),
        )
}

fn info_pill(colors: &ThemeColors, label: SharedString) -> Div {
    div()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(999.))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.48,
            ..colors.surface
        })
        .child(
            div()
                .text_size(px(12.5))
                .text_color(colors.text_secondary)
                .child(label),
        )
}
