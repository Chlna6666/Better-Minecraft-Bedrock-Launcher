# Input, Actions, And Key Dispatch

[Chinese](input_and_actions.zh-CN.md)

GPUI routes user input through elements, window focus, key bindings, and typed
actions.

## Element Event Handlers

Register pointer and input handlers on elements:

```rust
div().on_click(|event, window, cx: &mut App| {
    window.dispatch_action(MyAction.boxed_clone(), cx);
})
```

When a handler needs the current entity, use `cx.listener`:

```rust
div().on_click(cx.listener(|this, _event, _window, cx| {
    this.selected = true;
    cx.notify();
}))
```

Use the inner `cx` passed to the listener body for mutations and notifications.

## Actions

Use `actions!(namespace, [ActionName])` for data-free actions. Use the action
derive for actions with payloads. Register global action handlers with
`cx.on_action`, window action handlers with `window.on_action`, and element
handlers with `.on_action(...)`.

Actions should be named for user intent rather than input device details.
Keyboard shortcuts, menu items, buttons, and programmatic dispatch can all
target the same action.

## Key Bindings

Register key bindings on the app:

```rust
cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
```

Use contextual predicates when a binding should only apply in a specific part of
the focused element tree. Keep fallback or global bindings broad and reserve
contextual bindings for collisions or mode-specific behavior.

## Focus

Use focus handles and explicit `Window` parameters for focus-sensitive code.
Actions dispatched through the focused element tree should be handled as close
to the owning view as practical, usually through `cx.listener`.

## Guidelines

- Prefer typed actions over ad hoc key checks in render code.
- Keep input handlers small; move durable state transitions into entity update
  methods.
- Avoid dispatching an action that synchronously re-enters the same entity
  update.
- Make platform-specific shortcuts explicit and test the Windows key path when
  touching examples.
