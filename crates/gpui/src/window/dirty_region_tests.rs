use super::*;
use crate::{PlatformFrameRequest, TestAppContext, WindowOptions, point, px, size};

struct LocalDamageView {
    revision: usize,
}

impl Render for LocalDamageView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .absolute()
            .left(px(24.0))
            .top(px(32.0))
            .w(px(48.0))
            .h(px(36.0))
            .bg(if self.revision.is_multiple_of(2) {
                crate::red()
            } else {
                crate::blue()
            })
    }
}

struct FullWindowRootView {
    child: Entity<LocalDamageView>,
}

struct StableOutputView {
    revision: usize,
}

struct AnimationTickView {
    renders: usize,
}

impl Render for AnimationTickView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        crate::div().w(px(48.0)).h(px(36.0)).bg(crate::red())
    }
}

impl Render for StableOutputView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .absolute()
            .left(px(24.0))
            .top(px(32.0))
            .w(px(48.0))
            .h(px(36.0))
            .bg(crate::red())
    }
}

impl Render for FullWindowRootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .relative()
            .size_full()
            .bg(crate::black())
            .child(self.child.clone())
    }
}

#[gpui::test]
fn async_owner_refresh_preserves_sibling_window_caches(cx: &mut TestAppContext) {
    let windows: [AnyWindowHandle; 2] = cx.update(|app| {
        std::array::from_fn(|_| {
            app.open_window(
                WindowOptions {
                    focus: false,
                    ..WindowOptions::default()
                },
                |_, app| app.new(|_| AnimationTickView { renders: 0 }),
            )
            .expect("test window")
            .into()
        })
    });
    for handle in windows {
        cx.update_window(handle, |_, window, app| {
            window.draw(app).clear();
            window.test_complete_frame(None, app);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.force_view_cache_refresh());
        })
        .expect("initial draw");
    }
    cx.run_until_parked();
    let owner = windows[0];
    let sibling = windows[1];
    let task = cx.update(|app| {
        app.spawn(async move |mut app| {
            app.update(|app| {
                app.update_window(sibling, |_, window, _| {
                    assert!(!window.invalidator.is_dirty());
                    assert!(!window.force_view_cache_refresh());
                })
                .expect("clean sibling baseline");
                app.update_window(owner, |_, window, _| {
                    window.refresh();
                    assert!(window.invalidator.is_dirty());
                    assert!(window.force_view_cache_refresh());
                })
                .expect("owner window still exists");
                app.update_window(sibling, |_, window, _| {
                    assert!(!window.invalidator.is_dirty());
                    assert!(!window.force_view_cache_refresh());
                })
                .expect("sibling cache remains clean before frame processing");
            })
            .expect("application still exists");
        })
    });
    cx.run_until_parked();
    drop(task);
}

#[gpui::test]
fn child_notify_damages_child_without_promoting_root_bounds(cx: &mut TestAppContext) {
    let (child, root, window) = cx.update(|cx| {
        let child = cx.new(|_| LocalDamageView { revision: 0 });
        let window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| FullWindowRootView {
                    child: child.clone(),
                })
            })
            .expect("test window should open");
        let root = cx
            .read_window(&window, |root, _cx| root)
            .expect("test root should be readable");
        (child, root, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
    })
    .expect("initial frame should draw");

    cx.update(|cx| {
        child.update(cx, |child, cx| {
            child.revision = child.revision.saturating_add(1);
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();

            let child_bounds = window
                .rendered_frame
                .retained_scene_segments
                .iter()
                .find(|segment| segment.entity_id == child.entity_id())
                .map(|segment| segment.bounds)
                .expect("child should retain painted bounds");
            let root_bounds = window
                .rendered_frame
                .retained_scene_segments
                .iter()
                .find(|segment| segment.entity_id == root.entity_id())
                .map(|segment| segment.bounds)
                .expect("root should retain painted bounds");
            let viewport =
                Bounds::new(Point::default(), window.viewport_size).scale(window.scale_factor);

            assert_eq!(window.render_present_mode, PartialPresentMode::Partial);
            assert_eq!(
                window.render_dirty_region.union_bounds(),
                Some(child_bounds)
            );
            assert_ne!(child_bounds, root_bounds);
            assert_eq!(root_bounds, viewport);
            assert_eq!(
                child_bounds,
                Bounds::new(point(px(24.0), px(32.0)), size(px(48.0), px(36.0)))
                    .scale(window.scale_factor)
            );
        })
        .expect("notified frame should draw");
    });
}

#[gpui::test]
fn visually_identical_notify_skips_gpu_present(cx: &mut TestAppContext) {
    let (view, window) = cx.update(|cx| {
        let view = cx.new(|_| StableOutputView { revision: 0 });
        let window = cx
            .open_window(WindowOptions::default(), {
                let view = view.clone();
                move |_, _| view
            })
            .expect("test window should open");
        (view, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
        window.needs_present.set(false);
    })
    .expect("initial frame should draw");

    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.revision = view.revision.saturating_add(1);
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear();

            assert!(window.render_dirty_region.is_empty());
            assert!(!window.needs_present.get());
        })
        .expect("identical frame should be retained without presentation");
    });
}

#[gpui::test]
fn clean_animation_tick_does_not_render_view(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| AnimationTickView { renders: 0 })
        })
        .expect("test window should open")
    });
    cx.update_window(window.into(), |view, window, cx| {
        window.active.set(true);
        window.visibility = WindowVisibility::Visible;
        window.draw(cx).clear();
        assert!(window.present().is_accepted());
        window.invalidator.set_dirty(false);
        let view = view
            .downcast::<AnimationTickView>()
            .expect("test view type");
        let renders = view.read(cx).renders;
        let test_window = window.platform_window.as_test().unwrap().clone();
        let draws = test_window.draw_count();
        let presents = test_window.present_framebuffer_only_count();

        window.run_platform_frame(PlatformFrameRequest::animation_tick(), cx);

        assert_eq!(view.read(cx).renders, renders);
        assert_eq!(test_window.draw_count(), draws);
        assert_eq!(test_window.present_framebuffer_only_count(), presents + 1);
    })
    .expect("a clean UI animation tick must preserve retained view output");
}

#[gpui::test]
fn clean_on_next_frame_callback_runs_without_rendering_view(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| AnimationTickView { renders: 0 })
        })
        .expect("test window should open")
    });
    let callback_runs = std::rc::Rc::new(std::cell::Cell::new(0));

    cx.update_window(window.into(), |view, window, cx| {
        window.active.set(true);
        window.visibility = WindowVisibility::Visible;
        window.draw(cx).clear();
        assert!(window.present().is_accepted());
        window.invalidator.set_dirty(false);

        let view = view
            .downcast::<AnimationTickView>()
            .expect("test view type");
        let renders = view.read(cx).renders;
        let test_window = window.platform_window.as_test().unwrap().clone();
        let draws = test_window.draw_count();
        let presents = test_window.present_framebuffer_only_count();
        let callback_runs_for_frame = callback_runs.clone();

        window.on_next_frame(move |_, _| {
            callback_runs_for_frame.set(callback_runs_for_frame.get() + 1);
        });
        assert_eq!(
            test_window.last_requested_frame(),
            Some(PlatformFrameRequest::animation_tick())
        );

        window.run_platform_frame(PlatformFrameRequest::animation_tick(), cx);

        assert_eq!(callback_runs.get(), 1);
        assert_eq!(view.read(cx).renders, renders);
        assert_eq!(test_window.draw_count(), draws);
        assert_eq!(test_window.present_framebuffer_only_count(), presents + 1);
    })
    .expect("a clean on-next-frame callback should run without rebuilding the view");
}

#[gpui::test]
fn animation_metadata_commit_presents_without_pixel_damage(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| StableOutputView { revision: 0 })
        })
        .expect("test window should open")
    });

    cx.update_window(window.into(), |_, window, cx| {
        window.active.set(true);
        window.visibility = WindowVisibility::Visible;
        window.draw(cx).clear();
        assert!(window.present().is_accepted());
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.draw_count();

        // A timeline update can leave every retained primitive unchanged. Its handoff must
        // happen in this UI commit, without a later pointer event or presentation request.
        window.scene_animation_needs_commit.set(true);
        window.run_platform_frame(PlatformFrameRequest::ui_commit(), cx);

        assert!(window.render_dirty_region.is_empty());
        assert_eq!(test_window.draw_count(), baseline + 1);
        assert!(!window.presentation_state.has_pending_scene());
        assert!(!window.scene_animation_needs_commit.get());
        assert!(!window.needs_present.get());

        window.run_platform_frame(PlatformFrameRequest::ui_commit(), cx);
        assert_eq!(test_window.draw_count(), baseline + 1);
    })
    .expect("animation metadata should reach the compositor without input");
}

#[gpui::test]
fn animation_frame_presents_even_when_scene_diff_is_empty(cx: &mut TestAppContext) {
    let (view, window) = cx.update(|cx| {
        let view = cx.new(|_| StableOutputView { revision: 0 });
        let window = cx
            .open_window(WindowOptions::default(), {
                let view = view.clone();
                move |_, _| view
            })
            .expect("test window should open");
        (view, AnyWindowHandle::from(window))
    });

    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear();
        window.needs_present.set(false);
    })
    .expect("initial frame should draw");

    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.revision = view.revision.saturating_add(1);
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline = test_window.draw_count();

            window.run_platform_frame(PlatformFrameRequest::ui_commit_and_presentation(), cx);

            assert!(window.render_dirty_region.is_empty());
            assert_eq!(test_window.draw_count(), baseline + 1);
        })
        .expect("animation frame should present");
    });
}
