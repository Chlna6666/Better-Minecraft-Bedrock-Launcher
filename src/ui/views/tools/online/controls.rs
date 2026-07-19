use crate::ui::components::icon::themed_icon;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::online_state_text;
use super::widgets::icon_button;

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

pub(super) fn render_session_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .rounded(px(20.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.78,
            ..colors.surface
        })
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(render_session_header(colors, state))
        .child(render_session_details(colors, state))
}

fn session_accent(colors: &ThemeColors, state: &ToolsPageState) -> Hsla {
    if state.easytier_running {
        colors.accent
    } else if state.online_error.is_some() {
        colors.danger
    } else {
        colors.text_secondary
    }
}

fn render_session_header(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let accent = session_accent(colors, state);
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(render_session_identity(colors, state, accent))
        .child(
            icon_button(
                colors,
                "online-settings",
                lucide_icons::icon_settings(),
                false,
            )
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.update_global(|state: &mut ToolsPageState, _cx| {
                    state.easytier_settings_open = true;
                });
            }),
        )
}

fn render_session_identity(colors: &ThemeColors, state: &ToolsPageState, accent: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(34.))
                .rounded(px(12.))
                .bg(Hsla { a: 0.14, ..accent })
                .flex()
                .items_center()
                .justify_center()
                .child(themed_icon(lucide_icons::icon_radio_tower(), 17.0, accent)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child("当前会话"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(accent)
                        .child(online_state_text(state)),
                ),
        )
}

fn render_session_details(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .rounded(px(15.))
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.45,
            ..colors.settings_field_bg
        })
        .px(px(14.))
        .py(px(12.))
        .flex()
        .flex_col()
        .gap(px(9.))
        .child(detail_row(
            colors,
            "虚拟 IP",
            state
                .easytier_ipv4
                .clone()
                .unwrap_or_else(|| SharedString::from("连接后显示")),
        ))
        .child(detail_row(
            colors,
            "节点名称",
            if state.easytier_hostname.as_ref().is_empty() {
                SharedString::from("尚未连接")
            } else {
                state.easytier_hostname.clone()
            },
        ))
        .child(detail_row(
            colors,
            "Minecraft 地址",
            match (
                state.easytier_game_host.as_ref().is_empty(),
                state.easytier_game_port,
            ) {
                (false, Some(port)) => {
                    SharedString::from(format!("{}:{port}", state.easytier_game_host))
                }
                _ => SharedString::from("连接后显示"),
            },
        ))
        .child(detail_row(
            colors,
            "NAT",
            match (state.nat_udp_type, state.nat_tcp_type) {
                (Some(udp), Some(tcp)) => SharedString::from(format!(
                    "UDP：{} / TCP：{}",
                    nat_type_label(udp),
                    nat_type_label(tcp)
                )),
                _ => SharedString::from("尚未检测"),
            },
        ))
}

fn nat_type_label(value: i32) -> &'static str {
    match value {
        0 => "检测中或未知",
        1 => "开放网络",
        2 => "完全锥形 NAT",
        3 => "受限锥形 NAT",
        4 => "端口受限 NAT",
        5 => "对称 NAT",
        _ => "未知类型",
    }
}

fn detail_row(colors: &ThemeColors, label: &'static str, value: SharedString) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(
            div()
                .min_w(px(0.))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_secondary)
                .truncate()
                .child(value),
        )
}
