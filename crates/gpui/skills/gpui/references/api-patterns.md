# GPUI API Patterns

## Contexts

Use `App` as the root context. Use `Context<T>` while creating, updating,
rendering, or listening for events on an `Entity<T>`. Use the inner `cx`
provided to nested closures instead of an outer context.

Pass `Window` explicitly before `cx` when both are present. Use it for focus,
input state, action dispatch, frame requests, image-cache scope, and custom
element state.

## Entities

Use `Entity<T>` for strong state handles and `WeakEntity<T>` for callbacks,
subscriptions, and detached tasks that must not keep state alive.

Read through `read` or `read_with`; mutate through `update` or `update_in`.
Avoid updating an entity while it is already being updated.

Call `cx.notify()` after mutations that affect rendering.

## Rendering

Use `Render` for stateful views:

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().child("content")
    }
}
```

Use `RenderOnce` for consumed component values that do not own GPUI state. Use
`#[derive(IntoElement)]` when a component should be inserted directly into an
element tree.

## Events And Actions

Use `cx.listener` when an element event handler needs the current entity:

```rust
div().on_click(cx.listener(|this, _event, _window, cx| {
    this.active = true;
    cx.notify();
}))
```

Use typed actions for commands that can come from menus, keyboard shortcuts,
buttons, or programmatic dispatch. Register data-free actions with
`actions!(namespace, [Action])`.

## Async

Foreground app task:

```rust
cx.spawn(async move |cx| {
    cx.update(|cx| cx.refresh_windows())?;
    anyhow::Ok(())
}).detach_and_log_err(cx);
```

Foreground entity task:

```rust
cx.spawn(async move |handle, cx| {
    handle.update(cx, |state, cx| {
        state.loaded = true;
        cx.notify();
    })?;
    anyhow::Ok(())
}).detach_and_log_err(cx);
```

Store a `Task<T>` when cancellation should be tied to an owner. Detach only when
the task should continue independently.
