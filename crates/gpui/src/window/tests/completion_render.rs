use super::*;
use crate::{AppContext, Context, Entity, ParentElement, Render, Styled, TestAppContext, div, px};

struct Leaf {
    renders: Rc<Cell<usize>>,
    target: f32,
    child: Entity<Content>,
}

struct Content {
    renders: Rc<Cell<usize>>,
}

impl Render for Content {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size(px(10.)).bg(crate::rgb(0x00ff00))
    }
}

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .size(px(20.))
            .bg(crate::rgb(0xff0000))
            .child(
                self.child
                    .clone()
                    .cached(crate::StyleRefinement::default().size(px(10.))),
            )
            .with_visual_animation(
                "motion",
                Animation::from_spec(
                    crate::AnimationSpec::new(Duration::from_millis(100))
                        .ease(crate::Easing::OutCubic)
                        .fill_mode(crate::FillMode::Forwards),
                )
                .with_opacity(0.2, self.target),
            )
            .expect("opacity is a visual property")
    }
}

struct Root {
    leaf: Entity<Leaf>,
    renders: Rc<Cell<usize>>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().child(
            self.leaf
                .clone()
                .cached(crate::StyleRefinement::default().size(px(20.))),
        )
    }
}

#[gpui::test]
fn visual_completion_renders_owner_once_and_preserves_ancestor(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let root_renders = Rc::new(Cell::new(0));
    let child_renders = Rc::new(Cell::new(0));
    let child = cx.new(|_| Content {
        renders: child_renders.clone(),
    });
    let leaf = cx.new(|_| Leaf {
        renders: renders.clone(),
        target: 1.,
        child,
    });
    let (root, visual) = cx.add_window_view(|window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .set_scene_animation_owner(true);
        Root {
            leaf: leaf.clone(),
            renders: root_renders.clone(),
        }
    });
    let completion = visual.update(|window, cx| {
        window.draw(cx).clear();
        assert!(window.test_present().is_accepted());
        let mut timeline = window.animation_engine.borrow().presentation_timelines()[0].clone();
        assert!(
            timeline
                .sample_at(window.animation_time() + Duration::from_millis(50))
                .completion
                .is_none()
        );
        timeline
            .sample_at(window.animation_time() + Duration::from_millis(200))
            .completion
            .unwrap()
    });
    let baseline = renders.get();
    let root_baseline = root_renders.get();
    let child_baseline = child_renders.get();
    visual.update(|window, cx| {
        window.presentation_animation_completed(completion.clone());
        window.draw(cx).clear();
        assert!(
            !window.animation_engine.borrow().has_active_timelines(),
            "rendering the same forwards target must not restart its timeline"
        );
    });
    assert_eq!(renders.get(), baseline + 1);
    assert_eq!(
        child_renders.get(),
        child_baseline + 1,
        "subtree completion must refresh nested caches below the animated target"
    );
    assert_eq!(
        root_renders.get(),
        root_baseline,
        "static ancestor must stay retained"
    );
    let settled = renders.get();
    visual.update(|window, cx| {
        window.presentation_animation_completed(completion);
        window.draw(cx).clear();
    });
    assert_eq!(
        renders.get(),
        settled,
        "duplicate completion must not render again"
    );
}

#[gpui::test]
fn retargeted_visual_completion_cannot_render_for_old_generation(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let root_renders = Rc::new(Cell::new(0));
    let child = cx.new(|_| Content {
        renders: Rc::new(Cell::new(0)),
    });
    let leaf = cx.new(|_| Leaf {
        renders: renders.clone(),
        target: 1.,
        child,
    });
    let (root, visual) = cx.add_window_view(|window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .set_scene_animation_owner(true);
        Root {
            leaf: leaf.clone(),
            renders: root_renders.clone(),
        }
    });
    let stale = visual.update(|window, cx| {
        window.draw(cx).clear();
        assert!(window.test_present().is_accepted());
        window.animation_engine.borrow().presentation_timelines()[0]
            .clone()
            .sample_at(window.animation_time() + Duration::from_millis(200))
            .completion
            .unwrap()
    });
    leaf.update(visual, |leaf, cx| {
        leaf.target = 0.;
        cx.notify();
    });
    let current = visual.update(|window, cx| {
        window.draw(cx).clear();
        let current = window.animation_engine.borrow().presentation_timelines()[0]
            .clone()
            .sample_at(window.animation_time() + Duration::from_millis(200))
            .completion
            .unwrap();
        current
    });
    assert_ne!(stale.generation, current.generation);
    let baseline = renders.get();
    visual.update(|window, cx| {
        window.presentation_animation_completed(stale);
        window.draw(cx).clear();
    });
    assert_eq!(renders.get(), baseline);
    visual.update(|window, cx| {
        window.presentation_animation_completed(current);
        window.draw(cx).clear();
    });
    assert_eq!(renders.get(), baseline + 1);
}
