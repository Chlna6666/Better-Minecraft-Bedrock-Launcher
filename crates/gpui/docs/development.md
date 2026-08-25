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

## Rust Naming

Names rely on their module and type context. Add words only when they distinguish
one concept from another; do not repeat the path in every item.

- Prefer conventional domain modules such as `events`, `windows`, `assets`,
  `layout`, `scene`, and `webp`. Do not expand them to `event_observers`,
  `window_registry`, `image_decode`, or `raster_image_decoder` unless the added
  word identifies a genuinely separate abstraction.
- Modules and files name a cohesive domain or type family. Do not use
  `manager`, `service`, `handler`, `processor`, `helper`, `utils`, `common`,
  `decoder`, or `data` as a substitute for identifying the owned object.
- Types are nouns. Traits describe a capability. Functions are concise actions
  whose object is supplied by the surrounding module or receiver: within a
  WebP module prefer `dimensions`, `render`, or `frames` over `decode_webp_*`.
- Reserve `from_*` for direct, side-effect-free conversion constructors.
  Reading a path is `load` or `open`; sizing is an explicit argument or options
  type. Do not encode IO, format, policy, and target size into names such as
  `from_path_at_size`.
- Use plural modules for collections of peers (`events`, `windows`) and singular
  modules for one concept or algorithm (`window`, `layout`, `webp`). Follow
  standard-library and ecosystem terminology when it is already precise.
- Avoid both vague names and exhaustive names. `Options` alone is vague;
  `AnimatedImageConfig` is contextual and sufficient;
  `AnimatedRasterImageDecoderConfiguration` repeats implementation detail.
- A rename is complete only after code, tests, benches, examples, rustdoc, and
  re-exports use the new name. This development fork keeps no compatibility
  aliases, deprecated wrappers, or dual public paths.

For automated refactoring, first identify the object's domain and module, then
remove words already stated by that context, and finally check that the
remaining name still distinguishes sibling concepts. Search all call sites
before and after the change; never rename from string similarity alone.

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

## Independent Maintenance

This GPUI is maintained as an independent framework. Zed's GPUI is a comparison
source for established semantics and naming, not a runtime, source, or release
dependency. Upstream changes are reviewed selectively against a pinned commit;
they are not copied wholesale and never override local renderer, platform,
memory, or API decisions without local evidence.

Every imported idea must have a local owner, tests for its contract, benchmark
coverage when it claims performance, and documentation for any divergence.
Breaking changes migrate every repository caller in one change and expose one
authoritative API. A platform is listed as supported only after its configured
feature set compiles and its platform tests have actually run; source-level
`cfg` coverage alone is not support evidence.
