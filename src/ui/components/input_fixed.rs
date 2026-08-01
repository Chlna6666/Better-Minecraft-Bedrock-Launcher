#[path = "input.rs"]
mod legacy;

pub use legacy::{InputEvent, InputSize, InputState, init};

use gpui::{
    App, Entity, EntityInputHandler, Focusable, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, Styled, Window, div,
};

/// BMCBL 输入框包装层。
///
/// Windows 的 winit 键盘事件会把已提交字符放入 `Keystroke::key_char`。当前自维护
/// GPUI 输入链在部分事件传播路径下不会把该字符提交给 `PlatformInputHandler`，表现为
/// Backspace/Delete 可用，但普通字符无法输入。这里在组件边界补齐字符提交，同时保留
/// 原组件的 IME、选区、剪贴板与视觉实现。
#[derive(IntoElement)]
pub struct Input {
    state: Entity<InputState>,
    inner: legacy::Input,
}

impl Input {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            inner: legacy::Input::new(state),
        }
    }

    pub fn appearance(mut self, appearance: bool) -> Self {
        self.inner = self.inner.appearance(appearance);
        self
    }

    pub fn bordered(mut self, bordered: bool) -> Self {
        self.inner = self.inner.bordered(bordered);
        self
    }

    pub fn focus_bordered(mut self, focus_bordered: bool) -> Self {
        self.inner = self.inner.focus_bordered(focus_bordered);
        self
    }

    pub fn cleanable(mut self, cleanable: bool) -> Self {
        self.inner = self.inner.cleanable(cleanable);
        self
    }

    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.inner = self.inner.prefix(prefix);
        self
    }

    pub fn with_size(mut self, size: InputSize) -> Self {
        self.inner = self.inner.with_size(size);
        self
    }
}

impl Styled for Input {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.inner.style()
    }
}

impl RenderOnce for Input {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus_handle = self.state.read(cx).focus_handle(cx);
        let state = self.state.clone();

        let mut root = div().track_focus(&focus_handle);

        #[cfg(target_os = "windows")]
        {
            root = root.on_key_down(move |event, window, cx| {
                let modifiers = event.keystroke.modifiers;

                // 保留常规快捷键；Ctrl+Alt 允许作为 AltGr 输入可打印字符。
                if modifiers.platform
                    || modifiers.function
                    || (modifiers.control && !modifiers.alt)
                    || (modifiers.alt && !modifiers.control)
                {
                    return;
                }

                let Some(text) = event.keystroke.key_char.as_deref() else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }

                state.update(cx, |input, cx| {
                    input.replace_text_in_range(None, text, window, cx);
                });

                // 防止 Window::dispatch_event 中的 Windows 兜底再次插入同一字符。
                cx.stop_propagation();
            });
        }

        root.child(self.inner)
    }
}
