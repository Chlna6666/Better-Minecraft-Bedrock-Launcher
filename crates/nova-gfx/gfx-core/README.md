# gfx-core

[中文文档](README.zh-CN.md)

`gfx-core` defines the canonical backend-neutral API for `nova-gfx`.

It contains:

- typed generational resource handles;
- resource, pipeline, command, surface, presentation, and diagnostics descriptors;
- `Error` and `Result`;
- the public backend capability traits:
  `Backend`, `SurfaceDevice`, `ResourceDevice`, `PipelineDevice`,
  `CommandDevice`, `SubmissionDevice`, `PresentationDevice`,
  `DiagnosticsDevice`, and `Device`.

The vocabulary mirrors `wgpu`: `BindGroup`/`BindGroupLayout`,
`RenderCommand`, `RenderPassCommands`, `PresentFrameDescriptor`, and
`render_to_textures`. There is no compatibility-alias trait layer and no
UI-framework policy in this crate.

`gfx-core` intentionally has no dependency on any UI framework, winit,
`raw-window-handle`, Vulkan, Direct3D 12, or Metal. Native window targets are
expressed through `SurfaceDevice::SurfaceTarget`, an associated type selected by
each backend crate.

## Recommended Use

Use the narrowest trait that a helper requires:

```rust
use gfx_core::{BufferDescriptor, BufferId, ResourceDevice, Result};

fn create_upload_buffer<D>(device: &mut D, desc: &BufferDescriptor) -> Result<BufferId>
where
    D: ResourceDevice,
{
    device.create_buffer(desc)
}
```

Use `Device` only for code that genuinely needs the full device contract.

## API Stability

`0.1.x` is an early breaking API line. The traits in this crate are the
maintained public contract; backend crates should implement these traits instead
of exposing duplicate public inherent methods.
