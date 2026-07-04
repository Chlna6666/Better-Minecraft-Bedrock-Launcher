# Renderer Backend 与帧调度

[English](renderer_backend.md)

GPUI 暴露 renderer startup options，让应用可以选择 backend、adapter、present 行
为、render policy 和 diagnostic metrics，而不把平台策略硬编码进 framework
defaults。

## Renderer Options

`RendererOptions` 包含：

- `backend`：`RendererBackend::Auto`、`NovaVulkan`、`NovaDx12` 或
  `HeadlessTest`；
- `adapter_name`：可选的精确 GPU adapter preference；
- `power_preference`：默认 low-power 或 high-performance preference；
- `present_mode`：支持时使用 vsync、mailbox 或 immediate preference；
- `render_policy`：event-driven、continuous 或 on-demand composition；
- `frame_metrics`：用于 debugging 和 profiling 的额外 metrics collection。

启动时使用 `Application::new_with_renderer_options(options)` 或
`Application::new_with_renderer_backend(backend)`。

## 环境变量 Override

`GPUI_RENDERER` 可以覆盖配置的 backend。可接受值包括 `auto`、`vulkan`、
`vulkan`、`dx12`、`dx12` 和 `headless`。

renderer choice 的应用 UI 应放在 GPUI 外部。GPUI 暴露 options 和 metrics；应用
决定自己的 defaults。

## Windows GPU 路径

Windows 平台路径是 GPU-first 且基于 winit。`RendererBackend::Auto` 会按平台顺序
尝试支持的 GPU backends；显式 Vulkan 或 DX12 preference 会选择单一 backend，并
提供 fallback diagnostics。

Windows renderer 也负责 event-driven、continuous 和 presentation-only frames 的帧
调度。presentation-only frames 用于已经准备好的内容可以显示，而不必重建完整 scene
的场景。

## Frame Policy

普通 UI 使用 `RenderPolicy::EventDriven`。只有窗口确实需要持续 composition 时才使
用 `RenderPolicy::Continuous`，例如动画或 live visualization。continuous frame
rate 会被 clamp 到 GPUI 支持的范围。

`RenderPolicy::OnDemand` 用于测试或所有帧都由集成方显式请求的特殊场景。

## Metrics

Renderer metrics 包括 backend selection、frame timing、image cache state、atlas
usage、backdrop blur 和 3D mesh counts、allocator totals，以及 retained resource
trimming。诊断 frame pacing 或 GPU resource lifetime 时启用 `frame_metrics`。
