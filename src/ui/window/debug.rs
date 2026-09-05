use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use gpui::*;

pub mod devtools;
pub mod state;
pub mod view;

pub use state::DebugState;

/// Debug window shell. The existing DevTools view remains responsible for inspector, metrics and
/// console functionality; this shell owns window-wide visual diagnostics that must be applied to
/// the main GPUI window rather than to one inspected element.
pub struct DebugView {
    inner: Entity<view::DebugView>,
    _i18n_subscription: Subscription,
    flash_surface_updates: bool,
    show_layout_bounds: bool,
    show_element_updates: bool,
}

impl DebugView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inner = cx.new(|cx| view::DebugView::new(window, cx));
        let i18n_subscription = cx.observe_global::<I18n>(|_this, cx| cx.notify());
        cx.on_release(|_, cx| {
            apply_main_window_visual_debug(false, false, false, cx);
        })
        .detach();

        Self {
            inner,
            _i18n_subscription: i18n_subscription,
            flash_surface_updates: false,
            show_layout_bounds: false,
            show_element_updates: false,
        }
    }
}

impl Render for DebugView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let i18n = cx.global::<I18n>().clone();
        let copy = VisualDebugCopy::from_i18n(&i18n);

        let flash_enabled = self.flash_surface_updates;
        let layout_enabled = self.show_layout_bounds;
        let element_updates_enabled = self.show_element_updates;
        let flash_toggle = debug_toggle(
            "debug-flash-surface-updates",
            copy.flash_surface_updates,
            copy.flash_surface_updates_desc,
            flash_enabled,
            &colors,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.flash_surface_updates = !this.flash_surface_updates;
                apply_main_window_visual_debug(
                    this.flash_surface_updates,
                    this.show_layout_bounds,
                    this.show_element_updates,
                    cx,
                );
                cx.notify();
            }),
        );

        let layout_toggle = debug_toggle(
            "debug-show-layout-bounds",
            copy.show_layout_bounds,
            copy.show_layout_bounds_desc,
            layout_enabled,
            &colors,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.show_layout_bounds = !this.show_layout_bounds;
                apply_main_window_visual_debug(
                    this.flash_surface_updates,
                    this.show_layout_bounds,
                    this.show_element_updates,
                    cx,
                );
                cx.notify();
            }),
        );

        let element_updates_toggle = debug_toggle(
            "debug-show-element-updates",
            copy.show_element_updates,
            copy.show_element_updates_desc,
            element_updates_enabled,
            &colors,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.show_element_updates = !this.show_element_updates;
                apply_main_window_visual_debug(
                    this.flash_surface_updates,
                    this.show_layout_bounds,
                    this.show_element_updates,
                    cx,
                );
                cx.notify();
            }),
        );

        div()
            .size_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(colors.bg)
            .child(
                div().flex_none().px(px(14.)).pt(px(12.)).child(
                    div()
                        .w_full()
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .border_1()
                        .border_color(Hsla {
                            a: 0.22,
                            ..colors.border
                        })
                        .bg(Hsla {
                            a: 0.86,
                            ..colors.settings_card_bg
                        })
                        .p(px(10.))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .min_w(px(160.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(copy.title),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .line_height(px(14.))
                                        .text_color(colors.text_muted)
                                        .child(copy.subtitle),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(360.))
                                .flex()
                                .flex_wrap()
                                .gap(px(8.))
                                .child(flash_toggle)
                                .child(layout_toggle)
                                .child(element_updates_toggle),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(9.))
                                .line_height(px(13.))
                                .text_color(colors.text_muted)
                                .child(copy.legend),
                        ),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .child(self.inner.clone()),
            )
    }
}

fn apply_main_window_visual_debug(
    flash_surface_updates: bool,
    show_layout_bounds: bool,
    show_element_updates: bool,
    cx: &mut App,
) {
    let main_window_id = cx.read_global(|debug: &DebugState, _cx| debug.main_window_id);
    let Some(main_window) = devtools::find_window_by_id(main_window_id, cx) else {
        return;
    };

    let _ = main_window.update(cx, |_root, window, cx| {
        window.set_debug_visualization(
            WindowDebugVisualization {
                flash_surface_updates,
                show_layout_bounds,
                show_element_updates,
            },
            cx,
        );
    });
}

fn debug_toggle(
    id: &'static str,
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    enabled: bool,
    colors: &ThemeColors,
) -> Stateful<Div> {
    let mut track = div()
        .w(px(38.))
        .h(px(22.))
        .flex_none()
        .rounded(px(crate::ui::theme::tokens::radius::FULL))
        .p(px(3.))
        .flex()
        .items_center()
        .bg(if enabled {
            colors.accent
        } else {
            Hsla {
                a: 0.44,
                ..colors.settings_field_bg
            }
        });
    track = if enabled {
        track.justify_end()
    } else {
        track.justify_start()
    };
    track = track.child(
        div()
            .w(px(16.))
            .h(px(16.))
            .rounded(px(crate::ui::theme::tokens::radius::FULL))
            .bg(if enabled {
                colors.btn_primary_text
            } else {
                colors.text_muted
            }),
    );

    div()
        .id(id)
        .flex_1()
        .min_w(px(250.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: if enabled { 0.34 } else { 0.14 },
            ..if enabled {
                colors.accent
            } else {
                colors.border
            }
        })
        .bg(if enabled {
            Hsla {
                a: 0.10,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.42,
                ..colors.surface
            }
        })
        .px(px(10.))
        .py(px(8.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .cursor_pointer()
        .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(label.into()),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .line_height(px(13.))
                        .text_color(colors.text_muted)
                        .child(description.into()),
                ),
        )
        .child(track)
}

#[derive(Clone)]
struct VisualDebugCopy {
    title: SharedString,
    subtitle: SharedString,
    flash_surface_updates: SharedString,
    flash_surface_updates_desc: SharedString,
    show_layout_bounds: SharedString,
    show_layout_bounds_desc: SharedString,
    show_element_updates: SharedString,
    show_element_updates_desc: SharedString,
    legend: SharedString,
}

impl VisualDebugCopy {
    fn from_i18n(i18n: &I18n) -> Self {
        Self {
            title: t!("Debug.visual_title"),
            subtitle: t!("Debug.visual_subtitle"),
            flash_surface_updates: t!("Debug.flash_surface_updates"),
            flash_surface_updates_desc: t!("Debug.flash_surface_updates_desc"),
            show_layout_bounds: t!("Debug.show_layout_bounds"),
            show_layout_bounds_desc: t!("Debug.show_layout_bounds_desc"),
            show_element_updates: t!("Debug.show_element_updates"),
            show_element_updates_desc: t!("Debug.show_element_updates_desc"),
            legend: t!("Debug.visual_legend"),
        }
    }
}