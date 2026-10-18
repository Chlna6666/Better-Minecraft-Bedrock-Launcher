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
        assert!(
            fixed_renders.get() == fixed_before,
            "fixed cached content with unchanged bounds/context should replay across client resize"
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
        // Replaying cached geometry must also replay its dispatch tree and
        // HitTest data. Pixel equality alone does not prove correctness.
        click_painted_fixed_child(window, cx);
        assert_eq!(
            fixed_clicks.get(),
            1,
            "retained fixed child must still receive input after client-only Resize"
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
            2,
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

#[gpui::test]
fn repeated_window_resize_commits_exact_root_coverage_without_stale_cached_bounds(
    cx: &mut TestAppContext,
) {
    let (_, visual) = cx.add_window_view(|_, cx| {
        let fixed = cx.new(|_| ResizeCachedChild {
            renders: Rc::new(Cell::new(0)),
            clicks: Rc::new(Cell::new(0)),
        });
        let growing = cx.new(|_| ResizeCachedChild {
            renders: Rc::new(Cell::new(0)),
            clicks: Rc::new(Cell::new(0)),
        });
        ResizeCachedRoot { fixed, growing }
    });
    visual.update(|window, cx| {
        let initial = window.viewport_size;
        // Simulate a fast expand/shrink sequence across retained scene
        // generations. Each committed root background must cover its own
        // authoritative client, rather than the previous cached viewport.
        for (grow_w, grow_h) in [(260., 160.), (80., 40.), (340., 200.), (0., 0.)] {
            let next = size(initial.width + px(grow_w), initial.height + px(grow_h));
            window.platform_window.as_test().unwrap().0.lock().bounds.size = next;
            window.content_bounds_changed(cx);
            if grow_w != 0. || grow_h != 0. {
                assert!(
                    !window.force_view_cache_refresh(),
                    "pure client resize uses dependency-checked replay instead of a global cache barrier"
                );
            }
            window.draw(cx).clear();
            assert!(
                window.rendered_frame.scene().quads.iter().any(|quad| {
                    quad.bounds.origin == crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.))
                        && quad.bounds.size == next.scale(window.scale_factor)
                }),
                "native client {:?} was not covered by the committed scene",
                next,
            );
        }
    });
}

// Status-only native modal exit must not trigger a new full root cache
// generation. The geometry- and DPI-changing tests above still require a
// complete new-size draw before a Scene is presented.
#[gpui::test]
fn unchanged_native_size_move_state_does_not_dirty_root(cx: &mut TestAppContext) {
    let (_, visual) = cx.add_window_view(|_, cx| {
        let fixed = cx.new(|_| ResizeCachedChild {
            renders: Rc::new(Cell::new(0)),
            clicks: Rc::new(Cell::new(0)),
        });
        let growing = cx.new(|_| ResizeCachedChild {
            renders: Rc::new(Cell::new(0)),
            clicks: Rc::new(Cell::new(0)),
        });
        ResizeCachedRoot { fixed, growing }
    });
    visual.update(|window, cx| {
        window.invalidator.set_dirty(false);
        window.force_full_redraw.set(false);
        // TestPlatform reports non-modal. Simulate GPUI having observed the
        // preceding live-size flag without changing the native client rect.
        window.live_resize_active = true;
        window.content_bounds_changed(cx);
        assert!(!window.invalidator.is_dirty());
        assert!(!window.force_view_cache_refresh());
        assert!(!window.force_full_redraw.get());
        assert!(!window.live_resize_active);
    });
}

struct ViewportDependentFixedChild {
    renders: Rc<Cell<usize>>,
}

impl Render for ViewportDependentFixedChild {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        // Its outer cache bounds are fixed, but its content reads the native
        // viewport. This dependency forbids reuse on a client-size change.
        let viewport_width = window.viewport_size().width;
        crate::div()
            .id("viewport-dependent-fixed-child")
            .size_full()
            .bg(if viewport_width > px(1000.) {
                crate::black()
            } else {
                crate::white()
            })
    }
}

struct ViewportDependentFixedRoot {
    child: Entity<ViewportDependentFixedChild>,
}

impl Render for ViewportDependentFixedRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().relative().size_full().child(
            crate::div()
                .absolute()
                .left(px(0.))
                .top(px(0.))
                .w(px(80.))
                .h(px(40.))
                .child(
                    self.child
                        .clone()
                        .cached(crate::StyleRefinement::default().size_full()),
                ),
        )
    }
}

#[gpui::test]
fn viewport_reader_rebuilds_even_when_cached_child_bounds_are_fixed(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let (_, visual) = cx.add_window_view(|_, cx| ViewportDependentFixedRoot {
        child: cx.new(|_| ViewportDependentFixedChild {
            renders: renders.clone(),
        }),
    });
    visual.update(|window, cx| {
        let before = renders.get();
        assert!(before > 0, "the initial cached child must be rendered");
        let original = window.viewport_size;
        // Cross the content's viewport threshold in either initial window size.
        let changed = size(
            if original.width > px(1000.) {
                px(900.)
            } else {
                px(1100.)
            },
            original.height + px(90.),
        );
        window.platform_window.as_test().unwrap().0.lock().bounds.size = changed;
        window.content_bounds_changed(cx);
        window.draw(cx).clear();
        assert!(
            renders.get() > before,
            "viewport-dependent cached contents must be rebuilt even with fixed outer bounds"
        );
        let expected = if changed.width > px(1000.) {
            crate::black()
        } else {
            crate::white()
        };
        assert!(
            window.rendered_frame.scene().quads.iter().any(|quad| {
                quad.bounds.size == size(px(80.), px(40.)).scale(window.scale_factor)
                    && quad.background == expected.into()
            }),
            "the fixed child must paint its new viewport-dependent color"
        );
    });
}
