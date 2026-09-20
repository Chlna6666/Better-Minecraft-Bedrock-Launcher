pub(super) mod auth;

use crate::ui::navigation::{self, AppRoute, RouteTarget};
use crate::ui::state::theme::ThemeState;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::{ThemeColors, glass_backdrop_blur_style};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
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

pub(super) struct NavItem {
    id: SharedString,
    icon_path: &'static str,
    image_icon_path: Option<std::sync::Arc<std::path::Path>>,
    label: SharedString,
    target: std::sync::Arc<RouteTarget>,
}

const BUILTIN_NAV: [(AppRoute, &'static str, crate::i18n::I18nKey); 6] = [
    (
        AppRoute::Home,
        lucide_gpui::icon!(house),
        crate::i18n_key!("Sidebar.launch"),
    ),
    (
        AppRoute::Download,
        lucide_gpui::icon!(download),
        crate::i18n_key!("Sidebar.download"),
    ),
    (
        AppRoute::Manage,
        lucide_gpui::icon!(list),
        crate::i18n_key!("Sidebar.versions"),
    ),
    (
        AppRoute::Tools,
        lucide_gpui::icon!(wrench),
        crate::i18n_key!("Sidebar.tools"),
    ),
    (
        AppRoute::Tasks,
        lucide_gpui::icon!(activity),
        crate::i18n_key!("Tasks.nav_title"),
    ),
    (
        AppRoute::Settings,
        lucide_gpui::icon!(settings),
        crate::i18n_key!("Sidebar.settings"),
    ),
];

pub(super) fn navigation_items(cx: &App) -> Vec<NavItem> {
    let i18n = cx.global::<crate::ui::state::i18n::I18n>();
    let plugin_pages = crate::plugins::runtime::navigation_pages(cx);
    let mut items = Vec::with_capacity(BUILTIN_NAV.len() + plugin_pages.len());
    items.extend(BUILTIN_NAV.iter().map(|(route, icon_path, key)| NavItem {
        id: route.pathname().into(),
        icon_path,
        image_icon_path: None,
        label: i18n.t_key(*key),
        target: std::sync::Arc::new(RouteTarget::Builtin(*route)),
    }));
    items.extend(plugin_pages.into_iter().map(|page| {
        let target = RouteTarget::Plugin {
            plugin_id: page.plugin_id,
            page_id: page.page_id,
        };
        NavItem {
            id: target.pathname().into(),
            icon_path: lucide_gpui::icon!(plug),
            image_icon_path: page.icon_path.map(Into::into),
            label: page
                .navigation
                .map_or(page.title, |navigation| navigation.label.into()),
            target: std::sync::Arc::new(target),
        }
    }));
    items
}

fn icon(path: &'static str, color: Hsla, size: Pixels) -> Svg {
    svg().path(path).size(size).text_color(color)
}

const APP_VERSION_LABEL: &str = concat!("v", env!("BMCBL_BUILD_VERSION"));

pub(super) struct NavRenderState {
    pub window_width: Pixels,
    pub visual_active_index: usize,
    pub pill_left_steps: f32,
    pub pill_right_steps: f32,
    pub labels_layout_factor: f32,
    pub labels_opacity_factor: f32,
    pub nav_animating: bool,
}

pub(super) fn render_nav(
    state: NavRenderState,
    nav_items: &[NavItem],
    colors: &ThemeColors,
) -> AnyElement {
    let window_width_px = state.window_width / px(1.);
    let labels_layout_factor = state.labels_layout_factor.clamp(0.0, 1.0);
    let labels_opacity_factor = state.labels_opacity_factor.clamp(0.0, 1.0);
    let nav_animating = state.nav_animating;

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

    // Only this absolute child changes geometry while the pill springs are active. Keep the
    // retained invalidation boundary here instead of wrapping the whole nav tree, so icons, labels
    // and hit targets remain replayable across pill frames.
    let pill = div()
        .absolute()
        .left(pill_offset)
        .top(capsule_padding)
        .w(pill_width)
        .h(item_height)
        .rounded(px(17.))
        .bg(colors.accent)
        .with_layout_animation_target(nav_animating);

    let nav = div()
        .relative()
        .flex()
        .items_center()
        .gap(capsule_gap)
        .p(capsule_padding)
        .rounded(px(24.))
        .bg(colors.text_primary.opacity(0.045))
        .child(pill)
        .children(nav_items.iter().enumerate().map(|(index, item)| {
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
            let target = item.target.clone();
            div()
                .id((ElementId::from("main-nav"), item.id.clone()))
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
                    navigation::navigate_target(cx, (*target).clone());
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

    nav.into_any_element()
}

pub(super) fn render_controls(theme_target_dark: bool, colors: &ThemeColors) -> AnyElement {
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
        .child(
            icon_button(
                "theme-toggle-linux",
                if theme_target_dark {
                    lucide_gpui::icon!(sun)
                } else {
                    lucide_gpui::icon!(moon)
                },
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
                ThemeState::toggle_global(cx);
            }),
        )
        .child(
            icon_button("window-minimize-linux", lucide_gpui::icon!(minus))
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
            icon_button("window-close-linux", lucide_gpui::icon!(x))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(|_, window, _| {
                    window.remove_window();
                }),
        );

    controls.into_any_element()
}

pub(super) fn render_brand(
    colors: &ThemeColors,
    update_available: bool,
    update_modal_open: bool,
) -> AnyElement {
    let update_active = update_available && !update_modal_open;
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(9.))
        .child(
            img("icons/logo.png")
                .size(px(34.))
                .flex_shrink_0()
                .rounded(px(0.))
                .object_fit(ObjectFit::Contain),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .when(update_active, |element| {
                    element
                        .cursor_pointer()
                        .occlude()
                        .window_control_area(WindowControlArea::Client)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                            cx.update_global(|update: &mut UpdateState, _| {
                                update.request_open_modal(Instant::now());
                            });
                        })
                })
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
                        .child(APP_VERSION_LABEL),
                ),
        )
        .when(update_active, |element| {
            element.child(
                div()
                    .id("topbar-update-badge")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(crate::ui::theme::tokens::radius::FULL))
                    .bg(colors.accent.opacity(0.14))
                    .border_1()
                    .border_color(colors.accent.opacity(0.30))
                    .cursor_pointer()
                    .occlude()
                    .window_control_area(WindowControlArea::Client)
                    .hover(|style| style.bg(colors.accent.opacity(0.22)))
                    .active(|style| style.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                        cx.update_global(|update: &mut UpdateState, _| {
                            update.request_open_modal(Instant::now());
                        });
                    })
                    .child(div().size(px(6.)).rounded_full().bg(colors.accent))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.accent)
                            .child(t!("Topbar.update_available")),
                    ),
            )
        })
        .into_any_element()
}

pub(super) fn render_shell(
    colors: &ThemeColors,
    glass_effect_enabled: bool,
    brand: AnyElement,
    controls: AnyElement,
    nav: AnyElement,
    auth: AnyElement,
) -> AnyElement {
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
        .bg(colors
            .surface
            .opacity(if glass_effect_enabled { 0.78 } else { 1.0 }))
        .when(glass_effect_enabled, |element| {
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
                    cx.update_global(|state: &mut AppChromeState, _| {
                        state.titlebar_gesture.handle_mouse_up();
                    });
                })
        })
        .child(
            div()
                .size_full()
                .px(px(18.))
                .flex()
                .items_center()
                .justify_between()
                .child(brand)
                .child(controls),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(nav),
        );

    div()
        .absolute()
        .inset_0()
        .child(topbar)
        .child(auth)
        .into_any_element()
}
