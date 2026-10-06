# gfx-shader

[中文文档](README.zh-CN.md)

`gfx-shader` validates WGSL source and emits backend shader payloads for
`nova-gfx`.

It uses Naga to parse and validate WGSL, then generates:

- SPIR-V for Vulkan;
- HLSL for Direct3D 12;
- MSL for Metal.

The generated payloads are returned as `gfx_core::ShaderBinary` values so they
can be passed directly into the `PipelineDevice` shader module API.

`compile_wgsl_to_msl` and `compile_wgsl_for_backend` target MSL 1.0 by default.
When a shader needs a later language feature, select the version explicitly:

```rust
use gfx_core::ShaderStage;
use gfx_shader::{MslVersion, compile_wgsl_to_msl_with_version};

let shader = compile_wgsl_to_msl_with_version(
    source,
    ShaderStage::Vertex,
    "vs_main",
    MslVersion::V1_2,
)?;
```

GPUI 3D uses MSL 1.2 on Metal for WGSL `instance_index`; other shaders keep the
default target. A successful translation confirms source generation only; the
native Metal backend still needs device-level validation.
