# gfx-shader-build

[English](README.md)

`gfx-shader-build` 负责构建期 WGSL 编译与嵌入：作为 **build dependency** 在 `build.rs` 调用，
为启用的后端翻译声明的 entry point，在构建主机可运行 D3D 编译器时把 DX12 shader 编译为
Direct3D bytecode，并生成供 crate 嵌入的表。

这意味着应用启动时不运行 shader 编译器；渲染器直接拿到可用的
`gfx_core::EmbeddedShader`。

## 边界

- 构建期工具，不进入运行时；不依赖完整 `gfx-dx12` runtime backend。
- 应用应只声明实际使用的 entry point 与后端，避免无谓编译。

## 使用

```rust
// build.rs
use gfx_shader_build::{Dx12ArtifactPolicy, Shader, ShaderSet, ShaderStage};
```

完整示例见 [English README](README.md)。

## 构建与测试

```powershell
cargo check -p gfx-shader-build
cargo test -p gfx-shader-build
```

## 文档

- [nova-gfx](../README.zh-CN.md)
