//! KeyDispatch is where GPUI deals with binding actions to key events.
//!
//! The key pieces to making a key binding work are to define an action,
//! implement a method that takes that action as a type parameter,
//! and then to register the action during render on a focused node
//! with a keymap context:
//!
//! ```ignore
//! actions!(editor,[Undo, Redo]);
//!
//! impl Editor {
//!   fn undo(&mut self, _: &Undo, _window: &mut Window, _cx: &mut Context<Self>) { ... }
//!   fn redo(&mut self, _: &Redo, _window: &mut Window, _cx: &mut Context<Self>) { ... }
//! }
//!
//! impl Render for Editor {
//!   fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
//!     div()
//!       .track_focus(&self.focus_handle(cx))
//!       .key_context("Editor")
//!       .on_action(cx.listener(Editor::undo))
//!       .on_action(cx.listener(Editor::redo))
//!     ...
//!    }
//! }
//!```
//!
//! The keybindings themselves are managed independently by calling cx.bind_keys().
//! (Though mostly when developing Zed itself, you just need to add a new line to
//!  assets/keymaps/default-{platform}.json).
//!
//! ```ignore
//! cx.bind_keys([
//!   KeyBinding::new("cmd-z", Editor::undo, Some("Editor")),
//!   KeyBinding::new("cmd-shift-z", Editor::redo, Some("Editor")),
//! ])
//! ```
//!
//! With all of this in place, GPUI will ensure that if you have an Editor that contains
//! the focus, hitting cmd-z will Undo.
//!
//! In real apps, it is a little more complicated than this, because typically you have
//! several nested views that each register keyboard handlers. In this case action matching
//! bubbles up from the bottom. For example in Zed, the Workspace is the top-level view, which contains Pane's, which contain Editors. If there are conflicting keybindings defined
//! then the Editor's bindings take precedence over the Pane's bindings, which take precedence over the Workspace.
//!
//! In GPUI, keybindings are not limited to just single keystrokes, you can define
//! sequences by separating the keys with a space:
//!
//!  KeyBinding::new("cmd-k left", pane::SplitLeft, Some("Pane"))

mod actions;
mod dispatch;
mod tree;
mod types;

#[cfg(test)]
#[path = "key_dispatch_tests.rs"]
mod key_dispatch_tests;

pub(crate) use types::{DispatchActionListener, DispatchNodeId, DispatchTree, Replay};
