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

`RenderPipelineDescriptor::raster` 使用 `RasterState` 指定投影后的正面绕序与剔除面。
默认 `FrontFace::Ccw`、`cull_mode: None` 保留双面绘制；设置 `Some(Face::Back)` 开启光栅器背面剔除。
DX11、DX12、Vulkan、OpenGL 与 Metal 映射此状态；WebGL 当前仍未实现渲染管线。

`SubmissionDevice::wait_queue_idle` 为低频维护提供阻塞式 GPU 完成证明；无法提供此能力的
后端返回 `Unavailable`。

`ResourceDevice::compute_capabilities` 声明可选的缓冲计算与间接绘制能力。
`dispatch_compute` 将计算阶段着色器及 group-zero 缓冲绑定按设备顺序排入后续操作之前，
不等待完成或回读 CPU。可写缓冲在 `STORAGE` 之外需要 `STORAGE_WRITE`。
`RenderCommand::DrawIndirect` 从带 `INDIRECT` 用途的缓冲读取四个 GPU 驻留 `u32` 参数：
顶点数、实例数、首顶点及首实例。默认能力集合为空，不支持的派发返回 `Unavailable`。
`dispatch_compute_batch` 保持阶段顺序与写入可见性；后端可在一次原生提交中记录多个阶段，
默认实现逐次排入派发。
`destroy_buffer_batch` 允许后端合并缓冲回收提交，其他后端按顺序逐个销毁；批次报错时，
部分先前句柄可能已经失效。
间接绘制的 `RenderCommand::instance_count` 返回 `None`；未知的 GPU 计数不能当作空绘制或已测得的 CPU 计数。

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
