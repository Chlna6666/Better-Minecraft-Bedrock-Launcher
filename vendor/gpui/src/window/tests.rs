use super::*;
use crate::{
    AnimationDriver, AnimationSequence, AnimationSpec, RepeatMode, TestAppContext,
    TransitionProperty, WindowOptions, performance_metrics_snapshot, point, px, size,
};

#[cfg(test)]
#[derive(Default)]
struct EmptyTestView;

#[cfg(test)]
impl Render for EmptyTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
    }
}

#[cfg(test)]
#[derive(Default)]
struct PaintedTestView;

#[cfg(test)]
impl Render for PaintedTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().w(px(1.)).h(px(1.)).bg(crate::white())
    }
}

#[cfg(test)]
#[derive(Default)]
struct ClickNotifyTestView {
    clicks: usize,
}

#[cfg(test)]
impl Render for ClickNotifyTestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .id("click-notify-target")
            .debug_selector(|| "click-notify-target".to_string())
            .w(px(100.))
            .h(px(100.))
            .on_click(cx.listener(|view, _, _, cx| {
                view.clicks += 1;
                cx.notify();
            }))
    }
}

fn test_global_element_id(name: &'static str) -> GlobalElementId {
    let mut path = SmallVec::new();
    path.push(ElementId::from(name));
    GlobalElementId(path)
}

#[cfg(test)]
impl Window {
    pub(crate) fn test_set_refreshing(&mut self, refreshing: bool) {
        self.refreshing = refreshing;
    }

    pub(crate) fn test_reset_before_first_frame(&mut self) {
        self.has_completed_rendered_frame = false;
        self.rendered_frame.clear();
        self.next_frame.clear();
        self.next_frame_callbacks.borrow_mut().clear();
        self.invalidator.set_phase(DrawPhase::None);
        self.invalidator.set_dirty(false);
        self.refreshing = false;
        self.dirty_frame_scheduled = false;
        self.dirty_frame_throttle_pending = false;
        self.dirty_frame_deferred_pending = false;
        self.needs_present.set(false);
        self.recovering_degraded_draw = false;
        self.last_input_timestamp
            .set(Instant::now() - Duration::from_secs(2));
        self.force_view_cache_refresh = true;
    }

    pub(crate) fn test_has_completed_rendered_frame(&self) -> bool {
        self.has_completed_rendered_frame
    }

    pub(crate) fn test_expire_draw_budget(&mut self) {
        self.has_completed_rendered_frame = true;
        self.recovering_degraded_draw = false;
        self.draw_deadline = Some(Instant::now() - Duration::from_millis(1));
    }

    pub(crate) fn test_inactive_animation_frame_pending(&self) -> bool {
        self.inactive_animation_frame_pending.get()
    }

    pub(crate) fn test_image_animation_frame_pending(&self) -> bool {
        !self.image_animation_deadline_pending.borrow().is_empty()
    }

    pub(crate) fn test_deadline_invalidation_pending(&self) -> bool {
        !self.deadline_invalidation_pending.borrow().is_empty()
    }

    pub(crate) fn test_dirty_frame_throttle_pending(&self) -> bool {
        self.dirty_frame_throttle_pending && self.invalidator.is_dirty()
    }

    pub(crate) fn test_dirty_frame_deferred_pending(&self) -> bool {
        self.dirty_frame_deferred_pending && self.invalidator.is_dirty()
    }

    pub(crate) fn test_dirty_frame_notify_invalidations(&self) -> usize {
        self.dirty_frame_diagnostics.borrow().notify_invalidations
    }

    pub(crate) fn test_dirty_frame_first_notify_entity(&self) -> Option<EntityId> {
        self.dirty_frame_diagnostics.borrow().first_notify_entity
    }

    pub(crate) fn test_recovering_degraded_draw(&self) -> bool {
        self.recovering_degraded_draw
    }

    pub(crate) fn test_complete_frame(
        &mut self,
        _drawn_frame_duration: Option<Duration>,
        _cx: &mut App,
    ) {
        self.complete_frame(FrameCompletion::Normal);
    }

    pub(crate) fn test_set_active_drag(&mut self, cx: &mut App, cursor_offset: Point<Pixels>) {
        cx.active_drag = Some(AnyDrag {
            value: Arc::new(()),
            view: cx.new(|_| EmptyTestView).into(),
            cursor_offset,
            cursor_style: None,
        });
    }

    pub(crate) fn test_request_image_animation_frame_at(
        &mut self,
        entity: EntityId,
        deadline: Instant,
        cx: &App,
    ) {
        self.invalidator.set_phase(DrawPhase::Paint);
        self.with_rendered_view(entity, |window| {
            window.request_image_animation_frame_at(
                deadline,
                cx,
                ImagePipelineConfig::default().animated,
            )
        });
        self.invalidator.set_phase(DrawPhase::None);
    }
}

#[gpui::test]
fn repeated_refresh_requests_are_coalesced(cx: &mut TestAppContext) {
    let before = performance_metrics_snapshot().coalesced_refresh_count;
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });

    window
        .update(cx, |_, window, _| {
            window.test_set_refreshing(false);
            window.refresh();
            window.refresh();
        })
        .unwrap();

    assert!(performance_metrics_snapshot().coalesced_refresh_count >= before + 1);
}

#[gpui::test]
fn paint_image_reuses_static_atlas_tile_cache(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });
    let image = Arc::new(
        RenderImage::from_raw_pixels(1, 1, RenderImagePixelFormat::Rgba8, vec![255, 0, 0, 255])
            .unwrap(),
    );

    window
        .update(cx, |_, window, _| {
            let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(1.0), px(1.0)));
            window.invalidator.set_phase(DrawPhase::Paint);

            window
                .paint_image(bounds, Corners::all(px(0.0)), image.clone(), 0, false)
                .unwrap();
            assert_eq!(window.image_paint_tile_cache.len(), 1);
            let first_tile = *window.image_paint_tile_cache.values().next().unwrap();

            window
                .paint_image(bounds, Corners::all(px(0.0)), image.clone(), 0, false)
                .unwrap();
            assert_eq!(window.image_paint_tile_cache.len(), 1);
            assert_eq!(
                window.image_paint_tile_cache.values().next().copied(),
                Some(first_tile)
            );

            window.drop_image(image).unwrap();
            assert!(window.image_paint_tile_cache.is_empty());
            window.invalidator.set_phase(DrawPhase::None);
        })
        .unwrap();
}

#[gpui::test]
fn paint_images_reuses_static_atlas_tile_cache(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });
    let image = Arc::new(
        RenderImage::from_raw_pixels(1, 1, RenderImagePixelFormat::Rgba8, vec![255, 0, 0, 255])
            .unwrap(),
    );

    window
        .update(cx, |_, window, _| {
            let first_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(1.0), px(1.0)));
            let second_bounds = Bounds::new(point(px(2.0), px(0.0)), size(px(1.0), px(1.0)));
            window.invalidator.set_phase(DrawPhase::Paint);

            window
                .paint_images([
                    ImagePaintRequest::new(first_bounds, image.as_ref()),
                    ImagePaintRequest::new(second_bounds, image.as_ref()),
                ])
                .unwrap();
            assert_eq!(window.image_paint_tile_cache.len(), 1);

            window.drop_image(image).unwrap();
            assert!(window.image_paint_tile_cache.is_empty());
            window.invalidator.set_phase(DrawPhase::None);
        })
        .unwrap();
}

#[gpui::test]
fn paint_images_budgeted_defers_uncached_images_after_the_frame_budget(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });
    let first_image = Arc::new(
        RenderImage::from_raw_pixels(1, 1, RenderImagePixelFormat::Rgba8, vec![255, 0, 0, 255])
            .unwrap(),
    );
    let second_image = Arc::new(
        RenderImage::from_raw_pixels(1, 1, RenderImagePixelFormat::Rgba8, vec![0, 255, 0, 255])
            .unwrap(),
    );

    window
        .update(cx, |_, window, _| {
            let first_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(1.0), px(1.0)));
            let second_bounds = Bounds::new(point(px(2.0), px(0.0)), size(px(1.0), px(1.0)));
            window.invalidator.set_phase(DrawPhase::Paint);

            let progress = window
                .paint_images_budgeted(
                    [
                        ImagePaintRequest::new(first_bounds, first_image.as_ref()),
                        ImagePaintRequest::new(second_bounds, second_image.as_ref()),
                    ],
                    1,
                )
                .unwrap();

            assert_eq!(progress.painted_requests, 1);
            assert_eq!(progress.deferred_requests, 1);
            assert_eq!(window.image_paint_tile_cache.len(), 1);
            window.invalidator.set_phase(DrawPhase::None);
        })
        .unwrap();
}

#[gpui::test]
fn repeated_refresh_windows_effects_are_coalesced(cx: &mut TestAppContext) {
    let before = performance_metrics_snapshot().coalesced_refresh_effect_count;

    cx.update(|cx| {
        cx.refresh_windows();
        cx.refresh_windows();
    });

    assert!(performance_metrics_snapshot().coalesced_refresh_effect_count >= before + 1);
}

#[gpui::test]
fn windows_are_mapped_before_becoming_visible_in_test_platform(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });

    let is_shown = window
        .update(cx, |_, window, _| {
            window
                .platform_window
                .as_test()
                .expect("test platform window")
                .is_shown()
        })
        .unwrap();

    assert!(is_shown);
}

#[gpui::test]
fn focused_windows_activate_after_map_in_test_platform(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        let mut options = WindowOptions::default();
        options.focus = true;
        options.show = true;
        cx.open_window(options, |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });

    let is_active = window
        .update(cx, |_, window, _| window.platform_window.is_active())
        .unwrap();

    assert!(is_active);
}

#[gpui::test]
fn test_platform_windows_are_mapped_before_becoming_active(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        let mut options = WindowOptions::default();
        options.focus = true;
        options.show = true;
        cx.open_window(options, |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });

    let shown = window
        .update(cx, |_, window, _| {
            window
                .platform_window
                .as_test()
                .expect("test window")
                .is_shown()
        })
        .unwrap();

    assert!(shown);
}

#[gpui::test]
fn inactive_image_animation_deadline_requests_are_coalesced(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });

    window
        .update(cx, |_, window, cx| {
            let entity = cx.entity_id();
            let deadline = Instant::now() + Duration::from_millis(10);
            window.test_request_image_animation_frame_at(entity, deadline, cx);
            window.test_request_image_animation_frame_at(
                entity,
                deadline + Duration::from_millis(5),
                cx,
            );
            assert!(window.test_image_animation_frame_pending());
        })
        .unwrap();
    cx.background_executor.allow_parking();
}

#[gpui::test]
fn active_image_animation_immediate_requests_are_coalesced(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
            .unwrap()
    });

    window
        .update(cx, |_, window, cx| {
            window.active.set(true);
            window.last_inactive_animation_frame.set(None);
            let entity = cx.entity_id();
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline = test_window.requested_frame_count();
            let deadline = Instant::now();
            window.test_request_image_animation_frame_at(entity, deadline, cx);
            window.test_request_image_animation_frame_at(entity, deadline, cx);

            assert_eq!(test_window.requested_frame_count(), baseline + 1);
        })
        .unwrap();
}

#[gpui::test]
fn drag_mouse_move_keeps_window_dirty(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        window.invalidator.set_dirty(false);
        window.test_set_active_drag(cx, point(px(4.), px(4.)));
        let event = MouseMoveEvent {
            position: point(px(8.), px(8.)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        };
        window.dispatch_mouse_event(&event, cx);
        assert!(window.invalidator.is_dirty());
    });
}

#[gpui::test]
fn pure_window_move_does_not_dirty_window(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        {
            let test_window = window.platform_window.as_test().unwrap();
            test_window.0.lock().bounds.origin = point(px(64.), px(64.));
        }

        window.invalidator.set_dirty(false);
        window.window_origin_changed(cx);

        assert!(!window.invalidator.is_dirty());
    });
}

#[gpui::test]
fn content_bounds_change_still_dirties_window(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        {
            let test_window = window.platform_window.as_test().unwrap();
            test_window.0.lock().bounds.size.width += px(1.);
        }

        window.invalidator.set_dirty(false);
        window.content_bounds_changed(cx);

        assert!(window.invalidator.is_dirty());
    });
}

#[gpui::test]
fn background_pointer_button_does_not_request_frame(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );

        assert_eq!(test_window.requested_frame_count(), baseline);
    });
}

#[gpui::test]
fn mouse_hit_test_uses_event_position(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();

        {
            let mut lock = test_window.0.lock();
            lock.bounds.size = size(px(120.), px(120.));
        }

        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(12.), px(12.)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );

        assert_eq!(window.mouse_hit_test.ids.len(), 0);
    });
}

#[gpui::test]
fn dirty_window_key_event_requests_frame_without_synchronous_draw(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.refreshing = false;
        window.invalidator.set_phase(DrawPhase::None);
        window.invalidator.set_dirty(true);

        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: Keystroke::parse("a").unwrap(),
                is_held: false,
            }),
            cx,
        );

        assert!(
            window.invalidator.is_dirty(),
            "key input should not synchronously redraw a dirty window"
        );
        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions::from_refresh())
        );
    });
}

#[gpui::test]
fn input_before_first_frame_requests_initial_frame_without_dispatch(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.test_reset_before_first_frame();
        let baseline = test_window.requested_frame_count();

        let result = window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: Keystroke::parse("a").unwrap(),
                is_held: false,
            }),
            cx,
        );

        assert!(result.propagate);
        assert!(!result.default_prevented);
        assert!(window.invalidator.is_dirty());
        assert!(!window.test_has_completed_rendered_frame());
        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions {
                require_presentation: false,
                force_render: true,
            })
        );
    });
}

#[gpui::test]
fn minimized_initial_frame_is_deferred(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.test_reset_before_first_frame();
        window.active.set(false);
        test_window.0.lock().shown = false;
        window.invalidator.set_dirty(true);
        window.refreshing = true;
        window.dirty_frame_scheduled = true;

        window.run_platform_frame(
            RequestFrameOptions {
                require_presentation: false,
                force_render: true,
            },
            cx,
        );
        assert!(window.invalidator.is_dirty());
        assert!(!window.refreshing);
        assert!(!window.test_has_completed_rendered_frame());
        assert_eq!(window.rendered_frame.scene.len(), 0);
    });
}

#[gpui::test]
fn window_control_drag_waits_for_drag_threshold(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(120.), px(32.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::Normal,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        window.rendered_frame.hitboxes.push(hitbox.clone());
        window
            .rendered_frame
            .window_control_hitboxes
            .push((WindowControlArea::Drag, hitbox));

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(10.), px(10.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        assert_eq!(test_window.start_window_move_count(), 0);

        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(13.), px(10.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        assert_eq!(test_window.start_window_move_count(), 1);
    });
}

#[gpui::test]
fn covered_window_control_drag_does_not_steal_mouse_down(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let drag_hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(120.), px(32.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::Normal,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        let foreground_hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(8.), px(4.)), size(px(48.), px(24.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::BlockMouse,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        window.rendered_frame.hitboxes.push(drag_hitbox.clone());
        window.rendered_frame.hitboxes.push(foreground_hitbox);
        window
            .rendered_frame
            .window_control_hitboxes
            .push((WindowControlArea::Drag, drag_hitbox));

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(10.), px(10.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );

        assert_eq!(test_window.start_window_move_count(), 0);
        assert!(
            cx.propagate_event,
            "foreground controls should receive mouse down instead of the drag area"
        );
    });
}

#[gpui::test]
fn transparent_caption_drag_uses_configured_height_without_hitbox(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.transparent_caption_height = Some(px(66.));

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(300.), px(12.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        assert_eq!(test_window.start_window_move_count(), 0);

        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(304.), px(12.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );

        assert_eq!(test_window.start_window_move_count(), 1);
    });
}

#[gpui::test]
fn transparent_caption_double_click_toggles_maximize_without_hitbox(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        window.transparent_caption_height = Some(px(66.));
        assert!(!window.is_maximized());

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(300.), px(12.)),
                modifiers: Modifiers::default(),
                click_count: 2,
                first_mouse: false,
            }),
            cx,
        );

        assert!(window.is_maximized());
    });
}

#[gpui::test]
fn transparent_caption_double_click_restores_after_maximize_without_hitbox(
    cx: &mut TestAppContext,
) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        window.transparent_caption_height = Some(px(66.));
        assert!(!window.is_maximized());

        let double_click = || {
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(300.), px(12.)),
                modifiers: Modifiers::default(),
                click_count: 2,
                first_mouse: false,
            })
        };

        window.dispatch_event(double_click(), cx);
        assert!(window.is_maximized());

        window.dispatch_event(double_click(), cx);
        assert!(!window.is_maximized());
    });
}

#[gpui::test]
fn transparent_caption_uses_observed_height_without_hitbox(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        window.transparent_caption_enabled = true;
        let hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(360.), px(60.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::Normal,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        window
            .rendered_frame
            .window_control_hitboxes
            .push((WindowControlArea::Drag, hitbox));
        window.observe_caption_height();
        window.rendered_frame.window_control_hitboxes.clear();

        assert!(!window.is_maximized());
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(300.), px(12.)),
                modifiers: Modifiers::default(),
                click_count: 2,
                first_mouse: false,
            }),
            cx,
        );

        assert!(window.is_maximized());
    });
}

#[gpui::test]
fn transparent_caption_drag_ignores_positions_below_configured_height(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.transparent_caption_height = Some(px(66.));

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(300.), px(80.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(304.), px(80.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );

        assert_eq!(test_window.start_window_move_count(), 0);
        assert!(
            cx.propagate_event,
            "content below the transparent caption should receive mouse events"
        );
    });
}

#[gpui::test]
fn refresh_requests_dirty_frame(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.refreshing = false;
        window.invalidator.set_phase(DrawPhase::None);
        window.refresh();

        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions::from_refresh())
        );
    });
}

#[gpui::test]
fn inactive_visible_dirty_frames_refresh_after_background_delay(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| PaintedTestView);
    let (test_window, baseline) = cx.update(|window, _| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.active.set(false);
        window.needs_present.set(false);
        window
            .last_input_timestamp
            .set(Instant::now() - Duration::from_secs(2));
        window.invalidator.set_dirty(false);
        (test_window, baseline)
    });

    cx.update(|window, _| {
        window.refresh();
        window.refresh();

        assert!(window.test_dirty_frame_deferred_pending());
        assert_eq!(test_window.requested_frame_count(), baseline);
        let retry_generation = window.frame_watchdog.get().generation;
        window.retry_deferred_dirty_frame(retry_generation);

        assert!(!window.test_dirty_frame_deferred_pending());
    });

    assert_eq!(test_window.requested_frame_count(), baseline + 1);
    assert_eq!(
        test_window.last_requested_frame(),
        Some(RequestFrameOptions {
            require_presentation: true,
            force_render: true,
        })
    );
}

#[gpui::test]
fn notify_on_rendered_view_requests_dirty_frame(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| EmptyTestView);
    let (test_window, baseline) = cx.update(|window, _| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        (test_window, baseline)
    });

    view.update(cx, |_, cx| cx.notify());

    assert!(test_window.requested_frame_count() > baseline);
    assert_eq!(
        test_window.last_requested_frame(),
        Some(RequestFrameOptions::from_refresh())
    );
}

#[gpui::test]
fn dirty_frame_diagnostics_record_notify_and_reset_on_complete(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| EmptyTestView);
    let entity_id = view.entity_id();

    view.update(cx, |_, cx| cx.notify());

    cx.update(|window, cx| {
        assert_eq!(window.test_dirty_frame_notify_invalidations(), 1);
        assert_eq!(
            window.test_dirty_frame_first_notify_entity(),
            Some(entity_id)
        );

        window.test_complete_frame(None, cx);

        assert_eq!(window.test_dirty_frame_notify_invalidations(), 0);
        assert_eq!(window.test_dirty_frame_first_notify_entity(), None);
    });
}

#[derive(Clone)]
struct TestUseAssetLoader;

impl Asset for TestUseAssetLoader {
    type Source = u64;
    type Output = u64;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let executor = cx.background_executor().clone();
        async move {
            executor.timer(Duration::from_millis(1)).await;
            source
        }
    }
}

struct UseAssetView {
    source: u64,
    loaded: bool,
}

impl Render for UseAssetView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.loaded = window
            .use_asset::<TestUseAssetLoader>(&self.source, cx)
            .is_some();
        crate::div().child(if self.loaded { "loaded" } else { "loading" })
    }
}

#[gpui::test]
fn completed_asset_load_requests_dirty_frame(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| UseAssetView {
        source: 1,
        loaded: false,
    });
    let (test_window, baseline) = cx.update(|window, _| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        (test_window, baseline)
    });

    cx.background_executor
        .advance_clock(Duration::from_millis(1));
    cx.run_until_parked();

    assert!(test_window.requested_frame_count() > baseline);
}

#[gpui::test]
fn completed_preloaded_asset_load_requests_dirty_frame(cx: &mut TestAppContext) {
    let mut preload_task = None;
    cx.update(|cx| {
        preload_task = Some(cx.fetch_asset::<TestUseAssetLoader>(&2).0);
    });
    let (_view, cx) = cx.add_window_view(|_, _| UseAssetView {
        source: 2,
        loaded: false,
    });
    let (test_window, baseline) = cx.update(|window, _| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        (test_window, baseline)
    });

    cx.background_executor
        .advance_clock(Duration::from_millis(1));
    cx.run_until_parked();

    drop(preload_task);
    assert!(test_window.requested_frame_count() > baseline);
}

#[gpui::test]
fn click_notify_requests_dirty_frame_without_animation(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, _| ClickNotifyTestView::default());
    let bounds = cx
        .debug_bounds("click-notify-target")
        .expect("click target should render");
    let (test_window, baseline) = cx.update(|window, _| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        (test_window, baseline)
    });

    cx.simulate_click(bounds.center(), Modifiers::default());

    view.read_with(cx, |view, _| assert_eq!(view.clicks, 1));
    assert!(test_window.requested_frame_count() > baseline);
    assert_eq!(
        test_window.last_requested_frame(),
        Some(RequestFrameOptions::from_refresh())
    );
}

#[gpui::test]
fn on_next_frame_requests_animation_frame(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.on_next_frame(|_, _| {});

        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            })
        );
    });
}

#[gpui::test]
fn animation_engine_frame_requests_are_coalesced(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        let element_id = test_global_element_id("animation-engine-coalesced");

        window.animation_engine.borrow_mut().start_transition(
            &element_id,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .driver(AnimationDriver::Paint),
            window.animation_time(),
        );
        window.request_animation_engine_frame(AnimationDriver::Paint);
        window.request_animation_engine_frame(AnimationDriver::Paint);

        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            })
        );
    });
}

#[gpui::test]
fn window_animation_group_api_starts_samples_and_cancels(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();

        let group_id = window.start_animation_sequence(AnimationSequence::new(vec![
            AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Paint),
        ]));

        assert!(window.sample_animation_group(group_id).is_some());
        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            })
        );
        assert!(window.cancel_animation_group(group_id));
        assert!(window.sample_animation_group(group_id).is_none());
    });
}

#[gpui::test]
fn paint_animation_engine_frame_does_not_notify_view(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let (test_window, baseline_requests, baseline_notify_invalidations) =
        window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline_requests = test_window.requested_frame_count();
            let baseline_notify_invalidations = window.test_dirty_frame_notify_invalidations();
            let element_id = test_global_element_id("animation-engine-paint");

            window.animation_engine.borrow_mut().start_transition(
                &element_id,
                TransitionProperty::Opacity,
                AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Paint),
                window.animation_time(),
            );
            window.request_animation_engine_frame(AnimationDriver::Paint);

            (
                test_window,
                baseline_requests,
                baseline_notify_invalidations,
            )
        });

    assert_eq!(test_window.requested_frame_count(), baseline_requests + 1);
    test_window.simulate_request_frame(RequestFrameOptions {
        require_presentation: true,
        force_render: false,
    });
    window.run_until_parked();

    window.update(|window, _cx| {
        assert_eq!(
            window.test_dirty_frame_notify_invalidations(),
            baseline_notify_invalidations
        );
        assert_eq!(window.animation_engine.borrow().active_count(), 0);
        assert_eq!(test_window.requested_frame_count(), baseline_requests + 1);
    });
}

#[gpui::test]
fn paint_animation_engine_frame_marks_precise_dirty_region(cx: &mut TestAppContext) {
    let (_view, window) = cx.add_window_view(|_, _| PaintedTestView);
    let dirty_bounds = Bounds::new(point(px(4.0), px(5.0)), size(px(10.0), px(12.0)));
    let (test_window, baseline_requests, baseline_notify_invalidations) =
        window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline_requests = test_window.requested_frame_count();
            let baseline_notify_invalidations = window.test_dirty_frame_notify_invalidations();
            let element_id = test_global_element_id("animation-engine-dirty-region");

            window.animation_engine.borrow_mut().start_transition(
                &element_id,
                TransitionProperty::Opacity,
                AnimationSpec::new(Duration::from_millis(100))
                    .repeat(RepeatMode::Forever)
                    .driver(AnimationDriver::Paint),
                window.animation_time(),
            );
            assert!(window.animation_engine.borrow_mut().set_transition_bounds(
                &element_id,
                TransitionProperty::Opacity,
                dirty_bounds
            ));
            window.request_animation_engine_frame(AnimationDriver::Paint);

            (
                test_window,
                baseline_requests,
                baseline_notify_invalidations,
            )
        });

    test_window.simulate_request_frame(RequestFrameOptions {
        require_presentation: true,
        force_render: false,
    });
    window.run_until_parked();

    window.update(|window, _cx| {
        assert_eq!(
            window.test_dirty_frame_notify_invalidations(),
            baseline_notify_invalidations
        );
        assert_eq!(window.render_present_mode, PartialPresentMode::Partial);
        assert_eq!(window.render_dirty_region.rect_count(), 1);
        assert_eq!(
            window.render_dirty_region.union_bounds(),
            Some(dirty_bounds.scale(window.scale_factor))
        );
        assert_eq!(test_window.requested_frame_count(), baseline_requests + 2);
    });
}

#[gpui::test]
fn layout_animation_engine_frame_uses_view_animation_frame(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();

        window.request_animation_engine_frame(AnimationDriver::Layout);

        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions {
                require_presentation: true,
                force_render: true,
            })
        );
    });
}

#[gpui::test]
fn inactive_request_animation_frame_requests_animation_frame(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.active.set(false);

        window.request_animation_frame();
        window.request_animation_frame();

        assert!(window.test_inactive_animation_frame_pending());
        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            })
        );
    });
}

#[gpui::test]
fn repeated_on_next_frame_requests_are_coalesced(cx: &mut TestAppContext) {
    let callbacks_ran = Rc::new(Cell::new(0));
    let window = cx.add_empty_window();
    let test_window =
        window.update(|window, _cx| window.platform_window.as_test().unwrap().clone());
    let baseline = test_window.requested_frame_count();

    window.update(|window, _cx| {
        for _ in 0..3 {
            let callbacks_ran = callbacks_ran.clone();
            window.on_next_frame(move |_, _| {
                callbacks_ran.set(callbacks_ran.get() + 1);
            });
        }
    });

    assert_eq!(test_window.requested_frame_count(), baseline + 1);
    test_window.simulate_request_frame(RequestFrameOptions {
        require_presentation: true,
        force_render: false,
    });
    cx.run_until_parked();
    assert_eq!(callbacks_ran.get(), 3);
}

#[gpui::test]
fn deadline_invalidation_does_not_request_immediate_frame(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();

        window.request_invalidation_at(Instant::now() + Duration::from_secs(1), cx);

        assert!(window.test_deadline_invalidation_pending());
        assert_eq!(test_window.requested_frame_count(), baseline);
    });
}

#[gpui::test]
fn deadline_invalidation_notifies_after_deadline(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| EmptyTestView);
    cx.update(|window, cx| {
        window.request_invalidation_at(Instant::now() + Duration::from_millis(50), cx);
    });

    cx.executor().advance_clock(Duration::from_millis(50));
    cx.run_until_parked();

    let test_window = cx.update(|window, _| window.platform_window.as_test().unwrap().clone());
    assert!(!cx.update(|window, _| window.test_deadline_invalidation_pending()));
    assert_eq!(
        test_window.last_requested_frame(),
        Some(RequestFrameOptions::from_refresh())
    );
}

#[gpui::test]
fn passive_mouse_move_does_not_extend_recent_input_present(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let stale_timestamp = Instant::now() - Duration::from_secs(2);
        window.last_input_timestamp.set(stale_timestamp);

        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(8.), px(8.)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );

        assert_eq!(window.last_input_timestamp.get(), stale_timestamp);
    });
}

#[gpui::test]
fn dragging_mouse_move_extends_recent_input_present(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let stale_timestamp = Instant::now() - Duration::from_secs(2);
        window.last_input_timestamp.set(stale_timestamp);

        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(8.), px(8.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );

        assert!(window.last_input_timestamp.get() > stale_timestamp);
        assert_ne!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            })
        );
    });
}

#[gpui::test]
fn background_pointer_button_does_not_extend_recent_input_present(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let stale_timestamp = Instant::now() - Duration::from_secs(2);
        window.last_input_timestamp.set(stale_timestamp);

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );

        assert_eq!(window.last_input_timestamp.get(), stale_timestamp);
    });
}

#[gpui::test]
fn inert_hitbox_pointer_button_does_not_extend_recent_input_present(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let stale_timestamp = Instant::now() - Duration::from_secs(2);
        window.last_input_timestamp.set(stale_timestamp);

        let hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::Normal,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        window.rendered_frame.hitboxes.push(hitbox);

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );

        assert_eq!(window.last_input_timestamp.get(), stale_timestamp);
    });
}

#[gpui::test]
fn handled_pointer_button_does_not_extend_recent_input_present(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let stale_timestamp = Instant::now() - Duration::from_secs(2);
        window.last_input_timestamp.set(stale_timestamp);

        let hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::Normal,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        window.rendered_frame.hitboxes.push(hitbox);
        window
            .rendered_frame
            .mouse_listeners
            .push(MouseListener::new::<MouseDownEvent>(Box::new(
                move |event, _, _, cx| {
                    if event.is::<MouseDownEvent>() {
                        cx.stop_propagation();
                    }
                },
            )));

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );

        assert_eq!(window.last_input_timestamp.get(), stale_timestamp);
    });
}

#[gpui::test]
fn passive_mouse_move_over_empty_area_skips_listener_dispatch(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let listener_calls = Rc::new(Cell::new(0));
        let listener_calls_clone = listener_calls.clone();
        window
            .rendered_frame
            .mouse_listeners
            .push(MouseListener::new::<MouseMoveEvent>(Box::new(
                move |event, _, _, _| {
                    if event.is::<MouseMoveEvent>() {
                        listener_calls_clone.set(listener_calls_clone.get() + 1);
                    }
                },
            )));

        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(8.), px(8.)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );

        assert_eq!(listener_calls.get(), 0);
    });
}

#[gpui::test]
fn passive_mouse_move_entering_hitbox_still_dispatches(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let listener_calls = Rc::new(Cell::new(0));
        let listener_calls_clone = listener_calls.clone();
        let hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::Normal,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        window.rendered_frame.hitboxes.push(hitbox);
        window
            .rendered_frame
            .mouse_listeners
            .push(MouseListener::new::<MouseMoveEvent>(Box::new(
                move |event, _, _, _| {
                    if event.is::<MouseMoveEvent>() {
                        listener_calls_clone.set(listener_calls_clone.get() + 1);
                    }
                },
            )));

        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(8.), px(8.)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );

        assert_eq!(listener_calls.get(), 2);
    });
}

#[gpui::test]
fn mouse_dispatch_skips_unrelated_listener_types(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let listener_calls = Rc::new(Cell::new(0));
        let listener_calls_clone = listener_calls.clone();
        let hitbox = Hitbox {
            id: window.next_hitbox_id,
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
            content_mask: ContentMask {
                bounds: Bounds::new(Point::default(), window.viewport_size),
            },
            behavior: HitboxBehavior::Normal,
        };
        window.next_hitbox_id = window.next_hitbox_id.next();
        window.rendered_frame.hitboxes.push(hitbox);
        window
            .rendered_frame
            .mouse_listeners
            .push(MouseListener::new::<MouseMoveEvent>(Box::new(
                move |_, _, _, _| {
                    listener_calls_clone.set(listener_calls_clone.get() + 1);
                },
            )));

        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(8.), px(8.)),
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );

        assert_eq!(listener_calls.get(), 0);
    });
}

#[gpui::test]
fn refresh_requests_force_render(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.refreshing = false;
        window.invalidator.set_phase(DrawPhase::None);
        window.refresh();

        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions::from_refresh())
        );
    });
}

#[gpui::test]
fn present_framebuffer_only_clears_needs_present(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.needs_present.set(true);

        window.present_framebuffer_only();

        assert!(!window.needs_present.get());
        assert_eq!(test_window.present_framebuffer_only_count(), 1);
    });
}

#[gpui::test]
fn clean_active_window_frame_skips_present(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        window.active.set(true);
        window.needs_present.set(false);
        window.invalidator.set_dirty(false);
        window.test_complete_frame(None, _cx);
    });
}

#[gpui::test]
fn clean_completed_frame_releases_dirty_frame_schedule_gate(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        window.dirty_frame_scheduled = true;
        window.invalidator.set_dirty(false);

        window.test_complete_frame(None, cx);

        assert!(!window.dirty_frame_scheduled);
        let baseline = test_window.requested_frame_count();
        window.invalidator.set_dirty(true);
        window.schedule_dirty_frame();
        assert_eq!(test_window.requested_frame_count(), baseline + 1);
    });
}

#[gpui::test]
fn retained_prepaint_reuse_rejects_invalid_frame_range(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let mut start = PrepaintStateIndex::default();
        start.hitboxes_index = 1;
        let end = start.clone();

        assert!(!window.reuse_prepaint(start..end));
    });
}

#[gpui::test]
fn retained_paint_reuse_rejects_invalid_frame_range(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let mut start = PaintIndex::default();
        start.scene_index = 1;
        let end = start.clone();

        assert!(!window.reuse_paint(start..end));
    });
}

#[gpui::test]
fn over_budget_completed_dirty_frame_schedules_next_frame(cx: &mut TestAppContext) {
    let visual = cx.add_empty_window();
    let (test_window, baseline) = visual.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.invalidator.set_dirty(true);
        window.refreshing = false;
        window.test_complete_frame(Some(DIRTY_FRAME_BACKPRESSURE_BUDGET), cx);
        assert!(!window.test_dirty_frame_throttle_pending());
        (test_window, baseline)
    });

    assert_eq!(test_window.requested_frame_count(), baseline + 1);
    assert_eq!(
        test_window.last_requested_frame(),
        Some(RequestFrameOptions::from_refresh())
    );
}

#[gpui::test]
fn slow_platform_draw_does_not_trigger_generation_backpressure(cx: &mut TestAppContext) {
    let visual = cx.add_empty_window();
    let test_window = visual.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        test_window.set_draw_delay(DIRTY_FRAME_BACKPRESSURE_BUDGET.saturating_mul(2));
        window.invalidator.set_dirty(true);
        window.refreshing = true;
        test_window
    });

    test_window.simulate_request_frame(RequestFrameOptions::from_refresh());

    assert!(!visual.update(|window, _| window.test_dirty_frame_throttle_pending()));
}

#[gpui::test]
fn deferred_dirty_frame_retry_rechecks_until_frame_can_schedule(cx: &mut TestAppContext) {
    let (_view, visual) = cx.add_window_view(|_, _| PaintedTestView);
    let (test_window, baseline) = visual.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.active.set(false);
        window.needs_present.set(false);
        window
            .last_input_timestamp
            .set(Instant::now() - Duration::from_secs(2));
        window.invalidator.set_dirty(false);
        window.refreshing = false;
        window.dirty_frame_scheduled = false;
        window.dirty_frame_throttle_pending = false;
        window.dirty_frame_deferred_pending = false;
        window.frame_watchdog.set(FrameWatchdog::default());
        (test_window, baseline)
    });

    visual.update(|window, _cx| {
        window.refresh();
        assert!(window.test_dirty_frame_deferred_pending());
        assert!(window.frame_watchdog.get().pending);

        let retry_generation = window.frame_watchdog.get().generation;
        window.active.set(true);
        window.retry_deferred_dirty_frame(retry_generation);

        assert!(!window.frame_watchdog.get().pending);
        assert!(!window.dirty_frame_deferred_pending);
        assert!(window.refreshing);
    });

    assert_eq!(test_window.requested_frame_count(), baseline + 1);
    assert_eq!(
        test_window.last_requested_frame(),
        Some(RequestFrameOptions::from_refresh())
    );
}

#[gpui::test]
fn deferred_dirty_frame_retry_does_not_self_rearm_while_still_deferred(cx: &mut TestAppContext) {
    let (_view, visual) = cx.add_window_view(|_, _| PaintedTestView);
    let (test_window, baseline) = visual.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.active.set(false);
        window.needs_present.set(false);
        window
            .last_input_timestamp
            .set(Instant::now() - Duration::from_secs(2));
        window.platform_window.minimize();
        window.invalidator.set_dirty(false);
        window.refreshing = false;
        window.dirty_frame_scheduled = false;
        window.dirty_frame_throttle_pending = false;
        window.dirty_frame_deferred_pending = false;
        window.frame_watchdog.set(FrameWatchdog::default());
        (test_window, baseline)
    });

    visual.update(|window, _cx| {
        window.refresh();
        assert!(window.test_dirty_frame_deferred_pending());
        assert!(window.frame_watchdog.get().pending);
        assert!(window.platform_window.is_minimized());

        let retry_generation = window.frame_watchdog.get().generation;
        window.retry_deferred_dirty_frame(retry_generation);

        assert!(window.test_dirty_frame_deferred_pending());
        assert!(!window.frame_watchdog.get().pending);
        assert!(!window.refreshing);
        assert!(!window.dirty_frame_scheduled);
    });

    assert_eq!(test_window.requested_frame_count(), baseline);
}

#[gpui::test]
fn stalled_platform_frame_request_recovers_by_running_frame(cx: &mut TestAppContext) {
    let visual = cx.add_empty_window();
    visual.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.active.set(true);
        window.invalidator.set_dirty(true);
        window.refreshing = false;
        window.dirty_frame_scheduled = false;
        window.dirty_frame_throttle_pending = false;
        window.dirty_frame_deferred_pending = false;
        window.frame_throttle.clear_delay();
        window.frame_watchdog.set(FrameWatchdog::default());

        window.schedule_dirty_frame();

        assert!(window.refreshing);
        assert!(window.dirty_frame_scheduled);
        assert!(window.frame_watchdog.get().platform_pending);
        assert!(window.platform_frame_watchdog_task.is_some());
        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        let stalled_generation = window.frame_watchdog.get().platform_generation;

        window.recover_stalled_platform_frame(stalled_generation, cx);

        assert!(!window.refreshing);
        assert!(!window.dirty_frame_scheduled);
        assert!(!window.frame_watchdog.get().platform_pending);
        assert!(window.platform_frame_watchdog_task.is_none());
        assert_eq!(
            window.frame_watchdog.get().platform_generation,
            stalled_generation
        );
        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        assert_eq!(
            test_window.last_requested_frame(),
            Some(RequestFrameOptions::from_refresh())
        );
    });
}

#[gpui::test]
fn completed_dirty_frame_does_not_arm_generation_backpressure(cx: &mut TestAppContext) {
    let visual = cx.add_empty_window();
    let (test_window, baseline) = visual.update(|window, cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        let baseline = test_window.requested_frame_count();
        window.invalidator.set_dirty(true);
        window.refreshing = false;
        window.test_complete_frame(Some(DIRTY_FRAME_BACKPRESSURE_BUDGET), cx);
        window.invalidator.set_dirty(false);
        (test_window, baseline)
    });

    visual.run_until_parked();

    assert_eq!(test_window.requested_frame_count(), baseline + 1);
    assert!(!visual.update(|window, _| window.test_dirty_frame_throttle_pending()));
}

#[gpui::test]
fn dirty_region_full_redraw_for_large_area(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _cx| {
        let viewport =
            Bounds::new(Point::default(), window.viewport_size).scale(window.scale_factor);
        let mut region = DirtyRegion::empty();
        region.push(viewport);

        region.coalesce_if_large(viewport, DIRTY_REGION_FULL_REDRAW_RATIO);

        assert!(region.is_full());
    });
}
