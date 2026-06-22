use crate::ui::components::input::Input;
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;

use super::{online_controls, online_peers, online_room, online_widgets};

pub(super) fn render_online_panel(
    colors: &ThemeColors,
    state: &ToolsPageState,
    window_width: Pixels,
) -> impl IntoElement {
    let room = online_room::render_room_card(colors, state);
    let controls = online_controls::render_controls_card(colors, state);
    let peers = online_peers::render_peers_card(colors, state);
    let stacked = window_width <= px(1180.);
    let has_side_details = state.easytier_running
        || state.online_error.is_some()
        || !state.online_log.as_ref().trim().is_empty()
        || !state.active_room_code.as_ref().trim().is_empty()
        || !state.host_room_code.as_ref().trim().is_empty()
        || state.nat_udp_type.is_some()
        || state.nat_tcp_type.is_some();

    let left = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(room)
        .child(controls)
        .children(state.easytier_running.then(|| peers.into_any_element()));

    let body = if stacked {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(left)
            .when(has_side_details, |this| {
                this.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(render_status_section(colors, state))
                        .child(render_log_section(colors, state)),
                )
            })
    } else {
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(px(16.))
            .child(left)
            .when(has_side_details, |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .min_w(px(0.))
                        .w(px(320.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(render_status_section(colors, state))
                        .child(render_log_section(colors, state)),
                )
            })
    };

    div()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .h_full()
        .rounded_xl()
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.70,
            ..colors.surface
        })
        .p(px(18.))
        .overflow_y_scrollbar()
        .scrollbar_width(px(0.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div().flex().flex_col().gap(px(2.)).child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.text_primary)
                    .child("联机"),
            ),
        )
        .child(body)
}

pub(super) fn render_online_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    _window_height: Pixels,
    state: &ToolsPageState,
) -> Option<AnyElement> {
    if !state.easytier_settings_open {
        return None;
    }

    let card_width = if window_width <= px(760.) {
        window_width - px(32.)
    } else {
        px(680.)
    };
    let close = Rc::new(|cx: &mut App| {
        cx.update_global(|state: &mut ToolsPageState, _cx| {
            state.easytier_settings_open = false;
        });
    });

    Some(
        modal::modal_layer_dismissible(
            div()
                .w(card_width.max(px(320.)))
                .max_w(px(680.))
                .rounded(px(20.))
                .border_1()
                .border_color(Hsla {
                    a: 0.22,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.98,
                    ..colors.surface
                })
                .shadow(vec![BoxShadow {
                    color: Hsla {
                        a: 0.24,
                        ..rgb(0x000000).into()
                    },
                    blur_radius: px(40.),
                    spread_radius: px(0.),
                    offset: point(px(0.), px(16.)),
                }])
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .px(px(20.))
                        .py(px(18.))
                        .border_b_1()
                        .border_color(Hsla {
                            a: 0.14,
                            ..colors.border
                        })
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(16.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(px(20.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child("EasyTier 设置"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child("留空时自动获取公共节点；手动指定时可输入一个或多个节点。"),
                                ),
                        )
                        .child(
                            online_widgets::icon_button(
                                colors,
                                "online-settings-close",
                                lucide_icons::icon_x(),
                            )
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.update_global(|state: &mut ToolsPageState, _cx| {
                                    state.easytier_settings_open = false;
                                });
                            }),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .px(px(20.))
                        .py(px(18.))
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(render_setting_field(
                            colors,
                            "引导节点",
                            "多个节点可用空格或逗号分隔。",
                            state.bootstrap_peers_input.as_ref(),
                            "留空自动获取公共节点",
                        ))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(16.))
                                .child(toggle_field(
                                    colors,
                                    "disable_p2p",
                                    "disable_p2p",
                                    "禁用 P2P，优先走中继节点。",
                                    state.disable_p2p,
                                    |state| state.disable_p2p = !state.disable_p2p,
                                ))
                                .child(toggle_field(
                                    colors,
                                    "no_tun",
                                    "no_tun",
                                    "不创建虚拟网卡，兼容性更高。",
                                    state.no_tun,
                                    |state| state.no_tun = !state.no_tun,
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(colors.text_secondary)
                                .child(
                                    "当前桌面版已按新版公共节点源自动拉取配置；仅当你明确需要固定节点时再手动填写。",
                                ),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .px(px(20.))
                        .py(px(16.))
                        .border_t_1()
                        .border_color(Hsla {
                            a: 0.14,
                            ..colors.border
                        })
                        .flex()
                        .justify_end()
                        .child(
                            online_widgets::primary_button(
                                colors,
                                "online-settings-confirm",
                                "确认",
                            )
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.update_global(|state: &mut ToolsPageState, _cx| {
                                    state.easytier_settings_open = false;
                                });
                            }),
                        ),
                ),
            hsla(0., 0., 0., 0.32),
            close,
        )
        .into_any_element(),
    )
}

pub(super) fn parse_bootstrap_peers(text: &str) -> Vec<String> {
    text.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn primary_game_port(state: &ToolsPageState) -> u16 {
    state
        .game_ports
        .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
        .find_map(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(7551)
}

pub(super) fn normalized_player_name(state: &ToolsPageState) -> String {
    let value = state.player_name.as_ref().trim();
    if value.is_empty() {
        "BMCBL_USER".to_string()
    } else {
        value.chars().take(32).collect()
    }
}

pub(super) fn append_online_log(message: impl Into<String>, cx: &mut App) {
    let message = message.into();
    if message.trim().is_empty() {
        return;
    }

    cx.update_global(|state: &mut ToolsPageState, _cx| {
        let next = if state.online_log.as_ref().trim().is_empty() {
            message
        } else {
            format!("{}\n{}", state.online_log.as_ref(), message)
        };
        state.online_log = SharedString::from(next);
    });
}

fn render_status_section(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let nat_summary = match (state.nat_udp_type, state.nat_tcp_type) {
        (Some(udp), Some(tcp)) => format!("UDP {udp} / TCP {tcp}"),
        (Some(udp), None) => format!("UDP {udp}"),
        (None, Some(tcp)) => format!("TCP {tcp}"),
        (None, None) => "未检测".to_string(),
    };
    let peer_source = if state.bootstrap_peers.as_ref().trim().is_empty() {
        SharedString::from("自动公共节点")
    } else {
        SharedString::from(state.bootstrap_peers.as_ref().trim().to_string())
    };

    compact_panel(colors, "联机状态")
        .child(kv_row(colors, "状态", online_state_text(state)))
        .child(kv_row(
            colors,
            "主机名",
            state.easytier_hostname.clone().into_any_element(),
        ))
        .child(kv_row(
            colors,
            "虚拟 IP",
            state
                .easytier_ipv4
                .clone()
                .unwrap_or_else(|| SharedString::from("-"))
                .into_any_element(),
        ))
        .child(kv_row(colors, "NAT", SharedString::from(nat_summary)))
        .child(kv_row(colors, "引导节点", peer_source))
        .when(!state.active_room_code.as_ref().trim().is_empty(), |this| {
            this.child(kv_row(
                colors,
                "当前房间",
                state.active_room_code.clone().into_any_element(),
            ))
        })
        .when(!state.host_room_code.as_ref().trim().is_empty(), |this| {
            this.child(kv_row(
                colors,
                "房主联机码",
                state.host_room_code.clone().into_any_element(),
            ))
        })
}

fn render_log_section(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    compact_panel(colors, "联机日志").child(
        div()
            .w_full()
            .min_h(px(120.))
            .max_h(px(240.))
            .rounded(px(14.))
            .border_1()
            .border_color(Hsla {
                a: 0.10,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.36,
                ..colors.surface
            })
            .px(px(12.))
            .py(px(10.))
            .overflow_y_scrollbar()
            .scrollbar_width(px(0.))
            .child(render_log_lines(colors, state)),
    )
}

fn render_log_lines(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let lines = if state.online_log.as_ref().trim().is_empty() {
        vec![SharedString::from(
            "暂无日志。执行生成房间、加入房间、刷新状态后会在这里记录结果。",
        )]
    } else {
        state
            .online_log
            .as_ref()
            .lines()
            .map(|line| SharedString::from(line.to_string()))
            .collect::<Vec<_>>()
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .children(lines.into_iter().map(|line| {
            div()
                .w_full()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(colors.text_secondary)
                .whitespace_normal()
                .child(line)
                .into_any_element()
        }))
}

fn render_setting_field(
    colors: &ThemeColors,
    title: &'static str,
    desc: &'static str,
    input: Option<&Entity<crate::ui::components::input::InputState>>,
    placeholder: &'static str,
) -> Div {
    let field: AnyElement = if let Some(input) = input {
        Input::new(input)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .cleanable(true)
            .w_full()
            .h(px(38.))
            .px(px(4.))
            .into_any_element()
    } else {
        div()
            .w_full()
            .h(px(38.))
            .px(px(12.))
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .child(placeholder),
            )
            .into_any_element()
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(desc),
                ),
        )
        .child(
            div()
                .w_full()
                .rounded(px(14.))
                .border_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.78,
                    ..colors.settings_field_bg
                })
                .px(px(12.))
                .py(px(8.))
                .child(field),
        )
}

fn toggle_field(
    colors: &ThemeColors,
    id: &'static str,
    title: &'static str,
    desc: &'static str,
    enabled: bool,
    on_toggle: fn(&mut ToolsPageState),
) -> Div {
    div()
        .min_w(px(220.))
        .flex_1()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .px(px(14.))
        .py(px(12.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(desc),
                ),
        )
        .child(ToggleSwitch::new(id, colors, enabled, move |cx| {
            cx.update_global(|state: &mut ToolsPageState, _cx| {
                on_toggle(state);
            });
        }))
}

fn compact_panel(colors: &ThemeColors, title: &'static str) -> Div {
    div()
        .w_full()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.22,
            ..colors.surface
        })
        .px(px(14.))
        .py(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(title),
        )
}

fn kv_row(colors: &ThemeColors, label: &'static str, value: impl IntoElement) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(colors.text_primary)
                .whitespace_normal()
                .child(value),
        )
}

pub(super) fn online_state_text(state: &ToolsPageState) -> SharedString {
    if state.online_loading {
        SharedString::from("处理中")
    } else if let Some(error) = state.online_error.clone() {
        SharedString::from(format!("失败: {error}"))
    } else if state.easytier_running {
        SharedString::from("运行中")
    } else {
        SharedString::from("未运行")
    }
}
