# GPUI Router + Hooks (BMCBL)

本项目在 `crates/gpui-router` 和 `crates/gpui-hooks` 里提供了一套偏 React 风格的“路由 + hooks”工具。

目标：
- 写法更接近前端（`use_state/use_effect/use_memo/use_selector`）
- 低开销：仅在 deps / selector 值变化时更新缓存
- 避免常见借用冲突：提供 `*_cloned` 版本

## Router

### 1) 定义路由枚举

参考 `src/ui/route.rs`：
- 实现 `gpui_router::Route`：提供 `index()`（用于导航条等）
- 实现 `Default`：提供初始路由

### 2) 初始化路由

在 `src/main.rs` 初始化 globals 的位置调用：

```rust
gpui_router::init_router(cx, ui::route::AppRoute::Home);
```

### 3) 路由跳转

最简单：

```rust
gpui_router::set_route(cx, AppRoute::Download);
cx.refresh_windows();
```

推荐（内部会判断是否真的变化，并自动刷新窗口）：

```rust
gpui_router::navigate(cx, AppRoute::Download);
```

### 4) 根据路由切换页面

`switch_element!` 适合直接返回 `AnyElement`：

```rust
let page = gpui_router::switch_element!(cx, AppRoute, |route| {
    match route {
        AppRoute::Home => home_view.into_any_element(),
        _ => placeholder(route).into_any_element(),
    }
});
```

## Hooks

### 1) 在 View 里持有 Hooks

```rust
pub struct MyView {
    hooks: gpui_hooks::Hooks,
}
```

在 `new` 里初始化：

```rust
Self { hooks: gpui_hooks::Hooks::default() }
```

并实现 `gpui_hooks::HookHost`（异步/计时器 hooks 需要用它从任务里回写状态）：

```rust
impl gpui_hooks::HookHost for MyView {
    fn hooks(&self) -> &gpui_hooks::Hooks { &self.hooks }
    fn hooks_mut(&mut self) -> &mut gpui_hooks::Hooks { &mut self.hooks }
}
```

### 2) 每帧 render 开始时 begin()

```rust
self.hooks.begin();
```

### 3) use_state

```rust
let count = gpui_hooks::use_state!(&mut self.hooks, || 0u64);
let current = *count.get(&self.hooks);
// 更新并触发重渲染：
count.set_and_notify(&mut self.hooks, current + 1, cx);
```

### 4) use_memo / use_memo_cloned

`use_memo` 返回 `&T`（更省），但可能会导致后续对 `self.hooks` 的可变借用冲突。
`use_memo_cloned` 返回 `T`（要求 `Clone`），通常更好用。

```rust
let label = gpui_hooks::use_memo_cloned!(&mut self.hooks, current, || {
    SharedString::from(format!("count={current}"))
});
```

### 5) use_effect

deps 变化时，将 effect defer 到本帧 render 完成之后执行：

```rust
gpui_hooks::use_effect!(&mut self.hooks, current, move |cx: &mut gpui::App| {
    let _ = cx;
});
self.hooks.run_effects_in(window, cx);
```

### 6) use_selector / use_selector_cloned

从 GPUI `Global` 中派生出一个值，并在值变化时更新缓存：

```rust
let update_available = gpui_hooks::use_selector_cloned!(
    &mut self.hooks,
    cx,
    crate::ui::update_state::UpdateState,
    |u| u.available.is_some()
);
```

### 7) use_ref

用于“跨帧持久化，但不直接触发重渲染”的数据（缓存、句柄、计时等）：

```rust
let cache = gpui_hooks::use_ref!(&mut self.hooks, || Vec::<u8>::new());
cache.get_mut(&mut self.hooks).push(1);
```

### 8) use_callback

deps 不变时复用同一个 `Arc<F>`，便于在多处复用 callback（或降低重复构造成本）：

```rust
let cb = gpui_hooks::use_callback!(&mut self.hooks, current, move || {
    // 这里是 F（任意类型），通常是一个 closure
    move |delta: i64| delta
});
let _ = cb;
```

### 9) use_timeout

deps 变化时会自动取消旧任务，重新计时：

```rust
gpui_hooks::use_timeout!(
    &mut self.hooks,
    cx,
    current,
    std::time::Duration::from_secs(2),
    move |this, cx| {
        let _ = (this, cx);
    }
);
```

### 10) use_interval

deps 变化时会自动取消旧任务，重新开始 loop：

```rust
let ticks = gpui_hooks::use_ref!(&mut self.hooks, || 0u64);
gpui_hooks::use_interval!(
    &mut self.hooks,
    cx,
    (),
    std::time::Duration::from_secs(1),
    move |this, _cx| {
        *ticks.get_mut(&mut this.hooks) += 1;
    }
);
```

### 11) use_async

deps 变化会取消旧任务并重跑。返回 `AsyncState<T, E>`：

```rust
let state = gpui_hooks::use_async!(&mut self.hooks, cx, current, move || async move {
    Ok::<_, String>(format!("value={current}"))
});

if state.loading {
    // loading...
} else if let Some(v) = &state.value {
    // success
} else if let Some(e) = &state.error {
    // error
    let _ = e;
}
```

如果任务需要跑到后台线程（避免阻塞 UI），用 `use_async_background!`：
- 需要 `T/E/Fut: Send + 'static`

```rust
let state = gpui_hooks::use_async_background!(&mut self.hooks, cx, current, move || async move {
    Ok::<_, String>(format!("background value={current}"))
});
```

## Demo

参考 `src/ui/views/debug.rs`：展示 `use_state/use_memo_cloned/use_effect/use_selector_cloned` 的组合用法。
