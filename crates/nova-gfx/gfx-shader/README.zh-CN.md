# gfx-shader

[English documentation](README.md)

`gfx-shader` 为 `nova-gfx` 校验 WGSL 源码，并生成后端 shader payload。

它使用 Naga 解析并校验 WGSL，然后生成：

- Vulkan 使用的 SPIR-V；
- Direct3D 12 使用的 HLSL；
- Metal 使用的 MSL。

生成结果以 `gfx_core::ShaderBinary` 返回，可直接传给
`GfxPipelineDevice` 的 shader module 创建接口。
