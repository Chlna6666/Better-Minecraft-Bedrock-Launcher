# gfx-shader

[English documentation](README.md)

`gfx-shader` 为 `nova-gfx` 校验 WGSL 源码，并生成后端 shader payload。

它使用 Naga 解析并校验 WGSL，然后生成：

- Vulkan 使用的 SPIR-V；
- Direct3D 12 使用的 HLSL；
- Metal 使用的 MSL。

生成结果以 `gfx_core::ShaderBinary` 返回，可直接传给
`PipelineDevice` 的 shader module 创建接口。

`compile_wgsl_to_msl` 和 `compile_wgsl_for_backend` 默认目标为 MSL 1.0。shader
需要较新语言特性时，可显式选择版本：

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

GPUI 3D 在 Metal 上为 WGSL `instance_index` 选择 MSL 1.2；其他 shader 仍使用默认目标。
翻译成功只证明生成了 MSL 源码，仍需在原生 Metal 设备上验证。
