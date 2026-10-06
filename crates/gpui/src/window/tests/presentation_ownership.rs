use super::*;

fn start_finite_scene_animation(window: &mut Window) -> crate::SceneAnimationCompletion {
    window.start_scene_animation(
        &test_global_element_id("native-owned-opacity"),
        TransitionProperty::Opacity,
        AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Gpu),
        Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0))),
        [0.0; 4],
        [1.0, 0.0, 0.0, 0.0],
    );
    let mut timelines = window.animation_engine.borrow().presentation_timelines();
    timelines[0]
        .sample_at(window.animation_time())
        .completion
        .expect("a bound finite scene timeline has a completion token")
}

#[gpui::test]
fn native_owner_ticks_ui_animation_without_querying_active_scene(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let test_window = window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        test_window.set_scene_animation_owner(true);
        window.animation_engine.borrow_mut().start_transition(
            &test_global_element_id("ui-owned-opacity"),
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Paint),
            window.animation_time(),
        );
        window.request_animation_engine_frame(AnimationDriver::Paint);
        test_window
    });
    test_window.simulate_request_frame(PlatformFrameRequest::animation_tick());
    window.run_until_parked();
    let query_count = test_window.scene_animation_query_count();
    let active_count = window.update(|window, _cx| window.animation_engine.borrow().active_count());
    window.update(|window, _cx| window.remove_window());
    drop(test_window);
    assert_eq!(query_count, 0);
    assert_eq!(active_count, 0);
}

#[gpui::test]
fn native_owner_keeps_pending_scene_animation_until_completion_ack(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let (test_window, completion) = window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        test_window.set_scene_animation_owner(true);
        test_window.set_frame_result(PlatformFrameResult::Queued);
        let completion = start_finite_scene_animation(window);
        (test_window, completion)
    });
    test_window.simulate_request_frame(PlatformFrameRequest::animation_tick());
    window.run_until_parked();
    let pending_count =
        window.update(|window, _cx| window.animation_engine.borrow().active_count());
    let query_count = test_window.scene_animation_query_count();
    let completed_count = window.update(|window, _cx| {
        window.presentation_animation_completed(completion);
        window.animation_engine.borrow().active_count()
    });
    window.update(|window, _cx| window.remove_window());
    drop(test_window);
    assert_eq!(pending_count, 1);
    assert_eq!(query_count, 0);
    assert_eq!(completed_count, 0);
}

#[gpui::test]
fn ui_owner_completes_scene_animation_without_native_ack(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let test_window = window.update(|window, _cx| {
        let test_window = window.platform_window.as_test().unwrap().clone();
        start_finite_scene_animation(window);
        test_window
    });
    test_window.simulate_request_frame(PlatformFrameRequest::animation_tick());
    window.run_until_parked();
    let query_count = test_window.scene_animation_query_count();
    let active_count = window.update(|window, _cx| window.animation_engine.borrow().active_count());
    window.update(|window, _cx| window.remove_window());
    drop(test_window);
    assert_eq!(query_count, 1);
    assert_eq!(active_count, 0);
}
