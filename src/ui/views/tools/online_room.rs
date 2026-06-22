use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::Input;
use crate::ui::components::toast;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::online::{
    append_online_log, normalized_player_name, parse_bootstrap_peers, primary_game_port,
};
use super::online_widgets::{icon_button, primary_button};

pub(super) fn render_room_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
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
        .gap(px(16.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("房间"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(colors.text_secondary)
                        .child("生成房间会创建新的 EasyTier 网络；加入房间会按联机码进入已有网络。"),
                ),
        )
        .child(render_join_row(colors, state))
        .child(
            div()
                .w_full()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .child(
                    primary_button(colors, "online-generate", "生成房间").on_mouse_down(
                        MouseButton::Left,
                        |_ev, _window, cx| {
                            let (peers, disable_p2p, no_tun, player_name, room_port) = cx
                                .read_global(|state: &ToolsPageState, _cx| {
                                    (
                                        parse_bootstrap_peers(state.bootstrap_peers.as_ref()),
                                        state.disable_p2p,
                                        state.no_tun,
                                        normalized_player_name(state),
                                        primary_game_port(state),
                                    )
                                });

                            cx.update_global(|state: &mut ToolsPageState, _cx| {
                                state.online_loading = true;
                                state.online_error = None;
                            });
                            append_online_log(
                                format!(
                                    "生成房间: player_name={player_name}, game_port={room_port}, peers={}",
                                    if peers.is_empty() { "auto".to_string() } else { peers.join(", ") }
                                ),
                                cx,
                            );

                            cx.spawn(async move |cx| {
                                let room = crate::core::online::paperconnect_generate_room().await;
                                match room {
                                    Ok(room) => {
                                        let hostname = format!("paper-connect-server-{room_port}");
                                        let options = crate::core::online::EasyTierStartOptions {
                                            disable_p2p: Some(disable_p2p),
                                            no_tun: Some(no_tun),
                                            compression: Some("zstd".to_string()),
                                            ipv4: None,
                                        };
                                        let start_result = crate::core::online::easytier_start(
                                            room.network_name.clone(),
                                            room.network_secret.clone(),
                                            peers,
                                            Some(hostname),
                                            Some(options),
                                        )
                                        .await;

                                        let status = if start_result.is_ok() {
                                            crate::core::online::easytier_embedded_status()
                                                .await
                                                .ok()
                                                .flatten()
                                        } else {
                                            None
                                        };
                                        let peers = if start_result.is_ok() {
                                            crate::core::online::easytier_embedded_peers()
                                                .await
                                                .unwrap_or_default()
                                                .into_iter()
                                                .map(|peer| crate::ui::views::tools::state::OnlinePeerEntry {
                                                    hostname: SharedString::from(peer.hostname),
                                                    ipv4: peer.ipv4.map(SharedString::from),
                                                })
                                                .collect::<Vec<_>>()
                                        } else {
                                            Vec::new()
                                        };

                                        let error = start_result.err().map(SharedString::from);
                                        let _ = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                            state.online_loading = false;
                                            state.online_error = error;
                                            state.easytier_running = state.online_error.is_none();
                                            state.host_room_code =
                                                SharedString::from(room.room_code.clone());
                                            state.active_room_code =
                                                SharedString::from(room.room_code.clone());
                                            state.active_network_name =
                                                SharedString::from(room.network_name.clone());
                                            if let Some(status) = status.clone() {
                                                state.easytier_hostname =
                                                    SharedString::from(status.hostname);
                                                state.easytier_ipv4 =
                                                    status.ipv4.map(SharedString::from);
                                            }
                                            state.peers = peers;
                                            state.peers_loading = false;
                                        });

                                        let created_room_code = room.room_code.clone();
                                        let _ = cx.update(|cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                created_room_code.clone(),
                                            ));
                                            append_online_log(
                                                format!("房间已创建: {}", created_room_code),
                                                cx,
                                            );
                                            toast::push(
                                                cx,
                                                SharedString::from("房间已创建，联机码已复制"),
                                            );
                                        });
                                    }
                                    Err(error) => {
                                        let _ = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                            state.online_loading = false;
                                            state.online_error = Some(SharedString::from(error.clone()));
                                        });
                                        let _ = cx.update(|cx| {
                                            append_online_log(format!("生成房间失败: {error}"), cx);
                                        });
                                    }
                                }
                            })
                            .detach();
                        },
                    ),
                )
                .child(
                    primary_button(colors, "online-join", "加入房间").on_mouse_down(
                        MouseButton::Left,
                        |_ev, _window, cx| {
                            let (room_code, peers, disable_p2p, no_tun, player_name) = cx
                                .read_global(|state: &ToolsPageState, _cx| {
                                    (
                                        state.room_code.to_string(),
                                        parse_bootstrap_peers(state.bootstrap_peers.as_ref()),
                                        state.disable_p2p,
                                        state.no_tun,
                                        normalized_player_name(state),
                                    )
                                });

                            cx.update_global(|state: &mut ToolsPageState, _cx| {
                                state.online_loading = true;
                                state.online_error = None;
                            });
                            append_online_log(
                                format!("加入房间: room_code={room_code}, player_name={player_name}"),
                                cx,
                            );

                            cx.spawn(async move |cx| {
                                let room =
                                    crate::core::online::paperconnect_parse_room_code(room_code.clone())
                                        .await;
                                match room {
                                    Ok(room) => {
                                        let options = crate::core::online::EasyTierStartOptions {
                                            disable_p2p: Some(disable_p2p),
                                            no_tun: Some(no_tun),
                                            compression: Some("zstd".to_string()),
                                            ipv4: None,
                                        };
                                        let start_result = crate::core::online::easytier_start(
                                            room.network_name.clone(),
                                            room.network_secret.clone(),
                                            peers,
                                            Some(format!("bmcbl-client-{player_name}")),
                                            Some(options),
                                        )
                                        .await;

                                        let status = if start_result.is_ok() {
                                            crate::core::online::easytier_embedded_status()
                                                .await
                                                .ok()
                                                .flatten()
                                        } else {
                                            None
                                        };
                                        let peers = if start_result.is_ok() {
                                            crate::core::online::easytier_embedded_peers()
                                                .await
                                                .unwrap_or_default()
                                                .into_iter()
                                                .map(|peer| crate::ui::views::tools::state::OnlinePeerEntry {
                                                    hostname: SharedString::from(peer.hostname),
                                                    ipv4: peer.ipv4.map(SharedString::from),
                                                })
                                                .collect::<Vec<_>>()
                                        } else {
                                            Vec::new()
                                        };

                                        let error = start_result.err().map(SharedString::from);
                                        let _ = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                            state.online_loading = false;
                                            state.online_error = error;
                                            state.easytier_running = state.online_error.is_none();
                                            state.active_room_code =
                                                SharedString::from(room.room_code.clone());
                                            state.active_network_name =
                                                SharedString::from(room.network_name.clone());
                                            if let Some(status) = status.clone() {
                                                state.easytier_hostname =
                                                    SharedString::from(status.hostname);
                                                state.easytier_ipv4 =
                                                    status.ipv4.map(SharedString::from);
                                            }
                                            state.peers = peers;
                                            state.peers_loading = false;
                                        });

                                        let joined_room_code = room.room_code.clone();
                                        let _ = cx.update(|cx| {
                                            append_online_log(
                                                format!("已加入房间: {}", joined_room_code),
                                                cx,
                                            );
                                        });
                                    }
                                    Err(error) => {
                                        let _ = cx.update_global(|state: &mut ToolsPageState, _cx| {
                                            state.online_loading = false;
                                            state.online_error = Some(SharedString::from(error.clone()));
                                        });
                                        let _ = cx.update(|cx| {
                                            append_online_log(format!("解析联机码失败: {error}"), cx);
                                        });
                                    }
                                }
                            })
                            .detach();
                        },
                    ),
                ),
        )
        .child(render_advanced_card(colors, state))
        .when(!state.host_room_code.as_ref().trim().is_empty(), |this| {
            this.child(render_host_room_code(colors, state.host_room_code.clone()))
        })
}

fn render_join_row(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let room_input = render_join_input(colors, state);
    let clear_input = state.room_code_input.clone();
    let paste_input = state.room_code_input.clone();

    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(room_input)
        .child(
            icon_button(colors, "online-room-clear", lucide_icons::icon_x()).on_mouse_down(
                MouseButton::Left,
                move |_ev, window, cx| {
                    set_room_code_value(clear_input.clone(), SharedString::from(""), window, cx);
                },
            ),
        )
        .child(
            icon_button(colors, "online-room-paste", lucide_icons::icon_clipboard()).on_mouse_down(
                MouseButton::Left,
                move |_ev, window, cx| {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .map(|value| value.trim().replace('\n', " ").replace('\r', " "))
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        toast::error(cx, SharedString::from("剪贴板中没有可用的联机码"));
                        return;
                    }
                    set_room_code_value(paste_input.clone(), SharedString::from(text), window, cx);
                },
            ),
        )
}

fn render_join_input(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let room_input: AnyElement = if let Some(input_state) = state.room_code_input.as_ref() {
        Input::new(input_state)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .cleanable(true)
            .prefix(themed_icon(
                lucide_icons::icon_hash(),
                16.0,
                colors.text_secondary,
            ))
            .w_full()
            .h(px(40.))
            .px(px(14.))
            .into_any_element()
    } else {
        div()
            .w_full()
            .h(px(40.))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .child(themed_icon(
                lucide_icons::icon_hash(),
                16.0,
                colors.text_secondary,
            ))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .child("P/NNNN-NNNN-SSSS-SSSS"),
            )
            .into_any_element()
    };

    div()
        .w_full()
        .rounded(px(14.))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .child(room_input)
}

fn render_advanced_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.52,
            ..colors.surface
        })
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child("高级参数"),
        )
        .child(render_inline_input(
            colors,
            "玩家名",
            state.player_name_input.as_ref(),
            "BMCBL_USER",
        ))
        .child(render_inline_input(
            colors,
            "开放端口",
            state.game_ports_input.as_ref(),
            "7551, 19132",
        ))
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(colors.text_secondary)
                .child("开放端口当前主要用于记录和展示；首个端口会作为本次会话的默认游戏端口。"),
        )
}

fn render_inline_input(
    colors: &ThemeColors,
    label: &'static str,
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
        .gap(px(6.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(label),
        )
        .child(
            div()
                .w_full()
                .rounded(px(14.))
                .border_1()
                .border_color(Hsla {
                    a: 0.16,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.72,
                    ..colors.settings_field_bg
                })
                .px(px(12.))
                .py(px(8.))
                .child(field),
        )
}

fn render_host_room_code(colors: &ThemeColors, room_code: SharedString) -> Div {
    let copy_room_code = room_code.clone();

    div()
        .w_full()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.accent
        })
        .bg(Hsla {
            a: 0.10,
            ..colors.accent
        })
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child("当前房主联机码"),
        )
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(room_code),
                )
                .child(
                    primary_button(colors, "online-copy-room-code", "复制联机码").on_mouse_down(
                        MouseButton::Left,
                        move |_ev, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                copy_room_code.to_string(),
                            ));
                            toast::push(cx, SharedString::from("联机码已复制"));
                        },
                    ),
                ),
        )
}

fn set_room_code_value(
    input: Option<Entity<crate::ui::components::input::InputState>>,
    value: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.room_code = value.clone();
    });

    if let Some(input) = input {
        let _ = input.update(cx, |state, cx| {
            state.set_value(value, window, cx);
        });
    }
}
