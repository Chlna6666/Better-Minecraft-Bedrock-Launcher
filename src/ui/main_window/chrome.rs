use super::TopbarRenderState;
pub(super) mod auth;

use crate::ui::navigation::{self, AppRoute, RouteTarget};
use crate::ui::state::theme::ThemeState;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::{dark_colors, glass_backdrop_blur_style, lerp_theme_colors, light_colors};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::time::Instant;

pub(crate) struct AppChromeState {
    pub(crate) titlebar_gesture: crate::ui::window::chrome::TitlebarGesture,
}

impl Default for AppChromeState {
    fn default() -> Self {
        Self {
            titlebar_gesture: crate::ui::window::chrome::TitlebarGesture::default(),
        }
    }
}

impl Global for AppChromeState {}

#[derive(Clone)]
struct NavItem {
    icon_path: &'static str,
    image_icon_path: Option<std::path::PathBuf>,
    label: SharedString,
    target: RouteTarget,
}

fn icon(path: &'static str, color: Hsla, size: Pixels) -> Svg {
    svg().path(path).size(size).text_color(color)
}

pub(super) fn render_app_chrome(
    state: TopbarRenderState,
    _route: RouteTarget,
    update_modal_open: bool,
) -> AnyElement {
    let colors = lerp_theme_colors(
        light_colors(),
        dark_colors(),
        state.theme_k,
        state.theme_accent,
    );
    let window_width_px = state.window_width / px(1.);
    let labels_layout_factor = state.labels_layout_factor.clamp(0.0, 1.0);
    let labels_opacity_factor = state.labels_opacity_factor.clamp(0.0, 1.0);
    let mut nav_items = vec![
        (
            lucide_icons::icon_house(),
            t!("Sidebar.launch"),
            AppRoute::Home,
        ),
        (
            lucide_icons::icon_download(),
            t!("Sidebar.download"),
            AppRoute::Download,
        ),
        (
            lucide_icons::icon_list(),
            t!("Sidebar.versions"),
            AppRoute::Manage,
        ),
        (
            lucide_icons::icon_wrench(),
            t!("Sidebar.tools"),
            AppRoute::Tools,
        ),
        (
            lucide_icons::icon_activity(),
            t!("Tasks.title"),
            AppRoute::Tasks,
        ),
        (
            lucide_icons::icon_settings(),
            t!("Sidebar.settings"),
            AppRoute::Settings,
        ),
    ]
    .into_iter()
    .map(|(icon_path, label, target)| NavItem {
        icon_path,
        image_icon_path: None,
        label,
        target: RouteTarget::Builtin(target),
    })
    .collect::<Vec<_>>();
    nav_items.extend(state.plugin_navigation_pages.iter().map(|page| NavItem {
        icon_path: lucide_icons::icon_plug(),
        image_icon_path: page.icon_path.clone(),
        label: page.navigation.as_ref().map_or_else(
            || page.title.clone(),
            |navigation| SharedString::from(navigation.label.clone()),
        ),
        target: RouteTarget::Plugin {
            plugin_id: page.plugin_id.clone(),
            page_id: page.page_id.clone(),
        },
    }));

    let link_padding_x = if window_width_px <= 1000.0 {
        px(10.)
    } else {
        px(13.)
    };
    let icon_width = px(18.);
    let label_width = px(33.) * labels_layout_factor;
    let label_gap = px(7.) * labels_layout_factor;
    let item_width = link_padding_x * 2. + icon_width + label_gap + label_width;
    let item_height = px(34.);
    let capsule_gap = px(3.);
    let capsule_padding = px(5.);
    let navigation_length = nav_items.len();
    let active_index = state
        .visual_active_index
        .min(navigation_length.saturating_sub(1));
    let step_width_px = (item_width + capsule_gap) / px(1.);
    let maximum_offset_px = step_width_px * navigation_length.saturating_sub(1) as f32;
    let overshoot_slack_px = step_width_px * 0.30;
    let maximum_right_px = maximum_offset_px + item_width / px(1.);
    let left_edge_px =
        (step_width_px * state.pill_left_steps).clamp(-overshoot_slack_px, maximum_right_px);
    let right_edge_px = (step_width_px * state.pill_right_steps + item_width / px(1.))
        .clamp(0.0, maximum_right_px + overshoot_slack_px);
    let pill_inner_inset_px = 1.5;
    let pill_offset = capsule_padding + px(left_edge_px.min(right_edge_px) + pill_inner_inset_px);
    let pill_width = px(((right_edge_px - left_edge_px).abs() - pill_inner_inset_px * 2.).max(0.));

    let nav = div()
        .relative()
        .flex()
        .items_center()
        .gap(capsule_gap)
        .p(capsule_padding)
        .rounded(px(24.))
        .bg(colors.text_primary.opacity(0.045))
        .child(
            div()
                .absolute()
                .left(pill_offset)
                .top(capsule_padding)
                .w(pill_width)
                .h(item_height)
                .rounded(px(17.))
                .bg(colors.accent),
        )
        .children(nav_items.into_iter().enumerate().map(|(index, item)| {
            let active = index == active_index;
            let foreground = if active {
                rgb(0xffffff).into()
            } else {
                colors.text_primary
            };
            let icon_element = item.image_icon_path.clone().map_or_else(
                || icon(item.icon_path, foreground, px(18.)).into_any_element(),
                |path| {
                    img(path)
                        .size(px(18.))
                        .rounded(px(4.))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element()
                },
            );
            // Collapsed labels must not enter the scene at all: a zero-width
            // overflow-hidden text still emits clip/glyph primitives that show up
            // as white blocks during interactive resize.
            let show_label = labels_layout_factor > 0.02 && labels_opacity_factor > 0.02;
            div()
                .id(SharedString::from(format!("main-nav-{index}")))
                .relative()
                .w(item_width)
                .h(item_height)
                .rounded(px(17.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .occlude()
                .window_control_area(WindowControlArea::Client)
                .text_color(foreground)
                .hover(move |style| style.opacity(0.88))
                .active(|style| style.scale(0.94))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    navigation::navigate_target(cx, item.target.clone());
                })
                .child(
                    div()
                        .w(icon_width)
                        .h_full()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .child(icon_element),
                )
                .children(show_label.then(|| {
                    div()
                        .w(label_width)
                        .ml(label_gap)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .opacity(labels_opacity_factor)
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(item.label.clone())
                }))
        }));

    let auth_inline = auth::trigger(&state.auth, &colors);

    let icon_button = |id: &'static str, path: &'static str| {
        div()
            .id(id)
            .size(px(38.))
            .rounded(px(9.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .occlude()
            .window_control_area(WindowControlArea::Client)
            .text_color(colors.text_primary)
            .hover(|style| style.bg(colors.text_primary.opacity(0.07)))
            .active(|style| style.bg(colors.text_primary.opacity(0.12)))
            .child(icon(path, colors.text_primary, px(16.)))
    };
    let controls = div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(auth_inline)
        .child(
            icon_button(
                "theme-toggle-linux",
                if state.theme_target_dark {
                    lucide_icons::icon_sun()
                } else {
                    lucide_icons::icon_moon()
                },
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
                ThemeState::toggle_global(cx);
            }),
        )
        .child(
            icon_button("window-minimize-linux", lucide_icons::icon_minus())
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    // Do not hide the native window on mouse-down. GPUI must first receive the
                    // corresponding mouse-up so the transient :active state cannot survive a
                    // minimize/restore cycle.
                    cx.stop_propagation();
                })
                .on_click(|_, window, _| {
                    window.refresh();
                    window.minimize_window();
                }),
        )
        .child(
            icon_button("window-close-linux", lucide_icons::icon_x())
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(|_, window, _| {
                    window.remove_window();
                }),
        );

    let titlebar_mouse_down = |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
        cx.update_global(|state: &mut AppChromeState, _cx| {
            state
                .titlebar_gesture
                .handle_mouse_down(event, window, Instant::now());
        });
    };
    let titlebar_mouse_move = |event: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
        if event.dragging() {
            cx.update_global(|state: &mut AppChromeState, _cx| {
                state.titlebar_gesture.handle_mouse_move(event, window);
            });
        }
    };

    let topbar = div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(px(60.))
        .px(px(18.))
        .flex()
        .items_center()
        .justify_between()
        .bg(colors.surface.opacity(if state.glass_effect_enabled {
            0.78
        } else {
            1.0
        }))
        .when(state.glass_effect_enabled, |element| {
            element.backdrop_blur(glass_backdrop_blur_style())
        })
        .border_b_1()
        .border_color(colors.border.opacity(0.55))
        .when(cfg!(target_os = "windows"), |element| {
            element.window_control_area(WindowControlArea::Drag)
        })
        .when(!cfg!(target_os = "windows"), |element| {
            element
                .on_mouse_down(MouseButton::Left, titlebar_mouse_down)
                .on_mouse_move(titlebar_mouse_move)
                .on_mouse_up(MouseButton::Left, |_, _, cx| {
                    cx.update_global(|state: &mut AppChromeState, _cx| {
                        state.titlebar_gesture.handle_mouse_up();
                    });
                })
        })
        .child(
            div()
                .w(px(162.))
                .flex()
                .items_center()
                .gap(px(9.))
                .child(
                    img("icons/logo.png")
                        .size(px(34.))
                        .rounded(px(0.))
                        .object_fit(ObjectFit::Contain),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.accent)
                                .child("BMCBL"),
                        )
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(colors.text_secondary)
                                .child(format!("v{}", crate::utils::app_info::get_version())),
                        ),
                )
                .when(state.update_available && !update_modal_open, |element| {
                    element.child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .bg(colors.accent)
                            .cursor_pointer()
                            .occlude()
                            .window_control_area(WindowControlArea::Client)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                                cx.update_global(|update: &mut UpdateState, _cx| {
                                    update.request_open_modal(Instant::now());
                                });
                            }),
                    )
                }),
        )
        .child(nav)
        .child(controls);

    div()
        .absolute()
        .inset_0()
        .child(topbar)
        .when(state.auth.visible(), |root| {
            root.child(auth::panel(
                &state.auth,
                &colors,
                size(state.window_width, state.window_height),
                state.glass_effect_enabled,
            ))
        })
        .into_any_element()
}
