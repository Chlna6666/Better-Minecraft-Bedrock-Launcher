# gfx-core

`RenderPipelineDescriptor::raster` specifies projected front-face winding and face culling through
`RasterState`. The default is `FrontFace::Ccw` with `cull_mode: None` (two-sided rendering).
Use `Some(Face::Back)` for rasterizer back-face culling. DX11, DX12, Vulkan, OpenGL and Metal map
this state; WebGL still has no render-pipeline implementation.

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

`SubmissionDevice::wait_queue_idle` is a blocking completion proof for infrequent maintenance;
backends that cannot provide it return `Unavailable`.

`ResourceDevice::compute_capabilities` advertises optional buffer compute and indirect drawing.
`dispatch_compute` orders a compute-stage shader and group-zero buffer bindings before subsequent
device operations, without waiting or CPU readback. Writable buffers require `STORAGE_WRITE` in
addition to `STORAGE`. `RenderCommand::DrawIndirect` consumes four GPU-resident `u32` arguments:
vertex count, instance count, first vertex and first instance; its buffer requires `INDIRECT`.
The default capability set is empty and unsupported dispatches return `Unavailable`.
`dispatch_compute_batch` preserves stage order and visibility; backends may record the stages
in one native submission. Its default implementation queues individual dispatches.
`destroy_buffer_batch` retires buffers together when supported; other backends use ordered
individual destruction. Earlier handles may already be invalidated if a batch returns an error.
`RenderCommand::instance_count` returns `None` for indirect draws; an unknown GPU count must
not be mistaken for an empty draw or a measured CPU-side count.

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

## Compute Shader Hardware Limits

`ResourceDevice::compute_capabilities()` exposes the **actual compute dispatch
grid limits**, **per-axis local workgroup limits**, **total local invocations**
and **workgroup-shared memory budget**. Vulkan and OpenGL query the physical
adapter/context, while D3D11/D3D12 expose their Shader Model contractual
limits. Zero means a limit is unavailable.

Shader local workgroup size is defined in the **compiled entry point** (WGSL
`@workgroup_size`), not the number of workgroups passed to
`ComputeDispatch`. Changing dispatch counts cannot resize a compiled shader.

```rust
use gfx_core::{ComputeKernelProfile, ResourceDevice, Result};

fn choose_compute_kernel<D: ResourceDevice>(device: &D) -> Result<([u32; 3], usize)> {
    let caps = device.compute_capabilities();
    // These must correspond to separately precompiled shader modules.
    let variants = [32, 64, 128].map(|threads| ComputeKernelProfile {
        workgroup_size: [threads, 1, 1],
        shared_memory_bytes: 128,
    });
    let index = caps.select_precompiled_kernel(&variants, 64)
        .ok_or_else(|| gfx_core::Error::Unavailable(
            "no compute variant fits device limits".into()
        ))?;
    let grid = caps.plan_linear_dispatch_2d(10_000)?;
    Ok((grid, index))
}
```

`supports_kernel` validates local dimensions, total threads and declared
shared memory. `plan_linear_dispatch_2d` prevents oversized dispatches:
the shader must flatten `workgroup_id.x + workgroup_id.y *
num_workgroups.x` and discard padded groups.

**Limits are not optimal performance targets.** Subgroup/wave sizes,
register occupancy, bandwidth, cache behavior and GPU timestamp benchmarks
still require separate backend-specific profiling. These APIs do not compile
shader variants at runtime or promise automated performance tuning.
