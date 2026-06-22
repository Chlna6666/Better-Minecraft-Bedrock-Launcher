# Examples

[Chinese](examples.zh-CN.md)

GPUI examples should be small, current, and compile under the standard example
check for the active platform.

## API Shape

Use the current API surface:

- `Application::new().run(|cx: &mut App| ...)`;
- `cx.open_window(options, |window, cx| cx.new(|cx| View { ... }))`;
- `impl Render for View { fn render(&mut self, window: &mut Window, cx:
  &mut Context<Self>) -> impl IntoElement { ... } }`;
- `Entity<T>` and `WeakEntity<T>` for state handles;
- `cx.listener(...)` for event handlers that mutate the current entity;
- `cx.spawn(async move |cx| ...)` or `cx.spawn(async move |handle, cx| ...)`
  for foreground async work.

Do not demonstrate obsolete names such as `Model<T>`, `View<T>`,
`ModelContext<T>`, `WindowContext`, or `ViewContext<T>`.

## Dependencies

Use dependencies already exported by GPUI where possible. Image examples should
use `gpui::http_client` for HTTP client setup and avoid references to
nonexistent crates.

Examples may use dev-dependencies declared by GPUI, such as `tobj` for model
loading or `bytemuck` for GPU buffers, when the example is explicitly about
that domain.

## Platform Guards

Examples that depend on a platform-specific API must:

- guard the implementation with `cfg`;
- keep a small fallback `main` for unsupported targets;
- avoid failing `cargo check --examples` on unsupported platforms;
- document the active platform in the example output or docs.

## Error Handling

Examples may print startup failures to stderr, but they should not hide errors
that make the example unusable. Prefer `if let Err(error) = ...` for window
creation and setup paths. Library-style helper functions should return
`Result`.

## Visual Scope

Examples should focus on GPUI framework capability. Do not add product routes,
application launch policy, default backgrounds, or application-specific window
chrome to GPUI examples.
