# gfx-shader

[English](README.md)

`gfx-shader` 校验 WGSL 并为 `nova-gfx` 生成后端 shader payload：

- Vulkan：SPIR-V；
- Direct3D 12：HLSL；
- Metal：MSL。

生成结果以 `gfx_core::ShaderBinary` 返回，可直接传入 `PipelineDevice` 的 shader module API。
`compile_wgsl_to_msl`/`compile_wgsl_to_backend` 默认目标 MSL 1.0；需要更新语言特性时显式
选择版本，例如 `compile_wgsl_to_msl_versioned`。

## 边界

- 只做 shader 校验/翻译；不创建 GPU 资源。
- 生产构建优先使用 `gfx-shader-build` 在构建期编译并嵌入。

## 构建与测试

```powershell
cargo check -p gfx-shader
cargo test -p gfx-shader
```

## 文档

- [nova-gfx](../README.zh-CN.md)
- [该 crate 的 English README](README.md)
