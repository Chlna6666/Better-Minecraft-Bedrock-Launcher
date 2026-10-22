# gfx-dx12

[English](README.md)

`gfx-dx12` 是 `gfx-core` 设备 trait 的 Direct3D 12 实现。Windows 上创建原生 D3D12 设备、
资源、pipeline、command encoder 与 DXGI swapchain；非 Windows 目标仍实现 trait，但以
`Error::Unavailable` 的 stub 返回，便于跨平台 crate 编译非目标路径。

`raw-window-handle` 只在本 backend 内用于 native surface 创建；`gfx-core` 不依赖它。

## Shader 编译

生产调用方应提供预编译的 `ShaderCode::DxBytecode`。运行时 HLSL/FXC 编译默认关闭；
`shader-compiler` feature 显式启用 FXC 绑定，面向 shader 工具/示例，而不是应用启动。

## 边界

- 纯图形后端；不依赖 UI 框架/应用代码。
- 呈现与 surface 生命周期通过 `gfx-core` trait 暴露。

## 构建与测试

```powershell
cargo check -p gfx-dx12
cargo test -p gfx-dx12 -- --ignored --nocapture --test-threads=1
```

## 文档

- [nova-gfx](../README.zh-CN.md)
