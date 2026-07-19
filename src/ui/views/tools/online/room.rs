use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::Input;
use crate::ui::components::toast;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::actions;
use super::room_options;
use super::widgets::{action_button, icon_button};

pub(super) fn render_room_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let busy = state.online_operation.is_busy();
    let actions_disabled = busy || state.easytier_running;

    div()
        .w_full()
        .rounded(px(22.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.82,
            ..colors.surface
        })
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(render_header(colors, state))
        .when(!busy && !state.easytier_running, |this| {
            this.child(render_create_action(colors, actions_disabled))
                .child(render_join_action(colors, state, actions_disabled))
                .child(room_options::render_advanced_section(colors, state))
        })
        .when(busy, |this| {
            this.child(render_connecting_state(colors, state))
        })
        .when(!busy && state.easytier_running, |this| {
            this.child(render_connected_state(colors, state))
        })
        .when(!state.host_room_code.as_ref().trim().is_empty(), |this| {
            this.child(render_host_room_code(colors, state.host_room_code.clone()))
        })
}

fn render_header(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let latency = state.host_or_avg_latency();
    let player_count = if state.players.is_empty() && state.easytier_running {
        1
    } else {
        state.players.len()
    };

    div()
        .w_full()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .text_size(px(19.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child("开始联机"),
                        )
                        .when(state.easytier_running, |this| {
                            this.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.))
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
                                            .child(themed_icon(
                                                lucide_icons::icon_wifi(),
                                                13.0,
                                                colors.accent,
                                            ))
                                            .child(match latency {
                                                Some(ms) => format!("延迟 {ms} ms"),
                                                None => "延迟 -- ms".to_string(),
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.))
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
                                            .child(themed_icon(
                                                lucide_icons::icon_users(),
                                                13.0,
                                                colors.accent,
                                            ))
                                            .child(format!("玩家 {player_count} 人")),
                                    ),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(colors.text_secondary)
                        .child("创建新房间，或使用好友分享的联机码直接加入。"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when(state.online_operation.is_busy(), |this| {
                    this.child(
                        div()
                            .flex_none()
                            .rounded(px(999.))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.24,
                                ..colors.accent
                            })
                            .bg(Hsla {
                                a: 0.12,
                                ..colors.accent
                            })
                            .px(px(11.))
                            .py(px(7.))
                            .text_size(px(12.))
                            .text_color(colors.accent)
                            .child(state.online_operation.label()),
                    )
                })
                .when(
                    state.easytier_running
                        || matches!(
                            state.online_operation,
                            crate::ui::views::tools::state::OnlineOperation::CreatingRoom
                                | crate::ui::views::tools::state::OnlineOperation::JoiningRoom
                                | crate::ui::views::tools::state::OnlineOperation::Stopping
                        ),
                    |this| this.child(render_quick_action(colors, state)),
                ),
        )
}

fn render_quick_action(colors: &ThemeColors, state: &ToolsPageState) -> Stateful<Div> {
    let stopping =
        state.online_operation == crate::ui::views::tools::state::OnlineOperation::Stopping;
    let (id, label, icon) = if state.easytier_running {
        ("online-stop", "断开连接", lucide_icons::icon_log_out())
    } else if state.online_operation.is_busy() {
        ("online-cancel", "取消", lucide_icons::icon_x())
    } else {
        ("online-stop", "断开连接", lucide_icons::icon_log_out())
    };
    action_button(colors, id, label, icon, stopping, true).when(!stopping, |this| {
        this.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            actions::stop_session(cx);
        })
    })
}

fn render_connecting_state(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .rounded(px(17.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.accent
        })
        .bg(Hsla {
            a: 0.08,
            ..colors.accent
        })
        .p(px(18.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(themed_icon(
            lucide_icons::icon_loader_circle(),
            20.0,
            colors.accent,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(state.online_operation.label()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child("正在建立网络，完成后即可进入 Minecraft。"),
                ),
        )
}

fn render_connected_state(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .rounded(px(17.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.accent
        })
        .bg(Hsla {
            a: 0.08,
            ..colors.accent
        })
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child("已连接，可以进入 Minecraft"),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(format!("联机码：{}", state.active_room_code)),
        )
}

fn render_create_action(colors: &ThemeColors, disabled: bool) -> Div {
    div()
        .w_full()
        .rounded(px(17.))
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.accent
        })
        .bg(Hsla {
            a: 0.10,
            ..colors.accent
        })
        .p(px(15.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(render_create_description(colors))
        .child(render_create_button(colors, disabled))
}

fn render_create_description(colors: &ThemeColors) -> Div {
    div()
        .min_w(px(0.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .size(px(42.))
                .rounded(px(14.))
                .bg(Hsla {
                    a: 0.18,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(themed_icon(lucide_icons::icon_plus(), 19.0, colors.accent)),
        )
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child("创建我的房间"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child("自动生成联机码并复制到剪贴板"),
                ),
        )
}

fn render_create_button(colors: &ThemeColors, disabled: bool) -> Stateful<Div> {
    action_button(
        colors,
        "online-create-room",
        "立即创建",
        lucide_icons::icon_arrow_right(),
        disabled,
        false,
    )
    .when(!disabled, |this| {
        this.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            actions::create_room(cx);
        })
    })
}

fn render_join_action(colors: &ThemeColors, state: &ToolsPageState, disabled: bool) -> Div {
    let paste_input = state.room_code_input.clone();
    div()
        .w_full()
        .rounded(px(17.))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.52,
            ..colors.settings_field_bg
        })
        .p(px(15.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_secondary)
                .child("加入好友房间"),
        )
        .child(render_join_controls(colors, state, disabled, paste_input))
}

fn render_join_controls(
    colors: &ThemeColors,
    state: &ToolsPageState,
    disabled: bool,
    paste_input: Option<Entity<crate::ui::components::input::InputState>>,
) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(9.))
        .child(render_join_input(colors, state))
        .child(
            icon_button(
                colors,
                "online-room-paste",
                lucide_icons::icon_clipboard(),
                disabled,
            )
            .when(!disabled, |this| {
                this.on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                    paste_room_code(paste_input.clone(), window, cx);
                })
            }),
        )
        .child(render_join_button(colors, disabled))
}

fn render_join_button(colors: &ThemeColors, disabled: bool) -> Stateful<Div> {
    action_button(
        colors,
        "online-join-room",
        "加入",
        lucide_icons::icon_log_in(),
        disabled,
        false,
    )
    .when(!disabled, |this| {
        this.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            actions::join_room(cx);
        })
    })
}

fn render_join_input(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let field: AnyElement = if let Some(input_state) = state.room_code_input.as_ref() {
        Input::new(input_state)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .cleanable(true)
            .prefix(themed_icon(
                lucide_icons::icon_hash(),
                16.0,
                colors.text_muted,
            ))
            .w_full()
            .h(px(42.))
            .px(px(13.))
            .into_any_element()
    } else {
        div()
            .h(px(42.))
            .px(px(13.))
            .flex()
            .items_center()
            .text_size(px(13.))
            .text_color(colors.text_muted)
            .child("P/NNNN-NNNN-SSSS-SSSS")
            .into_any_element()
    };

    div()
        .flex_1()
        .min_w(px(180.))
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(colors.surface)
        .child(field)
}

fn render_host_room_code(colors: &ThemeColors, room_code: SharedString) -> Div {
    let copy_room_code = room_code.clone();
    div()
        .w_full()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.25,
            ..colors.accent
        })
        .bg(Hsla {
            a: 0.12,
            ..colors.accent
        })
        .p(px(14.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .child(render_host_room_code_text(colors, room_code))
        .child(
            action_button(
                colors,
                "online-copy-room-code",
                "复制",
                lucide_icons::icon_copy(),
                false,
                false,
            )
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_room_code.to_string()));
                toast::push(cx, SharedString::from("联机码已复制"));
            }),
        )
}

fn render_host_room_code_text(colors: &ThemeColors, room_code: SharedString) -> Div {
    div()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_secondary)
                .child("分享此联机码"),
        )
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .truncate()
                .child(room_code),
        )
}

fn paste_room_code(
    input: Option<Entity<crate::ui::components::input::InputState>>,
    window: &mut Window,
    cx: &mut App,
) {
    let value = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .map(|value| value.trim().replace(['\n', '\r'], " "))
        .unwrap_or_default();
    if value.trim().is_empty() {
        toast::error(cx, SharedString::from("剪贴板中没有可用的联机码"));
        return;
    }

    let value = SharedString::from(value);
    cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.room_code = value.clone();
    });
    if let Some(input) = input {
        input.update(cx, |state, cx| {
            state.set_value(value, window, cx);
        });
    }
}
