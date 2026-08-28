use crate::ui::animation::{spring_bouncy, spring_motion};
use crate::ui::components::input::Input;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::theme::tokens::motion;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::widgets::subtle_button;

pub(super) fn render_advanced_section(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
) -> Div {
    let mut section = div().w_full().flex().flex_col().gap(px(10.)).child(
        subtle_button(
            colors,
            "online-room-advanced",
            if state.room_advanced_open {
                t!("Online.room_options_close")
            } else {
                t!("Online.room_options")
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
        section = section.child(render_advanced_panel(colors, i18n, state));
    }

    section
}

fn render_advanced_panel(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &ToolsPageState,
) -> impl IntoElement {
    crate::ui::components::page_shell::inner_well(colors)
        .w_full()
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(render_inline_input(
            colors,
            t!("Online.player_name"),
            t!("Online.player_name_hint"),
            state.player_name_input.as_ref(),
            t!("Online.player_name_placeholder"),
        ))
        .child(render_inline_input(
            colors,
            t!("Online.open_ports"),
            t!("Online.open_ports_hint_short"),
            state.game_ports_input.as_ref(),
            t!("Online.open_ports_placeholder"),
        ))
        .with_animation(
            "online-room-advanced-panel",
            spring_motion(spring_bouncy(), motion::BOUNCY_WINDOW),
            |panel, progress| {
                panel
                    .opacity(progress.clamp(0.0, 1.0))
                    .relative()
                    .top(px((1.0 - progress) * motion::ENTRANCE_OFFSET))
            },
        )
}

fn render_inline_input(
    colors: &ThemeColors,
    label: SharedString,
    helper: SharedString,
    input: Option<&Entity<crate::ui::components::input::InputState>>,
    placeholder: SharedString,
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

fn render_input_label(colors: &ThemeColors, label: SharedString, helper: SharedString) -> Div {
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
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(colors.settings_field_bg)
        .px(px(12.))
        .child(field)
}
