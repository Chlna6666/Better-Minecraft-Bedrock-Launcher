# gfx-dx12

Graphics passes skip unchanged pipelines, scissors, and root tables. Resource-table keys include
buffer ranges and texture-view handles; root-signature changes invalidate the table cache.
Pipeline and layout objects are borrowed rather than cloned for every draw.

[中文文档](README.zh-CN.md)

`gfx-dx12` is the Direct3D 12 implementation of the `gfx-core` device traits.

Buffer compute and non-indexed indirect draws use the shared `ResourceDevice` contract.
Compute batches share one command list/fence and reuse identical buffer descriptors within
the batch. Buffer uploads reuse fence-completed command objects and mapped upload-ring pages.
Indirect argument conversion reuses a bounded pool of fence-completed scratch buffers; explicit
memory trimming releases this pool, whose retained bytes are included in resource statistics.
Compute pipelines are cached; descriptor allocations and indirect argument scratch buffers
remain alive until their submission fence completes. Portable draw records are converted on
the GPU into root constants and native draw arguments. Dispatches and draws use the ordered
graphics queue without CPU readback or a per-dispatch completion wait.

`GFX_DX12_TRACE_TIMINGS=1` reports host indirect-argument staging and draw recording separately;
these are not GPU execution times. Debug builds enable the native D3D12 debug layer by default.
`GFX_DX12_DEBUG_LAYER=0` explicitly opts out for performance comparisons; `1` requests it even
in release builds. Set it before creating any device and keep validation enabled for correctness
tests. Diagnostics do not change the portable indirect draw ABI or culling policy.

Device-local buffer writes use ordered upload batches and fence-retained staging without
waiting for completion on the CPU. CPU-visible mapped writes still require caller-owned
resource slots that are no longer being read by earlier submissions.

On Windows it creates native D3D12 devices, resources, pipelines, command
encoders, and DXGI swapchains. On non-Windows targets it still implements the
core traits with a minimal stub that returns `Error::Unavailable`, allowing
cross-platform crates to compile their non-target paths.

`raw-window-handle` is used only in this backend crate for native surface
creation. `gfx-core` does not depend on it.


## Shader compilation

Production callers should provide precompiled `ShaderCode::DxBytecode`. Runtime HLSL/FXC compilation is disabled by default; the `shader-compiler` feature explicitly enables the FXC binding and is intended for shader tools and examples rather than application startup.
