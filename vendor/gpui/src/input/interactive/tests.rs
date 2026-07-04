use crate::{
    self as gpui, AppContext as _, Context, FocusHandle, InteractiveElement, IntoElement,
    KeyBinding, Keystroke, ParentElement, Render, TestAppContext, Window, div,
};

struct TestView {
    saw_key_down: bool,
    saw_action: bool,
    focus_handle: FocusHandle,
}

actions!(test_only, [TestAction]);

impl Render for TestView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().id("testview").child(
            div()
                .key_context("parent")
                .on_key_down(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.saw_key_down = true
                }))
                .on_action(
                    cx.listener(|this: &mut TestView, _: &TestAction, _, _| this.saw_action = true),
                )
                .child(
                    div()
                        .key_context("nested")
                        .track_focus(&self.focus_handle)
                        .into_element(),
                ),
        )
    }
}

#[gpui::test]
fn test_on_events(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| TestView {
                saw_key_down: false,
                saw_action: false,
                focus_handle: cx.focus_handle(),
            })
        })
        .unwrap()
    });

    cx.update(|cx| {
        cx.bind_keys(vec![KeyBinding::new("ctrl-g", TestAction, Some("parent"))]);
    });

    window
        .update(cx, |test_view, window, _cx| {
            window.focus(&test_view.focus_handle)
        })
        .unwrap();

    cx.dispatch_keystroke(*window, Keystroke::parse("a").unwrap());
    cx.dispatch_keystroke(*window, Keystroke::parse("ctrl-g").unwrap());

    window
        .update(cx, |test_view, _, _| {
            assert!(test_view.saw_key_down || test_view.saw_action);
            assert!(test_view.saw_key_down);
            assert!(test_view.saw_action);
        })
        .unwrap();
}
