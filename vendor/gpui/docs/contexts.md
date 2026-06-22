# Contexts And Entities

[Chinese](contexts.zh-CN.md)

GPUI state lives in entities and is accessed through context objects. Rendering,
event handling, async work, and tests should use the narrowest context that
matches the operation.

## `App`

`App` is the root context. Use it to:

- create entities with `cx.new`;
- open and manage windows;
- register actions, menus, and key bindings;
- set globals, assets, and the HTTP client;
- spawn foreground or background tasks;
- notify entities by id when window-level code has to trigger a render.

The prelude imports the traits needed for extension-style methods, so most
application code can import `gpui::prelude::*` instead of naming those traits
directly.

## `Context<T>`

`Context<T>` is provided while creating, updating, rendering, or listening for
events on an `Entity<T>`. Use the `Context<T>` passed into the current closure.
Do not keep using an outer `cx` after a nested read or update closure receives a
new context.

Common operations:

- `cx.entity()` returns the current `Entity<T>`.
- `cx.weak_entity()` returns a `WeakEntity<T>`.
- `cx.notify()` schedules observers and rendering for the current entity.
- `cx.listener(...)` adapts an element callback so it can update the current
  entity.
- `cx.spawn(async move |handle, cx| ...)` starts foreground async work that can
  upgrade the weak entity handle later.

## `Window`

`Window` is explicit. Use it for:

- focus and tab navigation;
- input state and mouse or keyboard handlers;
- action dispatch from the focused element tree;
- frame requests and next-frame callbacks;
- layout, paint, and custom element state;
- image cache scoping;
- GPU surface creation and painting.

The conventional parameter order is `window` before `cx` when both are present.

## `Entity<T>` And `WeakEntity<T>`

`Entity<T>` is a strong handle to GPUI-owned state:

- `entity.read(cx)` returns `&T`;
- `entity.read_with(cx, |state, cx| ...)` returns the closure result;
- `entity.update(cx, |state, cx| ...)` mutates the state;
- `entity.update_in(cx, |state, window, cx| ...)` mutates with a window context;
- `entity.downgrade()` returns a `WeakEntity<T>`.

`WeakEntity<T>` avoids ownership cycles and is the right handle for detached
tasks, subscriptions, and callbacks that should fail gracefully after the entity
is dropped.

Do not recursively update an entity while it is already being updated. Structure
state changes so one entity update owns the mutation, then notify observers or
dispatch follow-up work after the update returns.

## Events And Subscriptions

Entities that emit events implement `EventEmitter<Event>`. While updating the
entity, call `cx.emit(event)`.

Other entities subscribe with `cx.subscribe(entity, |this, emitter, event, cx| {
... })`. Store the returned `Subscription` in the owner entity so the
subscription is removed when the owner is dropped.
