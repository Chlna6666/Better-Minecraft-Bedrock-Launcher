# gfx-dx11

Graphics bindings are compared per native slot and shader stage within each render pass.
Unchanged resources skip native binding calls; uniform offsets and sizes remain part of the key.
Graphics and compute reuse one cached raw SRV range per buffer. Replacing the cached range keeps
older bind-group views alive. Presentation timings separate host command recording, DXGI Present,
and query signaling; these are host-call durations rather than GPU execution times.

[中文文档](README.zh-CN.md)

Native Windows Direct3D 11 backend for nova-gfx, requiring hardware feature
level 11.0. It uses D3D11 immediate-context commands, DXGI flip-model swapchains
and DirectComposition for premultiplied window surfaces.

Production WGSL is compiled to Shader Model 5.0 bytecode at build time. Fixed
resource slots preserve the shared rendering contracts without changing the
DX12 shader path. Draw offsets use the reserved internal constant buffer b13;
application uniforms use b0 through b12.

The backend supports sampled BGRA/RGBA textures, mip views, shader-pulled
vertices, indexed draws, uniform ranges, depth and dual-source blending, GPU
completion queries, native pacing and readback. Resize retains the native chain;
opaque/premultiplied transitions prepare a replacement chain. Format changes
require a new surface.

Ordered buffer compute supports read-only storage, raw writable storage and uniform ranges.
Dispatch unbinds conflicting graphics SRVs before writing and clears CS bindings afterward.
Compute SRV/UAV views are retained per buffer; a range change replaces the cached view, so
stable dispatches do not recreate views and repeated range changes cannot grow that cache.
GPU-generated non-indexed indirect draws stage base offsets into separate 256-byte constant
ranges before each pass when constant-buffer offsetting is supported; older drivers use b13
per draw. Reusable pages avoid repeated writes to constants still being consumed by prior draws.
Counts and offsets never require CPU readback. This is individual
indirect drawing, not a native multi-draw command.

Hardware gates:

```powershell
cargo test -p gfx-dx11 -- --ignored --nocapture --test-threads=1
```

Production glyph and image paths are validated by the consumer renderer with
its production shader artifacts, atlas formats, encoders, sprite packing, and
layouts.
