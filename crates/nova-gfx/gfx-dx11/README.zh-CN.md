# gfx-dx11

[English](README.md)

`gfx-dx11` 是 nova-gfx 的 Windows Direct3D 11 原生后端，要求硬件 feature level 11.0。
使用 D3D11 immediate context、DXGI flip-model swapchain 与 DirectComposition 呈现
premultiplied 窗口表面。

生产 WGSL 在构建期编译为 Shader Model 5.0 bytecode。固定资源槽保持与 DX12 共享的渲染契约；
draw offset 使用保留的内部常量缓冲 b13，应用 uniform 使用 b0–b12。

## 能力

sampled BGRA/RGBA 纹理、mip view、shader-pulled vertices、indexed draw、uniform range、
depth/dual-source blending、GPU 完成查询、原生 pacing 与 readback。resize 保留原链；
opaque/premultiplied 切换准备替换链；format 变更需要新 surface。

## 边界

- 纯图形后端；不依赖 UI 框架/应用代码。
- 呈现与 surface 生命周期通过 `gfx-core` trait 暴露；固有方法不作为公共契约。

## 构建与测试

```powershell
cargo check -p gfx-dx11
cargo test -p gfx-dx11 -- --ignored --nocapture --test-threads=1
```

## 文档

- [nova-gfx](../README.zh-CN.md)
- [构建与测试](../../../docs/BUILD_AND_TEST.md)
