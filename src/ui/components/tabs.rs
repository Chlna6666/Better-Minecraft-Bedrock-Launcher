use crate::ui::animation::{
    ease_out_cubic, ease_out_cubic_motion, raw_progress, tab_underline_motion,
};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

const ANIMATED_TAB_DURATION: Duration = Duration::from_millis(180);
const UNDERLINE_TAB_DURATION: Duration = Duration::from_millis(220);

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

#[derive(Clone, Copy, Debug)]
struct UnderlineTabsState {
    from_slot: f32,
    active_index: usize,
    started_at: Option<Instant>,
    sequence: u64,
}

fn sample_underline_tab_slot(state: UnderlineTabsState, now: Instant) -> (f32, bool) {
    let Some(started_at) = state.started_at else {
        return (state.active_index as f32, false);
    };
    let progress = raw_progress(now, started_at, UNDERLINE_TAB_DURATION);
    let eased = ease_out_cubic(progress);
    (
        state.from_slot + (state.active_index as f32 - state.from_slot) * eased,
        progress < 1.0,
    )
}

#[derive(IntoElement)]
pub struct UnderlineTabs {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    gap: Pixels,
    item_width: Option<Pixels>,
}

impl UnderlineTabs {
    pub fn new(
        id: impl Into<SharedString>,
        colors: &ThemeColors,
        items: Vec<TabItem>,
    ) -> Self {
        Self {
            id: id.into(),
            items,
            colors: *colors,
            gap: px(14.),
            item_width: None,
        }
    }

    pub fn gap(mut self, gap: Pixels) -> Self {
        self.gap = gap;
        self
    }

    /// Use equal-width tab slots so the underline can move as one continuous retained indicator.
    pub fn item_width(mut self, item_width: Pixels) -> Self {
        self.item_width = Some(item_width);
        self
    }
}

impl RenderOnce for UnderlineTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }

        let now = window.animation_time();
        let colors = self.colors;
        let selected_index = self.items.iter().position(|item| item.active).unwrap_or(0);
        let reduced_motion = crate::core::ui_prefs::reduced_motion();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| UnderlineTabsState {
            from_slot: selected_index as f32,
            active_index: selected_index,
            started_at: None,
            sequence: 0,
        });

        let mut snapshot = *state.read(cx);
        if snapshot.active_index != selected_index {
            let (current_slot, _) = sample_underline_tab_slot(snapshot, now);
            state.update(cx, |tab_state, _| {
                tab_state.from_slot = if reduced_motion {
                    selected_index as f32
                } else {
                    current_slot
                };
                tab_state.active_index = selected_index;
                tab_state.started_at = (!reduced_motion).then_some(now);
                tab_state.sequence = tab_state.sequence.wrapping_add(1);
            });
            snapshot = *state.read(cx);
        }

        let (_, animating) = sample_underline_tab_slot(snapshot, now);
        if snapshot.started_at.is_some() && !animating {
            state.update(cx, |tab_state, _| {
                tab_state.from_slot = tab_state.active_index as f32;
                tab_state.started_at = None;
            });
            snapshot = *state.read(cx);
        }

        let tabs_id = self.id.clone();
        let item_width = self.item_width;
        let gap = self.gap;
        let shared_underline = item_width.map(|item_width| {
            let item_width_px: f32 = item_width.into();
            let gap_px: f32 = gap.into();
            let step_px = item_width_px + gap_px;
            let from_left_px = step_px * snapshot.from_slot + 4.0;
            let target_left_px = step_px * snapshot.active_index as f32 + 4.0;
            let indicator = div()
                .absolute()
                .bottom(px(0.))
                .w(px((item_width_px - 8.0).max(8.0)))
                .h(px(2.))
                .rounded(px(1.))
                .bg(colors.accent);

            if snapshot.started_at.is_some() && !reduced_motion {
                indicator
                    .with_animation(
                        SharedString::from(format!(
                            "{}-shared-underline-{}",
                            tabs_id.as_ref(),
                            snapshot.sequence
                        )),
                        ease_out_cubic_motion(UNDERLINE_TAB_DURATION),
                        move |indicator, progress| {
                            let progress = progress.clamp(0.0, 1.0);
                            let left =
                                from_left_px + (target_left_px - from_left_px) * progress;
                            indicator.left(px(left))
                        },
                    )
                    .into_any_element()
            } else {
                indicator.left(px(target_left_px)).into_any_element()
            }
        });

        let mut root = div()
            .id(self.id)
            .relative()
            .min_w(px(0.))
            .overflow_x_scrollbar()
            .scrollbar_width(px(0.))
            .flex()
            .gap(gap);

        if let Some(shared_underline) = shared_underline {
            root = root.child(shared_underline);
        }

        root.children(self.items.into_iter().map(move |item| {
            let active = item.active;
            let label = item.label.clone();
            let icon_path = item.icon_path;
            let on_select = item.on_select.clone();
            let mut content = div().flex().items_center().gap(px(8.));

            if let Some(icon_path) = icon_path {
                content = content.child(
                    svg()
                        .path(icon_path)
                        .w(px(15.))
                        .h(px(15.))
                        .text_color(if active {
                            colors.accent
                        } else {
                            colors.text_secondary
                        }),
                );
            }

            let local_underline = (item_width.is_none() && active).then(|| {
                let indicator = div()
                    .absolute()
                    .left(px(2.))
                    .right(px(2.))
                    .bottom(px(0.))
                    .h(px(2.))
                    .rounded(px(1.))
                    .bg(colors.accent);

                if snapshot.started_at.is_some() && !reduced_motion {
                    let from_index = snapshot.from_slot.round().max(0.0) as usize;
                    indicator
                        .composite_layer()
                        .with_animation(
                            SharedString::from(format!(
                                "{}-underline-{}",
                                tabs_id.as_ref(),
                                snapshot.sequence
                            )),
                            tab_underline_motion(from_index, snapshot.active_index),
                            |indicator, _progress| indicator,
                        )
                        .into_any_element()
                } else {
                    indicator.into_any_element()
                }
            });

            div()
                .id(item.id.clone())
                .relative()
                .flex_shrink_0()
                .px(px(4.))
                .py(px(6.))
                .border_b_2()
                .border_color(hsla(0., 0., 0., 0.))
                .cursor_pointer()
                .when_some(item_width, |tab, width| {
                    tab.w(width)
                        .px(px(2.))
                        .flex()
                        .items_center()
                        .justify_center()
                })
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
                .when_some(local_underline, |tab, underline| tab.child(underline))
                .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
                    (on_select)(window, cx);
                })
        }))
        .into_any_element()
    }
}

#[derive(Clone, Copy, Debug)]
struct AnimatedTabsState {
    from_slot: f32,
    active_index: usize,
    started_at: Option<Instant>,
    sequence: u64,
}

fn sample_animated_tab_slot(state: AnimatedTabsState, now: Instant) -> (f32, bool) {
    let Some(started_at) = state.started_at else {
        return (state.active_index as f32, false);
    };
    let progress = raw_progress(now, started_at, ANIMATED_TAB_DURATION);
    let eased = ease_out_cubic(progress);
    (
        state.from_slot + (state.active_index as f32 - state.from_slot) * eased,
        progress < 1.0,
    )
}

#[derive(IntoElement)]
pub struct AnimatedSegmentTabs {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    height: Pixels,
    item_width: Option<Pixels>,
    indicator_shadow: bool,
}

impl AnimatedSegmentTabs {
    pub fn new(id: impl Into<SharedString>, colors: &ThemeColors, items: Vec<TabItem>) -> Self {
        Self {
            id: id.into(),
            items,
            colors: *colors,
            height: px(34.),
            item_width: None,
            indicator_shadow: true,
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

    pub fn without_indicator_shadow(mut self) -> Self {
        self.indicator_shadow = false;
        self
    }
}

impl RenderOnce for AnimatedSegmentTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }

        let now = window.animation_time();
        let colors = self.colors;
        let dark_mode = colors.bg.l < 0.5;
        let item_count = self.items.len();
        let selected_index = self.items.iter().position(|item| item.active).unwrap_or(0);
        let segment_width = 1.0 / item_count as f32;
        let item_width = self.item_width;
        let indicator_shadow = self.indicator_shadow;
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

        let reduced_motion = crate::core::ui_prefs::reduced_motion();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| AnimatedTabsState {
            from_slot: selected_index as f32,
            active_index: selected_index,
            started_at: None,
            sequence: 0,
        });

        let mut snapshot = *state.read(cx);
        if snapshot.active_index != selected_index {
            let (current_slot, _) = sample_animated_tab_slot(snapshot, now);
            state.update(cx, |tab_state, _| {
                tab_state.from_slot = if reduced_motion {
                    selected_index as f32
                } else {
                    current_slot
                };
                tab_state.active_index = selected_index;
                tab_state.started_at = (!reduced_motion).then_some(now);
                tab_state.sequence = tab_state.sequence.wrapping_add(1);
            });
            snapshot = *state.read(cx);
        }

        let (_, animating) = sample_animated_tab_slot(snapshot, now);
        if snapshot.started_at.is_some() && !animating {
            state.update(cx, |tab_state, _| {
                tab_state.from_slot = tab_state.active_index as f32;
                tab_state.started_at = None;
            });
            snapshot = *state.read(cx);
        }

        let indicator = if let Some(item_width) = item_width {
            let item_width_px: f32 = item_width.into();
            let from_left_px = item_width_px * snapshot.from_slot + 2.0;
            let target_left_px = item_width_px * snapshot.active_index as f32 + 2.0;
            let indicator = div()
                .absolute()
                .top(px(2.))
                .bottom(px(2.))
                .w(px(item_width_px - 4.0))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .bg(active_background)
                .border_1()
                .border_color(active_border)
                .when(indicator_shadow, |indicator| {
                    indicator.shadow(vec![BoxShadow {
                        color: Hsla {
                            a: if dark_mode { 0.18 } else { 0.10 },
                            ..colors.accent
                        },
                        blur_radius: px(10.0),
                        spread_radius: px(-4.0),
                        offset: point(px(0.), px(2.)),
                    }])
                });

            if snapshot.started_at.is_some() && !reduced_motion {
                indicator
                    .with_animation(
                        SharedString::from(format!(
                            "{}-indicator-{}",
                            self.id, snapshot.sequence
                        )),
                        ease_out_cubic_motion(ANIMATED_TAB_DURATION),
                        move |indicator, progress| {
                            let progress = progress.clamp(0.0, 1.0);
                            let left =
                                from_left_px + (target_left_px - from_left_px) * progress;
                            indicator.left(px(left))
                        },
                    )
                    .into_any_element()
            } else {
                indicator.left(px(target_left_px)).into_any_element()
            }
        } else {
            let from_left = snapshot.from_slot * segment_width;
            let target_left = snapshot.active_index as f32 * segment_width;
            let indicator = div()
                .absolute()
                .top(px(2.))
                .bottom(px(2.))
                .w(relative(segment_width))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .bg(active_background)
                .border_1()
                .border_color(active_border)
                .when(indicator_shadow, |indicator| {
                    indicator.shadow(vec![BoxShadow {
                        color: Hsla {
                            a: if dark_mode { 0.18 } else { 0.10 },
                            ..colors.accent
                        },
                        blur_radius: px(10.0),
                        spread_radius: px(-4.0),
                        offset: point(px(0.), px(2.)),
                    }])
                });

            if snapshot.started_at.is_some() && !reduced_motion {
                indicator
                    .with_animation(
                        SharedString::from(format!(
                            "{}-indicator-{}",
                            self.id, snapshot.sequence
                        )),
                        ease_out_cubic_motion(ANIMATED_TAB_DURATION),
                        move |indicator, progress| {
                            let progress = progress.clamp(0.0, 1.0);
                            let left = from_left + (target_left - from_left) * progress;
                            indicator.left(relative(left))
                        },
                    )
                    .into_any_element()
            } else {
                indicator.left(relative(target_left)).into_any_element()
            }
        };

        let mut root = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .h(self.height)
            .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                                .min_w(px(0.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if active { active_text } else { inactive_text })
                                .child(label),
                        ),
                    );

                if let Some(item_width) = item_width {
                    tab = tab.w(item_width).overflow_hidden();
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
