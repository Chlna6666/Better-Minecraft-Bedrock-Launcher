# Renderer Backend And Frame Scheduling

[Chinese](renderer_backend.zh-CN.md)

GPUI exposes renderer startup options so applications can choose a backend,
adapter, present behavior, render policy, and diagnostic metrics without
hard-coding platform policy into framework defaults.

## Renderer Options

`RendererOptions` contains:

- `backend`: `RendererBackend::Auto`, `NovaVulkan`, `NovaDx12`, `NovaMetal`, or
  `HeadlessTest`;
- `adapter_name`: optional exact GPU adapter preference;
- `power_preference`: low-power default or high-performance preference;
- `present_mode`: vsync, mailbox, or immediate preference where supported;
- `render_policy`: event-driven, continuous, or on-demand composition;
- `frame_metrics`: extra metrics collection for debugging and profiling.

Use `Application::new_with_renderer_options(options)` or
`Application::new_with_renderer_backend(backend)` at startup.

## Environment Override

`GPUI_RENDERER` can override the configured backend. Accepted values include
`auto`, `vulkan`, `dx12`, `metal`, and `headless`.

Keep application UI for renderer choice outside GPUI itself. GPUI should expose
the options and metrics; applications decide their own defaults.

## Nova-gfx Path

The GPUI platform path uses nova-gfx. `RendererBackend::Auto` chooses the
platform default backend, while explicit Vulkan, DX12, or Metal preferences
select a native backend with fallback diagnostics.

The Windows renderer also owns frame scheduling for event-driven, continuous,
and presentation-only frames. Presentation-only frames are used when already
prepared content can be shown without rebuilding the full scene.

## Frame Policy

Use `RenderPolicy::EventDriven` for ordinary UI. Use
`RenderPolicy::Continuous` only for windows that truly need ongoing composition,
such as animation or live visualization. The continuous frame rate is clamped to
GPUI's supported range.

`RenderPolicy::OnDemand` is for tests or specialized integrations that request
all frames explicitly.

## Metrics

Renderer metrics include backend selection, frame timing, image cache state,
atlas usage, backdrop blur and 3D mesh counts, allocator totals, and retained
resource trimming. Enable `frame_metrics` when diagnosing frame pacing or GPU
resource lifetime.
