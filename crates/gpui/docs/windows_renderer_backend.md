# Windows Renderer Backend

[Chinese](windows_renderer_backend.zh-CN.md)

The Windows platform path uses nova-gfx with the native DX12 backend by default.
Optional Vulkan selection is exposed through `RendererBackend` when that feature
is enabled.

## Backend Selection

`RendererBackend::Auto` uses the Windows backend order defined by the platform.
Explicit `NovaVulkan` or `NovaDx12` preferences select one backend. The
`GPUI_RENDERER` environment variable can override startup configuration.

The renderer should report the backend it actually selected through frame and
performance metrics.

## Frame Delivery

Windows frame requests merge `force_render` and `require_presentation` so that
multiple requests before the event loop wakes up coalesce into one frame. This
keeps event-driven rendering idle while still allowing prepared GPU output to be
presented promptly.

Use `RenderPolicy::Continuous` only for windows that need ongoing composition.

## GPU Surfaces

Custom GPU content should go through GPUI scene primitives and nova-gfx renderer
extensions rather than the removed framework-managed surface API.
