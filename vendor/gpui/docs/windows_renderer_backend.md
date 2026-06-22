# Windows Renderer Backend

[Chinese](windows_renderer_backend.zh-CN.md)

The Windows platform path uses Nova GPU on a native Win32 event loop. It
supports Vulkan and DX12 backend selection through `RendererBackend`, and
creates renderer surfaces through the `raw-window-handle` implementation
exposed by each platform window.

## Backend Selection

`RendererBackend::Auto` uses the Windows backend order defined by the platform.
Explicit `NovaVulkan` or `NovaDx12` preferences select one backend. The
`GPUI_RENDERER` environment variable can override startup configuration.

The renderer should report the backend it actually selected through frame and
performance metrics.

## Frame Delivery

Windows frame requests merge `force_render` and `require_presentation` so that
multiple requests before the Win32 event loop wakes up coalesce into one frame.
This keeps event-driven rendering idle while still allowing GPU surface output
to be presented promptly.

Use `RenderPolicy::Continuous` only for windows that need ongoing composition.

## GPU Surfaces

Windows exposes `Window::paint_gpu_mesh_3d` for custom GPU content. Surface
handles share the renderer device and queue and are painted back into the GPUI
scene with `Window::paint_gpu_mesh_3d`.
