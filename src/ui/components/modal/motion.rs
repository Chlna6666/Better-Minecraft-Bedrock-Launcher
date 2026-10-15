use super::*;

pub(super) fn transition(control: ModalAnimationControl) -> (f32, f32, Animation) {
    let (from, to, duration, easing) = if control.closing_started_at.is_some() {
        (
            control.closing_from,
            0.0,
            MODAL_CLOSE_DURATION,
            Easing::InCubic,
        )
    } else {
        (0.0, 1.0, MODAL_OPEN_DURATION, Easing::OutCubic)
    };
    let animation = Animation::from_spec(
        AnimationSpec::new(duration)
            .ease(easing)
            .fill_mode(FillMode::Forwards),
    );
    (from, to, animation)
}

pub(super) fn backdrop(background: Hsla, from: f32, to: f32, animation: Animation) -> Div {
    let overlay = Hsla {
        a: background
            .a
            .clamp(MODAL_BACKDROP_MIN_ALPHA, MODAL_BACKDROP_MAX_ALPHA),
        ..black()
    };
    div()
        .absolute()
        .inset_0()
        .occlude()
        .child(
            div()
                .absolute()
                .inset_0()
                .background_blur(crate::ui::theme::glass_backdrop_blur_style().saturation(1.05)),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .bg(overlay)
                .with_visual_animation(
                    "dismissible-modal-backdrop",
                    animation.with_property(AnimationProperty::opacity(from, to)),
                )
                .expect("opacity is a declared presentation property"),
        )
}
