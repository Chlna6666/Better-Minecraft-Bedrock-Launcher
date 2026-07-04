# Rendering And Elements

[Chinese](rendering.zh-CN.md)

GPUI views build an element tree from application state. The tree is laid out
with flexbox-like style APIs and then painted by the renderer backend.

## `Render`

Use `Render` for stateful views stored in `Entity<T>`:

```rust
struct Counter {
    value: usize,
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(format!("Count: {}", self.value))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.value += 1;
                cx.notify();
            }))
    }
}
```

The render method receives both `Window` and `Context<Self>` because rendering
can depend on window-local state and can wire callbacks back into entity state.

## `RenderOnce`

Use `RenderOnce` for lightweight component values that are immediately consumed
as elements:

```rust
#[derive(IntoElement)]
struct Label {
    text: SharedString,
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(self.text)
    }
}
```

Prefer `RenderOnce` for reusable UI fragments that do not own GPUI state.

## Elements And Styling

Elements use builder-style methods for layout, paint, and interaction. The style
API intentionally resembles Tailwind vocabulary for spacing, flexbox, color,
border, text, and overflow.

Use conditional builders for optional structure:

- `.when(condition, |this| ...)`
- `.when_some(option, |this, value| ...)`
- `.children(iterator)`

Use `SharedString` for reusable text and `SharedUri` for reusable URI values.

## Custom Elements And Paint

Implement `Element` directly only when a normal element tree is not enough.
Custom elements are appropriate for:

- direct drawing calls;
- window-local element state;
- manual input routing;
- custom scene primitives;
- advanced layout or prepaint behavior.

Custom paint code must respect the layout bounds passed by GPUI, use `Window`
for drawing APIs, and keep expensive CPU or GPU setup out of per-frame paint
when possible.

## Invalidation

Call `cx.notify()` after mutating view state that affects rendering. Use
`window.refresh()` when window-level state or direct paint output needs another
frame. Prefer event-driven invalidation over continuous rendering unless the UI
is genuinely animation-heavy.
