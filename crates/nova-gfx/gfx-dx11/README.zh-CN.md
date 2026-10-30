# gfx-dx11

图形绑定在每个 render pass 内按原生槽位和 shader stage 比较；资源不变时跳过原生绑定调用，
uniform 的偏移与大小仍参与比较。图形与计算共用每个缓冲的一份 raw SRV 范围缓存，范围替换
不会使旧 bind group 持有的视图失效。呈现计时分别记录主机命令录制、DXGI Present 与 query
信号调用耗时，不能当作 GPU 执行时间。

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

有序缓冲计算支持只读 storage、raw 可写 storage 与 uniform range。派发前解绑冲突的图形
SRV，派发后清除 CS 绑定。每个缓冲保留计算用 SRV/UAV；范围变化时替换旧视图，稳定派发无需
重建视图，反复改变范围也不会使该缓存增长。支持常量缓冲偏移的驱动会在 pass 绘制前，把 GPU 生成的
间接基础偏移预存到独立的 256 字节常量范围；其他驱动按次更新 b13。可复用的分页避免连续覆盖仍被前序
绘制读取的常量。计数与偏移不需要 CPU 回读。当前为逐次间接绘制，不是原生 multi-draw。

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
