use crate::ui::animation::{spring_motion, spring_smooth};
use crate::ui::components::input::Input;
use crate::ui::components::modal;
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::theme::tokens::motion;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::AnimationExt as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;

use super::controls::persist_tools_online_settings;
use super::widgets::{action_button, icon_button};

type DismissAction = Rc<dyn Fn(&mut App)>;

pub(super) fn render_settings_overlay(
    colors: &ThemeColors,
    i18n: &I18n,
    window_width: Pixels,
    _window_height: Pixels,
    state: &ToolsPageState,
) -> Option<AnyElement> {
    if !state.easytier_settings_open {
        return None;
    }

    let width = if window_width <= px(720.) {
        (window_width - px(32.)).max(px(320.))
    } else {
        px(620.)
    };
    let dismiss: DismissAction = Rc::new(|cx: &mut App| {
        cx.update_global(|state: &mut ToolsPageState, _cx| {
            state.easytier_settings_open = false;
        });
    });

    // Capture the complete settings card once and animate only its final compositor record.
    // The previous layout-driven `top(...) + opacity(...)` path rebuilt the card's layout and text
    // prepaint on every spring sample. A very small scale-in preserves the entrance affordance
    // without changing layout identity or touching the card's glyph atlas entries.
    let card = render_settings_card(colors, i18n, width, state, dismiss.clone())
        .composite_layer()
        .with_animation(
            "online-settings-card-enter",
            spring_motion(spring_smooth()).with_property(AnimationProperty::scale_opacity(
                0.985,
                1.0,
                0.0,
                1.0,
                TransformOrigin::CENTER,
            )),
            |card, _progress| card,
        );

    Some(modal::modal_layer_dismissible(card, colors.backdrop, dismiss).into_any_element())
}

fn render_settings_card(
    colors: &ThemeColors,
    i18n: &I18n,
    width: Pixels,
    state: &ToolsPageState,
    close: DismissAction,
) -> Div {
    div()
        .w(width)
        .max_w(px(620.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.98,
            ..colors.settings_panel_bg
        })
        .shadow(crate::ui::components::page_shell::panel_shadow())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(render_settings_header(colors, i18n, close))
        .child(render_settings_body(colors, i18n, state))
        .child(render_settings_footer(colors, i18n))
}

fn render_settings_header(colors: &ThemeColors, i18n: &I18n, close: DismissAction) -> Div {
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
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(t!("Online.settings_title")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(t!("Online.settings_description")),
                ),
        )
        .child(
            icon_button(
                colors,
                "online-settings-close",
                lucide_icons::icon_x(),
                false,
            )
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| close(cx)),
        )
}

fn render_settings_body(colors: &ThemeColors, i18n: &I18n, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(render_bootstrap_field(colors, i18n, state))
        .child(render_toggle_row(
            colors,
            t!("Online.relay_first"),
            t!("Online.relay_first_description"),
            "online-disable-p2p",
            state.disable_p2p,
            |state| state.disable_p2p = !state.disable_p2p,
        ))
}

fn render_settings_footer(colors: &ThemeColors, i18n: &I18n) -> Div {
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
            action_button(
                colors,
                "online-settings-done",
                t!("Online.settings_done"),
                lucide_icons::icon_check(),
                false,
                false,
            )
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                persist_tools_online_settings(cx);
                cx.update_global(|state: &mut ToolsPageState, _cx| {
                    state.easytier_settings_open = false;
                });
            }),
        )
}

fn render_bootstrap_field(colors: &ThemeColors, i18n: &I18n, state: &ToolsPageState) -> Div {
    let input = render_bootstrap_input(colors, i18n, state);

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_secondary)
                .child(t!("Online.bootstrap_peers")),
        )
        .child(
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
                .child(input),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(t!("Online.bootstrap_peers_hint")),
        )
}

fn render_bootstrap_input(colors: &ThemeColors, i18n: &I18n, state: &ToolsPageState) -> AnyElement {
    state.bootstrap_peers_input.as_ref().map_or_else(
        || {
            div()
                .h(px(42.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(t!("Online.bootstrap_peers_empty"))
                .into_any_element()
        },
        |input| {
            Input::new(input)
                .id("online-bootstrap-peers")
                .placeholder(t!("Online.bootstrap_peers_placeholder"))
                .into_any_element()
        },
    )
}

fn render_toggle_row(
    colors: &ThemeColors,
    label: SharedString,
    description: SharedString,
    toggle_id: &'static str,
    checked: bool,
    toggle: impl Fn(&mut ToolsPageState) + 'static,
) -> Div {
    div()
        .w_full()
        .min_h(px(58.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .bg(colors.settings_field_bg)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_primary)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(description),
                ),
        )
        .child(ToggleSwitch::new(toggle_id, checked).on_toggle(move |_checked, _window, cx| {
            cx.update_global(|state: &mut ToolsPageState, cx| {
                toggle(state);
                persist_tools_online_settings(cx);
            });
        }))
}
