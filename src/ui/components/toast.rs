use crate::ui::animation::{
    ease_in_cubic, ease_out_back, ease_out_cubic, request_animation_frame_if,
};
use crate::ui::theme::colors::ThemeColors;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_TOASTS: usize = 4;
const DEFAULT_DURATION: Duration = Duration::from_secs(3);
const TOAST_WAKE_EPSILON: Duration = Duration::from_millis(8);
// --- 动效参数（建议集中放这里，方便调参）---
// Web 参考：`.upstream_bmbl_1/src/components/Toast.css`（`--toast-anim-duration`）
const FADE_IN: Duration = Duration::from_millis(280); // 进入动画时长
const FADE_OUT: Duration = Duration::from_millis(280); // 退出 + 回收（折叠）动画时长

// 多个 Toast 连续弹出时，后续 Toast 的入场延迟。
// 0 = 同时入场；多个 Toast 连续触发时可避免排队造成的层叠抖动感。
const STAGGER_IN_MS: u64 = 0;

// 外观几何参数。
const TOAST_RADIUS_PX: f32 = 8.0; // 圆角半径（越小越“方”）
const TOAST_SPACING_PX: f32 = 6.0; // Toast 堆叠间距（垂直）
const TOAST_MIN_WIDTH_PX: f32 = 120.0;
const TOAST_MAX_WIDTH_PX: f32 = 240.0;
const TOAST_SIDE_PADDING_PX: f32 = 24.0;
const TOAST_CONTENT_GAP_PX: f32 = 8.0;
const TOAST_ICON_PX: f32 = 16.0;
const TOAST_BODY_H_PX: f32 = 32.0;
const TOAST_SLOT_H_PX: f32 = 40.0;

// 进入/退出滑动距离（像素）。
const ENTER_SLIDE_PX: f32 = 56.0; // 入场时从屏外滑入的距离
const EXIT_SLIDE_PX: f32 = 24.0; // 退场时滑走的距离

fn toast_width_for_message(message: &str) -> Pixels {
    let mut body_width = 0.0;
    for character in message.chars() {
        body_width += if character.is_ascii() {
            match character {
                ' ' => 4.5,
                'i' | 'l' | 'I' | '1' | '!' | ':' | ';' | '.' | ',' => 4.5,
                'm' | 'w' | 'M' | 'W' => 9.5,
                _ => 7.0,
            }
        } else {
            12.0
        };
    }

    let estimated_width = TOAST_SIDE_PADDING_PX
        + TOAST_ICON_PX
        + TOAST_CONTENT_GAP_PX
        + body_width
        + TOAST_SIDE_PADDING_PX;

    px(estimated_width.clamp(TOAST_MIN_WIDTH_PX, TOAST_MAX_WIDTH_PX))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToastPlacement {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
    TopCenter,
    BottomCenter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToastStackDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToastSlideDirection {
    FromTop,
    FromBottom,
    FromLeft,
    FromRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastInsets {
    pub top: Pixels,
    pub right: Pixels,
    pub bottom: Pixels,
    pub left: Pixels,
}

impl ToastInsets {
    pub fn all(value: Pixels) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub fn with_bottom(mut self, bottom: Pixels) -> Self {
        self.bottom = bottom;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToastOverlayOptions {
    pub placement: ToastPlacement,
    pub stack_direction: ToastStackDirection,
    pub slide_direction: ToastSlideDirection,
    pub insets: ToastInsets,
}

impl ToastOverlayOptions {
    pub fn new(placement: ToastPlacement) -> Self {
        Self {
            placement,
            ..Self::default()
        }
    }

    pub fn with_insets(mut self, insets: ToastInsets) -> Self {
        self.insets = insets;
        self
    }

    pub fn with_bottom_inset(mut self, bottom: Pixels) -> Self {
        self.insets.bottom = bottom;
        self
    }

    pub fn with_stack_direction(mut self, direction: ToastStackDirection) -> Self {
        self.stack_direction = direction;
        self
    }

    pub fn with_slide_direction(mut self, direction: ToastSlideDirection) -> Self {
        self.slide_direction = direction;
        self
    }
}

impl Default for ToastOverlayOptions {
    fn default() -> Self {
        Self {
            placement: ToastPlacement::BottomRight,
            stack_direction: ToastStackDirection::Up,
            slide_direction: ToastSlideDirection::FromRight,
            insets: ToastInsets::all(px(22.0)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ToastId(u64);

#[derive(Clone)]
struct ToastItem {
    id: ToastId,
    kind: ToastKind,
    message: SharedString,
    created_at: Instant,
    anim_delay: Duration,
    duration: Duration,
    dismissed_at: Option<Instant>,
}

pub struct ToastState {
    next_id: u64,
    items: VecDeque<ToastItem>,
    breadcrumb: Option<ToastItem>,

    placement: ToastPlacement,
    stack_direction: ToastStackDirection,
    slide_direction: ToastSlideDirection,
    insets: ToastInsets,
}

impl Default for ToastState {
    fn default() -> Self {
        Self {
            next_id: 1,
            items: VecDeque::new(),
            breadcrumb: None,
            placement: ToastPlacement::BottomRight,
            stack_direction: ToastStackDirection::Up,
            slide_direction: ToastSlideDirection::FromRight,
            insets: ToastInsets::all(px(22.0)),
        }
    }
}

impl Global for ToastState {}

impl ToastState {
    fn push(
        &mut self,
        kind: ToastKind,
        message: SharedString,
        duration: Duration,
        now: Instant,
    ) -> ToastId {
        if message.as_ref().trim().is_empty() {
            return ToastId(0);
        }

        let id = ToastId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let queue_index = self.items.len().min(MAX_TOASTS.saturating_sub(1)) as u64;
        let anim_delay = Duration::from_millis(STAGGER_IN_MS.saturating_mul(queue_index));

        self.items.push_back(ToastItem {
            id,
            kind,
            message,
            created_at: now,
            anim_delay,
            duration,
            dismissed_at: None,
        });

        // If we exceed the visible limit, start an exit animation for the oldest toast instead of
        // removing it immediately. Immediate removal causes visible stack "jitter".
        let mut active_count = self
            .items
            .iter()
            .filter(|item| item.dismissed_at.is_none())
            .count();
        while active_count > MAX_TOASTS {
            if let Some(oldest) = self
                .items
                .iter_mut()
                .find(|item| item.dismissed_at.is_none())
            {
                oldest.dismissed_at.get_or_insert(now);
            } else {
                break;
            }
            active_count = self
                .items
                .iter()
                .filter(|item| item.dismissed_at.is_none())
                .count();
        }

        // Hard cap to prevent unbounded growth if pushes happen faster than we can animate out.
        while self.items.len() > MAX_TOASTS.saturating_add(6) {
            self.items.pop_front();
        }

        id
    }

    fn push_breadcrumb(&mut self, message: SharedString, now: Instant) -> ToastId {
        if message.as_ref().trim().is_empty() {
            return ToastId(0);
        }

        let id = if let Some(item) = self.breadcrumb.as_mut() {
            item.message = message;
            item.created_at = now;
            item.anim_delay = Duration::ZERO;
            item.duration = DEFAULT_DURATION;
            item.dismissed_at = None;
            item.id
        } else {
            let id = ToastId(self.next_id);
            self.next_id = self.next_id.saturating_add(1);
            self.breadcrumb = Some(ToastItem {
                id,
                kind: ToastKind::Info,
                message,
                created_at: now,
                anim_delay: Duration::ZERO,
                duration: DEFAULT_DURATION,
                dismissed_at: None,
            });
            id
        };

        id
    }

    fn fade_from(item: &ToastItem) -> Instant {
        item.dismissed_at
            .unwrap_or_else(|| item.created_at + item.anim_delay + item.duration)
    }

    fn is_expired(item: &ToastItem, now: Instant) -> bool {
        now.saturating_duration_since(Self::fade_from(item)) >= FADE_OUT
    }

    fn prune_expired(&mut self, now: Instant) {
        while let Some(front) = self.items.front() {
            if !Self::is_expired(front, now) {
                break;
            }
            self.items.pop_front();
        }

        let breadcrumb_expired = self
            .breadcrumb
            .as_ref()
            .is_some_and(|item| Self::is_expired(item, now));
        if breadcrumb_expired {
            self.breadcrumb = None;
        }
    }

    fn next_static_deadline(&self, now: Instant) -> Option<Instant> {
        self.items
            .iter()
            .chain(self.breadcrumb.iter())
            .filter_map(|item| {
                if Self::is_expired(item, now) {
                    return None;
                }

                let fade_from = Self::fade_from(item);
                if now < fade_from {
                    Some(fade_from)
                } else if now < fade_from + FADE_OUT {
                    None
                } else {
                    Some(now)
                }
            })
            .min()
    }

    fn dismiss(&mut self, id: ToastId, now: Instant) {
        if id.0 == 0 {
            return;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.dismissed_at.get_or_insert(now);
            return;
        }

        if let Some(item) = self.breadcrumb.as_mut().filter(|item| item.id == id) {
            item.dismissed_at.get_or_insert(now);
        }
    }

    fn resolve(&mut self, id: ToastId, kind: ToastKind, message: SharedString, now: Instant) {
        if id.0 == 0 {
            return;
        }
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.kind = kind;
            item.message = message;
            item.created_at = now;
            item.anim_delay = Duration::ZERO;
            item.duration = DEFAULT_DURATION;
            item.dismissed_at = None;
            return;
        }

        if let Some(item) = self.breadcrumb.as_mut().filter(|item| item.id == id) {
            item.kind = kind;
            item.message = message;
            item.created_at = now;
            item.anim_delay = Duration::ZERO;
            item.duration = DEFAULT_DURATION;
            item.dismissed_at = None;
        }
    }
}

pub fn set_placement(cx: &mut App, placement: ToastPlacement) {
    cx.update_global(|state: &mut ToastState, cx| {
        state.placement = placement;
    });
}

pub fn set_stack_direction(cx: &mut App, direction: ToastStackDirection) {
    cx.update_global(|state: &mut ToastState, cx| {
        state.stack_direction = direction;
    });
}

pub fn set_slide_direction(cx: &mut App, direction: ToastSlideDirection) {
    cx.update_global(|state: &mut ToastState, cx| {
        state.slide_direction = direction;
    });
}

pub fn set_margin(cx: &mut App, margin: Pixels) {
    set_insets(cx, ToastInsets::all(margin));
}

pub fn set_insets(cx: &mut App, insets: ToastInsets) {
    cx.update_global(|state: &mut ToastState, cx| {
        state.insets = insets;
    });
}

pub fn push(cx: &mut App, message: SharedString) -> ToastId {
    push_kind(cx, ToastKind::Info, message)
}

pub fn success(cx: &mut App, message: SharedString) -> ToastId {
    push_kind(cx, ToastKind::Success, message)
}

pub fn error(cx: &mut App, message: SharedString) -> ToastId {
    push_kind(cx, ToastKind::Error, message)
}

pub fn pending(cx: &mut App, message: SharedString) -> ToastId {
    push_kind_duration(cx, ToastKind::Info, message, Duration::from_secs(60))
}

pub fn push_kind(cx: &mut App, kind: ToastKind, message: SharedString) -> ToastId {
    push_kind_duration(cx, kind, message, DEFAULT_DURATION)
}

pub fn push_kind_duration(
    cx: &mut App,
    kind: ToastKind,
    message: SharedString,
    duration: Duration,
) -> ToastId {
    let now = Instant::now();
    cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        let id = state.push(kind, message, duration, now);
        id
    })
}

pub fn resolve(cx: &mut App, id: ToastId, kind: ToastKind, message: SharedString) {
    let now = Instant::now();
    cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        state.resolve(id, kind, message, now);
    });
}

pub fn dismiss(cx: &mut App, id: ToastId) {
    let now = Instant::now();
    cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        state.dismiss(id, now);
    });
}

pub fn push_breadcrumb(cx: &mut App, parts: &[SharedString]) -> ToastId {
    let mut message = String::new();
    for part in parts {
        let trimmed = part.as_ref().trim();
        if trimmed.is_empty() {
            continue;
        }
        if !message.is_empty() {
            message.push_str(" / ");
        }
        message.push_str(trimmed);
        if message.len() >= 220 {
            break;
        }
    }

    let now = Instant::now();
    cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        let id = state.push_breadcrumb(SharedString::from(message), now);
        id
    })
}

pub fn push_async(cx: &mut AsyncApp, kind: ToastKind, message: SharedString) -> ToastId {
    let now = Instant::now();
    match cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        let id = state.push(kind, message, DEFAULT_DURATION, now);
        id
    }) {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!("toast push_async: update_global failed: {err:?}");
            ToastId(0)
        }
    }
}

pub fn pending_async(cx: &mut AsyncApp, message: SharedString) -> ToastId {
    let now = Instant::now();
    match cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        let id = state.push(ToastKind::Info, message, Duration::from_secs(60), now);
        id
    }) {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!("toast pending_async: update_global failed: {err:?}");
            ToastId(0)
        }
    }
}

pub fn resolve_async(cx: &mut AsyncApp, id: ToastId, kind: ToastKind, message: SharedString) {
    let now = Instant::now();
    if let Err(err) = cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        state.resolve(id, kind, message, now);
    }) {
        tracing::warn!("toast resolve_async: update_global failed: {err:?}");
    }
}

pub fn push_breadcrumb_async(cx: &mut AsyncApp, parts: &[SharedString]) -> ToastId {
    let mut message = String::new();
    for part in parts {
        let trimmed = part.as_ref().trim();
        if trimmed.is_empty() {
            continue;
        }
        if !message.is_empty() {
            message.push_str(" / ");
        }
        message.push_str(trimmed);
        if message.len() >= 220 {
            break;
        }
    }
    let now = Instant::now();
    match cx.update_global(|state: &mut ToastState, cx| {
        state.prune_expired(now);
        let id = state.push_breadcrumb(SharedString::from(message), now);
        id
    }) {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!("toast push_breadcrumb_async: update_global failed: {err:?}");
            ToastId(0)
        }
    }
}

pub fn render_overlay(
    window: &mut Window,
    cx: &App,
    colors: &ThemeColors,
    now: Instant,
    state: &ToastState,
) -> AnyElement {
    render_overlay_with_options(window, cx, colors, now, state, state.overlay_options())
}

pub fn render_overlay_with_options(
    window: &mut Window,
    cx: &App,
    colors: &ThemeColors,
    now: Instant,
    state: &ToastState,
    options: ToastOverlayOptions,
) -> AnyElement {
    if !has_visible_toasts(now, state) {
        return div().into_any_element();
    }

    let mut any_animating = false;
    let mut outer = div()
        .absolute()
        .inset_0()
        .pt(options.insets.top)
        .pr(options.insets.right)
        .pb(options.insets.bottom)
        .pl(options.insets.left)
        .flex();

    outer = match options.placement {
        ToastPlacement::BottomRight => outer.justify_end().items_end(),
        ToastPlacement::BottomLeft => outer.justify_start().items_end(),
        ToastPlacement::TopRight => outer.justify_end().items_start(),
        ToastPlacement::TopLeft => outer.justify_start().items_start(),
        ToastPlacement::TopCenter => outer.justify_center().items_start(),
        ToastPlacement::BottomCenter => outer.justify_center().items_end(),
    };

    let toast_iter: Box<dyn Iterator<Item = &ToastItem>> = match options.stack_direction {
        ToastStackDirection::Up => Box::new(state.items.iter()),
        ToastStackDirection::Down => Box::new(state.items.iter().rev()),
    };
    let visible_items: Vec<&ToastItem> = toast_iter
        .filter(|item| !ToastState::is_expired(item, now))
        .collect();
    let mut layout_items = Vec::with_capacity(visible_items.len());
    for item in visible_items.iter().copied() {
        let visible_from = item.created_at + item.anim_delay;
        if now < visible_from {
            any_animating = true;
        }

        let dt = now.saturating_duration_since(visible_from);
        let appear_t = if now < visible_from {
            0.0
        } else {
            (dt.as_secs_f32() / FADE_IN.as_secs_f32()).clamp(0.0, 1.0)
        };
        if appear_t < 1.0 {
            any_animating = true;
        }

        let fade_from = ToastState::fade_from(item);
        let disappear_t = if now >= fade_from {
            any_animating = true;
            (now.saturating_duration_since(fade_from).as_secs_f32() / FADE_OUT.as_secs_f32())
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let disappear_k = ease_out_cubic(disappear_t);

        let opacity = (appear_t * (1.0 - disappear_k)).clamp(0.0, 1.0);
        let enter_slide = (1.0 - ease_out_back(appear_t, 2.35)) * ENTER_SLIDE_PX;
        let exit_slide = ease_in_cubic(disappear_t) * EXIT_SLIDE_PX;

        let toast_id = item.id;
        let toast_width = toast_width_for_message(item.message.as_ref());
        let shell = toast_shell(colors, item);

        layout_items.push((
            toast_id,
            toast_width,
            shell,
            opacity,
            enter_slide,
            exit_slide,
            1.0,
        ));
    }

    let mut occupied_sizes: Vec<f32> = layout_items
        .iter()
        .map(|(_, _, _, _, _, _, slot_factor)| (TOAST_SLOT_H_PX + TOAST_SPACING_PX) * *slot_factor)
        .collect();

    let mut offsets = Vec::with_capacity(layout_items.len());
    match options.placement {
        ToastPlacement::BottomRight | ToastPlacement::BottomLeft | ToastPlacement::BottomCenter => {
            let mut running = 0.0;
            for size in occupied_sizes.iter().rev() {
                offsets.push(running);
                running += *size;
            }
            offsets.reverse();
        }
        ToastPlacement::TopRight | ToastPlacement::TopLeft | ToastPlacement::TopCenter => {
            let mut running = 0.0;
            for size in &occupied_sizes {
                offsets.push(running);
                running += *size;
            }
        }
    }

    let mut lane = div().relative().w(px(TOAST_MAX_WIDTH_PX)).h_full();
    for (index, layout_item) in layout_items.into_iter().enumerate() {
        let (toast_id, toast_width, shell, opacity, enter_slide, exit_slide, _) = layout_item;
        let offset = offsets[index];
        let mut toast = div()
            .id(SharedString::from(format!("toast-wrap-{}", toast_id.0)))
            .absolute()
            .w(toast_width)
            .cursor_pointer()
            .opacity(opacity)
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                cx.update_global(|state: &mut ToastState, cx| {
                    let now = Instant::now();
                    state.prune_expired(now);
                    state.dismiss(toast_id, now);
                });
            });

        let slide = -(enter_slide + exit_slide);
        toast = match options.placement {
            ToastPlacement::BottomRight => toast.right(px(slide)).bottom(px(offset)),
            ToastPlacement::BottomLeft => toast.left(px(slide)).bottom(px(offset)),
            ToastPlacement::TopRight => toast.right(px(slide)).top(px(offset)),
            ToastPlacement::TopLeft => toast.left(px(slide)).top(px(offset)),
            ToastPlacement::TopCenter => match options.slide_direction {
                ToastSlideDirection::FromLeft => toast.left(px(slide)).top(px(offset)),
                ToastSlideDirection::FromRight
                | ToastSlideDirection::FromTop
                | ToastSlideDirection::FromBottom => toast.right(px(slide)).top(px(offset)),
            },
            ToastPlacement::BottomCenter => match options.slide_direction {
                ToastSlideDirection::FromLeft => toast.left(px(slide)).bottom(px(offset)),
                ToastSlideDirection::FromRight
                | ToastSlideDirection::FromTop
                | ToastSlideDirection::FromBottom => toast.right(px(slide)).bottom(px(offset)),
            },
        };

        toast = toast.child(shell);

        lane = lane.child(toast);
    }

    request_animation_frame_if(window, any_animating);
    if !any_animating && let Some(deadline) = state.next_static_deadline(now) {
        window.request_invalidation_at(deadline + TOAST_WAKE_EPSILON, cx);
    }
    outer.child(lane).into_any_element()
}

impl ToastState {
    pub fn overlay_options(&self) -> ToastOverlayOptions {
        ToastOverlayOptions {
            placement: self.placement,
            stack_direction: self.stack_direction,
            slide_direction: self.slide_direction,
            insets: self.insets,
        }
    }
}

pub fn has_visible_toasts(now: Instant, state: &ToastState) -> bool {
    state
        .items
        .iter()
        .any(|item| !ToastState::is_expired(item, now))
}

pub fn render_breadcrumb_overlay(
    window: &mut Window,
    cx: &App,
    colors: &ThemeColors,
    now: Instant,
    state: &ToastState,
) -> AnyElement {
    let Some(item) = state.breadcrumb.as_ref() else {
        return div().into_any_element();
    };
    if ToastState::is_expired(item, now) {
        return div().into_any_element();
    }

    let visible_from = item.created_at + item.anim_delay;
    let dt = now.saturating_duration_since(visible_from);
    let appear_t = if now < visible_from {
        0.0
    } else {
        (dt.as_secs_f32() / FADE_IN.as_secs_f32()).clamp(0.0, 1.0)
    };
    let appear_k = if appear_t < 1.0 {
        ease_out_cubic(appear_t)
    } else {
        1.0
    };

    let fade_from = ToastState::fade_from(item);
    let disappear_t = if now >= fade_from {
        (now.saturating_duration_since(fade_from).as_secs_f32() / FADE_OUT.as_secs_f32())
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let disappear_k = ease_out_cubic(disappear_t);
    let opacity = (appear_t * (1.0 - disappear_k)).clamp(0.0, 1.0);
    let slide = -((1.0 - ease_out_back(appear_t, 2.35)) * 16.0 + ease_in_cubic(disappear_t) * 10.0);

    let toast = div()
        .absolute()
        .w(toast_width_for_message(item.message.as_ref()))
        .opacity(opacity)
        .top(px(10.0))
        .child(
            div()
                .max_w(px(TOAST_MAX_WIDTH_PX))
                .min_w(px(TOAST_MIN_WIDTH_PX))
                .child(toast_shell(colors, item)),
        );

    let toast = match state.slide_direction {
        ToastSlideDirection::FromLeft => toast.left(px(slide)),
        ToastSlideDirection::FromRight
        | ToastSlideDirection::FromTop
        | ToastSlideDirection::FromBottom => toast.right(px(slide)),
    };

    request_animation_frame_if(window, appear_t < 1.0 || disappear_t < 1.0);
    if appear_t >= 1.0
        && disappear_t <= 0.0
        && let Some(deadline) = state.next_static_deadline(now)
    {
        window.request_invalidation_at(deadline + TOAST_WAKE_EPSILON, cx);
    }
    toast.into_any_element()
}

pub fn has_visible_breadcrumb(now: Instant, state: &ToastState) -> bool {
    state
        .breadcrumb
        .as_ref()
        .is_some_and(|item| !ToastState::is_expired(item, now))
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_DURATION, FADE_IN, FADE_OUT, ToastKind, ToastState};
    use gpui::SharedString;
    use std::time::{Duration, Instant};

    #[test]
    fn static_toast_wake_deadline_is_fade_start() {
        let mut state = ToastState::default();
        let now = Instant::now();

        state.push(
            ToastKind::Info,
            SharedString::from("ready"),
            DEFAULT_DURATION,
            now,
        );
        let steady = now + FADE_IN + Duration::from_millis(1);

        assert_eq!(
            state.next_static_deadline(steady),
            Some(now + DEFAULT_DURATION)
        );
    }

    #[test]
    fn prune_expired_removes_toast_after_fade_out() {
        let mut state = ToastState::default();
        let now = Instant::now();

        state.push(
            ToastKind::Info,
            SharedString::from("done"),
            Duration::ZERO,
            now,
        );
        state.prune_expired(now + FADE_OUT + Duration::from_millis(1));

        assert!(state.items.is_empty());
    }
}

fn toast_shell(colors: &ThemeColors, item: &ToastItem) -> impl IntoElement {
    let (accent, icon) = match item.kind {
        ToastKind::Info => (colors.accent, lucide_icons::icon_info()),
        ToastKind::Success => (
            hsla(142.0 / 360.0, 0.62, 0.42, 1.0),
            lucide_icons::icon_check(),
        ),
        ToastKind::Error => (colors.danger, lucide_icons::icon_circle_x()),
    };

    // 取消倒计时边框描边：不再渲染描边 canvas，因此这里不需要为边框预留 padding。
    div()
        .id(SharedString::from(format!("toast-{}", item.id.0)))
        .w_full()
        .bg(Hsla {
            a: 0.92,
            ..colors.surface
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.14,
                ..rgb(0x000000).into()
            },
            blur_radius: px(18.0),
            spread_radius: px(0.0),
            offset: point(px(0.), px(6.)),
        }])
        .rounded(px(TOAST_RADIUS_PX))
        .px(px(12.))
        .py(px(8.))
        .min_h(px(TOAST_BODY_H_PX))
        .hover(|this| {
            this.bg(Hsla {
                a: 0.96,
                ..colors.surface
            })
        })
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    svg()
                        .path(icon)
                        .w(px(16.))
                        .h(px(16.))
                        .text_color(accent)
                        .opacity(0.92),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(12.))
                        .truncate()
                        .text_color(colors.text_primary)
                        .child(item.message.clone()),
                ),
        )
}
