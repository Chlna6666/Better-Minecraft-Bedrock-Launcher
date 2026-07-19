use crate::ui::components::icon::themed_icon;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::{controls, peers, room, settings};

pub(in crate::ui::views::tools) fn render_online_panel(
    colors: &ThemeColors,
    state: &ToolsPageState,
    window_width: Pixels,
) -> impl IntoElement {
    let compact = window_width <= px(1280.);
    crate::ui::components::page_shell::split_content_panel(colors)
        .overflow_y_scrollbar()
        .scrollbar_width(px(0.))
        .p(px(14.))
        .child(render_online_body(colors, state, compact))
}

fn render_online_body(colors: &ThemeColors, state: &ToolsPageState, compact: bool) -> Div {
    let primary = div()
        .when(compact, |this| this.w_full())
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(room::render_room_card(colors, state));
    let secondary = div()
        .min_w(px(0.))
        .when(!compact, |this| this.w(px(330.)).flex_none())
        .when(compact, |this| this.w_full())
        .flex()
        .flex_col()
        .gap(px(14.))
        .when(state.easytier_running, |this| {
            this.child(peers::render_room_members_card(colors, state))
        })
        .child(render_activity_card(colors, state))
        .child(controls::render_session_card(colors, state))
        .child(peers::render_network_nodes_card(colors, state));

    div()
        .w_full()
        .flex()
        .when(compact, |this| this.flex_col())
        .items_start()
        .gap(px(14.))
        .child(primary)
        .child(secondary)
}

fn render_activity_card(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .rounded(px(20.))
        .border_1()
        .border_color(Hsla {
            a: 0.15,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.68,
            ..colors.surface
        })
        .p(px(17.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(render_activity_header(colors))
        .when_some(state.online_error.clone(), |this, error| {
            this.child(render_error_banner(colors, error))
        })
        .when(state.online_log.as_ref().trim().is_empty(), |this| {
            this.child(render_activity_hint(colors))
        })
        .when(!state.online_log.as_ref().trim().is_empty(), |this| {
            this.child(render_log_lines(colors, state))
        })
}

fn render_activity_header(colors: &ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(themed_icon(
            lucide_icons::icon_activity(),
            16.0,
            colors.text_secondary,
        ))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child("活动与提示"),
        )
}

fn render_error_banner(colors: &ThemeColors, error: SharedString) -> Div {
    div()
        .w_full()
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla {
            a: 0.28,
            ..colors.danger
        })
        .bg(Hsla {
            a: 0.10,
            ..colors.danger
        })
        .px(px(13.))
        .py(px(11.))
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(colors.danger)
        .child(error)
}

fn render_activity_hint(colors: &ThemeColors) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(19.))
        .text_color(colors.text_secondary)
        .child("创建或加入后，等待数秒让路由建立，再进入 Minecraft。高级网络选项保持默认即可满足大多数场景。")
}

fn render_log_lines(colors: &ThemeColors, state: &ToolsPageState) -> impl IntoElement {
    div().w_full().flex().flex_col().gap(px(6.)).children(
        state.online_log.as_ref().lines().rev().take(8).map(|line| {
            div()
                .w_full()
                .rounded(px(10.))
                .bg(Hsla {
                    a: 0.38,
                    ..colors.settings_field_bg
                })
                .px(px(10.))
                .py(px(7.))
                .text_size(px(11.5))
                .text_color(colors.text_secondary)
                .child(line.to_string())
        }),
    )
}

pub(in crate::ui::views::tools) fn render_online_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    state: &ToolsPageState,
) -> Option<AnyElement> {
    settings::render_settings_overlay(colors, window_width, window_height, state)
}

pub(in crate::ui::views::tools) fn parse_bootstrap_peers(text: &str) -> Vec<String> {
    text.split(|character: char| character.is_whitespace() || matches!(character, ',' | ';'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(in crate::ui::views::tools) fn primary_game_port(state: &ToolsPageState) -> u16 {
    state
        .game_ports
        .split(|character: char| character.is_whitespace() || matches!(character, ',' | ';'))
        .find_map(|value| {
            value
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| (1025..=65535).contains(port))
        })
        .unwrap_or(7551)
}

pub(in crate::ui::views::tools) fn normalized_player_name(state: &ToolsPageState) -> String {
    let value = state.player_name.as_ref().trim();
    if value.is_empty() {
        crate::config::config::default_online_player_name()
    } else {
        value.chars().take(32).collect()
    }
}

pub(in crate::ui::views::tools) fn append_online_log(message: impl Into<String>, cx: &mut App) {
    let message = message.into();
    if message.trim().is_empty() {
        return;
    }

    cx.update_global(|state: &mut ToolsPageState, _cx| {
        let mut lines = state
            .online_log
            .as_ref()
            .lines()
            .rev()
            .take(79)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        lines.reverse();
        lines.push(message);
        state.online_log = SharedString::from(lines.join("\n"));
    });
}

pub(in crate::ui::views::tools) fn online_state_text(state: &ToolsPageState) -> SharedString {
    if state.online_operation.is_busy() {
        SharedString::from(state.online_operation.label())
    } else if state.online_error.is_some() {
        SharedString::from("需要处理")
    } else if state.easytier_running {
        SharedString::from("已连接")
    } else {
        SharedString::from("等待连接")
    }
}

#[cfg(test)]
mod tests {
    use gpui::SharedString;

    use super::{parse_bootstrap_peers, primary_game_port};
    use crate::ui::views::tools::state::ToolsPageState;

    #[test]
    fn parses_bootstrap_peers_across_supported_separators() {
        assert_eq!(
            parse_bootstrap_peers("tcp://one:1, tcp://two:2;tcp://three:3"),
            vec!["tcp://one:1", "tcp://two:2", "tcp://three:3"]
        );
    }

    #[test]
    fn primary_port_skips_invalid_and_zero_values() {
        let mut state = ToolsPageState::default();
        state.game_ports = SharedString::from("invalid, 0; 19132");
        assert_eq!(primary_game_port(&state), 19132);
    }

    #[test]
    fn primary_port_skips_reserved_system_ports() {
        let mut state = ToolsPageState::default();
        state.game_ports = SharedString::from("1024, 19132");
        assert_eq!(primary_game_port(&state), 19132);
    }
}
