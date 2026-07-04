# GPUI Development Guide

[Chinese](development.zh-CN.md)

This guide defines the current GPUI API style for framework development,
examples, and downstream applications.

## Contexts

- Use `App` as the root context for global state, windows, menus, key bindings,
  assets, and platform services.
- Use `Context<T>` inside `Entity<T>` creation, updates, event listeners, and
  `Render` implementations. When a closure receives an inner `cx`, use that
  inner context instead of an outer one.
- Use `Window` explicitly for focus, input state, drawing, frame requests,
  actions, and window-local element state.
- Use `AsyncApp` and `AsyncWindowContext` only across await points.

Do not introduce obsolete application API names: `Model<T>`, `View<T>`,
`AppContext` as a context type, `ModelContext<T>`, `WindowContext`, or
`ViewContext<T>`.

## Entities And Rendering

`Entity<T>` is the state handle. Read with `read` or `read_with`; mutate with
`update` or `update_in`. Do not update an entity while it is already being
updated.

Views implement `Render`:

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().child("content")
    }
}
```

Use `RenderOnce` for components that are constructed only to become elements.
Call `cx.notify()` when state changes should affect rendering.

## Async Work

Foreground async work uses async closures:

```rust
cx.spawn(async move |cx| {
    gpui::Timer::after(std::time::Duration::from_millis(100)).await;
    cx.update(|cx| cx.refresh_windows())?;
    anyhow::Ok(())
}).detach_and_log_err(cx);
```

When spawning from `Context<T>`, the async closure receives the weak entity
handle first:

```rust
cx.spawn(async move |handle, cx| {
    handle.update(cx, |state, cx| {
        state.loaded = true;
        cx.notify();
    })?;
    anyhow::Ok(())
}).detach_and_log_err(cx);
```

Store or detach tasks that must continue after the current scope. Use
`background_spawn` for expensive work and propagate errors back to foreground
state.

## Renderer And Frames

`RendererOptions` carries backend, adapter, power, present mode, render policy,
and metrics preferences. `RendererBackend::Auto` chooses the platform default;
Windows supports explicit `NovaVulkan` and `NovaDx12`.

Use frame requests precisely:

- `force_render` means layout or paint scene state changed.
- `require_presentation` means prepared GPU content needs to
  be presented without necessarily rebuilding the scene.

The normal idle model is event driven. Continuous composition requires explicit
`RenderPolicy::Continuous`.

## GPU Surface Examples

Custom GPU examples should use current GPUI scene primitives and nova-gfx
renderer extension points. Keep platform-specific examples behind `cfg` guards
and provide a small fallback `main` for unsupported platforms.

## Lint And Documentation Rules

- Prefer fixing warnings over suppressing them.
- Use local `#[expect(..., reason = "...")]` only when code is intentionally
  platform-reserved or diagnostic-only.
- Avoid `unwrap` and `expect` in library code unless the invariant is immediate
  and obvious. Prefer `?`, `let Some(...) = ... else`, or explicit error
  handling.
- Keep comments for non-obvious reasoning, safety, platform constraints, or
  performance tradeoffs.
- Public APIs should have rustdoc that explains behavior, errors, panics, and
  safety obligations where relevant.
