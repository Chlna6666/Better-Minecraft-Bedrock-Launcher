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

## Compute Shader 硬件能力查询

`ResourceDevice::compute_capabilities()` 提供 **Dispatch 网格上限**、
**Workgroup 各轴线程上限**、**工作组总线程上限**、**工作组共享内存字节上限**。
Vulkan/OpenGL 读取实际设备或上下文限制；D3D11/D3D12 使用 Shader Model
规定的资源限制。未支持或无法提供的限制用零表示。

注意：WGSL `@workgroup_size(x,y,z)` 是 **编译期 Shader 属性**，
`ComputeDispatch.workgroups` 是运行时的 **工作组数量**，两者不是一回事。

- `ComputeKernelProfile`：描述预编译计算 Shader 的线程形状与共享内存需求。
- `supports_kernel`：验证某一预编译内核能否合法运行。
- `select_precompiled_kernel`：在多个预编译变体间安全选择，并返回索引。
- `plan_linear_dispatch_2d`：按设备的 X/Y 网格上限规划 Dispatch；Shader 应将
  `workgroup_id.x + workgroup_id.y * num_workgroups.x` 作为线性工作组索引，
  并忽略网格末尾多派发的工作组。

这些接口只验证 **合法性与启发式选择**，不能推导最优 Occupancy 或性能；
Subgroup/Wave 宽度、寄存器压力、显存带宽和实际 GPU 耗时仍需要硬件基准。
Shader 变体须预编译，不会在绘制过程中动态编译。
