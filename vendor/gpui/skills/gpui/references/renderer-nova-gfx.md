# Renderer And Nova GFX Reference

## Renderer Options

Use `RendererOptions` for backend, adapter, power preference, present mode,
render policy, and frame metrics. Use `RendererBackend::Auto` for platform
defaults. On Windows, explicit `NovaVulkan` and `NovaDx12` are available.

`GPUI_RENDERER` may override the backend. Accepted values include `auto`,
`vulkan`, `nova-vulkan`, `dx12`, `nova-dx12`, and `headless`.

Keep application-level renderer preferences outside GPUI framework defaults.

## Frame Scheduling

Event-driven rendering is the default. Use `RenderPolicy::Continuous` only when
ongoing composition is required. Use on-demand rendering only for tests or
specialized integrations.

Treat `force_render` as scene invalidation and `require_presentation` as a
request to present already prepared content without necessarily rebuilding the
scene.

## Custom GPU Content

GPUI applications should submit custom 3D content through retained GPU mesh
primitives. Use `Window::paint_gpu_mesh_3d` when an element needs to place an
application-owned mesh in the scene. The platform renderer owns the concrete
Nova backend resources and translates the scene into Nova draw calls.

Do not expose backend device, queue, command buffer, or swapchain handles from
GPUI application APIs. New custom renderer features belong in Nova GFX first,
then in a GPUI scene adapter.

## Runtime WGSL

Use `WgslShaderSource`, `compile_wgsl_shader_module`, or
`compile_wgsl_shader_module_from_path` for runtime WGSL validation. Surface
parse and validation diagnostics to users or logs. Keep built-in renderer shader
validation in build-time paths.

## Platform Guards

Nova DX12 is Windows-facing and Nova Vulkan is used for Linux/FreeBSD. Platform
specific examples must use `cfg` guards and keep a fallback `main` for
unsupported targets.
