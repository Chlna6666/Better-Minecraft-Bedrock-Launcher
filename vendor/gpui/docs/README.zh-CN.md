# GPUI 文档

[English](README.md)

本文档以独立 UI 框架的方式描述 GPUI。英文文件是 canonical 文档；中文翻译使用同目
录匹配的 `.zh-CN.md` 文件。

## 指南

- [开发指南](development.zh-CN.md)：当前 API 写法、异步任务、renderer policy
  和 lint 规则。
- [上下文与实体](contexts.zh-CN.md)：`App`、`Context<T>`、`Window`、
  `Entity<T>`、events 和 subscriptions。
- [渲染与元素](rendering.zh-CN.md)：`Render`、`RenderOnce`、elements、style、
  layout 和自定义 paint hook。
- [输入与 actions](input_and_actions.zh-CN.md)：事件处理、listeners、actions、
  key bindings 和 key dispatch。
- [Key dispatch](key_dispatch.zh-CN.md)：focused key contexts 与 action
  dispatch。
- [Assets 与图片](assets_and_images.zh-CN.md)：asset loading、HTTP images、
  image cache providers 和 texture lifetime。
- [Image cache](image_cache.zh-CN.md)：scoped image cache 行为和自定义 cache
  providers。
- [动画引擎](animation_engine.zh-CN.md)：timing、easing、transition metadata、
  drivers、窗口调度和 scene/nova animation 数据通道。
- [Renderer backend](renderer_backend.zh-CN.md)：backend options、frame
  scheduling、Windows GPU startup 和 metrics。
- [Windows renderer backend](windows_renderer_backend.zh-CN.md)：Windows GPU
  backend selection 和 winit frame delivery。
- [运行时 WGSL shaders](runtime_wgsl_shaders.zh-CN.md)：运行时 shader 校验和自
  定义 GPU shader modules。
- [Backdrop blur](backdrop_blur.zh-CN.md)：GPU backdrop blur pipeline 行为。
- [默认字体](default_fonts.zh-CN.md)：font setup boundaries 和平台 defaults。
- [Performance pipeline](performance_pipeline.zh-CN.md)：frame metrics 和 retained
  resource trimming。
- [示例](examples.zh-CN.md)：示例编写规则和当前 API 模式。
- [验证](validation.zh-CN.md)：formatting、checks、clippy、examples 和 docs
  validation。

## 兼容性说明

所有新代码和示例使用当前 API 名称：

- `App`
- `Context<T>`
- `Window`
- `Entity<T>`
- `WeakEntity<T>`
- `Render`
- `RenderOnce`

不要在新的应用代码中使用旧名称，例如 `Model<T>`、`View<T>`、`ModelContext<T>`、
`WindowContext` 或 `ViewContext<T>`。
