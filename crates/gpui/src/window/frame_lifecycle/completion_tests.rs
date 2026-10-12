use super::*;
use crate::{
    Animation, AnimationExt, Context, IntoElement, Render, Styled, TestAppContext, div, px,
};
use std::{cell::Cell, rc::Rc};

struct Owner {
    renders: Rc<Cell<usize>>,
}

impl Render for Owner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .size(px(20.))
            .bg(crate::rgb(0xff0000))
            .with_visual_animation(
                "motion",
                Animation::from_spec(
                    crate::AnimationSpec::new(Duration::from_millis(100))
                        .driver(AnimationDriver::Paint)
                        .fill_mode(crate::FillMode::Forwards),
                )
                .with_opacity(0.2, 1.),
            )
            .expect("opacity is a visual property")
    }
}

#[gpui::test]
fn ui_owned_visual_completion_renders_only_the_endpoint(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let (_, visual) = cx.add_window_view(|_, _| Owner {
        renders: renders.clone(),
    });
    let started_at = visual.update(|window, cx| {
        window.draw(cx).clear();
        assert!(window.test_present().is_accepted());
        window.animation_time()
    });
    let baseline = renders.get();
    visual.update(|window, cx| {
        window
            .animation_time
            .set(started_at + Duration::from_millis(50));
        window.request_animation_engine_frame(AnimationDriver::Paint);
        window.run_animation_engine_frame(false);
        window.draw(cx).clear();
    });
    assert_eq!(
        renders.get(),
        baseline,
        "intermediate samples must stay retained"
    );
    visual.update(|window, cx| {
        window
            .animation_time
            .set(started_at + Duration::from_millis(200));
        window.request_animation_engine_frame(AnimationDriver::Paint);
        window.run_animation_engine_frame(false);
        window.draw(cx).clear();
    });
    assert_eq!(renders.get(), baseline + 1);
    visual.update(|window, _| {
        assert!(
            !window.animation_engine.borrow().has_active_timelines(),
            "rendering the same forwards target must not restart its timeline"
        );
    });
}
