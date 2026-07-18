use crate::ui::animation::ease_out_cubic_motion;
use crate::ui::components::input::Input;
use crate::ui::components::modal;
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;
use gpui::AnimationExt as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;
use std::time::Duration;

use super::controls::persist_tools_online_settings;
use super::widgets::{action_button, icon_button};

type DismissAction = Rc<dyn Fn(&mut App)>;

pub(super) fn render_settings_overlay(
    colors: &ThemeColors,
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
    let card = render_settings_card(colors, width, state, dismiss.clone()).with_animation(
        "online-settings-card-enter",
        ease_out_cubic_motion(Duration::from_millis(240)),
        |card, progress| {
            card.opacity(progress)
                .relative()
                .top(px((1.0 - progress) * 14.0))
        },
    );

    Some(modal::modal_layer_dismissible(card, colors.backdrop, dismiss).into_any_element())
}

fn render_settings_card(
    colors: &ThemeColors,
    width: Pixels,
    state: &ToolsPageState,
    close: DismissAction,
) -> Div {
    div()
        .w(width)
        .max_w(px(620.))
        .rounded(px(22.))
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.98,
            ..colors.settings_panel_bg
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.26,
                ..rgb(0x000000).into()
            },
            blur_radius: px(42.),
            spread_radius: px(-8.),
            offset: point(px(0.), px(20.)),
        }])
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(render_settings_header(colors, close))
        .child(render_settings_body(colors, state))
        .child(render_settings_footer(colors))
}

fn render_settings_header(colors: &ThemeColors, close: DismissAction) -> Div {
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
                        .child("EasyTier 网络设置"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child("默认设置适合大多数网络，只在连接受限时调整。"),
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

fn render_settings_body(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    div()
        .w_full()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(render_bootstrap_field(colors, state))
        .child(render_toggle_row(
            colors,
            "online-disable-p2p",
            "优先中继",
            "禁用 P2P 直连，适合严格防火墙或校园网环境。",
            state.disable_p2p,
            |state| state.disable_p2p = !state.disable_p2p,
        ))
        .child(render_toggle_row(
            colors,
            "online-no-tun",
            "兼容模式",
            "不创建虚拟网卡，减少驱动和权限问题。",
            state.no_tun,
            |state| state.no_tun = !state.no_tun,
        ))
}

fn render_settings_footer(colors: &ThemeColors) -> Div {
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
                "完成",
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

fn render_bootstrap_field(colors: &ThemeColors, state: &ToolsPageState) -> Div {
    let input = render_bootstrap_input(colors, state);

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
                .child("固定引导节点"),
        )
        .child(
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
                .child(input),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child("可用空格、逗号或分号分隔多个 tcp:// 节点。"),
        )
}

fn render_bootstrap_input(colors: &ThemeColors, state: &ToolsPageState) -> AnyElement {
    state.bootstrap_peers_input.as_ref().map_or_else(
        || {
            div()
                .h(px(42.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child("留空自动选择公共节点")
                .into_any_element()
        },
        |input| {
            Input::new(input)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .cleanable(true)
                .w_full()
                .h(px(42.))
                .into_any_element()
        },
    )
}

fn render_toggle_row(
    colors: &ThemeColors,
    id: &'static str,
    title: &'static str,
    description: &'static str,
    enabled: bool,
    toggle: fn(&mut ToolsPageState),
) -> Div {
    div()
        .w_full()
        .rounded(px(15.))
        .border_1()
        .border_color(Hsla {
            a: 0.13,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.42,
            ..colors.surface
        })
        .px(px(14.))
        .py(px(12.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(render_toggle_copy(colors, title, description))
        .child(ToggleSwitch::new(
            SharedString::from(id),
            colors,
            enabled,
            move |cx| {
                cx.update_global(|state: &mut ToolsPageState, _cx| toggle(state));
                persist_tools_online_settings(cx);
            },
        ))
}

fn render_toggle_copy(colors: &ThemeColors, title: &'static str, description: &'static str) -> Div {
    div()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(colors.text_secondary)
                .child(description),
        )
}
