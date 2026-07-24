---
name: gpui
description: "Use when writing, reviewing, documenting, or updating GPUI framework code, GPUI examples, or applications that use GPUI. Triggers include GPUI, App, Context, Window, Entity, Render, RenderOnce, RendererBackend, WindowOptions, GPUI docs, and GPUI examples."
---

# GPUI

Use this skill for GPUI framework work, examples, documentation, and downstream
GPUI application code.

## Core Workflow

1. Inspect the current GPUI source before changing code or examples.
2. Use the current API surface: `App`, `Context<T>`, explicit `Window`,
   `Entity<T>`, `WeakEntity<T>`, `Render`, and `RenderOnce`.
3. Keep framework code application-neutral. Do not add product routes, assets,
   launch policy, default backgrounds, or app window policy to GPUI.
4. Fix warnings directly where practical. Use local
   `#[expect(..., reason = "...")]` only for intentional compatibility or
   diagnostic code.
5. Validate with the focused GPUI format, check, clippy, examples, and docs
   searches before finishing.

## Required API Rules

- Use `App` as the root context and `Context<T>` inside entity creation,
  updates, listeners, and `Render` implementations.
- Pass `Window` explicitly when code needs focus, input state, drawing, frame
  requests, actions, or image-cache scope.
- Use `Entity<T>` and `WeakEntity<T>` for GPUI-owned state.
- Use `cx.spawn(async move |cx| ...)` from `App`.
- Use `cx.spawn(async move |handle, cx| ...)` from `Context<T>`.
- Use `window.spawn(cx, async move |cx| ...)` when async work is tied to a
  window.
- Use `cx.background_spawn` for expensive work and propagate errors to UI state.

Do not introduce obsolete application API names: `Model<T>`, `View<T>`,
`AppContext` as a concrete context type, `ModelContext<T>`, `WindowContext`, or
`ViewContext<T>`.

## Documentation Rules

- Write official GPUI docs in standalone library voice.
- Use English canonical docs with paired `.zh-CN.md` Chinese translations.
- Keep `SKILL.md` and all skill references English-only.
- Avoid describing GPUI official docs or skill content as a local vendored path.

## Reference Files

Load these only when relevant:

- `references/api-patterns.md`: contexts, entities, rendering, input, actions,
  and async patterns.
- `references/examples-lint-docs.md`: example rules, lint policy, docs
  validation, and expected commands.
