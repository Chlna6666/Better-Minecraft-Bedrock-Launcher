use super::*;
use crate::Styled;

struct SampledView {
    offset: f32,
    animating: bool,
}

#[gpui::test]
fn stable_sampled_static_nonidentity_sample_does_not_schedule_frames(cx: &mut TestAppContext) {
    let (_, visual) = cx.add_window_view(|_, _| SampledView {
        offset: 6.,
        animating: false,
    });
    visual.update(|window, _| {
        assert!(
            window.rendered_frame.scene().quads[0]
                .animation_id
                .is_some()
        );
        assert!(window.next_frame_callbacks.borrow().is_empty());
    });
}

impl Render for SampledView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .size(px(30.))
            .bg(crate::white())
            .with_stable_sampled_animation(
                "sampled",
                AnimationProperty::translation_opacity(
                    Point::default(),
                    point(px(self.offset), px(0.)),
                    1.,
                    0.6,
                ),
                1.,
                self.animating,
            )
    }
}

#[gpui::test]
fn stable_sampled_retarget_and_settle_keep_binding_and_final_sample(cx: &mut TestAppContext) {
    let (view, visual) = cx.add_window_view(|_, _| SampledView {
        offset: 12.,
        animating: true,
    });
    let id = visual.update(|window, _| window.rendered_frame.scene().quads[0].animation_id);
    for (offset, animating) in [(-3., true), (6., false)] {
        view.update(visual, |view, cx| {
            view.offset = offset;
            view.animating = animating;
            cx.notify();
        });
        visual.update(|window, cx| {
            window.draw(cx).clear();
            assert_eq!(window.rendered_frame.scene().quads[0].animation_id, id);
            let value = window
                .rendered_frame
                .scene()
                .animation_values
                .iter()
                .find(|value| Some(value.animation_id) == id)
                .unwrap_or_else(|| {
                    panic!(
                        "sample remains bound: offset={offset} id={id:?} values={:?}",
                        window.rendered_frame.scene().animation_values
                    )
                });
            assert_eq!(value.to[0], offset * window.scale_factor());
            assert_eq!(value.to[2], 0.6);
        });
    }
}
