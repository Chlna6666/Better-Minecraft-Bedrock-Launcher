use super::*;
use crate::{RequestFrameOptions, TestAppContext, WindowOptions, point, px, size};

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

            window.run_platform_frame(
                RequestFrameOptions {
                    require_presentation: true,
                    force_render: true,
                },
                cx,
            );

            assert!(window.render_dirty_region.is_empty());
            assert_eq!(test_window.draw_count(), baseline + 1);
        })
        .expect("animation frame should present");
    });
}
