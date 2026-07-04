use crate::ui::animation::{ease_out_cubic, request_animation_frame_if};
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;
use std::time::{Duration, Instant};

const OPEN_DURATION_SECS: f32 = 0.22;
const CLOSE_DURATION_SECS: f32 = 0.16;

const TRIGGER_HEIGHT: f32 = 40.0;
const MENU_MAX_HEIGHT: f32 = 240.0;
const MENU_ROW_HEIGHT: f32 = 32.0;
const MENU_VERTICAL_PADDING: f32 = 10.0;
const MENU_GAP: f32 = 6.0;
const MENU_TRANSLATE_PX: f32 = 6.0;
const MENU_WINDOW_EDGE_PADDING: f32 = 10.0;
const MENU_MIN_PREVIEW_ROWS: f32 = 3.0;
const MENU_CONTENT_PADDING_X: f32 = 44.0;
const MENU_CHECK_ICON_ALLOWANCE: f32 = 28.0;
const MENU_MIN_WIDTH: f32 = 120.0;

#[derive(Clone)]
pub struct DropdownOption {
    pub label: SharedString,
}

impl From<&'static str> for DropdownOption {
    fn from(value: &'static str) -> Self {
        Self {
            label: SharedString::from(value),
        }
    }
}

impl From<SharedString> for DropdownOption {
    fn from(value: SharedString) -> Self {
        Self { label: value }
    }
}

struct DropdownState {
    phase: DropdownPhase,
    trigger_bounds: Option<Bounds<Pixels>>,
    menu_scroll_handle: ScrollHandle,
    open_generation: u64,
    scroll_bound_generation: u64,
}

type DropdownTriggerBuilder =
    Rc<dyn Fn(&ThemeColors, Pixels, Pixels, bool, f32, &SharedString) -> AnyElement>;

pub struct DropdownOverlayState {
    active: Option<DropdownOverlaySnapshot>,
}

impl Default for DropdownOverlayState {
    fn default() -> Self {
        Self { active: None }
    }
}

impl Global for DropdownOverlayState {}

#[derive(Clone)]
struct DropdownOverlaySnapshot {
    id: ElementId,
    parent_view_id: EntityId,
    state: WeakEntity<DropdownState>,
    colors: ThemeColors,
    width: Pixels,
    trigger_bounds: Option<Bounds<Pixels>>,
    options: Rc<Vec<DropdownOption>>,
    selected_index: usize,
    menu_scroll_handle: ScrollHandle,
    on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    scroll_id: SharedString,
    top_left: Point<Pixels>,
    open_up: bool,
    phase: DropdownPhase,
    menu_h: Pixels,
    animated_h: Pixels,
    panel_opacity: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DropdownPhase {
    Closed,
    Opening { at: Instant },
    Open,
    Closing { at: Instant },
}

impl DropdownPhase {
    fn is_visible(self) -> bool {
        matches!(
            self,
            Self::Opening { .. } | Self::Open | Self::Closing { .. }
        )
    }
}

impl DropdownOverlayState {
    fn set_active(&mut self, snapshot: DropdownOverlaySnapshot) {
        self.active = Some(snapshot);
    }

    pub(crate) fn clear(&mut self) {
        self.active = None;
    }

    fn clear_if_matches(&mut self, id: &ElementId) {
        if self.active.as_ref().is_some_and(|active| &active.id == id) {
            self.active = None;
        }
    }
}

fn opening_progress(now: Instant, at: Instant) -> f32 {
    let t = (now.duration_since(at).as_secs_f32() / OPEN_DURATION_SECS).clamp(0.0, 1.0);
    ease_out_cubic(t)
}

fn closing_progress(now: Instant, at: Instant) -> f32 {
    let t = (now.duration_since(at).as_secs_f32() / CLOSE_DURATION_SECS).clamp(0.0, 1.0);
    1.0 - ease_out_cubic(t)
}

fn phase_after_deadline(phase: DropdownPhase, now: Instant) -> DropdownPhase {
    match phase {
        DropdownPhase::Opening { at }
            if now.saturating_duration_since(at) >= Duration::from_secs_f32(OPEN_DURATION_SECS) =>
        {
            DropdownPhase::Open
        }
        DropdownPhase::Closing { at }
            if now.saturating_duration_since(at)
                >= Duration::from_secs_f32(CLOSE_DURATION_SECS) =>
        {
            DropdownPhase::Closed
        }
        phase => phase,
    }
}

fn dropdown_min_preview_height() -> Pixels {
    px(MENU_VERTICAL_PADDING * 2.0 + MENU_ROW_HEIGHT * MENU_MIN_PREVIEW_ROWS)
}

fn choose_dropdown_direction(
    available_above: Pixels,
    available_below: Pixels,
    desired_height: Pixels,
) -> bool {
    let can_fit_below = available_below >= desired_height;
    let can_fit_above = available_above >= desired_height;

    if can_fit_below && !can_fit_above {
        return false;
    }
    if can_fit_above && !can_fit_below {
        return true;
    }
    if can_fit_above && can_fit_below {
        return available_above > available_below;
    }

    available_above > available_below
}

fn effective_dropdown_height(available_height: Pixels, desired_height: Pixels) -> Pixels {
    if available_height <= px(0.0) {
        return px(0.0);
    }

    let minimum_preview = dropdown_min_preview_height().min(desired_height);
    if available_height >= minimum_preview {
        desired_height.min(available_height)
    } else {
        available_height.min(desired_height)
    }
}

fn measure_dropdown_label_width(window: &Window, label: &SharedString) -> Pixels {
    if label.is_empty() {
        return px(0.0);
    }

    let text_style = window.text_style();
    let font_size = px(13.0);
    let line = window.text_system().shape_line(
        label.clone(),
        font_size,
        &[TextRun {
            len: label.len(),
            font: Font {
                family: text_style.font_family,
                features: text_style.font_features,
                fallbacks: text_style.font_fallbacks,
                weight: FontWeight::MEDIUM,
                style: text_style.font_style,
            },
            color: text_style.color,
            background_color: None,
            background_corner_radius: None,
            background_padding: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );

    line.width
}

fn desired_dropdown_menu_width(
    window: &Window,
    trigger_width: Pixels,
    options: &[DropdownOption],
    window_width: Pixels,
) -> Pixels {
    let widest_label = options.iter().fold(px(0.0), |widest, option| {
        widest.max(measure_dropdown_label_width(window, &option.label))
    });
    let desired_width = widest_label + px(MENU_CONTENT_PADDING_X + MENU_CHECK_ICON_ALLOWANCE);
    let max_width = (window_width - px(MENU_WINDOW_EDGE_PADDING * 2.0)).max(px(MENU_MIN_WIDTH));

    desired_width
        .max(trigger_width)
        .max(px(MENU_MIN_WIDTH))
        .min(max_width)
}

pub fn render_overlay(
    window: &mut Window,
    now: Instant,
    state: &DropdownOverlayState,
) -> AnyElement {
    let Some(active) = state.active.as_ref() else {
        return div().into_any_element();
    };

    let colors = active.colors;
    let menu_scroll_handle = active.menu_scroll_handle.clone();
    let options = active.options.clone();
    let selected_index = normalize_selected_index(active.selected_index, options.len());
    let open_k = match active.phase {
        DropdownPhase::Closed => 0.0,
        DropdownPhase::Open => 1.0,
        DropdownPhase::Opening { at } => opening_progress(now, at),
        DropdownPhase::Closing { at } => closing_progress(now, at),
    }
    .clamp(0.0, 1.0);

    if matches!(active.phase, DropdownPhase::Closing { .. }) && open_k <= 0.02 {
        return div().into_any_element();
    }

    request_animation_frame_if(
        window,
        matches!(
            active.phase,
            DropdownPhase::Opening { .. } | DropdownPhase::Closing { .. }
        ),
    );

    if active.animated_h <= px(1.0) || active.width <= px(1.0) {
        return div().absolute().inset_0().into_any_element();
    }

    let panel_opacity = active.panel_opacity;
    let translate_y = px(if active.open_up {
        -MENU_TRANSLATE_PX * (1.0 - open_k)
    } else {
        MENU_TRANSLATE_PX * (1.0 - open_k)
    });
    let panel_top_left = if active.open_up {
        point(
            active.top_left.x,
            active.top_left.y + (active.menu_h - active.animated_h) + translate_y,
        )
    } else {
        point(active.top_left.x, active.top_left.y + translate_y)
    };
    let popup = div()
        .absolute()
        .left(panel_top_left.x)
        .top(panel_top_left.y)
        .w(active.width)
        .h(active.animated_h)
        .rounded(px(14.))
        .overflow_hidden()
        .relative()
        .occlude()
        .bg(Hsla {
            a: 0.96,
            ..colors.settings_field_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.22,
            },
            blur_radius: px(30.0),
            spread_radius: px(-8.0),
            offset: point(px(0.), px(12.)),
        }])
        .opacity(panel_opacity)
        .on_scroll_wheel(|_, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Middle, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Right, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Middle, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .child(
            div().w(active.width).h(active.menu_h).child(
                div()
                    .id(active.scroll_id.clone())
                    .size_full()
                    .overflow_y_scroll()
                    .scrollbar_width(px(0.))
                    .track_scroll(&menu_scroll_handle)
                    .p(px(8.))
                    .flex()
                    .flex_col()
                    .justify_start()
                    .gap(px(4.))
                    .children(options.iter().enumerate().map(|(ix, opt)| {
                        let is_selected = ix == selected_index;

                        let item_bg = if is_selected {
                            Hsla {
                                a: 0.12,
                                ..colors.accent
                            }
                        } else {
                            Hsla {
                                a: 0.0,
                                ..colors.surface
                            }
                        };

                        let item_fg = colors.text_primary;

                        div()
                            .h(px(MENU_ROW_HEIGHT))
                            .rounded(px(10.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .cursor_pointer()
                            .bg(item_bg)
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(if is_selected {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .text_color(item_fg)
                                    .child(opt.label.clone()),
                            )
                            .when(is_selected, |this| {
                                this.child(
                                    svg()
                                        .path(lucide_icons::icon_check())
                                        .w(px(16.))
                                        .h(px(16.))
                                        .opacity(0.9)
                                        .text_color(item_fg),
                                )
                            })
                            .hover(|s| {
                                s.bg(Hsla {
                                    a: 0.06,
                                    ..colors.text_secondary
                                })
                            })
                            .on_mouse_down(MouseButton::Left, {
                                let state = active.state.clone();
                                let on_select = active.on_select.clone();
                                let parent_view_id = active.parent_view_id;
                                let overlay_id = active.id.clone();
                                move |_ev, window, cx| {
                                    cx.stop_propagation();
                                    (on_select)(ix, window, cx);

                                    cx.update_global(|overlay: &mut DropdownOverlayState, _cx| {
                                        overlay.clear_if_matches(&overlay_id);
                                    });

                                    let now = Instant::now();
                                    if let Err(err) = state.update(cx, |s, _| {
                                        s.phase = DropdownPhase::Closing { at: now };
                                    }) {
                                        tracing::debug!(
                                            "dropdown close after selection skipped: {err:?}"
                                        );
                                    } else {
                                        cx.notify(parent_view_id);
                                    }
                                }
                            })
                    })),
            ),
        );

    let popup = popup.on_mouse_down_out({
        let state = active.state.clone();
        let parent_view_id = active.parent_view_id;
        let trigger_bounds = active.trigger_bounds;
        let overlay_id = active.id.clone();
        move |ev, _window, cx| {
            if trigger_bounds.is_some_and(|bounds| bounds.contains(&ev.position)) {
                return;
            }

            cx.update_global(|overlay: &mut DropdownOverlayState, _cx| {
                overlay.clear_if_matches(&overlay_id);
            });

            let now = Instant::now();
            if let Err(err) = state.update(cx, |s, _| {
                s.phase = DropdownPhase::Closing { at: now };
            }) {
                tracing::debug!("dropdown close from outside click skipped: {err:?}");
            } else {
                cx.notify(parent_view_id);
            }
        }
    });

    div().absolute().inset_0().child(popup).into_any_element()
}

pub fn has_visible_overlay(now: Instant, state: &DropdownOverlayState) -> bool {
    let Some(active) = state.active.as_ref() else {
        return false;
    };

    if !active.phase.is_visible() {
        return false;
    }

    !matches!(active.phase, DropdownPhase::Closing { at } if closing_progress(now, at) <= 0.02)
}

/// Lightweight dropdown with:
/// - fixed-edge open/close animation (no "middle expansion")
/// - clipped outer height animation + fixed inner content height
/// - space-aware flip toward the side with more room
/// - hidden scrollbars
#[derive(IntoElement)]
pub struct Dropdown {
    id: ElementId,
    colors: ThemeColors,
    width: Pixels,
    trigger_height: Pixels,
    enabled: bool,
    label: SharedString,
    options: Vec<DropdownOption>,
    selected_index: usize,
    trigger_builder: DropdownTriggerBuilder,
    on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    rounded: Option<Pixels>,
}

impl Dropdown {
    pub fn new(
        id: impl Into<ElementId>,
        colors: &ThemeColors,
        width: Pixels,
        label: SharedString,
        options: Vec<DropdownOption>,
        selected_index: usize,
        enabled: bool,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            colors: colors.clone(),
            width,
            trigger_height: px(TRIGGER_HEIGHT),
            enabled,
            label,
            options,
            selected_index,
            trigger_builder: Rc::new(default_dropdown_trigger),
            on_select: Rc::new(on_select),
            rounded: None,
        }
    }

    pub fn with_trigger(
        id: impl Into<ElementId>,
        colors: &ThemeColors,
        width: Pixels,
        trigger_height: Pixels,
        label: SharedString,
        options: Vec<DropdownOption>,
        selected_index: usize,
        enabled: bool,
        trigger_builder: impl Fn(&ThemeColors, Pixels, Pixels, bool, f32, &SharedString) -> AnyElement
        + 'static,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            colors: colors.clone(),
            width,
            trigger_height,
            enabled,
            label,
            options,
            selected_index,
            trigger_builder: Rc::new(trigger_builder),
            on_select: Rc::new(on_select),
            rounded: None,
        }
    }

    pub fn rounded(mut self, rounded: Pixels) -> Self {
        self.rounded = Some(rounded);
        self
    }

    pub fn with_height(mut self, trigger_height: Pixels) -> Self {
        self.trigger_height = trigger_height;
        self
    }
}

impl RenderOnce for Dropdown {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Dropdown {
            id,
            colors,
            width,
            trigger_height,
            enabled,
            label,
            options,
            selected_index,
            trigger_builder,
            on_select,
            rounded,
        } = self;

        let options = Rc::new(options);
        let selected_index = normalize_selected_index(selected_index, options.len());

        let state = window.use_keyed_state(id.clone(), cx, |_, _| DropdownState {
            phase: DropdownPhase::Closed,
            trigger_bounds: None,
            menu_scroll_handle: ScrollHandle::new(),
            open_generation: 0,
            scroll_bound_generation: 0,
        });

        let parent_view_id = window.current_view();
        let scroll_id: SharedString = SharedString::from(format!("dropdown-scroll-{:?}", id));

        let snapshot = state.read(cx);
        let phase = snapshot.phase;
        let trigger_bounds = snapshot.trigger_bounds;

        let now = Instant::now();
        let open_k = match phase {
            DropdownPhase::Closed => 0.0,
            DropdownPhase::Open => 1.0,
            DropdownPhase::Opening { at } => opening_progress(now, at),
            DropdownPhase::Closing { at } => closing_progress(now, at),
        }
        .clamp(0.0, 1.0);

        let phase_animating = matches!(
            phase,
            DropdownPhase::Opening { .. } | DropdownPhase::Closing { .. }
        );
        request_animation_frame_if(window, phase_animating);

        if matches!(phase, DropdownPhase::Closing { .. }) && open_k <= 0.001 {
            state.update(cx, |s, _| s.phase = DropdownPhase::Closed);
            cx.notify(parent_view_id);
        } else if matches!(phase, DropdownPhase::Opening { .. }) && open_k >= 0.999 {
            state.update(cx, |s, _| s.phase = DropdownPhase::Open);
        }

        let trigger_content =
            (trigger_builder)(&colors, width, trigger_height, enabled, open_k, &label);

        let trigger = div()
            .id(id.clone())
            .w(width)
            .h(trigger_height)
            .rounded(rounded.unwrap_or(px(12.)))
            .relative()
            .text_color(if enabled {
                colors.text_primary
            } else {
                colors.text_muted
            })
            .bg(Hsla {
                a: 0.84,
                ..colors.settings_card_bg
            })
            .border_1()
            .border_color(Hsla {
                a: 0.24,
                ..colors.border
            })
            .shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.12,
                    ..rgb(0x000000).into()
                },
                blur_radius: px(14.0),
                spread_radius: px(-6.0),
                offset: point(px(0.), px(4.)),
            }])
            .when(!enabled, |this| this.opacity(0.65))
            .active(|s| {
                s.bg(Hsla {
                    a: 0.92,
                    ..colors.surface_hover
                })
            })
            .hover(|s| {
                s.bg(Hsla {
                    a: 0.90,
                    ..colors.surface_hover
                })
                .border_color(Hsla {
                    a: 0.30,
                    ..colors.border
                })
            })
            .child(trigger_content)
            .on_click({
                let state = state.clone();
                move |_ev, _window, cx| {
                    if !enabled {
                        return;
                    }

                    let now = Instant::now();
                    state.update(cx, |s, _| {
                        s.phase = match s.phase {
                            DropdownPhase::Closed | DropdownPhase::Closing { .. } => {
                                DropdownPhase::Opening { at: now }
                            }
                            DropdownPhase::Open | DropdownPhase::Opening { .. } => {
                                DropdownPhase::Closing { at: now }
                            }
                        };

                        if matches!(s.phase, DropdownPhase::Opening { .. }) {
                            s.open_generation = s.open_generation.wrapping_add(1);
                            s.scroll_bound_generation = 0;
                        }
                    });
                    cx.notify(parent_view_id);
                }
            })
            .child(
                canvas(
                    {
                        let state = state.clone();
                        let parent_view_id = parent_view_id;
                        move |bounds, _, cx| {
                            let should_notify = state.update(cx, |s, _| {
                                if s.trigger_bounds == Some(bounds) {
                                    return false;
                                }
                                s.trigger_bounds = Some(bounds);
                                s.phase.is_visible()
                            });

                            if should_notify {
                                cx.notify(parent_view_id);
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .right(px(0.))
                .bottom(px(0.)),
            );

        let phase_after_cleanup = phase_after_deadline(state.read(cx).phase, now);
        if phase_after_cleanup != state.read(cx).phase {
            state.update(cx, |s, _| s.phase = phase_after_cleanup);
        }
        if !phase_after_cleanup.is_visible() {
            return trigger;
        }

        let (scroll_bound_generation, open_generation, menu_scroll_handle) = {
            let snapshot = state.read(cx);
            (
                snapshot.scroll_bound_generation,
                snapshot.open_generation,
                snapshot.menu_scroll_handle.clone(),
            )
        };

        if scroll_bound_generation != open_generation {
            state.update(cx, |s, _| {
                if s.scroll_bound_generation == s.open_generation {
                    return;
                }

                if selected_index != usize::MAX {
                    s.menu_scroll_handle.scroll_to_item(selected_index);
                }
                s.scroll_bound_generation = s.open_generation;
            });
        }

        let element_offset = window.element_offset();
        let trigger_bounds = trigger_bounds.map(|bounds| Bounds {
            origin: bounds.origin + element_offset,
            size: bounds.size,
        });
        let window_size = window.bounds().size;
        let row_h = px(MENU_ROW_HEIGHT);
        let menu_width =
            desired_dropdown_menu_width(window, width, options.as_ref(), window_size.width);
        let max_h = px(MENU_MAX_HEIGHT);
        let desired_h =
            px(MENU_VERTICAL_PADDING) + row_h * (options.len() as f32) + px(MENU_VERTICAL_PADDING);
        let capped_h = desired_h.min(max_h);
        let available_space = trigger_bounds.map(|bounds| {
            let safe_top = px(MENU_WINDOW_EDGE_PADDING);
            let safe_bottom = window_size.height - px(MENU_WINDOW_EDGE_PADDING);
            let above = (bounds.origin.y - safe_top - px(MENU_GAP)).max(px(0.0));
            let below =
                (safe_bottom - (bounds.origin.y + bounds.size.height) - px(MENU_GAP)).max(px(0.0));

            (above, below)
        });
        let open_up = available_space
            .map(|(above, below)| {
                let preferred_up = choose_dropdown_direction(above, below, capped_h);
                if preferred_up {
                    return true;
                }

                let minimum_preview = dropdown_min_preview_height().min(capped_h);
                below < minimum_preview && above > below
            })
            .unwrap_or(false);
        let available_h = available_space
            .map(|(above, below)| if open_up { above } else { below })
            .unwrap_or(max_h);
        let menu_h = effective_dropdown_height(available_h, capped_h);
        let animated_h = menu_h * open_k;
        let safe_left = px(MENU_WINDOW_EDGE_PADDING);
        let safe_right = window_size.width - px(MENU_WINDOW_EDGE_PADDING);
        let max_left = (safe_right - menu_width).max(safe_left);

        let final_top_left = trigger_bounds
            .map(|b| {
                let left = b.origin.x.clamp(safe_left, max_left);
                if open_up {
                    point(left, b.origin.y - px(MENU_GAP) - menu_h)
                } else {
                    point(left, b.origin.y + b.size.height + px(MENU_GAP))
                }
            })
            .unwrap_or(point(px(0.), px(0.)));

        let panel_opacity = 0.78 + 0.22 * open_k;
        let overlay_snapshot = DropdownOverlaySnapshot {
            id: id.clone(),
            parent_view_id,
            state: state.downgrade(),
            colors,
            width: menu_width,
            options: options.clone(),
            selected_index,
            menu_scroll_handle,
            on_select: on_select.clone(),
            scroll_id,
            top_left: final_top_left,
            trigger_bounds,
            open_up,
            phase: phase_after_cleanup,
            menu_h,
            animated_h,
            panel_opacity,
        };

        let should_update_overlay = cx.read_global(|overlay: &DropdownOverlayState, _cx| {
            match (phase_after_cleanup.is_visible(), overlay.active.as_ref()) {
                (false, Some(active)) => active.id == id,
                (false, None) => false,
                (true, Some(active)) => {
                    active.id != overlay_snapshot.id
                        || active.phase != overlay_snapshot.phase
                        || active.top_left != overlay_snapshot.top_left
                        || active.open_up != overlay_snapshot.open_up
                        || active.menu_h != overlay_snapshot.menu_h
                        || active.animated_h != overlay_snapshot.animated_h
                        || active.panel_opacity != overlay_snapshot.panel_opacity
                        || active.width != overlay_snapshot.width
                        || active.selected_index != overlay_snapshot.selected_index
                }
                (true, None) => true,
            }
        });

        if should_update_overlay {
            cx.update_global(|overlay: &mut DropdownOverlayState, _cx| {
                if phase_after_cleanup.is_visible() {
                    overlay.set_active(overlay_snapshot);
                } else {
                    overlay.clear_if_matches(&id);
                }
            });
        }

        div()
            .id(SharedString::from(format!("dropdown-shell-{:?}", id)))
            .relative()
            .child(trigger)
    }
}

fn normalize_selected_index(selected_index: usize, option_count: usize) -> usize {
    if selected_index == usize::MAX {
        return usize::MAX;
    }

    selected_index.min(option_count.saturating_sub(1))
}

fn default_dropdown_trigger(
    colors: &ThemeColors,
    _width: Pixels,
    _trigger_height: Pixels,
    enabled: bool,
    open_k: f32,
    label: &SharedString,
) -> AnyElement {
    let chevron = svg()
        .path(lucide_icons::icon_chevron_down())
        .w(px(16.))
        .h(px(16.))
        .opacity(if enabled { 0.80 } else { 0.35 })
        .text_color(colors.text_secondary)
        .with_transformation(Transformation::rotate(radians(
            open_k * std::f32::consts::PI,
        )));

    div()
        .size_full()
        .px(px(12.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(14.))
                .text_color(if enabled {
                    colors.text_primary
                } else {
                    colors.text_muted
                })
                .overflow_hidden()
                .text_ellipsis()
                .child(label.clone()),
        )
        .child(chevron)
        .into_any_element()
}
