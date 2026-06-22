# GPUI Rendering

## Current Direction

The GPUI renderer is being migrated to nova-gfx. Windows defaults to Nova DX12,
Linux and FreeBSD default to Nova Vulkan, and macOS normal windows use the
nova-gfx Metal backend. The macOS NovaMetal path was not compiled or
smoke-tested in the current Windows development environment.

The default model is event-driven composition. A clean static window must not
run a permanent vsync loop, rebuild a scene, or present frames just because time
passes.

## Frame Requests

Frame scheduling uses only `RequestFrameOptions`:

- `force_render = true` means scene state changed and GPUI must rebuild layout,
  paint, and submit a fresh compositor frame.
- `require_presentation = true` means prepared content or a GPU surface update
  needs presentation, but the scene does not need to be rebuilt.

`present_framebuffer_only()` is the presentation-only path. It reuses the last
prepared scene and lets the compositor present pending GPU output without
running layout or paint. The legacy surface API is being replaced by a
nova-gfx-oriented GPU surface API.

`RenderPolicy::Continuous` is reserved for explicit configuration. The normal
idle path remains event-driven, with temporary FPS elevation for animation,
dragging, and active surface updates.

## Resource Baseline

The `minimal_window` example is the smallest standalone window baseline.
Measure it with `scripts/profile_startup.ps1 -ExePath <path>` so RAM, CPU, and
GPU costs are sampled outside the window. Measurements should record driver,
nova-gfx device, swapchain, and font-system costs rather than weakening
rendering correctness to hit a fixed number.

## Metrics

Use `performance_metrics_snapshot()` to inspect renderer behavior. The snapshot
includes scheduler wakeups, frame request count, draw/present/skip counts, and
renderer backend information. These counters are intended to verify idle
behavior and guide later nova-gfx resource caching work.
