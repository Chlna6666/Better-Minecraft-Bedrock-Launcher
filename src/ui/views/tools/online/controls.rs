use crate::ui::components::icon::themed_icon;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::online_state_text;
use super::widgets::icon_button;

pub(crate) fn persist_tools_online_settings(cx: &mut App) {
    let (bootstrap_peers, player_name, game_ports, disable_p2p) =
        cx.read_global(|state: &ToolsPageState, _cx| {
            (
                state.bootstrap_peers.to_string(),
                state.player_name.to_string(),
                state.game_ports.to_string(),
                state.disable_p2p,
            )
        });

    cx.spawn(async move |_cx| {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            crate::config::config::update_config(|config| {
                config.online.bootstrap_peers = bootstrap_peers;
                config.online.player_name = player_name;
                config.online.game_ports = game_ports;
                config.online.disable_p2p = disable_p2p;
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

pub(super) fn render_session_card(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
) -> Div {
    crate::ui::components::page_shell::glass_card(colors)
        .w_full()
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(render_session_header(colors, i18n, state))
        .child(render_session_details(colors, i18n, state))
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

fn render_session_header(colors: &ThemeColors, i18n: &I18n, state: &ToolsPageState) -> Div {
    let accent = session_accent(colors, state);
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(render_session_identity(colors, i18n, state, accent))
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

fn render_session_identity(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
    accent: Hsla,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(34.))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                        .child(t!("Online.current_session")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(accent)
                        .child(online_state_text(i18n, state)),
                ),
        )
}

fn render_session_details(colors: &ThemeColors, i18n: &I18n, state: &ToolsPageState) -> Div {
    crate::ui::components::page_shell::inner_well(colors)
        .w_full()
        .px(px(14.))
        .py(px(12.))
        .flex()
        .flex_col()
        .gap(px(9.))
        .child(detail_row(
            colors,
            t!("Online.virtual_ip"),
            state
                .easytier_ipv4
                .clone()
                .unwrap_or_else(|| t!("Online.connect_after")),
        ))
        .child(detail_row(
            colors,
            t!("Online.node_name"),
            if state.easytier_hostname.as_ref().is_empty() {
                t!("Online.not_connected")
            } else {
                state.easytier_hostname.clone()
            },
        ))
        .child(detail_row(
            colors,
            t!("Online.minecraft_address"),
            match (
                state.easytier_game_host.as_ref().is_empty(),
                state.easytier_game_port,
            ) {
                (false, Some(port)) => {
                    SharedString::from(format!("{}:{port}", state.easytier_game_host))
                }
                _ => t!("Online.connect_after"),
            },
        ))
        .child(detail_row(
            colors,
            "NAT".into(),
            match (state.nat_udp_type, state.nat_tcp_type) {
                (Some(udp), Some(tcp)) => {
                    let udp = nat_type_label(i18n, udp);
                    let tcp = nat_type_label(i18n, tcp);
                    t!("Online.nat_summary", udp = udp, tcp = tcp)
                }
                _ => t!("Online.nat_not_checked"),
            },
        ))
}

fn nat_type_label(i18n: &I18n, value: i32) -> SharedString {
    match value {
        0 => t!("Online.nat_checking"),
        1 => t!("Online.nat_types.open_internet"),
        2 => t!("Online.nat_types.full_cone"),
        3 => t!("Online.nat_types.restricted"),
        4 => t!("Online.nat_types.port_restricted"),
        5 => t!("Online.nat_types.symmetric"),
        _ => t!("Online.nat_types.unknown"),
    }
}

fn detail_row(colors: &ThemeColors, label: SharedString, value: SharedString) -> Div {
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
