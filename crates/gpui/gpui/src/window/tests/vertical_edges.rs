use super::*;
use crate::{FillMode, ParentElement, ScrollHandle, Styled};

struct BarView {
    height: f32,
    scroll: ScrollHandle,
}

impl Render for BarView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .id("viewport")
            .w(px(60.))
            .h(px(60.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                crate::div()
                    .w(px(30.))
                    .h(px(self.height))
                    .flex_none()
                    .rounded(px(5.))
                    .bg(crate::white())
                    .with_visual_animation(
                        "bar",
                        Animation::from_spec(
                            AnimationSpec::new(Duration::from_millis(100))
                                .ease(Easing::Linear)
                                .fill_mode(FillMode::Both),
                        )
                        .with_property(AnimationProperty::vertical_edges(
                            [px(self.height), px(0.)],
                            [px(0.); 2],
                        )),
                    )
                    .expect("shape geometry is a visual property"),
            )
    }
}

#[gpui::test]
fn vertical_edges_completion_scroll_and_replay_keep_one_quad(cx: &mut TestAppContext) {
    let scroll = ScrollHandle::new();
    let (_, visual) = cx.add_window_view(|window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .set_scene_animation_owner(true);
        BarView {
            height: 140.,
            scroll: scroll.clone(),
        }
    });
    let completion = visual.update(|window, cx| {
        window.draw(cx).clear();
        assert!(window.test_present().is_accepted());
        assert_eq!(window.rendered_frame.scene().quads.len(), 1);
        window.animation_engine.borrow().presentation_timelines()[0]
            .clone()
            .sample_at(window.animation_time() + Duration::from_secs(1))
            .completion
            .unwrap()
    });
    visual.update(|window, cx| {
        window.presentation_animation_completed(completion);
        window.draw(cx).clear();
        assert!(!window.animation_engine.borrow().has_active_timelines());
    });
    for offset in [-20., -60., 0.] {
        scroll.set_offset(point(px(0.), px(offset)));
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
            let quads = &window.rendered_frame.scene().quads;
            assert_eq!(quads.len(), 1, "one object after completion/scroll");
            let quad = &quads[0];
            assert_eq!(
                quad.bounds.size.height,
                px(140.).scale(window.scale_factor())
            );
            assert_eq!(quad.bounds.size.width, px(30.).scale(window.scale_factor()));
            let values = window
                .animation_engine
                .borrow()
                .scene_values(window.animation_time());
            let value = values
                .iter()
                .find(|value| Some(value.animation_id) == quad.animation_id);
            assert!(value.is_none_or(crate::paint::animation_sample_is_identity));
            let expected = quad.clone();
            window.draw(cx).clear();
            assert_eq!(
                window.rendered_frame.scene().quads[0].bounds,
                expected.bounds
            );
            assert_eq!(
                window.rendered_frame.scene().quads[0].content_mask,
                expected.content_mask
            );
            assert!(!window.animation_engine.borrow().has_active_timelines());
        });
    }
}

#[gpui::test]
fn vertical_edges_retarget_compensates_final_height(cx: &mut TestAppContext) {
    let (view, visual) = cx.add_window_view(|_, _| BarView {
        height: 140.,
        scroll: ScrollHandle::new(),
    });
    let at = visual.update(|window, _| window.animation_time()) + Duration::from_millis(80);
    let before = visual.update(|window, _| {
        window.animation_time.set(at);
        let values = window.animation_engine.borrow().scene_values(at);
        let quad = &window.rendered_frame.scene().quads[0];
        crate::paint::animation_sampled_bounds(&Primitive::Quad(quad.clone()), values.first())
    });
    assert!(!before.is_empty());
    view.update(visual, |view, cx| {
        view.height = 180.;
        cx.notify();
    });
    visual.update(|window, cx| {
        window.animation_time.set(at);
        window.draw(cx).clear();
        let values = window.animation_engine.borrow().scene_values(at);
        assert_eq!(values.len(), 1);
        let quad = &window.rendered_frame.scene().quads[0];
        let after =
            crate::paint::animation_sampled_bounds(&Primitive::Quad(quad.clone()), values.first());
        assert_eq!(before, after, "retarget starts at the presented shape");
    });
}
