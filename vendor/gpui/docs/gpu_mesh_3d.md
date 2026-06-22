# GPU Mesh 3D

[Chinese](gpu_mesh_3d.zh-CN.md)

GPUI can paint renderer-owned 3D mesh primitives as part of a normal scene. This
is separate from custom GPU surfaces: mesh primitives are submitted to GPUI's
renderer, while GPU surfaces let applications own the full render pipeline.

## When To Use It

Use GPUI mesh primitives when UI rendering needs a bounded 3D primitive that
fits GPUI's scene batching and renderer resource model. Use a custom GPU
surface when the application needs full control over shaders, materials,
passes, or GPU resources.

## Renderer Behavior

The Nova renderer keeps mesh buffers and shader modules in retained resources.
It can trim those resources when windows are idle or memory pressure requires
cleanup.

Mesh paint order remains part of scene ordering. Avoid building application
stateful model loaders directly into renderer internals.

## Guidelines

- Keep mesh ids stable when geometry is reused across frames.
- Rebuild mesh buffers only when vertex data changes.
- Use metrics when diagnosing mesh-heavy scenes.
- Prefer GPU surfaces for full model viewers and material systems.
