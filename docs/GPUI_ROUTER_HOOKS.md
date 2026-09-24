# GPUI Router + Hooks (BMCBL)

本文档描述 BMCBL 当前实际使用的 `gpui-router 0.5.0` 与 `gpui-hooks`。不要再使用旧版 `init_router / set_route / navigate / switch_element!` 示例；这些不是当前 BMCBL 的 router API。

## Router

### 1. 依赖与后端

BMCBL 将 crates.io 的 `gpui-router` patch 到仓库内的 `vendor/gpui-router`：

```toml
gpui-router = { version = "0.5.0", default-features = false, features = ["gpui"] }

[patch.crates-io]
gpui-router = { path = "vendor/gpui-router" }
gpui = { path = "crates/gpui" }
```

BMCBL 使用自己的 `crates/gpui`，因此 router 只启用 `gpui` backend，不使用 `gpui-pre`。

vendored manifest 对 `gpui` 保持 `default-features = false`。Linux 的 Wayland/X11、Windows 的 DX12/Vulkan、macOS 的 Metal 等 feature 仍由 BMCBL 根 `Cargo.toml` 决定，router 不应重新打开 GPUI 默认 feature。

### 2. 初始化

实际初始化位置是 `src/app.rs`：

```rust
gpui_router::init(cx);
```

初始化后会安装全局 `gpui_router::RouterState`。

### 3. BMCBL 当前路由架构

BMCBL 目前没有把主窗口改造成完整的声明式 `Routes + Route + Outlet` 树。

当前职责划分：

```text
gpui-router RouterState
        │
        │ pathname
        ▼
src/ui/navigation.rs
        │
        ├─ RouteTarget::Builtin(AppRoute)
        └─ RouteTarget::Plugin { plugin_id, page_id }
        │
        ▼
PageRegistry / plugin route dispatch / NavState animation
```

也就是说，`gpui-router` 目前主要负责稳定的 pathname 全局状态；页面实例驻留、缓存、释放、切页动画和插件页面生命周期继续由 BMCBL 自己管理。

不要为了“使用新版 router”而把 `PageRegistry`、页面缓存或 retained rendering 生命周期整体迁移到 `Outlet`。

### 4. 读取当前路由

BMCBL 的统一入口在 `src/ui/navigation.rs`：

```rust
pub fn current_route_target(cx: &gpui::App) -> RouteTarget {
    let location = gpui_router::use_location(cx);
    RouteTarget::from_pathname(&location.pathname)
}
```

业务代码优先调用 `current_route_target` / `current_route`，不要到处重复解析 pathname。

只有确实属于局部子路由的代码才直接读取 `use_location`。例如 Level Dat Editor：

```rust
gpui_router::use_location(cx)
    .pathname
    .starts_with(LEVEL_DAT_EDITOR_ROUTE_PATH)
```

### 5. 路由跳转

BMCBL 对内优先使用：

```rust
navigate_to(cx, AppRoute::Manage);
navigate_plugin(cx, plugin_id, page_id);
navigate_target(cx, target);
```

`navigate_target` 会同时处理：

1. 目标 pathname；
2. 侧栏 pill 动画；
3. `gpui_router::use_navigate`；
4. plugin route changed 事件。

核心实现：

```rust
let mut navigate = gpui_router::use_navigate(cx);
navigate(path.into());
```

因此普通主页面代码不要绕过 `navigate_target` 直接调用 `use_navigate`，否则容易漏掉 BMCBL 自己的导航副作用。

局部 host route（例如 Level Dat Editor）可以直接使用 `use_navigate`。

### 6. 0.5.0 pathname normalization

`use_navigate` 现在通过 `RouterState::with_path` 写入路径，并统一规范化 pathname：

```text
""          -> "/"
"settings"  -> "/settings"
"/settings/" -> "/settings"
"/"         -> "/"
```

因此新代码不要依赖尾部 `/` 区分页面，也不要手工制造同一页面的多种 pathname 形式。

### 7. RouterState 订阅

主窗口页面 registry 通过 GPUI global observer 监听 router：

```rust
cx.observe_global::<gpui_router::RouterState>(|this, cx| {
    let route = crate::ui::navigation::current_route_target(cx);
    this.handle_route_change_without_window(route, cx);
    cx.notify();
})
```

这里才是主页面生命周期变化的入口之一。不要在 router hook 内直接创建、销毁所有页面 View。

### 8. 0.5.0 的声明式 Route + Outlet

0.5.0 支持 element route 自己带 children，并把匹配到的 child 渲染到 element 创建的第一个 `Outlet`：

```rust
use gpui_router::{Outlet, Route, Routes};

Routes::new().child(
    Route::new()
        .path("manage")
        .element(|_, _| {
            div()
                .child("Manage shell")
                .child(Outlet::new())
        })
        .children(vec![
            Route::new().index().element(|_, _| div().child("Manage home")),
            Route::new()
                .path("level-dat")
                .element(|_, _| div().child("Level Dat")),
        ]),
)
```

语义要点：

- `Route::element(...)` 可以有 children；
- child 只会进入 element closure 内创建的第一个 `Outlet`；
- element 没创建 `Outlet` 时，child 仍可匹配，但不会显示；
- 多级 element route 会逐级保存/恢复 outlet scope；
- `Route::layout(...)` / `IntoLayout` 仍然支持。

BMCBL 当前不需要把主页面全部迁移过去。后续若要采用，优先用于独立的子树，例如 `/manage/*`、`/tools/*` 或插件内部子路由，并确保不破坏 `PageRegistry` 的页面驻留策略。

### 9. NavLink

0.5.0 已实现 `NavLink::active`，不再是旧版的 `unimplemented!()`：

```rust
nav_link("/settings")
    .active(|style| style.bg(gpui::rgb(0x333333)))
    .end(true)
    .child("Settings")
```

默认情况下父路径可以在 descendant pathname 上保持 active；`.end(true)` 要求 exact match。根路径 `/` 始终按 exact match 处理。

BMCBL 当前侧栏仍由自己的 `NavState` 驱动，不需要为了这个能力替换现有导航动画。

### 10. Params 与匹配修复

0.5.0 的 route matching 会在每次新匹配时替换 params；无参数路由或未匹配路由不会继续携带上一次的旧 params。

新版同时覆盖：

- 静态 sibling 优先于动态参数 sibling；
- wildcard；
- nested index；
- 多参数；
- basename normalization；
- exact route / wildcard 边界。

如果后续使用 `Routes` / `use_params`，不要再额外维护“清理旧 params”的 workaround。

## Hooks

`gpui-hooks` 与 `gpui-router` 是两套独立能力。Router 升级到 0.5.0 不要求修改 Hooks 的生命周期。

### 1. 在 View 里持有 Hooks

```rust
pub struct MyView {
    hooks: gpui_hooks::Hooks,
}
```

在 `new` 里初始化：

```rust
Self {
    hooks: gpui_hooks::Hooks::default(),
}
```

需要异步/计时器 hooks 时实现 `gpui_hooks::HookHost`：

```rust
impl gpui_hooks::HookHost for MyView {
    fn hooks(&self) -> &gpui_hooks::Hooks {
        &self.hooks
    }

    fn hooks_mut(&mut self) -> &mut gpui_hooks::Hooks {
        &mut self.hooks
    }
}
```

### 2. 每次 render 开始调用 begin()

```rust
self.hooks.begin();
```

### 3. use_state

```rust
let count = gpui_hooks::use_state!(&mut self.hooks, || 0u64);
let current = *count.get(&self.hooks);
count.set_and_notify(&mut self.hooks, current + 1, cx);
```

### 4. use_memo / use_memo_cloned

`use_memo` 返回 `&T`，分配更少，但可能延长对 `self.hooks` 的借用。

`use_memo_cloned` 返回 `T`（要求 `Clone`），在复杂 View 中通常更容易避免 borrow conflict。

```rust
let label = gpui_hooks::use_memo_cloned!(&mut self.hooks, current, || {
    SharedString::from(format!("count={current}"))
});
```

### 5. use_effect

deps 变化时把 effect 延迟到本次 render 完成后执行：

```rust
gpui_hooks::use_effect!(&mut self.hooks, current, move |cx: &mut gpui::App| {
    let _ = cx;
});

self.hooks.run_effects_in(window, cx);
```

### 6. use_selector / use_selector_cloned

从 GPUI `Global` 派生状态：

```rust
let update_available = gpui_hooks::use_selector_cloned!(
    &mut self.hooks,
    cx,
    crate::ui::update_state::UpdateState,
    |u| u.available.is_some()
);
```

### 7. use_ref

用于跨 render 持久化但不直接触发重渲染的状态：

```rust
let cache = gpui_hooks::use_ref!(&mut self.hooks, || Vec::<u8>::new());
cache.get_mut(&mut self.hooks).push(1);
```

### 8. use_callback

deps 不变时复用同一个 `Arc<F>`：

```rust
let cb = gpui_hooks::use_callback!(&mut self.hooks, current, move || {
    move |delta: i64| delta
});
```

### 9. use_timeout

deps 变化时自动取消旧计时任务：

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

### 10. use_interval

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

### 11. use_async

deps 变化时取消旧任务并重新运行：

```rust
let state = gpui_hooks::use_async!(&mut self.hooks, cx, current, move || async move {
    Ok::<_, String>(format!("value={current}"))
});
```

需要真正放到后台线程的工作使用 `use_async_background!`，并满足对应 `Send + 'static` 约束。

## 维护规则

修改 router 相关代码时：

1. 先确认主窗口是否真的需要声明式 `Route + Outlet`；不要因为上游新增能力就重写现有 `PageRegistry`。
2. 主导航优先通过 `src/ui/navigation.rs` 的封装进入。
3. `RouterState` 只负责路由状态，不承担下载、IO、页面资源释放等业务任务。
4. 不要在 render 每帧轮询 pathname 做昂贵工作；路由变化应通过 global observer / GPUI 事件驱动。
5. 升级 `gpui-router` 时同时检查 `vendor/gpui-router`、根 `Cargo.toml`、`Cargo.lock` 和 `gpui-router-macros` 版本。
6. BMCBL 使用 `gpui` backend；除非整体切换 GPUI 发行体系，否则不要启用 `gpui-pre`。
