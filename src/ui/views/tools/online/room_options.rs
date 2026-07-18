use crate::ui::animation::ease_out_cubic_motion;
use crate::ui::components::input::Input;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::time::Duration;

use super::widgets::subtle_button;

pub(super) fn render_advanced_section(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let mut section = div().w_full().flex().flex_col().gap(px(10.)).child(
        subtle_button(
            colors,
            "online-room-advanced",
            if state.room_advanced_open {
                "收起房间参数"
            } else {
                "房间参数"
            },
            lucide_icons::icon_sliders_horizontal(),
            state.online_operation.is_busy(),
        )
        .when(!state.online_operation.is_busy(), |this| {
            this.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.update_global(|state: &mut ToolsPageState, _cx| {
                    state.room_advanced_open = !state.room_advanced_open;
                });
            })
        }),
    );

    if state.room_advanced_open {
        section = section.child(render_advanced_panel(colors, state));
    }

    section
}

fn render_advanced_panel(colors: &ThemeColors, state: &ToolsPageState) -> impl IntoElement {
    div()
        .w_full()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.38,
            ..colors.surface
        })
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(render_inline_input(
            colors,
            "玩家名",
            "留空时每次自动生成随机名称",
            state.player_name_input.as_ref(),
            "留空自动生成",
        ))
        .child(render_inline_input(
            colors,
            "开放端口",
            "首个有效端口用作本次房主标识",
            state.game_ports_input.as_ref(),
            "7551, 19132",
        ))
        .with_animation(
            "online-room-advanced-panel",
            ease_out_cubic_motion(Duration::from_millis(220)),
            |panel, progress| {
                panel
                    .opacity(progress)
                    .relative()
                    .top(px((1.0 - progress) * 9.0))
            },
        )
}

fn render_inline_input(
    colors: &ThemeColors,
    label: &'static str,
    helper: &'static str,
    input: Option<&Entity<crate::ui::components::input::InputState>>,
    placeholder: &'static str,
) -> Div {
    let field: AnyElement = input.map_or_else(
        || {
            div()
                .h(px(40.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(placeholder)
                .into_any_element()
        },
        |input| {
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .cleanable(true)
                .w_full()
                .h(px(40.))
                .into_any_element()
        },
    );

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(render_input_label(colors, label, helper))
        .child(render_input_field(colors, field))
}

fn render_input_label(colors: &ThemeColors, label: &'static str, helper: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_secondary)
                .child(label),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(helper),
        )
}

fn render_input_field(colors: &ThemeColors, field: AnyElement) -> Div {
    div()
        .w_full()
        .rounded(px(13.))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(colors.settings_field_bg)
        .px(px(12.))
        .child(field)
}
