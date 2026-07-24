# Key Dispatch

[Chinese](key_dispatch.zh-CN.md)

GPUI key dispatch connects keyboard input, key bindings, focused elements, and
typed actions.

## Flow

1. The platform reports a keystroke to the focused window.
2. GPUI builds the active key context stack from the focused element tree.
3. The keymap selects bindings whose predicate matches the context stack.
4. The selected action is dispatched through window and element action
   handlers.

Keep key dispatch behavior independent from application policy. Applications
register their own bindings and decide which actions are global or contextual.

## Bindings

Register bindings with `cx.bind_keys`:

```rust
cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
```

Use contextual predicates for mode-specific behavior or shortcut collisions.
Prefer broad global bindings only for commands that should work across the
entire application.

## Action Handlers

Handle commands as typed actions. Element action handlers are usually closest to
the state they mutate, while global handlers are appropriate for application
commands such as quit.

Avoid mutating the same entity synchronously from multiple action paths.
