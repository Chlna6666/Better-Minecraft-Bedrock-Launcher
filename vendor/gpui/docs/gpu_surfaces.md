# GPU Surfaces

[Chinese](gpu_surfaces.zh-CN.md)

The old application-owned surface API has been retired. Custom GPU content
should enter GPUI through scene primitives, currently `Window::paint_gpu_mesh_3d`
for retained 3D mesh content. The platform renderer owns the concrete Nova GFX
backend resources and handles presentation.

Applications that need new custom GPU features should add the capability to
`nova-gfx` first, then expose a GPUI scene primitive or adapter for it. GPUI
application code should not depend on backend device, queue, command buffer, or
swapchain handles.
