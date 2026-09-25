use crate::ui::animation::{apple_spring, tab_underline_motion, SpringValue};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

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

const OPTIMISTIC_TAB_SYNC_TIMEOUT: Duration = Duration::from_millis(500);

fn tab_indicator_spring() -> Spring {
    // Short, heavily damped response: immediate motion on press, while SpringValue preserves
    // position + velocity when the target changes mid-flight.
    apple_spring(0.22, 0.90)
}

fn symmetric_sample_progress(offset: f32, max_offset: f32) -> f32 {
    if max_offset <= f32::EPSILON {
        0.5
    } else {
        0.5 + offset / (2.0 * max_offset)
    }
}

struct UnderlineTabsView {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    gap: Pixels,
    item_width: Option<Pixels>,
    defer_select_until_next_frame: bool,
    slot: SpringValue,
    active_index: usize,
    from_index: usize,
    sequence: u64,
    optimistic_target: Option<(usize, Instant)>,
    selection_generation: u64,
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
            slot: SpringValue::new(active_index as f32).with_spring(tab_indicator_spring()),
            active_index,
            from_index: active_index,
            sequence: 0,
            optimistic_target: None,
            selection_generation: 0,
        }
    }

    fn retarget(&mut self, target_index: usize, now: Instant, reduced_motion: bool) -> bool {
        if self.active_index == target_index {
            return false;
        }

        self.from_index = self.active_index;
        self.active_index = target_index;
        if reduced_motion {
            self.slot.snap_to(target_index as f32);
        } else {
            self.slot
                .retarget_with_spring(target_index as f32, tab_indicator_spring(), now);
        }
        self.sequence = self.sequence.wrapping_add(1);
        true
    }

    fn begin_user_selection(
        &mut self,
        target_index: usize,
        now: Instant,
        reduced_motion: bool,
    ) -> Option<u64> {
        if self.active_index == target_index {
            return None;
        }

        if self.defer_select_until_next_frame {
            self.optimistic_target = Some((target_index, now));
        }
        self.selection_generation = self.selection_generation.wrapping_add(1);
        self.retarget(target_index, now, reduced_motion);
        Some(self.selection_generation)
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
        let external_target = selected_tab_index(&items);
        let visual_changed = self.colors != colors
            || self.gap != gap
            || self.item_width != item_width
            || self.defer_select_until_next_frame != defer_select_until_next_frame
            || !tab_items_visually_equal(&self.items, &items);

        // A parent rerender can still arrive before the deferred selection callback. Do not let
        // that stale parent snapshot retarget the local indicator back to its previous tab.
        let target_changed = if let Some((pending_target, pending_since)) = self.optimistic_target {
            if external_target == pending_target {
                self.optimistic_target = None;
                false
            } else if now.saturating_duration_since(pending_since) >= OPTIMISTIC_TAB_SYNC_TIMEOUT {
                self.optimistic_target = None;
                self.retarget(external_target, now, reduced_motion)
            } else {
                false
            }
        } else {
            self.retarget(external_target, now, reduced_motion)
        };

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
        let slot_sample = self.slot.sample(now);

        let colors = self.colors;
        let reduced_motion = crate::core::ui_prefs::reduced_motion();
        let local_underline_animating = !slot_sample.done && !reduced_motion;
        let tabs_id = self.id.clone();
        let active_index = self.active_index;
        let from_index = self.from_index;
        let sequence = self.sequence;
        let item_width = self.item_width;
        let gap = self.gap;
        let defer_select_until_next_frame = self.defer_select_until_next_frame;

        let shared_underline = item_width.map(|item_width| {
            let item_width_px: f32 = item_width.into();
            let gap_px: f32 = gap.into();
            let step_px = item_width_px + gap_px;
            let max_slot = self.items.len().saturating_sub(1) as f32;
            let max_offset_px = step_px * max_slot;
            let target_left_px = step_px * active_index as f32 + 4.0;
            let visual_offset_px = step_px * (slot_sample.value - active_index as f32);
            let progress = symmetric_sample_progress(visual_offset_px, max_offset_px);

            div()
                .absolute()
                // Layout snaps to the newest target immediately. While the spring is moving, the
                // retained translation exactly cancels that snap back to the current visible slot.
                // Retargeting therefore changes only the spring force, never the visible position.
                .left(px(target_left_px))
                .bottom(px(0.))
                .w(px((item_width_px - 8.0).max(8.0)))
                .h(px(2.))
                .rounded(px(1.))
                .bg(colors.accent)
                .with_stable_sampled_animation(
                    SharedString::from(format!("{}-shared-underline-motion", tabs_id.as_ref())),
                    AnimationProperty::translation(
                        point(px(-max_offset_px), px(0.0)),
                        point(px(max_offset_px), px(0.0)),
                    ),
                    progress,
                    !slot_sample.done && !reduced_motion,
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

                        if local_underline_animating {
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
                                // Input transitions use real monotonic event time. animation_time()
                                // is intentionally frozen for a platform frame, so using it here
                                // would collapse multiple rapid retargets onto one timestamp.
                                let now = Instant::now();
                                let Some(generation) =
                                    this.begin_user_selection(index, now, reduced_motion)
                                else {
                                    cx.stop_propagation();
                                    return;
                                };

                                cx.notify();
                                window.request_animation_frame();
                                cx.stop_propagation();

                                if defer_select_until_next_frame {
                                    let deferred_select = on_select.clone();
                                    let view = cx.entity().downgrade();
                                    window.on_next_frame(move |window, cx| {
                                        let still_current = view.upgrade().is_some_and(|view| {
                                            let view = view.read(cx);
                                            view.selection_generation == generation
                                                && view.active_index == index
                                        });
                                        if still_current {
                                            (deferred_select)(window, cx);
                                        }
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
    slot: SpringValue,
    active_index: usize,
    optimistic_target: Option<(usize, Instant)>,
    selection_generation: u64,
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
            slot: SpringValue::new(active_index as f32).with_spring(tab_indicator_spring()),
            active_index,
            optimistic_target: None,
            selection_generation: 0,
        }
    }

    fn retarget(&mut self, target_index: usize, now: Instant, reduced_motion: bool) -> bool {
        if self.active_index == target_index {
            return false;
        }

        self.active_index = target_index;
        if reduced_motion {
            self.slot.snap_to(target_index as f32);
        } else {
            self.slot
                .retarget_with_spring(target_index as f32, tab_indicator_spring(), now);
        }
        true
    }

    fn begin_user_selection(
        &mut self,
        target_index: usize,
        now: Instant,
        reduced_motion: bool,
    ) -> Option<u64> {
        if self.active_index == target_index {
            return None;
        }

        if self.defer_select_until_next_frame {
            self.optimistic_target = Some((target_index, now));
        }
        self.selection_generation = self.selection_generation.wrapping_add(1);
        self.retarget(target_index, now, reduced_motion);
        Some(self.selection_generation)
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
        let external_target = selected_tab_index(&items);
        let visual_changed = self.colors != colors
            || self.height != height
            || self.item_width != item_width
            || self.indicator_shadow != indicator_shadow
            || self.defer_select_until_next_frame != defer_select_until_next_frame
            || !tab_items_visually_equal(&self.items, &items);

        let target_changed = if let Some((pending_target, pending_since)) = self.optimistic_target {
            if external_target == pending_target {
                self.optimistic_target = None;
                false
            } else if now.saturating_duration_since(pending_since) >= OPTIMISTIC_TAB_SYNC_TIMEOUT {
                self.optimistic_target = None;
                self.retarget(external_target, now, reduced_motion)
            } else {
                false
            }
        } else {
            self.retarget(external_target, now, reduced_motion)
        };

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
        let slot_sample = self.slot.sample(now);

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
        let active_index = self.active_index;
        let defer_select_until_next_frame = self.defer_select_until_next_frame;

        let max_slot = item_count.saturating_sub(1) as f32;
        let slot_offset = slot_sample.value - active_index as f32;
        let sampled_progress = symmetric_sample_progress(slot_offset, max_slot);
        let indicator_animating = !slot_sample.done && !crate::core::ui_prefs::reduced_motion();

        let indicator = if let Some(item_width) = item_width {
            let item_width_px: f32 = item_width.into();
            let max_offset_px = item_width_px * max_slot;
            div()
                .absolute()
                .left(px(item_width_px * active_index as f32 + 2.0))
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
                })
                .with_stable_sampled_animation(
                    SharedString::from(format!("{}-indicator-motion", self.id.as_ref())),
                    AnimationProperty::translation(
                        point(px(-max_offset_px), px(0.0)),
                        point(px(max_offset_px), px(0.0)),
                    ),
                    sampled_progress,
                    indicator_animating,
                )
                .into_any_element()
        } else {
            div()
                .absolute()
                .left(relative(active_index as f32 * segment_width))
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
                })
                .with_stable_sampled_animation(
                    SharedString::from(format!("{}-indicator-motion", self.id.as_ref())),
                    AnimationProperty::relative_translation(
                        point(-max_slot, 0.0),
                        point(max_slot, 0.0),
                    ),
                    sampled_progress,
                    indicator_animating,
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
                                let now = Instant::now();
                                let Some(generation) =
                                    this.begin_user_selection(index, now, reduced_motion)
                                else {
                                    cx.stop_propagation();
                                    return;
                                };

                                cx.notify();
                                window.request_animation_frame();
                                cx.stop_propagation();

                                if defer_select_until_next_frame {
                                    let deferred_select = on_select.clone();
                                    let view = cx.entity().downgrade();
                                    window.on_next_frame(move |window, cx| {
                                        let still_current = view.upgrade().is_some_and(|view| {
                                            let view = view.read(cx);
                                            view.selection_generation == generation
                                                && view.active_index == index
                                        });
                                        if still_current {
                                            (deferred_select)(window, cx);
                                        }
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


#[cfg(test)]
mod interruptible_tab_tests {
    use super::*;

    #[test]
    fn spring_retarget_is_position_continuous() {
        let t0 = Instant::now();
        let spring = tab_indicator_spring();
        let mut slot = SpringValue::new(0.0).with_spring(spring);

        slot.retarget_with_spring(5.0, spring, t0);
        let mid = t0 + Duration::from_millis(70);
        let before = slot.sample(mid);

        slot.retarget_with_spring(1.0, spring, mid);
        let after = slot.sample(mid);

        assert!((before.value - after.value).abs() < 1e-4);
    }

    #[test]
    fn target_relative_translation_keeps_retarget_frame_stationary() {
        let visible_slot = 2.35;
        let old_target = 5.0;
        let new_target = 1.0;
        let max_slot = 6.0;

        let old_offset = visible_slot - old_target;
        let old_progress = symmetric_sample_progress(old_offset, max_slot);
        let old_translation = -max_slot + old_progress * (2.0 * max_slot);

        let new_offset = visible_slot - new_target;
        let new_progress = symmetric_sample_progress(new_offset, max_slot);
        let new_translation = -max_slot + new_progress * (2.0 * max_slot);

        assert!(((old_target + old_translation) - visible_slot).abs() < 1e-5);
        assert!(((new_target + new_translation) - visible_slot).abs() < 1e-5);
    }

    #[test]
    fn rapid_retargets_keep_latest_target() {
        let t0 = Instant::now();
        let spring = tab_indicator_spring();
        let mut slot = SpringValue::new(0.0).with_spring(spring);

        slot.retarget_with_spring(4.0, spring, t0);
        slot.retarget_with_spring(2.0, spring, t0 + Duration::from_millis(4));
        slot.retarget_with_spring(6.0, spring, t0 + Duration::from_millis(9));

        assert!((slot.target() - 6.0).abs() < f32::EPSILON);
    }
}
