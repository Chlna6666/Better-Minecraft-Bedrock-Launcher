use crate::{
    self as gpui, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Style,
};
use core::panic;
use std::{cell::RefCell, ops::Range, rc::Rc};

use crate::{
    Action, ActionRegistry, App, Bounds, Context, DispatchTree, FocusHandle, InputHandler,
    IntoElement, KeyBinding, KeyContext, Keymap, Pixels, Point, Render, TestAppContext,
    Utf16Selection, Window,
};

#[derive(PartialEq, Eq)]
struct TestAction;

impl Action for TestAction {
    fn name(&self) -> &'static str {
        "test::TestAction"
    }

    fn name_for_type() -> &'static str
    where
        Self: ::std::marker::Sized,
    {
        "test::TestAction"
    }

    fn partial_eq(&self, action: &dyn Action) -> bool {
        action.as_any().downcast_ref::<Self>() == Some(self)
    }

    fn boxed_clone(&self) -> std::boxed::Box<dyn Action> {
        Box::new(TestAction)
    }

    fn build(_value: serde_json::Value) -> anyhow::Result<Box<dyn Action>>
    where
        Self: Sized,
    {
        Ok(Box::new(TestAction))
    }
}

#[test]
fn test_keybinding_for_action_bounds() {
    let keymap = Keymap::new(vec![KeyBinding::new(
        "cmd-n",
        TestAction,
        Some("ProjectPanel"),
    )]);

    let mut registry = ActionRegistry::default();

    registry.load_action::<TestAction>();

    let keymap = Rc::new(RefCell::new(keymap));

    let tree = DispatchTree::new(keymap, Rc::new(registry));

    let contexts = vec![
        KeyContext::parse("Workspace").unwrap(),
        KeyContext::parse("ProjectPanel").unwrap(),
    ];

    let keybinding = tree.bindings_for_action(&TestAction, &contexts);

    assert!(keybinding[0].action.partial_eq(&TestAction))
}

#[crate::test]
fn test_input_handler_pending(cx: &mut TestAppContext) {
    #[derive(Clone)]
    struct CustomElement {
        focus_handle: FocusHandle,
        text: Rc<RefCell<String>>,
    }
    impl CustomElement {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                text: Rc::default(),
            }
        }
    }
    impl Element for CustomElement {
        type RequestLayoutState = ();

        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            Some("custom".into())
        }
        fn source_location(&self) -> Option<&'static panic::Location<'static>> {
            None
        }
        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            (window.request_layout(Style::default(), [], cx), ())
        }
        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut Self::RequestLayoutState,
            window: &mut Window,
            cx: &mut App,
        ) -> Self::PrepaintState {
            window.set_focus_handle(&self.focus_handle, cx);
        }
        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut Self::RequestLayoutState,
            _: &mut Self::PrepaintState,
            window: &mut Window,
            cx: &mut App,
        ) {
            let mut key_context = KeyContext::default();
            key_context.add("Terminal");
            window.set_key_context(key_context);
            window.set_input_handler(&self.focus_handle, self.clone(), cx);
            window.on_action(std::any::TypeId::of::<TestAction>(), |_, _, _, _| {});
        }
    }
    impl IntoElement for CustomElement {
        type Element = Self;

        fn into_element(self) -> Self::Element {
            self
        }
    }

    impl InputHandler for CustomElement {
        fn selected_text_range(
            &mut self,
            _: bool,
            _: &mut Window,
            _: &mut App,
        ) -> Option<Utf16Selection> {
            None
        }

        fn marked_text_range(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
            None
        }

        fn text_for_range(
            &mut self,
            _: Range<usize>,
            _: &mut Option<Range<usize>>,
            _: &mut Window,
            _: &mut App,
        ) -> Option<String> {
            None
        }

        fn replace_text_in_range(
            &mut self,
            replacement_range: Option<Range<usize>>,
            text: &str,
            _: &mut Window,
            _: &mut App,
        ) {
            if replacement_range.is_some() {
                unimplemented!()
            }
            self.text.borrow_mut().push_str(text)
        }

        fn replace_and_mark_text_in_range(
            &mut self,
            replacement_range: Option<Range<usize>>,
            new_text: &str,
            _: Option<Range<usize>>,
            _: &mut Window,
            _: &mut App,
        ) {
            if replacement_range.is_some() {
                unimplemented!()
            }
            self.text.borrow_mut().push_str(new_text)
        }

        fn unmark_text(&mut self, _: &mut Window, _: &mut App) {}

        fn bounds_for_range(
            &mut self,
            _: Range<usize>,
            _: &mut Window,
            _: &mut App,
        ) -> Option<Bounds<Pixels>> {
            None
        }

        fn character_index_for_point(
            &mut self,
            _: Point<Pixels>,
            _: &mut Window,
            _: &mut App,
        ) -> Option<usize> {
            None
        }
    }
    impl Render for CustomElement {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.clone()
        }
    }

    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-b", TestAction, Some("Terminal"))]);
        cx.bind_keys([KeyBinding::new("ctrl-b h", TestAction, Some("Terminal"))]);
    });
    let (test, cx) = cx.add_window_view(|_, cx| CustomElement::new(cx));
    cx.update(|window, cx| {
        window.focus(&test.read(cx).focus_handle);
        window.activate_window();
    });
    cx.simulate_keystrokes("ctrl-b [");
    test.update(cx, |test, _| assert_eq!(test.text.borrow().as_str(), "["))
}
