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

`Error::DeviceLost` reports GPU device removal or reset with the backend's removal reason as
its payload. After a lost device every resource handle and submission is invalid and the device
must be recreated; DX11 and DX12 map failed device-removal queries and Vulkan maps
`VK_ERROR_DEVICE_LOST` to it. `PipelineLayoutDescriptor` accepts an empty bind-group-layout
list so pipelines without resource bindings do not need a dummy layout.

`SurfaceConfig::desired_image_count` and `desired_maximum_frame_latency` request rotating
swapchain images and queued frames. Zero selects the backend default; use
`resolve_image_count` and `resolve_frame_latency` to clamp a request to a backend range.
`DeviceDescriptor::tuning` carries backend pool, descriptor-heap, upload-ring, pipeline-cache
and present-timeout policy. Every `DeviceTuning` field uses zero for the backend default, so a
default descriptor keeps each backend's tuned policy.

`DiagnosticsDevice::frame_gpu_timestamps_supported` and
`DiagnosticsDevice::last_frame_gpu_timestamps` expose the GPU duration of the most recently
completed probed frame as `FrameGpuTimestamps` (duration ticks plus timer frequency). Reading
never waits for GPU work. DX11, DX12 and Vulkan implement it; OpenGL, Metal and WebGL return
`false` and `None`.

`ResourceDevice::gpu_work_timestamps_supported`, `dispatch_compute_batch_profiled`,
`write_buffer_batch_profiled` and `drain_gpu_work_timestamps` provide a separate
non-blocking, batch-tagged workload timer. A `GpuTimestampTag` carries a stable
`batch_id` plus `GpuTimestampStage`; completed `GpuTimestampSample` values
return native ticks and frequency, never a CPU API-call duration. D3D11, D3D12,
Vulkan and desktop OpenGL 4.5 provide native compute/upload probes. Metal and
WebGL retain the unsupported default; profiled calls still execute normally.
The OpenGL buffer-write probe measures the GPU command stream around
`glBufferSubData` and does not guarantee isolation of a physical copy.
`DrawIndirect` is a reserved stage but currently has no per-draw probe:
a frame/offscreen-pass duration must not be mislabeled as indirect-draw time.

`RenderPassDescriptor.color_attachments`, `RenderPipelineDescriptor.color_targets`, and
`RenderPassCommands.color_attachments` accept multiple color attachments (up to
`MAX_COLOR_ATTACHMENTS = 8`). Each `ColorTargetState` carries its own `BlendMode` and
`ColorWrites` mask, so a pipeline can enable blending or restrict writes per attachment.
`ResourceDevice::max_color_attachments` reports the device limit; Vulkan and OpenGL clamp it to
`MAX_COLOR_ATTACHMENTS` from their native limits, while D3D11/D3D12 report eight. A render pass
and its pipelines must agree on attachment count and formats; backends validate this at pipeline
creation and at record time. Swapchain frames and `record_draw_desc` require exactly one color
attachment.

`SamplerDescriptor` supports shadow (compare) sampling, border colors, a per-axis W address mode
(`MirrorRepeat`/`ClampToBorder`), and LOD clamping/bias. `compare: Some(fn)` selects a comparison
filter for depth-texture shadow maps; `border_color` applies with `AddressMode::ClampToBorder`.
All desktop backends map these to their native sampler state. Metal stores the descriptor but does
not build the native sampler yet; WebGL is a stub. Backends validate the LOD range at creation.

Multisampled rendering and stencil support are configurable. `TextureDescriptor.sample_count`
and `RenderPassDescriptor.sample_count` (>1) request multisampling; `DepthState.stencil` and
`DepthAttachmentDescriptor.stencil_format` enable stencil testing. `ResourceDevice::
multisample_capabilities` reports the device's maximum sample count and whether stencil is
supported, so callers can gate MSAA/stencil paths on capability. DX11, DX12, Vulkan and OpenGL
implement multisampled texture allocation, multisampled render packs, and stencil state; Metal
returns `Unavailable` for MSAA/stencil until implemented.

Multisample resolve is wired end to end. `RenderPassCommands.resolve_target` and
`RenderPassDescriptor.resolve_target` name the single-sample texture view that receives the
resolved result of a multisampled pass (valid only when `sample_count > 1`). DX12 uses
`ResolveSubresource`, DX11 `ResolveSubresource`, Vulkan `vkCmdResolveImage`, and OpenGL
`glBlitFramebuffer` to resolve the MSAA color into the target. A host-side
`ExtensionDevice::multisample_capabilities` exposes a single unified query so a renderer can gate
MSAA and choose a sample count without knowing the concrete backend.

Multisampled textures are also bindable as sampled inputs, so a post pass can read the geometry
depth of a multisampled pass without resolving it. Backends select the multisample view dimension
themselves: DX12 and DX11 create `TEXTURE2DMS`/`TEXTURE2DMSARRAY` SRVs and `TEXTURE2DMS` DSVs, and
OpenGL binds the texture with its `GL_TEXTURE_2D_MULTISAMPLE` target. Depth-format resources stay
typeless so both the depth view and an `R32_FLOAT` sampled view can be created over them.

Backends do not re-derive the multisample invariants. `validate_pass_attachments` checks, once,
that every color and depth attachment shares the render pass sample count and that a resolve
destination is single-sampled; all four desktop backends call it before recording a pass. The
`conformance` feature exposes `conformance::msaa_resolve_roundtrip`, one backend-neutral scenario
(texture allocation, multisampled pass, resolve, readback, cleanup) that each backend's `#[ignore]`d
hardware test runs, so a new capability is verified against every backend from a single body.

`Format` covers uncompressed 8/16/32-bit color: normalized and sRGB 8-bit, half-float (`R16Float`,
`Rg16Float`, `Rgba16Float`), `R32Float`/`Rgba32Float`, and 8-bit unsigned integer identifiers
(`R8Uint`, `Rg8Uint`, `Rgba8Uint`). `is_depth`/`is_integer`/`is_texture` let callers ask before
choosing a usage, and pipeline validation rejects blending on integer targets. Vertex attributes
cover `Float32`/`Float32x2..4`, `Uint32`/`Uint32x2..4`, `Unorm8x4`, `Uint8x4`, `Unorm16x2` and
`Unorm16x4`; `VertexBufferLayoutDescriptor.step_mode` records per-vertex versus per-instance rate.
Vertex layouts are validated against their stride and shader locations. `conformance::
format_allocation_roundtrip` allocates and views every format on each backend.

 copies a rectangle between a buffer and a 2D texture in either direction.
Row pitch is device data, not a constant: `ResourceDevice::texture_copy_limits` reports the
backend's row-pitch and offset alignment (Vulkan queries the driver, D3D uses its documented
placement constants), and `texture_copy_row_pitch` / `texture_copy_buffer_bytes` size the buffer
from it. `TextureBufferCopy::validate` rejects depth formats, missing copy usages, out-of-bounds
rectangles and misaligned or undersized buffers. `conformance::texture_buffer_roundtrip` writes a
pattern through a buffer into a second texture and compares the pixels on every backend.

`RenderCommand::DrawIndexedIndirect` adds GPU-generated indexed draws: the index buffer is bound
state (`DrawIndexedIndirectCommand::index_buffer`) while the index range, instance count and base
vertex come from the 20-byte `DRAW_INDEXED` argument record. Backends validate the argument range
(4-byte aligned, `INDIRECT` usage, 20 bytes inside the buffer) and the index binding once, then
issue their native indexed-indirect call (DX12 `ExecuteIndirect` with an indexed signature, DX11
`DrawIndexedInstancedIndirect`, Vulkan `vkCmdDrawIndexedIndirect`, OpenGL `glDrawElementsIndirect`).
`RenderCommand::instance_count` returns `None` for it, because the count lives on the GPU.

`TextureUsage::STORAGE` marks a texture as a shader storage image, bound through
`BindingType::StorageTexture { format, read_only }`. Validation keeps storage images
single-sampled, single-level and non-depth, matching every native UAV/image-unit rule;
`read_only` bindings reject writes. Backend support is staged. OpenGL is complete: GLSL reflection reports image units
(`GlslShader::images` with name, logical slot, texel format and read-only access), pipelines
assign image units, and bind groups issue `glBindImageTexture`; a hardware test writes through a
fragment shader's image unit and reads the texels back. DX12 wires the same capability without an end-to-end test yet: the resource carries
`ALLOW_UNORDERED_ACCESS`, the root signature maps the binding to a UAV range, and bind groups
create unordered-access views. Vulkan creates storage images directly in `GENERAL`, so its
`STORAGE_IMAGE` binding needs no recorded transition; both backends still lack an end-to-end test.
DX11 returns `Unavailable` because its UAV path needs a compute bind-group entry that the
dispatch API does not expose yet, so a storage binding there fails loudly rather than silently
binding as a sampled texture.

`ComputeBindGroupDispatch` is the compute counterpart of a render bind group: it carries a
shader module, one resource set (textures, storage images, samplers) and workgroup counts, so a
storage image is reachable from a compute shader without the buffer-only `ComputeDispatch` path.
`ResourceDevice::dispatch_compute_bind_group` defaults to `Unavailable`, so a backend that cannot
bind a compute resource set never silently drops the caller's texture bindings. OpenGL implements
it: a compute shader writes a storage image through the set, verified by a hardware test.

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
