# gfx-vulkan

图形命令在每个 pass 内跳过不变的 pipeline、scissor 与描述符集合；描述符复用要求 pipeline
layout 相同。各 draw 的描述符共享一份连续临时存储，避免逐 draw 分配描述符向量。

[English](README.md)

计算批次共用一份命令缓冲和完成信号，并为相同绑定复用兼容的描述符集。
缓冲上传回收并复用 fence 已完成的上传命令池。
缓冲批量回收共用一次清理 fence 与队列提交；句柄先验证再失效，原生资源保留到 fence 完成。

所选 graphics queue 支持 compute 时，提供统一缓冲计算接口。非索引间接绘制要求
`drawIndirectFirstInstance` 特性。计算 pipeline 可复用，descriptor pool 保留到提交 fence 完成。
计算与绘制使用同一有序队列及内存屏障，无需 CPU 回读或每次派发等待完成。

设备本地缓冲写入通过目的缓冲屏障与有序上传同步，暂存资源保留到 fence 完成，CPU 不逐批等待。
CPU 可见的映射写入仍要求调用方使用已不再被前序提交读取的资源槽位。

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
