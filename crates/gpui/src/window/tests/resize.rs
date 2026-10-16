use super::*;

struct ResizeCachedChild {
    renders: Rc<Cell<usize>>,
    clicks: Rc<Cell<usize>>,
}

impl Render for ResizeCachedChild {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        crate::div()
            .id("resize-cached-child")
            .size_full()
            .bg(crate::white())
            .on_click(cx.listener(|child, _, _, _| child.clicks.set(child.clicks.get() + 1)))
    }
}

struct ResizeCachedRoot {
    fixed: Entity<ResizeCachedChild>,
    growing: Entity<ResizeCachedChild>,
}

impl Render for ResizeCachedRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .relative()
            .size_full()
            .child(
                self.growing
                    .clone()
                    .cached(crate::StyleRefinement::default().size_full()),
            )
            .child(
                crate::div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .w(px(80.))
                    .h(px(40.))
                    .overflow_hidden()
                    .child(
                        self.fixed
                            .clone()
                            .cached(crate::StyleRefinement::default().size_full()),
                    ),
            )
    }
}

#[gpui::test]
fn viewport_resize_reuses_fixed_cached_child_and_repaints_growing_child(cx: &mut TestAppContext) {
    let fixed_renders = Rc::new(Cell::new(0));
    let fixed_clicks = Rc::new(Cell::new(0));
    let growing_renders = Rc::new(Cell::new(0));
    let (_, visual) = cx.add_window_view(|_, cx| ResizeCachedRoot {
        fixed: cx.new(|_| ResizeCachedChild {
            renders: fixed_renders.clone(),
            clicks: fixed_clicks.clone(),
        }),
        growing: cx.new(|_| ResizeCachedChild {
            renders: growing_renders.clone(),
            clicks: Rc::new(Cell::new(0)),
        }),
    });
    visual.update(|window, cx| {
        let fixed_before = fixed_renders.get();
        let growing_before = growing_renders.get();
        let old_size = window.viewport_size;
        let new_size = size(old_size.width + px(160.), old_size.height + px(80.));
        window.invalidator.set_dirty(false);
        window
            .platform_window
            .as_test()
            .unwrap()
            .0
            .lock()
            .bounds
            .size = new_size;
        window.content_bounds_changed(cx);
        window.live_resize_active = true;
        window.draw(cx).clear();
        assert_eq!(
            fixed_renders.get(),
            fixed_before,
            "unchanged clipped subtree must replay"
        );
        assert!(
            growing_renders.get() > growing_before,
            "changed constraints must rebuild"
        );
        assert!(
            window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .any(|quad| { quad.bounds.size == new_size.scale(window.scale_factor) }),
            "the committed scene must cover the new viewport"
        );

        let fixed_before_scale_change = fixed_renders.get();
        let mut native = window.platform_window.as_test().unwrap().clone();
        native.simulate_scale_factor_change(window.scale_factor * 2.);
        window.content_bounds_changed(cx);
        window.live_resize_active = true;
        window.draw(cx).clear();
        assert!(!window.draw_was_degraded(), "DPI frame was discarded");
        assert!(
            fixed_renders.get() > fixed_before_scale_change,
            "equal logical bounds must not reuse a scene from the previous DPI"
        );
        assert!(
            window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .any(|quad| { quad.bounds.size == new_size.scale(window.scale_factor) }),
            "scale={} viewport={:?} quads={:?}",
            window.scale_factor,
            window.viewport_size,
            window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .map(|quad| quad.bounds.size)
                .collect::<Vec<_>>()
        );
        click_painted_fixed_child(window, cx);
        assert_eq!(
            fixed_clicks.get(),
            1,
            "painted center must hit the cached child after DPI change"
        );
    });
}

fn click_painted_fixed_child(window: &mut Window, cx: &mut App) {
    let physical_size = size(px(80.), px(40.)).scale(window.scale_factor);
    let physical_position = window
        .rendered_frame
        .scene()
        .quads
        .iter()
        .find(|quad| quad.bounds.size == physical_size)
        .expect("fixed child must paint at the new physical DPI size")
        .bounds
        .center();
    let position = point(
        px(physical_position.x.0 / window.scale_factor),
        px(physical_position.y.0 / window.scale_factor),
    );
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
        cx,
    );
}
