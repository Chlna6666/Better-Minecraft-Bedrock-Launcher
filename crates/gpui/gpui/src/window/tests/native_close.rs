use super::*;

#[gpui::test]
fn native_close_defers_app_access(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| EmptyTestView);
    let platform_window = cx.test_window(window.into());
    {
        let app = cx.app.borrow_mut();
        platform_window.simulate_close();
        assert!(app.windows.contains_key(window.window_id()));
    }
    cx.run_until_parked();
    assert!(!cx.app.borrow().windows.contains_key(window.window_id()));
}

#[gpui::test]
fn native_close_after_app_removal_is_harmless(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| EmptyTestView);
    let platform_window = cx.test_window(window.into());
    window
        .update(cx, |_, window, _| window.remove_window())
        .expect("window exists before removal");
    assert!(!cx.app.borrow().windows.contains_key(window.window_id()));
    platform_window.simulate_close();
    cx.run_until_parked();
    assert!(!cx.app.borrow().windows.contains_key(window.window_id()));
}
