use gpui::*;
use std::rc::Rc;

const DEFAULT_MODAL_BACKDROP_BLUR_PX: f32 = 0.01;

fn is_black_overlay(color: Hsla) -> bool {
    color.l <= 0.12 && color.s <= 0.20
}

fn frosted_backdrop_base(background: Hsla) -> Div {
    frosted_backdrop_base_with_overlay(background, true)
}

fn frosted_backdrop_base_with_overlay(background: Hsla, strengthen_black_overlay: bool) -> Div {
    let overlay = if strengthen_black_overlay && is_black_overlay(background) {
        Hsla {
            a: (background.a * 1.12).clamp(0.55, 0.78),
            ..background
        }
    } else {
        background
    };

    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(overlay)
        .backdrop_blur(
            BackdropBlurStyle::new(px(DEFAULT_MODAL_BACKDROP_BLUR_PX))
                .downsample(2)
                .levels(3)
                .saturation(1.08),
        )
}

/// Fullscreen backdrop that intercepts mouse interaction "outside" a modal.
pub fn modal_backdrop(background: Hsla) -> Div {
    intercepting_backdrop(frosted_backdrop_base(background))
}

/// Animated fullscreen backdrop for modal open/close transitions.
pub fn animated_modal_backdrop(background: Hsla, progress: f32) -> Div {
    let progress = progress.clamp(0.0, 1.0);
    let background = Hsla {
        a: background.a * progress,
        ..background
    };

    intercepting_backdrop(frosted_backdrop_base_with_overlay(background, false))
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
    animated_modal_layer_with_content_offset(
        content,
        background,
        progress,
        default_modal_content_offset(progress, visible),
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
                .p(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .child(div().mt(content_offset_y).opacity(progress).child(content)),
        )
}

/// Places `content` on top of a fullscreen modal layer with an intercepting backdrop.
pub fn modal_layer(content: impl IntoElement, background: Hsla) -> Div {
    div()
        .absolute()
        .inset_0()
        .child(modal_backdrop(background))
        .child(
            div()
                .absolute()
                .inset_0()
                .p(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .child(content),
        )
}

/// Like [`modal_layer`], but clicking the backdrop dismisses the modal.
pub fn modal_layer_dismissible(
    content: impl IntoElement,
    background: Hsla,
    on_dismiss: Rc<dyn Fn(&mut App)>,
) -> Div {
    modal_layer_dismissible_with_cleanup(content, background, Rc::new(|_: &mut App| {}), on_dismiss)
}

/// Like [`modal_layer_dismissible`], but runs `on_cleanup` before `on_dismiss`.
pub fn modal_layer_dismissible_with_cleanup(
    content: impl IntoElement,
    background: Hsla,
    on_cleanup: Rc<dyn Fn(&mut App)>,
    on_dismiss: Rc<dyn Fn(&mut App)>,
) -> Div {
    let dismiss_left = on_dismiss.clone();
    let cleanup_left = on_cleanup.clone();
    div()
        .absolute()
        .inset_0()
        .child(
            frosted_backdrop_base(background)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    (cleanup_left)(cx);
                    (dismiss_left)(cx);
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
                .p(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .child(content),
        )
}
