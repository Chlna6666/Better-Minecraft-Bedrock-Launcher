# gfx-vulkan

[English](README.md)

`gfx-vulkan` 是 `gfx-core` 设备 trait 的 Vulkan 实现，持有 Vulkan instance、device、
queue、swapchain、资源、pipeline 与 command encoder 状态。

`raw-window-handle` 只在本 backend 内用于 native presentation surface 创建；
`gfx-core` 保持不依赖窗口句柄 crate。

## API

导入相关的 `gfx_core` 能力 trait，在 `VulkanDevice` 上调用 trait 方法。帧入口：

```rust
use gfx_core::{
    ClearColor, LoadOp, PresentationDevice, PresentFrameDescriptor, RenderCommand,
    RenderPassId, SwapchainId,
};
use gfx_vulkan::VulkanDevice;
```

原生 semaphore/fence 等同步细节留在本 crate，不进入 core 公共 API。

## 边界

- 纯图形后端；不依赖 UI 框架/应用代码。
- 呈现与 surface 生命周期通过 `gfx-core` trait 暴露。

## 构建与测试

```powershell
cargo check -p gfx-vulkan
cargo test -p gfx-vulkan -- --ignored --nocapture --test-threads=1
```

## 文档

- [nova-gfx](../README.zh-CN.md)
- [crate 目录](../../../docs/CRATES.md)
