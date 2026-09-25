use crate::ui::animation::{apple_spring, request_layout_animation_frame_if, SpringValue};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;
use std::time::Instant;

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

fn tab_indicator_spring() -> Spring {
    // Tab selection should react immediately. A critically-near-damped spring gives the Apple-like
    // continuity without allowing old momentum to dominate a newly selected target.
    apple_spring(0.18, 0.94)
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

    pub fn item_width(mut self, item_width: Pixels) -> Self {
        self.item_width = Some(item_width);
        self
    }
}

struct UnderlineTabsView {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    gap: Pixels,
    item_width: Option<Pixels>,
    slot: SpringValue,
    active_index: usize,
}

impl UnderlineTabsView {
    fn new(
        id: SharedString,
        items: Vec<TabItem>,
        colors: ThemeColors,
        gap: Pixels,
        item_width: Option<Pixels>,
    ) -> Self {
        let active_index = selected_tab_index(&items);
        Self {
            id,
            items,
            colors,
            gap,
            item_width,
            slot: SpringValue::new(active_index as f32).with_spring(tab_indicator_spring()),
            active_index,
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

    fn sync(
        &mut self,
        items: Vec<TabItem>,
        colors: ThemeColors,
        gap: Pixels,
        item_width: Option<Pixels>,
        now: Instant,
        reduced_motion: bool,
        cx: &mut Context<Self>,
    ) {
        let target_index = selected_tab_index(&items);
        let visual_changed = self.colors != colors
            || self.gap != gap
            || self.item_width != item_width
            || !tab_items_visually_equal(&self.items, &items);
        let target_changed = self.retarget(target_index, now, reduced_motion);

        self.items = items;
        self.colors = colors;
        self.gap = gap;
        self.item_width = item_width;

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
        let reduced_motion = crate::core::ui_prefs::reduced_motion();
        let slot_sample = if reduced_motion {
            self.slot.snap_to(self.active_index as f32);
            self.slot.sample(now)
        } else {
            self.slot.sample(now)
        };
        request_layout_animation_frame_if(window, !slot_sample.done && !reduced_motion);

        let colors = self.colors;
        let active_index = self.active_index;
        let item_width = self.item_width;
        let gap = self.gap;

        let shared_underline = item_width.map(|item_width| {
            let item_width_px: f32 = item_width.into();
            let gap_px: f32 = gap.into();
            let step_px = item_width_px + gap_px;
            div()
                .absolute()
                .left(px(step_px * slot_sample.value + 4.0))
                .bottom(px(0.))
                .w(px((item_width_px - 8.0).max(8.0)))
                .h(px(2.))
                .rounded(px(1.))
                .bg(colors.accent)
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
                        div()
                            .absolute()
                            .left(px(2.))
                            .right(px(2.))
                            .bottom(px(0.))
                            .h(px(2.))
                            .rounded(px(1.))
                            .bg(colors.accent)
                            .into_any_element()
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
                                if this.retarget(index, Instant::now(), reduced_motion) {
                                    cx.notify();
                                }

                                // One authoritative selection update. Do not defer it and do not keep
                                // an optimistic second tab state: the tab highlight, content and
                                // ManagePageState must commit from the same input event.
                                cx.stop_propagation();
                                (on_select)(window, cx);
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

        let state_key =
            ElementId::Name(format!("{}-detached-tabs-view", self.id.as_ref()).into());
        let id = self.id;
        let items = self.items;
        let colors = self.colors;
        let gap = self.gap;
        let item_width = self.item_width;
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
                                    now,
                                    reduced_motion,
                                    cx,
                                );
                            });
                            view
                        } else {
                            cx.new(|_| {
                                UnderlineTabsView::new(id, items, colors, gap, item_width)
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

struct AnimatedSegmentTabsView {
    id: SharedString,
    items: Vec<TabItem>,
    colors: ThemeColors,
    height: Pixels,
    item_width: Option<Pixels>,
    indicator_shadow: bool,
    slot: SpringValue,
    active_index: usize,
}

impl AnimatedSegmentTabsView {
    fn new(
        id: SharedString,
        items: Vec<TabItem>,
        colors: ThemeColors,
        height: Pixels,
        item_width: Option<Pixels>,
        indicator_shadow: bool,
    ) -> Self {
        let active_index = selected_tab_index(&items);
        Self {
            id,
            items,
            colors,
            height,
            item_width,
            indicator_shadow,
            slot: SpringValue::new(active_index as f32).with_spring(tab_indicator_spring()),
            active_index,
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

    fn sync(
        &mut self,
        items: Vec<TabItem>,
        colors: ThemeColors,
        height: Pixels,
        item_width: Option<Pixels>,
        indicator_shadow: bool,
        now: Instant,
        reduced_motion: bool,
        cx: &mut Context<Self>,
    ) {
        let target_index = selected_tab_index(&items);
        let visual_changed = self.colors != colors
            || self.height != height
            || self.item_width != item_width
            || self.indicator_shadow != indicator_shadow
            || !tab_items_visually_equal(&self.items, &items);
        let target_changed = self.retarget(target_index, now, reduced_motion);

        self.items = items;
        self.colors = colors;
        self.height = height;
        self.item_width = item_width;
        self.indicator_shadow = indicator_shadow;

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
        let reduced_motion = crate::core::ui_prefs::reduced_motion();
        let slot_sample = if reduced_motion {
            self.slot.snap_to(self.active_index as f32);
            self.slot.sample(now)
        } else {
            self.slot.sample(now)
        };
        request_layout_animation_frame_if(window, !slot_sample.done && !reduced_motion);

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

        let indicator = if let Some(item_width) = item_width {
            let item_width_px: f32 = item_width.into();
            div()
                .absolute()
                .left(px(item_width_px * slot_sample.value + 2.0))
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
                .into_any_element()
        } else {
            div()
                .absolute()
                .left(relative(slot_sample.value * segment_width))
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
                                if this.retarget(index, Instant::now(), reduced_motion) {
                                    cx.notify();
                                }

                                cx.stop_propagation();
                                (on_select)(window, cx);
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

        let state_key =
            ElementId::Name(format!("{}-detached-tabs-view", self.id.as_ref()).into());
        let id = self.id;
        let items = self.items;
        let colors = self.colors;
        let height = self.height;
        let item_width = self.item_width;
        let indicator_shadow = self.indicator_shadow;
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
