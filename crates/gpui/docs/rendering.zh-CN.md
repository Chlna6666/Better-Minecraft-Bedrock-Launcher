# 渲染与元素

[English](rendering.md)

GPUI views 会从应用状态构建 element tree。这个 tree 通过类似 flexbox 的 style API
布局，然后由 renderer backend 绘制。

## `Render`

把 `Render` 用于存储在 `Entity<T>` 中的 stateful views：

```rust
struct Counter {
    value: usize,
}

impl Render for Counter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(format!("Count: {}", self.value))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.value += 1;
                cx.notify();
            }))
    }
}
```

render 方法同时收到 `Window` 和 `Context<Self>`，因为渲染可能依赖 window-local
state，也可能把 callbacks 连接回 entity state。

## `RenderOnce`

把 `RenderOnce` 用于会立即被消费为 elements 的轻量 component values：

```rust
#[derive(IntoElement)]
struct Label {
    text: SharedString,
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(self.text)
    }
}
```

不拥有 GPUI state 的可复用 UI fragments 优先使用 `RenderOnce`。

## Elements 与 Styling

Elements 使用 builder-style methods 表达 layout、paint 和 interaction。style API
在 spacing、flexbox、color、border、text 和 overflow 上接近 Tailwind 词汇。

可选结构使用条件 builders：

- `.when(condition, |this| ...)`
- `.when_some(option, |this, value| ...)`
- `.children(iterator)`

可复用文本使用 `SharedString`，可复用 URI 值使用 `SharedUri`。

## 自定义 Elements 与 Paint

只有普通 element tree 不够用时才直接实现 `Element`。适合 custom elements 的场景：

- 直接绘制调用；
- window-local element state；
- 手动输入路由；
- 自定义 GPU extension points；
- 高级 layout 或 prepaint 行为。

Custom paint code 必须遵守 GPUI 传入的 layout bounds，使用 `Window` 上的绘制 API，
并尽量避免在每帧 paint 中做昂贵的 CPU 或 GPU 初始化。

## Invalidation

修改会影响渲染的 view state 后调用 `cx.notify()`。window-level state 或 direct
paint output 需要新帧时使用 `window.refresh()`。除非 UI 确实以动画为主，否则优先
使用事件驱动 invalidation，而不是连续渲染。
