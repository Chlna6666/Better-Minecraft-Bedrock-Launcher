use std::time::Duration;

use gpui::{
    Animation, AnimationDirection, AnimationExt as _, AnimationGroup, AnimationProperty,
    AnimationSpec, HorizontalRevealEdge, Point, RepeatMode, SharedString, Spring, TransformOrigin,
    div, hsla, img, prelude::*, px, radians, rgb,
};

pub(crate) fn visual_cards(copies: usize, phase: bool) -> Vec<gpui::AnyElement> {
    let mut cards = Vec::with_capacity(copies.saturating_mul(12));
    for copy in 0..copies {
        let delay = Duration::from_millis((copy as u64 * 37) % 420);
        let duration = Duration::from_millis(900 + (copy as u64 % 7) * 130);

        cards.push(effect_card(
            "Opacity",
            animated(
                SharedString::from(format!("opacity-{copy}")),
                colored_sample(0x34d399, "opacity"),
                duration,
                delay,
                AnimationProperty::opacity(
                    if phase { 1.0 } else { 0.12 },
                    if phase { 0.12 } else { 1.0 },
                ),
            ),
        ));
        cards.push(effect_card(
            "Translation",
            animated(
                SharedString::from(format!("translation-{copy}")),
                colored_sample(0x60a5fa, "move"),
                duration,
                delay,
                AnimationProperty::translation(
                    point(px(if phase { 24.0 } else { 0.0 }), px(0.0)),
                    point(px(if phase { 0.0 } else { 24.0 }), px(0.0)),
                ),
            ),
        ));
        cards.push(effect_card(
            "Relative translation",
            animated(
                SharedString::from(format!("relative-translation-{copy}")),
                colored_sample(0xfbbf24, "relative"),
                duration,
                delay,
                AnimationProperty::relative_translation(
                    Point::new(if phase { 0.15 } else { 0.0 }, 0.0),
                    Point::new(if phase { 0.0 } else { 0.15 }, 0.0),
                ),
            ),
        ));
        cards.push(effect_card(
            "Uniform scale",
            animated(
                SharedString::from(format!("scale-{copy}")),
                colored_sample(0xf472b6, "scale"),
                duration,
                delay,
                AnimationProperty::uniform_scale(
                    if phase { 1.0 } else { 0.45 },
                    if phase { 0.45 } else { 1.0 },
                ),
            ),
        ));
        cards.push(effect_card(
            "Rotation",
            animated(
                SharedString::from(format!("rotation-{copy}")),
                colored_sample(0xa78bfa, "rotate"),
                duration,
                delay,
                AnimationProperty::rotation(
                    radians(if phase { std::f32::consts::TAU } else { 0.0 }),
                    radians(if phase { 0.0 } else { std::f32::consts::TAU }),
                ),
            ),
        ));
        cards.push(effect_card(
            "Blur",
            animated(
                SharedString::from(format!("blur-{copy}")),
                colored_sample(0x38bdf8, "blur subtree"),
                duration,
                delay,
                AnimationProperty::filter_blur(
                    px(if phase { 10.0 } else { 0.0 }),
                    px(if phase { 0.0 } else { 10.0 }),
                ),
            ),
        ));
        cards.push(effect_card(
            "Vertical reveal",
            animated(
                SharedString::from(format!("vertical-reveal-{copy}")),
                reveal_sample("vertical mask"),
                duration,
                delay,
                AnimationProperty::vertical_reveal(
                    gpui::VerticalRevealEdge::Top,
                    if phase { 1.0 } else { 0.05 },
                    if phase { 0.05 } else { 1.0 },
                ),
            ),
        ));
        cards.push(effect_card(
            "Horizontal reveal",
            animated(
                SharedString::from(format!("horizontal-reveal-{copy}")),
                reveal_sample("horizontal mask"),
                duration,
                delay,
                AnimationProperty::horizontal_reveal(
                    HorizontalRevealEdge::Left,
                    if phase { 1.0 } else { 0.05 },
                    if phase { 0.05 } else { 1.0 },
                ),
            ),
        ));
        cards.push(effect_card(
            "Captured image + text subtree",
            animated(
                SharedString::from(format!("captured-subtree-{copy}")),
                image_text_sample(),
                duration,
                delay,
                AnimationProperty::clipped_translation(
                    point(px(if phase { 0.0 } else { 20.0 }), px(0.0)),
                    point(px(if phase { 20.0 } else { 0.0 }), px(0.0)),
                ),
            ),
        ));
        cards.push(effect_card(
            "Scale + opacity / shared origin",
            animated(
                SharedString::from(format!("scale-opacity-{copy}")),
                colored_sample(0xf97316, "transform"),
                duration,
                delay,
                AnimationProperty::scale_opacity(
                    if phase { 1.0 } else { 0.6 },
                    if phase { 0.6 } else { 1.0 },
                    if phase { 1.0 } else { 0.25 },
                    if phase { 0.25 } else { 1.0 },
                    TransformOrigin::CENTER,
                ),
            ),
        ));
        cards.push(effect_card(
            "Translation + opacity",
            animated(
                SharedString::from(format!("translation-opacity-{copy}")),
                colored_sample(0x2dd4bf, "packed track"),
                duration,
                delay,
                AnimationProperty::translation_opacity(
                    point(px(if phase { 18.0 } else { 0.0 }), px(0.0)),
                    point(px(if phase { 0.0 } else { 18.0 }), px(0.0)),
                    if phase { 1.0 } else { 0.2 },
                    if phase { 0.2 } else { 1.0 },
                ),
            ),
        ));
        cards.push(effect_card("Spring", spring_sample(copy, phase)));
        cards.push(effect_card(
            "Three-track group",
            group_sample(copy, phase, duration, delay),
        ));
    }
    cards
}

fn animated(
    id: SharedString,
    element: impl IntoElement + 'static,
    duration: Duration,
    delay: Duration,
    property: AnimationProperty,
) -> gpui::AnyElement {
    let animation = continuous_animation(duration, delay).with_property(property);
    element
        .with_visual_animation(id, animation)
        .expect("gallery visual animation declares a presentation property")
        .into_any_element()
}

fn spring_sample(copy: usize, phase: bool) -> gpui::AnyElement {
    let from = if phase { 14.0 } else { 0.0 };
    let to = if phase { 0.0 } else { 14.0 };
    colored_sample(0xf87171, "spring")
        .with_visual_animation(
            SharedString::from(format!("spring-{copy}")),
            Animation::spring(Spring::default()).with_property(AnimationProperty::translation(
                point(px(from), px(0.0)),
                point(px(to), px(0.0)),
            )),
        )
        .expect("spring animation declares translation")
        .into_any_element()
}

fn group_sample(copy: usize, phase: bool, duration: Duration, delay: Duration) -> gpui::AnyElement {
    let (opacity_from, opacity_to) = if phase { (1.0, 0.15) } else { (0.15, 1.0) };
    let (translation_from, translation_to) = if phase {
        (point(px(12.0), px(0.0)), point(px(0.0), px(0.0)))
    } else {
        (point(px(0.0), px(0.0)), point(px(12.0), px(0.0)))
    };
    let group = AnimationGroup::parallel([
        Animation::from_spec(continuous_spec(duration, Duration::ZERO))
            .with_opacity(opacity_from, opacity_to),
        Animation::from_spec(continuous_spec(
            duration + Duration::from_millis(230),
            delay,
        ))
        .with_translation(translation_from, translation_to),
        Animation::from_spec(continuous_spec(
            duration + Duration::from_millis(470),
            Duration::ZERO,
        ))
        .with_scale(
            if phase { 1.0 } else { 0.65 },
            if phase { 0.65 } else { 1.0 },
        ),
    ])
    .expect("three unique visual properties fit one packed group");
    colored_sample(0x818cf8, "opacity + move + scale")
        .with_animation_group(SharedString::from(format!("group-{copy}")), group)
        .into_any_element()
}

fn colored_sample(color: u32, label: &'static str) -> impl IntoElement {
    div()
        .h(px(38.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(rgb(color))
        .px_2()
        .text_xs()
        .text_color(rgb(0xffffff))
        .child(label)
}

fn reveal_sample(label: &'static str) -> impl IntoElement {
    div()
        .w(px(152.0))
        .h(px(38.0))
        .overflow_hidden()
        .rounded_md()
        .bg(rgb(0x334155))
        .child(colored_sample(0x22c55e, label))
}

fn image_text_sample() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .h(px(42.0))
        .child(img("animation-perf-icon.png").size(px(32.0)))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0xf8fafc))
                .child("Image and glyph move together"),
        )
}

fn effect_card(label: &'static str, sample: gpui::AnyElement) -> gpui::AnyElement {
    div()
        .w(px(220.0))
        .h(px(104.0))
        .flex()
        .flex_col()
        .justify_between()
        .rounded_lg()
        .bg(rgb(0x1f2937))
        .p_3()
        .child(div().text_xs().text_color(rgb(0xcbd5e1)).child(label))
        .child(sample)
        .into_any_element()
}

pub(crate) fn layout_fallback_card() -> gpui::AnyElement {
    effect_card(
        "Layout fallback: width + height + inset + padding + gap",
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().size_6().rounded_sm().bg(rgb(0xfbbf24)))
            .child(div().size_6().rounded_sm().bg(rgb(0xf97316)))
            .with_animation(
                "layout-fallback",
                continuous_animation(Duration::from_millis(1300), Duration::ZERO),
                |element, progress| {
                    element
                        .w(px(96.0 + progress * 48.0))
                        .h(px(42.0 + progress * 18.0))
                        .left(px(progress * 12.0))
                        .p(px(2.0 + progress * 6.0))
                        .gap(px(4.0 + progress * 8.0))
                },
            )
            .into_any_element(),
    )
}

pub(crate) fn axis_comparison_card(phase: bool) -> gpui::AnyElement {
    div()
        .w(px(464.0))
        .h(px(184.0))
        .flex()
        .flex_col()
        .gap_2()
        .rounded_lg()
        .bg(rgb(0x1f2937))
        .p_3()
        .child(
            div()
                .text_xs()
                .text_color(rgb(0xcbd5e1))
                .child("0–12 logical px · 1300 ms · smoothstep · icon + glyph"),
        )
        .child(axis_track("X axis", MotionAxis::X, phase))
        .child(axis_track("Y axis", MotionAxis::Y, phase))
        .into_any_element()
}

pub(crate) fn spring_comparison_card(phase: bool) -> gpui::AnyElement {
    div()
        .w(px(464.0))
        .h(px(184.0))
        .flex()
        .flex_col()
        .gap_2()
        .rounded_lg()
        .bg(rgb(0x1f2937))
        .p_3()
        .child(
            div()
                .text_xs()
                .text_color(rgb(0xcbd5e1))
                .child("Spring X/Y · Spring::default() · no retarget · icon + glyph"),
        )
        .child(spring_axis_track("X axis", MotionAxis::X, phase))
        .child(spring_axis_track("Y axis", MotionAxis::Y, phase))
        .into_any_element()
}

#[derive(Clone, Copy)]
enum MotionAxis {
    X,
    Y,
}

fn axis_track(label: &'static str, axis: MotionAxis, phase: bool) -> gpui::AnyElement {
    let (from, to) = if phase { (12.0, 0.0) } else { (0.0, 12.0) };
    axis_comparison_track(
        label,
        axis,
        from,
        to,
        continuous_animation(Duration::from_millis(1300), Duration::ZERO),
        continuous_animation(Duration::from_millis(1300), Duration::ZERO),
    )
}

fn spring_axis_track(label: &'static str, axis: MotionAxis, phase: bool) -> gpui::AnyElement {
    let (from, to) = if phase { (12.0, 0.0) } else { (0.0, 12.0) };
    axis_comparison_track(
        label,
        axis,
        from,
        to,
        Animation::spring(Spring::default()),
        Animation::spring(Spring::default()),
    )
}

fn axis_comparison_track(
    label: &'static str,
    axis: MotionAxis,
    from: f32,
    to: f32,
    compositor_animation: Animation,
    layout_animation: Animation,
) -> gpui::AnyElement {
    let compositor = axis_sample(label, axis, from, to, false, compositor_animation);
    let layout = axis_sample(label, axis, from, to, true, layout_animation);

    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(38.0))
                .text_xs()
                .text_color(rgb(0xcbd5e1))
                .child(label),
        )
        .child(axis_motion_cell("Compositor", compositor))
        .child(axis_motion_cell("Layout", layout))
        .into_any_element()
}

fn axis_motion_cell(label: &'static str, sample: impl IntoElement + 'static) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(0x94a3b8)).child(label))
        .child(
            div()
                .relative()
                .w(px(170.0))
                .h(px(42.0))
                .overflow_hidden()
                .rounded_md()
                .bg(rgb(0x111827))
                .child(sample),
        )
}

fn axis_sample(
    label: &'static str,
    axis: MotionAxis,
    from: f32,
    to: f32,
    layout_driven: bool,
    animation: Animation,
) -> gpui::AnyElement {
    let element = div()
        .absolute()
        .left_0()
        .top_0()
        .h(px(28.0))
        .flex()
        .items_center()
        .gap_1()
        .rounded_sm()
        .bg(rgb(0x2563eb))
        .px_1()
        .child(img("animation-perf-icon.png").size(px(18.0)))
        .child(div().text_xs().text_color(rgb(0xffffff)).child("text"));
    let id = if layout_driven {
        SharedString::from(format!("axis-layout-{label}"))
    } else {
        SharedString::from(format!("axis-compositor-{label}"))
    };
    if layout_driven {
        element
            .with_animation(id, animation, move |element, progress| {
                let position = from + (to - from) * progress;
                match axis {
                    MotionAxis::X => element.left(px(position)),
                    MotionAxis::Y => element.top(px(position)),
                }
            })
            .into_any_element()
    } else {
        let translation = match axis {
            MotionAxis::X => {
                AnimationProperty::translation(point(px(from), px(0.0)), point(px(to), px(0.0)))
            }
            MotionAxis::Y => {
                AnimationProperty::translation(point(px(0.0), px(from)), point(px(0.0), px(to)))
            }
        };
        element
            .with_visual_animation(id, animation.with_property(translation))
            .expect("axis compositor animation declares translation")
            .into_any_element()
    }
}

pub(crate) fn color_fallback_card() -> gpui::AnyElement {
    effect_card(
        "Color callback fallback (UI sampled)",
        div()
            .h(px(38.0))
            .flex()
            .items_center()
            .child("Animated text color")
            .with_animation(
                "color-fallback",
                continuous_animation(Duration::from_millis(1600), Duration::ZERO),
                |element, progress| element.text_color(hsla(progress * 0.65, 0.8, 0.62, 1.0)),
            )
            .into_any_element(),
    )
}

fn point(x: gpui::Pixels, y: gpui::Pixels) -> Point<gpui::Pixels> {
    Point { x, y }
}

fn continuous_spec(duration: Duration, delay: Duration) -> AnimationSpec {
    AnimationSpec::new(duration)
        .delay(delay)
        .repeat(RepeatMode::Forever)
        .direction(AnimationDirection::Alternate)
}

fn continuous_animation(duration: Duration, delay: Duration) -> Animation {
    Animation::from_spec(continuous_spec(duration, delay))
        .with_easing(|progress| progress * progress * (3.0 - 2.0 * progress))
}
