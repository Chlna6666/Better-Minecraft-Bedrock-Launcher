# GPUI 开发指南

[English](development.md)

本指南定义当前 GPUI 在框架开发、示例和下游应用中的 API 写法。

## 上下文

- 使用 `App` 作为根上下文，管理 global state、windows、menus、key bindings、
  assets 和平台服务。
- 在 `Entity<T>` 创建、更新、事件 listener 和 `Render` 实现中使用 `Context<T>`。
  如果闭包收到内部 `cx`，应使用内部 context，而不是外层 context。
- 需要焦点、输入状态、绘制、帧请求、actions、自定义 GPU surface 或窗口局部
  element state 时，显式使用 `Window`。
- 只有跨 await 点时才使用 `AsyncApp` 和 `AsyncWindowContext`。

不要引入旧应用 API 名称：`Model<T>`、`View<T>`、把 `AppContext` 当作具体上下文
类型、`ModelContext<T>`、`WindowContext` 或 `ViewContext<T>`。

## 实体与渲染

`Entity<T>` 是状态句柄。用 `read` 或 `read_with` 读取；用 `update` 或
`update_in` 修改。不要在实体已经处于 update 过程中再次 update 同一个实体。

View 实现 `Render`：

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().child("content")
    }
}
```

只为了生成元素而构造的 component 使用 `RenderOnce`。状态变化会影响渲染时调用
`cx.notify()`。

## 异步任务

前台异步任务使用 async closure：

```rust
cx.spawn(async move |cx| {
    gpui::Timer::after(std::time::Duration::from_millis(100)).await;
    cx.update(|cx| cx.refresh_windows())?;
    anyhow::Ok(())
}).detach_and_log_err(cx);
```

从 `Context<T>` 中 spawn 时，async closure 的第一个参数是 weak entity handle：

```rust
cx.spawn(async move |handle, cx| {
    handle.update(cx, |state, cx| {
        state.loaded = true;
        cx.notify();
    })?;
    anyhow::Ok(())
}).detach_and_log_err(cx);
```

必须继续运行的任务要存储或 detach。耗时工作使用 `background_spawn`，并把错误传回
前台状态。

## Renderer 与帧

`RendererOptions` 包含 backend、adapter、power、present mode、render policy 和
metrics 偏好。`RendererBackend::Auto` 选择平台默认后端；Windows 支持显式
`NovaVulkan` 和 `NovaDx12`，macOS 使用 `NovaMetal` 接入 nova-gfx Metal。当前开发
环境是 Windows，本次没有编译或 smoke test macOS NovaMetal 路径。

精确使用帧请求：

- `force_render` 表示 layout 或 paint 场景状态发生变化。
- `require_presentation` 表示已准备内容或 GPU surface 输出需要 present，但不一定
  需要重建场景。

正常 idle 模型是事件驱动。连续合成必须显式使用 `RenderPolicy::Continuous`。

## GPU Surface 示例

自定义 Nova GPU 示例应当：

- 通过 `Window::paint_gpu_mesh_3d` 创建 surface；
- 渲染到 `GpuSurfaceHandle::back_buffer_view`；
- 通过 `GpuSurfaceHandle::queue` 提交工作；
- 当渲染结果需要请求后续 presentation 时调用 `present`；
- 当渲染发生在当前 paint 路径内时调用 `swap_buffers`；
- paint 阶段调用 `Window::paint_gpu_mesh_3d`。

平台专用示例使用 `cfg` guard，并为不支持的平台提供一个小的 fallback `main`。

## Lint 与文档规则

- 优先修复 warning，而不是压制 warning。
- 只有代码确实是平台预留或诊断专用时，才使用局部
  `#[expect(..., reason = "...")]`。
- library code 避免 `unwrap` 和 `expect`，除非 invariant 是局部且显然成立的。
  优先使用 `?`、`let Some(...) = ... else` 或显式错误处理。
- 注释只解释不明显的原因、安全性、平台约束或性能取舍。
- public API 的 rustdoc 应说明行为、错误、panic 和安全约束。
