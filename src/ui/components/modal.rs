use gpui::*;
use std::{
    cell::Cell,
    panic::Location,
    rc::Rc,
    time::{Duration, Instant},
};

const MODAL_BACKDROP_MIN_ALPHA: f32 = 0.46;
const MODAL_BACKDROP_MAX_ALPHA: f32 = 0.66;
const DEFAULT_MODAL_BACKDROP_BLUR_PX: f32 = 1.0;
const MIN_MODAL_BACKDROP_BLUR_PX: f32 = 0.2;
const MODAL_MIN_SCALE: f32 = 0.94;
const MODAL_OPEN_DURATION: Duration = Duration::from_millis(240);
const MODAL_CLOSE_DURATION: Duration = Duration::from_millis(220);

fn modal_content_scale(progress: f32) -> f32 {
    MODAL_MIN_SCALE + (1.0 - MODAL_MIN_SCALE) * progress.clamp(0.0, 1.0)
}

fn frosted_backdrop_base(background: Hsla) -> Div {
    frosted_backdrop_base_with_overlay(background, 1.0)
}

fn frosted_backdrop_base_with_overlay(background: Hsla, progress: f32) -> Div {
    let progress = progress.clamp(0.0, 1.0);
    let overlay = Hsla {
        a: background
            .a
            .clamp(MODAL_BACKDROP_MIN_ALPHA, MODAL_BACKDROP_MAX_ALPHA)
            * progress,
        ..black()
    };

    let backdrop = div().absolute().inset_0().occlude();
    let blur_radius = px(DEFAULT_MODAL_BACKDROP_BLUR_PX * progress);
    if blur_radius >= px(MIN_MODAL_BACKDROP_BLUR_PX) {
        backdrop.backdrop_blur(
            BackdropBlurStyle::new(blur_radius)
                .downsample(2)
                .levels(3)
                .saturation(1.08)
                .tint(overlay),
        )
    } else {
        backdrop.bg(overlay)
    }
}

/// Fullscreen backdrop that intercepts mouse interaction "outside" a modal.
pub fn modal_backdrop(background: Hsla) -> Div {
    intercepting_backdrop(frosted_backdrop_base(background))
}

/// Animated fullscreen backdrop for modal open/close transitions.
pub fn animated_modal_backdrop(background: Hsla, progress: f32) -> Div {
    intercepting_backdrop(frosted_backdrop_base_with_overlay(background, progress))
}

fn default_modal_content_offset(progress: f32, visible: bool) -> Pixels {
    if visible {
        px((1.0 - progress) * 14.0)
    } else {
        px((1.0 - progress) * 10.0)
    }
}

fn intercepting_backdrop(backdrop: Div) -> Div {
    // Prevent hover state changes and mouse interactions for hitboxes behind the backdrop.
    backdrop
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
}

/// Places `content` on top of a fullscreen modal layer and animates the backdrop
/// and content with the same progress value.
pub fn animated_modal_layer(
    content: impl IntoElement,
    background: Hsla,
    progress: f32,
    visible: bool,
) -> Div {
    let progress = progress.clamp(0.0, 1.0);
    let content_offset = default_modal_content_offset(progress, visible);
    div()
        .absolute()
        .inset_0()
        .child(animated_modal_backdrop(background, progress))
        .child(
            div()
                .absolute()
                .inset_0()
                .p(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .child(
                    div()
                        .percentage_passthrough()
                        .mt(content_offset)
                        .scale(modal_content_scale(progress))
                        .opacity(progress)
                        .child(content),
                ),
        )
}

/// Like [`animated_modal_layer`], but allows callers to keep custom vertical motion.
pub fn animated_modal_layer_with_content_offset(
    content: impl IntoElement,
    background: Hsla,
    progress: f32,
    content_offset_y: Pixels,
) -> Div {
    let progress = progress.clamp(0.0, 1.0);
    div()
        .absolute()
        .inset_0()
        .child(animated_modal_backdrop(background, progress))
        .child(
            div()
                .absolute()
                .inset_0()
                .p(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .child(
                    div()
                        .percentage_passthrough()
                        .mt(content_offset_y)
                        .scale(modal_content_scale(progress))
                        .opacity(progress)
                        .child(content),
                ),
        )
}

pub fn modal_layer(content: impl IntoElement, background: Hsla) -> AnyElement {
    let animated_inner = div()
        .absolute()
        .inset_0()
        .p(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .child(content)
        .with_animation(
            "modal-layer-content-zoom",
            crate::ui::animation::ease_out_cubic_motion(std::time::Duration::from_millis(240)),
            |inner, progress| inner.scale(modal_content_scale(progress)),
        );

    div()
        .absolute()
        .inset_0()
        .child(modal_backdrop(background))
        .child(animated_inner)
        .with_animation(
            "modal-layer-fade",
            crate::ui::animation::ease_out_cubic_motion(std::time::Duration::from_millis(240)),
            |outer, progress| outer.opacity(progress),
        )
        .into_any_element()
}

/// Shared modal card shell used inside modal layers.
pub fn modal_surface(
    background: Hsla,
    border: Hsla,
    width: Pixels,
    height: Pixels,
    radius: Pixels,
) -> Div {
    div()
        .w(width)
        .h(height)
        .max_w(relative(1.0))
        .max_h(relative(1.0))
        .rounded(radius)
        .bg(background)
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .flex()
        .flex_col()
}

#[derive(Clone)]
pub struct ModalDismissHandle {
    control: Rc<Cell<ModalAnimationControl>>,
}

impl Default for ModalDismissHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl ModalDismissHandle {
    pub fn new() -> Self {
        Self {
            control: Rc::new(Cell::new(ModalAnimationControl::default())),
        }
    }

    pub fn dismiss(&self, cx: &mut App) {
        let mut control = self.control.get();
        control.begin_close(Instant::now());
        self.control.set(control);
        cx.refresh_windows();
    }

    pub(crate) fn control(&self) -> Rc<Cell<ModalAnimationControl>> {
        self.control.clone()
    }
}

/// Like [`modal_layer`], but clicking the backdrop dismisses the modal.
#[track_caller]
pub fn modal_layer_dismissible(
    content: impl IntoElement + 'static,
    background: Hsla,
    on_dismiss: Rc<dyn Fn(&mut App)>,
) -> AnyElement {
    modal_layer_dismissible_with_cleanup(content, background, Rc::new(|_: &mut App| {}), on_dismiss)
}

/// Like [`modal_layer_dismissible`], but allows callers to trigger close manually via handle.
#[track_caller]
pub fn modal_layer_dismissible_with_handle(
    handle: ModalDismissHandle,
    content: impl IntoElement + 'static,
    background: Hsla,
    on_dismiss: Rc<dyn Fn(&mut App)>,
) -> AnyElement {
    modal_layer_dismissible_with_handle_and_cleanup(
        handle,
        content,
        background,
        Rc::new(|_: &mut App| {}),
        on_dismiss,
    )
}

/// Like [`modal_layer_dismissible`], but runs `on_cleanup` before `on_dismiss`.
#[track_caller]
pub fn modal_layer_dismissible_with_cleanup(
    content: impl IntoElement + 'static,
    background: Hsla,
    on_cleanup: Rc<dyn Fn(&mut App)>,
    on_dismiss: Rc<dyn Fn(&mut App)>,
) -> AnyElement {
    modal_layer_dismissible_with_handle_and_cleanup(
        ModalDismissHandle::new(),
        content,
        background,
        on_cleanup,
        on_dismiss,
    )
}

/// Like [`modal_layer_dismissible_with_cleanup`], but allows callers to trigger close manually via handle.
#[track_caller]
pub fn modal_layer_dismissible_with_handle_and_cleanup(
    handle: ModalDismissHandle,
    content: impl IntoElement + 'static,
    background: Hsla,
    on_cleanup: Rc<dyn Fn(&mut App)>,
    on_dismiss: Rc<dyn Fn(&mut App)>,
) -> AnyElement {
    let source_location = Location::caller();
    DismissibleModal {
        id: source_location.into(),
        content: Some(content),
        background,
        on_cleanup,
        on_dismiss,
        control: handle.control(),
        source_location,
    }
        .into_any_element()
}

#[derive(Clone, Copy, Debug, Default)]
struct ModalAnimationControl {
    progress: f32,
    closing_started_at: Option<Instant>,
    closing_from: f32,
}

impl ModalAnimationControl {
    fn begin_close(&mut self, now: Instant) {
        if self.closing_started_at.is_none() {
            self.closing_started_at = Some(now);
            self.closing_from = self.progress;
        }
    }

    fn sample(&mut self, now: Instant, opened_at: Instant) -> ModalAnimationSample {
        let (progress, animating) = if let Some(started_at) = self.closing_started_at {
            let raw = crate::ui::animation::raw_progress(now, started_at, MODAL_CLOSE_DURATION);
            (
                self.closing_from * (1.0 - crate::ui::animation::ease_in_cubic(raw)),
                raw < 1.0,
            )
        } else {
            let raw = crate::ui::animation::raw_progress(now, opened_at, MODAL_OPEN_DURATION);
            (crate::ui::animation::ease_out_cubic(raw), raw < 1.0)
        };
        self.progress = progress.clamp(0.0, 1.0);
        ModalAnimationSample {
            progress: self.progress,
            animating,
            close_completed: self.closing_started_at.is_some() && !animating,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ModalAnimationSample {
    progress: f32,
    animating: bool,
    close_completed: bool,
}

struct DismissibleModalState {
    opened_at: Instant,
    control: Rc<Cell<ModalAnimationControl>>,
    completion_fired: bool,
}

impl DismissibleModalState {
    fn take_close_completion(&mut self, close_completed: bool) -> bool {
        if !close_completed || self.completion_fired {
            return false;
        }
        self.completion_fired = true;
        true
    }
}

struct DismissibleModal<E> {
    id: ElementId,
    content: Option<E>,
    background: Hsla,
    on_cleanup: Rc<dyn Fn(&mut App)>,
    on_dismiss: Rc<dyn Fn(&mut App)>,
    control: Rc<Cell<ModalAnimationControl>>,
    source_location: &'static Location<'static>,
}

impl<E: IntoElement + 'static> IntoElement for DismissibleModal<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for DismissibleModal<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        Some(self.source_location)
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let global_id = global_id.expect("DismissibleModal always supplies an element id");
        let content = self
            .content
            .take()
            .expect("DismissibleModal layout is requested once per frame");
        let current_control = self.control.clone();

        window.with_element_state(global_id, |state, window| {
            let now = Instant::now();
            let mut state = state.unwrap_or_else(|| DismissibleModalState {
                opened_at: now,
                control: current_control.clone(),
                completion_fired: false,
            });
            if state.completion_fired {
                state.opened_at = now;
                state.control.set(ModalAnimationControl::default());
                state.completion_fired = false;
            }

            let mut control = state.control.get();
            let sample = control.sample(now, state.opened_at);
            current_control.set(control);
            state.control = current_control.clone();

            let mut element =
                dismissible_modal_layer(content, self.background, sample.progress, current_control)
                    .into_any_element();
            let layout_id = element.request_layout(window, cx);

            if sample.animating || sample.close_completed {
                window.request_animation_engine_frame(AnimationDriver::Layout);
            }
            if state.take_close_completion(sample.close_completed) {
                (self.on_cleanup)(cx);
                (self.on_dismiss)(cx);
            }

            ((layout_id, element), state)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

fn dismissible_modal_layer(
    content: impl IntoElement,
    background: Hsla,
    progress: f32,
    control: Rc<Cell<ModalAnimationControl>>,
) -> Div {
    let dismiss_control = control.clone();
    div()
        .absolute()
        .inset_0()
        .child(
            frosted_backdrop_base_with_overlay(background, progress)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let mut control = dismiss_control.get();
                    control.begin_close(Instant::now());
                    dismiss_control.set(control);
                    window.request_animation_engine_frame(AnimationDriver::Layout);
                    cx.stop_propagation();
                })
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_mouse_move(|_, _, cx| cx.stop_propagation()),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .p(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .child(
                    div()
                        .percentage_passthrough()
                        .scale(modal_content_scale(progress))
                        .opacity(progress)
                        .child(content),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        DismissibleModalState, MODAL_CLOSE_DURATION, MODAL_OPEN_DURATION, ModalAnimationControl,
    };
    use std::{
        cell::Cell,
        rc::Rc,
        time::{Duration, Instant},
    };

    #[test]
    fn modal_opens_from_zero_to_one() {
        let opened_at = Instant::now();
        let mut control = ModalAnimationControl::default();

        assert_eq!(control.sample(opened_at, opened_at).progress, 0.0);
        let finished = control.sample(opened_at + MODAL_OPEN_DURATION, opened_at);
        assert_eq!(finished.progress, 1.0);
        assert!(!finished.animating);
    }

    #[test]
    fn modal_close_reverses_from_current_progress() {
        let opened_at = Instant::now();
        let mut control = ModalAnimationControl::default();
        let halfway = opened_at + MODAL_OPEN_DURATION / 2;
        let before_close = control.sample(halfway, opened_at).progress;

        control.begin_close(halfway);
        assert_eq!(control.sample(halfway, opened_at).progress, before_close);
        let finished = control.sample(halfway + MODAL_CLOSE_DURATION, opened_at);
        assert_eq!(finished.progress, 0.0);
        assert!(finished.close_completed);
    }

    #[test]
    fn repeated_close_does_not_restart_the_animation() {
        let opened_at = Instant::now();
        let mut control = ModalAnimationControl::default();
        control.sample(opened_at + MODAL_OPEN_DURATION, opened_at);
        let first_close = opened_at + MODAL_OPEN_DURATION;
        control.begin_close(first_close);
        control.begin_close(first_close + Duration::from_millis(80));

        assert_eq!(control.closing_started_at, Some(first_close));
        assert_eq!(control.closing_from, 1.0);
    }

    #[test]
    fn close_completion_is_consumed_once() {
        let opened_at = Instant::now();
        let mut state = DismissibleModalState {
            opened_at,
            control: Rc::new(Cell::new(ModalAnimationControl::default())),
            completion_fired: false,
        };

        assert!(state.take_close_completion(true));
        assert!(!state.take_close_completion(true));
    }
}
