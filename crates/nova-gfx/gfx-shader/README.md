# gfx-shader

[中文文档](README.zh-CN.md)

`gfx-shader` validates WGSL source and emits backend shader payloads for
`nova-gfx`.

It uses Naga to parse and validate WGSL, then generates:

- SPIR-V for Vulkan;
- HLSL for Direct3D 12;
- MSL for Metal.

The generated payloads are returned as `gfx_core::ShaderBinary` values so they
can be passed directly into the `GfxPipelineDevice` shader module API.
