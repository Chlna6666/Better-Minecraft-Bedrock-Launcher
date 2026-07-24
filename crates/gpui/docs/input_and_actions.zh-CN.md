# 输入、Actions 与 Key Dispatch

[English](input_and_actions.md)

GPUI 会通过 elements、window focus、key bindings 和 typed actions 路由用户输入。

## Element Event Handlers

在 elements 上注册 pointer 和 input handlers：

```rust
div().on_click(|event, window, cx: &mut App| {
    window.dispatch_action(MyAction.boxed_clone(), cx);
})
```

handler 需要当前 entity 时，使用 `cx.listener`：

```rust
div().on_click(cx.listener(|this, _event, _window, cx| {
    this.selected = true;
    cx.notify();
}))
```

在 listener body 中使用传入的内部 `cx` 来 mutation 和 notification。

## Actions

无数据 actions 使用 `actions!(namespace, [ActionName])`。带 payload 的 actions 使用
action derive。用 `cx.on_action` 注册 global action handlers，用 `window.on_action`
注册 window action handlers，用 `.on_action(...)` 注册 element handlers。

Action 应按用户意图命名，而不是按输入设备细节命名。键盘快捷键、menu items、
buttons 和 programmatic dispatch 都可以指向同一个 action。

## Key Bindings

在 app 上注册 key bindings：

```rust
cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
```

当 binding 只应在 focused element tree 的特定区域生效时，使用 contextual
predicates。fallback 或 global bindings 保持宽泛，contextual bindings 只用于冲突
或 mode-specific behavior。

## Focus

focus-sensitive code 使用 focus handles 和显式 `Window` 参数。通过 focused element
tree dispatch 的 actions 应尽量在 owning view 附近处理，通常通过 `cx.listener`。

## Guidelines

- 优先使用 typed actions，而不是在 render code 中做临时 key checks。
- input handlers 保持短小；把 durable state transitions 移到 entity update
  methods。
- 避免 dispatch 会同步重入同一个 entity update 的 action。
- platform-specific shortcuts 要显式，并在触碰示例时验证 Windows key path。
