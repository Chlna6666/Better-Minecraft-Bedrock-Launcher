use crate::ui::animation::{
    ease_out_cubic, ease_out_cubic_motion, raw_progress, settled_animation, tab_underline_motion,
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


fn tab_items_visually_equal(left: &[TabItem], right: &[TabItem]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.id == right.id
                && left.label == right.label
                && left.icon_path == right.icon_path
                && left.active == right.active
        })
}

fn selected_tab_index(items: &[TabItem]) -> usize {
    items.iter().position(|item| item.active).unwrap_or(0)
}

#[derive(IntoElement)]
pub struct UnderlineTabs {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    gap: Pixels,
    item_width: Option<Pixels>,
    defer_select_until_next_frame: bool,
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
            defer_select_until_next_frame: false,
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

    pub fn defer_selection_until_next_frame(mut self) -> Self {
        self.defer_select_until_next_frame = true;
        self
    }
}

struct UnderlineTabsView {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    gap: Pixels,
    item_width: Option<Pixels>,
    defer_select_until_next_frame: bool,
    from_slot: f32,
    active_index: usize,
    started_at: Option<Instant>,
    sequence: u64,
}

impl UnderlineTabsView {
    fn new(
        id: SharedString,
        items: Vec<TabItem>,
        colors: ThemeColors,
        gap: Pixels,
        item_width: Option<Pixels>,
        defer_select_until_next_frame: bool,
    ) -> Self {
        let active_index = selected_tab_index(&items);
        Self {
            id,
            items,
            colors,
            gap,
            item_width,
            defer_select_until_next_frame,
            from_slot: active_index as f32,
            active_index,
            started_at: None,
            sequence: 0,
        }
    }

    fn sample_slot(&self, now: Instant) -> (f32, bool) {
        let Some(started_at) = self.started_at else {
            return (self.active_index as f32, false);
        };
        let progress = raw_progress(now, started_at, UNDERLINE_TAB_DURATION);
        let eased = ease_out_cubic(progress);
        (
            self.from_slot + (self.active_index as f32 - self.from_slot) * eased,
            progress < 1.0,
        )
    }

    fn retarget(&mut self, target_index: usize, now: Instant, reduced_motion: bool) -> bool {
        if self.active_index == target_index {
            return false;
        }

        let current_slot = self.sample_slot(now).0;
        self.from_slot = if reduced_motion {
            target_index as f32
        } else {
            current_slot
        };
        self.active_index = target_index;
        self.started_at = (!reduced_motion).then_some(now);
        self.sequence = self.sequence.wrapping_add(1);
        true
    }

    fn sync(
        &mut self,
        items: Vec<TabItem>,
        colors: ThemeColors,
        gap: Pixels,
        item_width: Option<Pixels>,
        defer_select_until_next_frame: bool,
        now: Instant,
        reduced_motion: bool,
        cx: &mut Context<Self>,
    ) {
        let target_index = selected_tab_index(&items);
        let visual_changed = self.colors != colors
            || self.gap != gap
            || self.item_width != item_width
            || self.defer_select_until_next_frame != defer_select_until_next_frame
            || !tab_items_visually_equal(&self.items, &items);
        let target_changed = self.retarget(target_index, now, reduced_motion);

        self.items = items;
        self.colors = colors;
        self.gap = gap;
        self.item_width = item_width;
        self.defer_select_until_next_frame = defer_select_until_next_frame;

        if visual_changed || target_changed {
            cx.notify();
        }
    }
}

impl Render for UnderlineTabsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }

        let now = window.animation_time();
        let colors = self.colors;
        let reduced_motion = crate::core::ui_prefs::reduced_motion();
        let (_, animating) = self.sample_slot(now);
        let animation_active = animating && !reduced_motion;
        let tabs_id = self.id.clone();
        let active_index = self.active_index;
        let from_slot = self.from_slot;
        let sequence = self.sequence;
        let item_width = self.item_width;
        let gap = self.gap;
        let defer_select_until_next_frame = self.defer_select_until_next_frame;

        let shared_underline = item_width.map(|item_width| {
            let item_width_px: f32 = item_width.into();
            let gap_px: f32 = gap.into();
            let step_px = item_width_px + gap_px;
            let from_left_px = step_px * from_slot + 4.0;
            let target_left_px = step_px * active_index as f32 + 4.0;
            let indicator = div()
                .absolute()
                .bottom(px(0.))
                .w(px((item_width_px - 8.0).max(8.0)))
                .h(px(2.))
                .rounded(px(1.))
                .bg(colors.accent);

            indicator
                .with_animation(
                    SharedString::from(format!(
                        "{}-shared-underline-{}",
                        tabs_id.as_ref(),
                        sequence
                    )),
                    if animation_active {
                        ease_out_cubic_motion(UNDERLINE_TAB_DURATION)
                    } else {
                        settled_animation()
                    },
                    move |indicator, progress| {
                        let progress = if animation_active {
                            progress.clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        let left = from_left_px + (target_left_px - from_left_px) * progress;
                        indicator.left(px(left))
                    },
                )
                .into_any_element()
        });

        let mut root = div()
            .id(self.id.clone())
            .relative()
            .min_w(px(0.))
            .overflow_x_scrollbar()
            .scrollbar_width(px(0.))
            .flex()
            .gap(gap);

        if let Some(shared_underline) = shared_underline {
            root = root.child(shared_underline);
        }

        root.children(
            self.items
                .clone()
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    let active = index == active_index;
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

                        if animation_active {
                            let from_index = from_slot.round().max(0.0) as usize;
                            indicator
                                .composite_layer()
                                .with_animation(
                                    SharedString::from(format!(
                                        "{}-underline-{}",
                                        tabs_id.as_ref(),
                                        sequence
                                    )),
                                    tab_underline_motion(from_index, active_index),
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
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                let reduced_motion = crate::core::ui_prefs::reduced_motion();
                                if this.retarget(
                                    index,
                                    window.animation_time(),
                                    reduced_motion,
                                ) {
                                    cx.notify();
                                }
                                cx.stop_propagation();
                                if defer_select_until_next_frame {
                                    let deferred_select = on_select.clone();
                                    window.on_next_frame(move |window, cx| {
                                        (deferred_select)(window, cx);
                                    });
                                } else {
                                    (on_select)(window, cx);
                                }
                            }),
                        )
                }),
        )
        .into_any_element()
    }
}

impl RenderOnce for UnderlineTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }

        let state_key = ElementId::Name(
            format!("{}-detached-tabs-view", self.id.as_ref()).into(),
        );
        let id = self.id;
        let items = self.items;
        let colors = self.colors;
        let gap = self.gap;
        let item_width = self.item_width;
        let defer_select_until_next_frame = self.defer_select_until_next_frame;
        let now = window.animation_time();
        let reduced_motion = crate::core::ui_prefs::reduced_motion();

        window
            .with_global_id(state_key, |global_id, window| {
                window.with_element_state::<Entity<UnderlineTabsView>, _>(
                    global_id,
                    |view, _window| {
                        let view = if let Some(view) = view {
                            view.update(cx, |view, cx| {
                                view.sync(
                                    items,
                                    colors,
                                    gap,
                                    item_width,
                                    defer_select_until_next_frame,
                                    now,
                                    reduced_motion,
                                    cx,
                                );
                            });
                            view
                        } else {
                            cx.new(|_| {
                                UnderlineTabsView::new(
                                    id,
                                    items,
                                    colors,
                                    gap,
                                    item_width,
                                    defer_select_until_next_frame,
                                )
                            })
                        };
                        (view.clone(), view)
                    },
                )
            })
            .into_any_element()
    }
}

#[derive(IntoElement)]
pub struct AnimatedSegmentTabs {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    height: Pixels,
    item_width: Option<Pixels>,
    indicator_shadow: bool,
    defer_select_until_next_frame: bool,
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
            defer_select_until_next_frame: false,
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

    pub fn defer_selection_until_next_frame(mut self) -> Self {
        self.defer_select_until_next_frame = true;
        self
    }
}

struct AnimatedSegmentTabsView {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    height: Pixels,
    item_width: Option<Pixels>,
    indicator_shadow: bool,
    defer_select_until_next_frame: bool,
    from_slot: f32,
    active_index: usize,
    started_at: Option<Instant>,
    sequence: u64,
}

impl AnimatedSegmentTabsView {
    fn new(
        id: SharedString,
        items: Vec<TabItem>,
        colors: ThemeColors,
        height: Pixels,
        item_width: Option<Pixels>,
        indicator_shadow: bool,
        defer_select_until_next_frame: bool,
    ) -> Self {
        let active_index = selected_tab_index(&items);
        Self {
            id,
            items,
            colors,
            height,
            item_width,
            indicator_shadow,
            defer_select_until_next_frame,
            from_slot: active_index as f32,
            active_index,
            started_at: None,
            sequence: 0,
        }
    }

    fn sample_slot(&self, now: Instant) -> (f32, bool) {
        let Some(started_at) = self.started_at else {
            return (self.active_index as f32, false);
        };
        let progress = raw_progress(now, started_at, ANIMATED_TAB_DURATION);
        let eased = ease_out_cubic(progress);
        (
            self.from_slot + (self.active_index as f32 - self.from_slot) * eased,
            progress < 1.0,
        )
    }

    fn retarget(&mut self, target_index: usize, now: Instant, reduced_motion: bool) -> bool {
        if self.active_index == target_index {
            return false;
        }

        let current_slot = self.sample_slot(now).0;
        self.from_slot = if reduced_motion {
            target_index as f32
        } else {
            current_slot
        };
        self.active_index = target_index;
        self.started_at = (!reduced_motion).then_some(now);
        self.sequence = self.sequence.wrapping_add(1);
        true
    }

    fn sync(
        &mut self,
        items: Vec<TabItem>,
        colors: ThemeColors,
        height: Pixels,
        item_width: Option<Pixels>,
        indicator_shadow: bool,
        defer_select_until_next_frame: bool,
        now: Instant,
        reduced_motion: bool,
        cx: &mut Context<Self>,
    ) {
        let target_index = selected_tab_index(&items);
        let visual_changed = self.colors != colors
            || self.height != height
            || self.item_width != item_width
            || self.indicator_shadow != indicator_shadow
            || self.defer_select_until_next_frame != defer_select_until_next_frame
            || !tab_items_visually_equal(&self.items, &items);
        let target_changed = self.retarget(target_index, now, reduced_motion);

        self.items = items;
        self.colors = colors;
        self.height = height;
        self.item_width = item_width;
        self.indicator_shadow = indicator_shadow;
        self.defer_select_until_next_frame = defer_select_until_next_frame;

        if visual_changed || target_changed {
            cx.notify();
        }
    }
}

impl Render for AnimatedSegmentTabsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }

        let now = window.animation_time();
        let colors = self.colors;
        let dark_mode = colors.bg.l < 0.5;
        let item_count = self.items.len();
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
        let (_, animating) = self.sample_slot(now);
        let animation_active = animating && !reduced_motion;
        let active_index = self.active_index;
        let from_slot = self.from_slot;
        let sequence = self.sequence;
        let defer_select_until_next_frame = self.defer_select_until_next_frame;

        let indicator = if let Some(item_width) = item_width {
            let item_width_px: f32 = item_width.into();
            let from_left_px = item_width_px * from_slot + 2.0;
            let target_left_px = item_width_px * active_index as f32 + 2.0;
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

            indicator
                .with_animation(
                    SharedString::from(format!(
                        "{}-indicator-{}",
                        self.id, sequence
                    )),
                    if animation_active {
                        ease_out_cubic_motion(ANIMATED_TAB_DURATION)
                    } else {
                        settled_animation()
                    },
                    move |indicator, progress| {
                        let progress = if animation_active {
                            progress.clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        let left = from_left_px + (target_left_px - from_left_px) * progress;
                        indicator.left(px(left))
                    },
                )
                .into_any_element()
        } else {
            let from_left = from_slot * segment_width;
            let target_left = active_index as f32 * segment_width;
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

            indicator
                .with_animation(
                    SharedString::from(format!(
                        "{}-indicator-{}",
                        self.id, sequence
                    )),
                    if animation_active {
                        ease_out_cubic_motion(ANIMATED_TAB_DURATION)
                    } else {
                        settled_animation()
                    },
                    move |indicator, progress| {
                        let progress = if animation_active {
                            progress.clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        let left = from_left + (target_left - from_left) * progress;
                        indicator.left(relative(left))
                    },
                )
                .into_any_element()
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
            .children(
                self.items
                    .clone()
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let active = index == active_index;
                        let label = item.label.clone();
                        let icon_path = item.icon_path;
                        let on_select = item.on_select.clone();
                        let mut content =
                            div().flex().items_center().justify_center().gap(px(4.));

                        if let Some(icon_path) = icon_path {
                            content = content.child(
                                svg()
                                    .path(icon_path)
                                    .w(px(12.))
                                    .h(px(12.))
                                    .text_color(if active {
                                        active_text
                                    } else {
                                        inactive_text
                                    }),
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
                                        .text_color(if active {
                                            active_text
                                        } else {
                                            inactive_text
                                        })
                                        .child(label),
                                ),
                            );

                        if let Some(item_width) = item_width {
                            tab = tab.w(item_width).overflow_hidden();
                        } else {
                            tab = tab.flex_1().min_w(px(0.));
                        }

                        tab.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                let reduced_motion =
                                    crate::core::ui_prefs::reduced_motion();
                                if this.retarget(
                                    index,
                                    window.animation_time(),
                                    reduced_motion,
                                ) {
                                    cx.notify();
                                }
                                cx.stop_propagation();
                                if defer_select_until_next_frame {
                                    let deferred_select = on_select.clone();
                                    window.on_next_frame(move |window, cx| {
                                        (deferred_select)(window, cx);
                                    });
                                } else {
                                    (on_select)(window, cx);
                                }
                            }),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl RenderOnce for AnimatedSegmentTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }

        let state_key = ElementId::Name(
            format!("{}-detached-tabs-view", self.id.as_ref()).into(),
        );
        let id = self.id;
        let items = self.items;
        let colors = self.colors;
        let height = self.height;
        let item_width = self.item_width;
        let indicator_shadow = self.indicator_shadow;
        let defer_select_until_next_frame = self.defer_select_until_next_frame;
        let now = window.animation_time();
        let reduced_motion = crate::core::ui_prefs::reduced_motion();

        window
            .with_global_id(state_key, |global_id, window| {
                window.with_element_state::<Entity<AnimatedSegmentTabsView>, _>(
                    global_id,
                    |view, _window| {
                        let view = if let Some(view) = view {
                            view.update(cx, |view, cx| {
                                view.sync(
                                    items,
                                    colors,
                                    height,
                                    item_width,
                                    indicator_shadow,
                                    defer_select_until_next_frame,
                                    now,
                                    reduced_motion,
                                    cx,
                                );
                            });
                            view
                        } else {
                            cx.new(|_| {
                                AnimatedSegmentTabsView::new(
                                    id,
                                    items,
                                    colors,
                                    height,
                                    item_width,
                                    indicator_shadow,
                                    defer_select_until_next_frame,
                                )
                            })
                        };
                        (view.clone(), view)
                    },
                )
            })
            .into_any_element()
    }
}
