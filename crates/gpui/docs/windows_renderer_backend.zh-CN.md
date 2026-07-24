# Windows Renderer Backend

[English](windows_renderer_backend.md)

Windows 平台路径使用 GPU 和 winit。它通过 `RendererBackend` 支持 Vulkan 和 DX12
backend selection。

## Backend Selection

`RendererBackend::Auto` 使用平台定义的 Windows backend order。显式
`NovaVulkan` 或 `NovaDx12` preference 会选择单个 backend。`GPUI_RENDERER` 环境变
量可以覆盖 startup configuration。

renderer 应通过 frame 和 performance metrics 报告实际选择的 backend。

## Frame Delivery

Windows frame requests 会合并 `force_render` 和 `require_presentation`，让 event
loop 唤醒前的多次请求合并为一帧。这样事件驱动渲染可以保持 idle，同时 GPU surface
输出仍能及时 present。

只有需要持续 composition 的窗口才使用 `RenderPolicy::Continuous`。

## GPU Surfaces

自定义 GPU 内容应通过当前 GPUI scene primitives 和 nova-gfx renderer extension
points 接入，而不是使用已经移除的 framework-managed surface API。
