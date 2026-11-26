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
            .child(crate::div().size_full().child(self.growing.clone()))
            .child(
                crate::div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .w(px(80.))
                    .h(px(40.))
                    .overflow_hidden()
                    .child(crate::div().size_full().child(self.fixed.clone())),
            )
    }
}

#[gpui::test]
fn viewport_resize_rebuilds_views_and_preserves_input(cx: &mut TestAppContext) {
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
            fixed_renders.get() > fixed_before,
            "ordinary views must render even when their bounds stay fixed during resize"
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
                .child(crate::div().size_full().child(self.child.clone())),
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
        window
            .platform_window
            .as_test()
            .unwrap()
            .0
            .lock()
            .bounds
            .size = changed;
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

struct ResizeGeometryLeaf;

impl Render for ResizeGeometryLeaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                crate::div()
                    .w(crate::relative(0.6))
                    .h(px(37.))
                    .bg(crate::rgb(0xfa1234))
                    .child("Resize geometry"),
            )
    }
}

struct ResizeGeometryRoot {
    child: Entity<ResizeGeometryLeaf>,
}

impl Render for ResizeGeometryRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                crate::div()
                    .w(crate::relative(0.7))
                    .h(crate::relative(0.8))
                    .p(px(11.))
                    .child(crate::div().size_full().child(self.child.clone())),
            )
    }
}

#[gpui::test]
fn repeated_resize_keeps_cached_descendant_geometry_equal_to_fresh_layout(cx: &mut TestAppContext) {
    let (_, visual) = cx.add_window_view(|_, cx| ResizeGeometryRoot {
        child: cx.new(|_| ResizeGeometryLeaf),
    });
    visual.update(|window, cx| {
        let initial = window.viewport_size;
        let marker_bounds = |window: &Window| {
            window
                .rendered_frame
                .scene()
                .quads
                .iter()
                .filter(|quad| quad.background == crate::rgb(0xfa1234).into())
                .map(|quad| quad.bounds)
                .collect::<Vec<_>>()
        };
        let mut warmed = Vec::new();
        // Keep the same retained engine through every turn, including repeated sizes.
        for step in 0..=48 {
            let angle = (step % 16) as f32 * std::f32::consts::TAU / 16.;
            let next = size(
                initial.width + px((200. + 200. * angle.cos()).round()),
                initial.height + px((200. + 200. * angle.sin()).round()),
            );
            window
                .platform_window
                .as_test()
                .unwrap()
                .0
                .lock()
                .bounds
                .size = next;
            window.content_bounds_changed(cx);
            window.live_resize_active = true;
            window.draw(cx).clear();
            let bounds = marker_bounds(window);
            assert_eq!(bounds.len(), 1);
            warmed.push((next, bounds));
        }
        // The reference pass starts a fresh layout engine for each identical size.
        for (next, warm) in warmed {
            window
                .platform_window
                .as_test()
                .unwrap()
                .0
                .lock()
                .bounds
                .size = next;
            window.content_bounds_changed(cx);
            window.force_view_cache_refresh = true;
            window.layout_engine = Some(crate::layout::TaffyLayoutEngine::new());
            window.invalidator.set_dirty(true);
            window.draw(cx).clear();
            assert_eq!(
                warm,
                marker_bounds(window),
                "cached geometry differs at {next:?}"
            );
        }
    });
}

struct ScrollViewportView {
    scroll: crate::ScrollHandle,
    rendered_height: Rc<Cell<Pixels>>,
}

impl Render for ScrollViewportView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.rendered_height.set(self.scroll.bounds().size.height);
        crate::div()
            .id("tracked-viewport")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(crate::div().h(px(2000.)).flex_none())
    }
}

#[gpui::test]
fn tracked_viewport_resize_notifies_after_grow_and_shrink(cx: &mut TestAppContext) {
    let scroll = crate::ScrollHandle::new();
    let rendered_height = Rc::new(Cell::new(px(0.)));
    let (_, visual) = cx.add_window_view(|_, _| ScrollViewportView {
        scroll: scroll.clone(),
        rendered_height: rendered_height.clone(),
    });
    for height in [300., 700., 180., 600., 300.] {
        visual.update(|window, cx| {
            let new_size = size(px(800.), px(height));
            window
                .platform_window
                .as_test()
                .unwrap()
                .0
                .lock()
                .bounds
                .size = new_size;
            window.content_bounds_changed(cx);
            window.draw(cx).clear();
            assert_eq!(scroll.bounds().size.height, px(height));
        });
        assert_eq!(
            rendered_height.get(),
            px(height),
            "deferred notification must rebuild the final viewport"
        );
    }
}
