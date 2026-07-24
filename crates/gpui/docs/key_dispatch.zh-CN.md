# Key Dispatch

[English](key_dispatch.md)

GPUI key dispatch 会连接 keyboard input、key bindings、focused elements 和 typed
actions。

## Flow

1. 平台把 keystroke 上报给 focused window。
2. GPUI 从 focused element tree 构建 active key context stack。
3. keymap 选择 predicate 匹配 context stack 的 bindings。
4. 选中的 action 通过 window 和 element action handlers dispatch。

key dispatch 行为应保持独立于 application policy。应用注册自己的 bindings，并决
定哪些 actions 是 global 或 contextual。

## Bindings

通过 `cx.bind_keys` 注册 bindings：

```rust
cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
```

mode-specific behavior 或 shortcut collisions 使用 contextual predicates。只有应在
整个应用生效的命令才使用宽泛的 global bindings。

## Action Handlers

把 commands 作为 typed actions 处理。Element action handlers 通常最接近它们要修
改的 state；global handlers 适合 quit 这样的 application commands。

避免从多个 action paths 同步修改同一个 entity。
