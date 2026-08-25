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

## Rust 命名

名称依赖 module 和 type context。只有在区分 sibling concept 时才增加单词，不在每个
item 中重复完整路径。

- 优先使用 `events`、`windows`、`assets`、`layout`、`scene`、`webp` 等惯用领域
  module。除非新增单词确实表示另一种抽象，否则不要扩展成 `event_observers`、
  `window_registry`、`image_decode` 或 `raster_image_decoder`。
- module/file 表示一个内聚领域或 type family。不能用 `manager`、`service`、
  `handler`、`processor`、`helper`、`utils`、`common`、`decoder` 或 `data` 代替被拥有
  对象的名称。
- type 使用名词，trait 描述 capability。函数是简短动作，其 object 由所在 module 或
  receiver 提供：在 WebP module 内使用 `dimensions`、`render`、`frames`，而不是
  `decode_webp_*`。
- `from_*` 只用于直接且无副作用的 conversion constructor。读取路径使用 `load` 或
  `open`；尺寸通过明确参数或 options type 表达。不要把 IO、format、policy 和 target
  size 全塞入 `from_path_at_size` 一类名字。
- peers 集合使用复数 module（`events`、`windows`），单一 concept/algorithm 使用单数
  module（`window`、`layout`、`webp`）。已有精确的标准库或生态术语时直接遵循。
- 同时避免含糊和穷举式名称。单独 `Options` 太泛；`AnimatedImageConfig` 已有足够
  context；`AnimatedRasterImageDecoderConfiguration` 则重复实现细节。
- rename 只有在 code、tests、benches、examples、rustdoc 和 re-export 全部迁移后才
  完成。当前 dev fork 不保留 compatibility alias、deprecated wrapper 或双 public path。

自动化整改时，先确定 object 的 domain/module，再删除 context 已表达的词，最后确认剩余
名称仍能区分 sibling concept。修改前后都搜索全部调用方，不能只按字符串相似度重命名。

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
`NovaVulkan` 和 `NovaDx12`。

精确使用帧请求：

- `force_render` 表示 layout 或 paint 场景状态发生变化。
- `require_presentation` 表示已准备内容或 GPU surface 输出需要 present，但不一定
  需要重建场景。

正常 idle 模型是事件驱动。连续合成必须显式使用 `RenderPolicy::Continuous`。

## GPU Surface 示例

自定义 GPU 示例应当使用当前 GPUI scene primitives 和 nova-gfx renderer extension
points。平台专用示例使用 `cfg` guard，并为不支持的平台提供一个小的 fallback
`main`。

## Lint 与文档规则

- 优先修复 warning，而不是压制 warning。
- 只有代码确实是平台预留或诊断专用时，才使用局部
  `#[expect(..., reason = "...")]`。
- library code 避免 `unwrap` 和 `expect`，除非 invariant 是局部且显然成立的。
  优先使用 `?`、`let Some(...) = ... else` 或显式错误处理。
- 注释只解释不明显的原因、安全性、平台约束或性能取舍。
- public API 的 rustdoc 应说明行为、错误、panic 和安全约束。

## 独立维护

本 GPUI 作为独立 framework 维护。Zed GPUI 是 established semantics 与命名的比较源，
不是 runtime、source 或 release dependency。上游变化只针对 pinned commit 选择性审查；
不能整批复制，也不能在缺少本地证据时覆盖本地 renderer、platform、memory 或 API 决策。

任何引入的设计都必须有本地 owner、contract tests；声称性能收益时必须有 benchmark；
发生 divergence 时必须有文档。breaking change 在一个变更中迁移仓库全部调用方，只暴露
一条权威 API。只有对应 feature 配置完成编译且 platform tests 实际运行后，才能声明支持
该平台；只有源码 `cfg` 覆盖不属于支持证据。
