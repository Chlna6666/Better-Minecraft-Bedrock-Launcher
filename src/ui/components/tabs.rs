use crate::ui::animation::{ease_out_cubic, raw_progress, request_animation_frame_if};
use crate::ui::theme::colors::ThemeColors;
use gpui::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

const ANIMATED_TAB_DURATION: Duration = Duration::from_millis(180);

#[derive(Clone)]
pub struct TabItem {
    id: SharedString,
    label: SharedString,
    icon_path: Option<&'static str>,
    active: bool,
    on_select: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl TabItem {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        on_select: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon_path: None,
            active,
            on_select: Rc::new(on_select),
        }
    }

    pub fn icon(mut self, icon_path: &'static str) -> Self {
        self.icon_path = Some(icon_path);
        self
    }
}

#[derive(IntoElement)]
pub struct UnderlineTabs {
    items: Vec<TabItem>,
    colors: ThemeColors,
    gap: Pixels,
}

impl UnderlineTabs {
    pub fn new(colors: &ThemeColors, items: Vec<TabItem>) -> Self {
        Self {
            items,
            colors: *colors,
            gap: px(14.),
        }
    }

    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = gap;
        self
    }
}

impl RenderOnce for UnderlineTabs {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.colors;

        div()
            .flex()
            .gap(self.gap)
            .children(self.items.into_iter().map(move |item| {
                let active = item.active;
                let label = item.label.clone();
                let icon_path = item.icon_path;
                let on_select = item.on_select.clone();
                let mut content = div().flex().items_center().gap(px(8.));

                if let Some(icon_path) = icon_path {
                    content =
                        content.child(svg().path(icon_path).w(px(15.)).h(px(15.)).text_color(
                            if active {
                                colors.accent
                            } else {
                                colors.text_secondary
                            },
                        ));
                }

                div()
                    .id(item.id.clone())
                    .px(px(4.))
                    .py(px(6.))
                    .border_b_2()
                    .border_color(if active {
                        colors.accent
                    } else {
                        hsla(0., 0., 0., 0.)
                    })
                    .cursor_pointer()
                    .child(
                        content.child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if active {
                                    colors.accent
                                } else {
                                    colors.text_secondary
                                })
                                .child(label),
                        ),
                    )
                    .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                        (on_select)(window, cx);
                    })
            }))
    }
}

#[derive(Clone, Copy, Debug)]
struct AnimatedTabsState {
    previous_index: usize,
    active_index: usize,
    started_at: Option<Instant>,
}

#[derive(IntoElement)]
pub struct AnimatedSegmentTabs {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    height: Pixels,
    item_width: Option<Pixels>,
}

impl AnimatedSegmentTabs {
    pub fn new(id: impl Into<SharedString>, colors: &ThemeColors, items: Vec<TabItem>) -> Self {
        Self {
            id: id.into(),
            items,
            colors: *colors,
            height: px(34.),
            item_width: None,
        }
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    pub fn item_width(mut self, item_width: Pixels) -> Self {
        self.item_width = Some(item_width);
        self
    }
}

impl RenderOnce for AnimatedSegmentTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }

        let colors = self.colors;
        let dark_mode = colors.bg.l < 0.5;
        let item_count = self.items.len();
        let selected_index = self.items.iter().position(|item| item.active).unwrap_or(0);
        let segment_width = 1.0 / item_count as f32;
        let item_width = self.item_width;
        let active_background = colors.settings_field_bg;
        let active_border = Hsla {
            a: if dark_mode { 0.52 } else { 0.28 },
            ..colors.accent
        };
        let track_background = if dark_mode {
            Hsla {
                a: 0.96,
                ..colors.surface
            }
        } else {
            Hsla {
                a: 1.0,
                ..colors.surface_hover
            }
        };
        let track_border = Hsla {
            a: if dark_mode { 0.26 } else { 0.16 },
            ..colors.accent
        };
        let active_text = colors.text_primary;
        let inactive_text = Hsla {
            a: if dark_mode { 0.78 } else { 0.84 },
            ..colors.text_secondary
        };

        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| AnimatedTabsState {
            previous_index: selected_index,
            active_index: selected_index,
            started_at: None,
        });

        if state.read(cx).active_index != selected_index {
            state.update(cx, |tab_state, _| {
                tab_state.previous_index = tab_state.active_index;
                tab_state.active_index = selected_index;
                tab_state.started_at = Some(Instant::now());
            });
        }

        let snapshot = *state.read(cx);
        let indicator_slot = if let Some(started_at) = snapshot.started_at {
            let progress = raw_progress(Instant::now(), started_at, ANIMATED_TAB_DURATION);
            let eased = ease_out_cubic(progress);

            if progress >= 1.0 {
                state.update(cx, |tab_state, _| {
                    tab_state.previous_index = tab_state.active_index;
                    tab_state.started_at = None;
                });
            } else {
                request_animation_frame_if(window, true);
            }

            snapshot.previous_index as f32
                + (snapshot.active_index as f32 - snapshot.previous_index as f32) * eased
        } else {
            snapshot.active_index as f32
        };

        let indicator = if let Some(item_width) = item_width {
            let item_width_px: f32 = item_width.into();
            div()
                .absolute()
                .top(px(2.))
                .bottom(px(2.))
                .left(px(item_width_px * indicator_slot + 2.0))
                .w(px(item_width_px - 4.0))
                .rounded(px(6.))
                .bg(active_background)
                .border_1()
                .border_color(active_border)
                .shadow(vec![BoxShadow {
                    color: Hsla {
                        a: if dark_mode { 0.18 } else { 0.10 },
                        ..colors.accent
                    },
                    blur_radius: px(10.0),
                    spread_radius: px(-4.0),
                    offset: point(px(0.), px(2.)),
                }])
                .into_any_element()
        } else {
            let indicator_left = relative(
                (indicator_slot * segment_width).clamp(0.0, (1.0 - segment_width).max(0.0)),
            );

            div()
                .absolute()
                .top(px(2.))
                .bottom(px(2.))
                .left(indicator_left)
                .w(relative(segment_width))
                .rounded(px(6.))
                .bg(active_background)
                .border_1()
                .border_color(active_border)
                .shadow(vec![BoxShadow {
                    color: Hsla {
                        a: if dark_mode { 0.18 } else { 0.10 },
                        ..colors.accent
                    },
                    blur_radius: px(10.0),
                    spread_radius: px(-4.0),
                    offset: point(px(0.), px(2.)),
                }])
                .into_any_element()
        };

        let mut root = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .h(self.height)
            .rounded(px(8.))
            .border_1()
            .border_color(track_border)
            .bg(track_background)
            .overflow_hidden();

        if let Some(item_width) = item_width {
            let item_width_px: f32 = item_width.into();
            root = root
                .w(px(item_width_px * item_count as f32 + 4.0))
                .px(px(2.));
        }

        root.child(indicator)
            .children(self.items.into_iter().map(move |item| {
                let active = item.active;
                let label = item.label.clone();
                let icon_path = item.icon_path;
                let on_select = item.on_select.clone();
                let mut content = div().flex().items_center().justify_center().gap(px(4.));

                if let Some(icon_path) = icon_path {
                    content = content.child(
                        svg()
                            .path(icon_path)
                            .w(px(12.))
                            .h(px(12.))
                            .text_color(if active { active_text } else { inactive_text }),
                    );
                }

                let mut tab = div()
                    .id(item.id.clone())
                    .relative()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(10.))
                    .cursor_pointer()
                    .child(
                        content.child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if active { active_text } else { inactive_text })
                                .child(label),
                        ),
                    );

                if let Some(item_width) = item_width {
                    tab = tab.w(item_width);
                } else {
                    tab = tab.flex_1().min_w(px(0.));
                }

                tab.on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                    (on_select)(window, cx);
                })
            }))
            .into_any_element()
    }
}
