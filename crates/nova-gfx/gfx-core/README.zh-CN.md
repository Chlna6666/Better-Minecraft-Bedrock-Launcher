# gfx-core

[English](README.md)

`gfx-core` 定义 `nova-gfx` 的后端无关公共 API：

- 类型化 generational 资源句柄；
- resource/pipeline/command/surface/presentation/diagnostics 描述符；
- `Error` 与 `Result`；
- 能力 trait：`Backend`、`SurfaceDevice`、`ResourceDevice`、`PipelineDevice`、`CommandDevice`、
  `SubmissionDevice`、`PresentationDevice`、`DiagnosticsDevice` 与 `Device`。

词汇与 wgpu 对齐：`BindGroup`/`BindGroupLayout`、`RenderCommand`、`RenderPassCommands`、
`PresentFrameDescriptor`、`render_to_textures`。没有兼容别名 trait，也没有 UI 框架策略。

## 边界

`gfx-core` 不依赖 UI 框架、winit、`raw-window-handle`、Vulkan、Direct3D 12 或 Metal。
native surface target 通过 `SurfaceDevice::SurfaceTarget` 由各 backend 定义。

## 使用建议

```rust
use gfx_core::{BufferDescriptor, BufferId, ResourceDevice, Result};

fn create_upload_buffer<D>(device: &mut D, desc: &BufferDescriptor) -> Result<BufferId>
where
    D: ResourceDevice,
{
    device.create_buffer(desc)
}
```

只在确实需要完整设备契约时使用 `Device`；backend 应实现 trait，而不是暴露重复的固有方法。

## API 稳定性

`0.1.x` 是早期破坏性 API 线；trait 是维护中的公共契约。

## 文档

- [nova-gfx](../README.zh-CN.md)
- [crate 目录](../../../docs/CRATES.md)
