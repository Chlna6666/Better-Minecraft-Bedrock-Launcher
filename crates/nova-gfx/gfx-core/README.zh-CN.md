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

`Error::DeviceLost` 携带后端的移除原因，报告 GPU 设备被移除或重置。设备丢失后所有资源句柄
与提交全部失效，必须重建设备；DX11、DX12 将设备移除查询失败、Vulkan 将
`VK_ERROR_DEVICE_LOST` 映射到该变体。`PipelineLayoutDescriptor` 允许空绑定组布局列表，
无资源绑定的管线不再需要占位布局。

`SurfaceConfig::desired_image_count` 与 `desired_maximum_frame_latency` 请求旋转交换链图像
数和排队帧数。0 表示采用后端默认值；调用方可用 `resolve_image_count` 与
`resolve_frame_latency` 将请求收敛到后端范围。`DeviceDescriptor::tuning` 承载后端的池、
描述符堆、上传环、管线缓存与呈现超时策略；`DeviceTuning` 的每个字段都以 0 表示后端默认，
因此默认描述符保持各后端原有的调优策略。

`DiagnosticsDevice::frame_gpu_timestamps_supported` 与
`DiagnosticsDevice::last_frame_gpu_timestamps` 以 `FrameGpuTimestamps`（时长 tick 与计时器
频率）暴露最近一个已完成探测帧的 GPU 耗时。读取从不等待 GPU。DX11、DX12 与 Vulkan 实现
该能力；OpenGL、Metal 与 WebGL 返回 `false` 与 `None`。

`RenderPassDescriptor.color_attachments`、`RenderPipelineDescriptor.color_targets` 与
`RenderPassCommands.color_attachments` 接受多个颜色附件（上限 `MAX_COLOR_ATTACHMENTS = 8`）。
每个 `ColorTargetState` 携带各自的 `BlendMode` 与 `ColorWrites` 掩码，因此管线可对单一附件
独立启用混合或限制写入。`ResourceDevice::max_color_attachments` 报告设备上限；Vulkan 与
OpenGL 从原生限制收敛到 `MAX_COLOR_ATTACHMENTS`，D3D11/D3D12 报告 8。渲染 pass 与其管线
必须在附件数量与格式上一致，各后端在管线创建与录制时校验。交换链帧与 `record_draw_desc`
要求恰好一个颜色附件。

`SamplerDescriptor` 支持阴影（compare）采样、边框色、逐轴 W 地址模式
（`MirrorRepeat`/`ClampToBorder`）与 LOD 钳制/偏置。`compare: Some(fn)` 为深度纹理阴影贴图
选择比较过滤器；`border_color` 与 `AddressMode::ClampToBorder` 搭配生效。所有桌面后端将其
映射到原生采样器状态。Metal 保存描述符但尚未构建原生采样器；WebGL 为占位。各后端在创建时
校验 LOD 范围。

多重采样渲染与模板测试可配置。`TextureDescriptor.sample_count` 与
`RenderPassDescriptor.sample_count`（>1）请求多重采样；`DepthState.stencil` 与
`DepthAttachmentDescriptor.stencil_format` 启用模板测试。`ResourceDevice::multisample_capabilities`
报告设备的最高采样数与是否支持模板，供调用方按能力门控 MSAA/模板路径。DX11、DX12、Vulkan
与 OpenGL 实现多重采样纹理分配、多重采样渲染包与模板状态；Metal 在实现前返回 `Unavailable`。

多重采样 resolve 已端到端接线。`RenderPassCommands.resolve_target` 与
`RenderPassDescriptor.resolve_target` 指定接收多重采样 pass 结果的单采样纹理视图（仅当
`sample_count > 1` 时有效）。DX12/DX11 用 `ResolveSubresource`、Vulkan 用 `vkCmdResolveImage`、
OpenGL 用 `glBlitFramebuffer` 把 MSAA 颜色解析到目标。宿主侧的 `ExtensionDevice::
multisample_capabilities` 提供单一统一查询，渲染器无需感知具体后端即可按能力门控 MSAA 并选择采样数。

多重采样纹理同样可以作为采样输入绑定，后处理 pass 因此能直接读取多重采样几何 pass 的深度，
无需先 resolve。视图维度由各后端自行选择：DX12/DX11 创建 `TEXTURE2DMS`/`TEXTURE2DMSARRAY` SRV 与
`TEXTURE2DMS` DSV，OpenGL 用 `GL_TEXTURE_2D_MULTISAMPLE` 目标绑定。深度格式资源保持 typeless，
以便同时创建深度视图和 `R32_FLOAT` 采样视图。

多重采样的不变量不在各后端重复推导。`validate_pass_attachments` 统一校验：每个颜色/深度附件
必须与 render pass 的采样数一致、resolve 目标必须是单采样；四个桌面后端在录制 pass 前都调用它。
`conformance` feature 提供 `conformance::msaa_resolve_roundtrip`：一个后端无关的场景（纹理分配、
多重采样 pass、resolve、回读、清理），各后端的 `#[ignore]` 硬件测试直接复用它，因此新增能力只需
一份场景代码即可在所有后端验证。

`Format` 覆盖未压缩 8/16/32 位颜色：8 位 normalized 与 sRGB、半浮点（`R16Float`、`Rg16Float`、
`Rgba16Float`）、`R32Float`/`Rgba32Float`，以及 8 位无符号整数标识（`R8Uint`、`Rg8Uint`、
`Rgba8Uint`）。选择用途前可用 `is_depth`/`is_integer`/`is_texture` 判断；管线校验拒绝对整数目标启用
混合。顶点属性覆盖 `Float32`/`Float32x2..4`、`Uint32`/`Uint32x2..4`、`Unorm8x4`、`Uint8x4`、
`Unorm16x2`、`Unorm16x4`；`VertexBufferLayoutDescriptor.step_mode` 记录逐顶点/逐实例速率。顶点布局会
按 stride 与 shader location 校验。`conformance::format_allocation_roundtrip` 在每个后端分配并创建
所有格式的视图。

`TextureBufferCopy` 支持 buffer 与 2D 纹理之间双向拷贝矩形区域。行距属于设备数据而非常量：
`ResourceDevice::texture_copy_limits` 报告后端的行距与偏移对齐（Vulkan 查询驱动，D3D 用其文档化的
放置常量），`texture_copy_row_pitch` / `texture_copy_buffer_bytes` 据此计算所需字节数。
`TextureBufferCopy::validate` 拒绝深度格式、缺失 copy usage、越界矩形、未对齐或过小的 buffer。
`conformance::texture_buffer_roundtrip` 通过 buffer 把图案写入第二张纹理并逐像素比对，在每个后端运行。

`RenderCommand::DrawIndexedIndirect` 提供 GPU 生成的索引绘制：索引缓冲属于绑定状态
（`DrawIndexedIndirectCommand::index_buffer`），索引范围、实例数与 base vertex 来自 20 字节的
`DRAW_INDEXED` 参数记录。各后端先校验参数范围（4 字节对齐、带 `INDIRECT` usage、20 字节在 buffer 内）
与索引绑定一次，再发出原生索引间接调用（DX12 用带索引签名 `ExecuteIndirect`、DX11
`DrawIndexedInstancedIndirect`、Vulkan `vkCmdDrawIndexedIndirect`、OpenGL `glDrawElementsIndirect`）。
`RenderCommand::instance_count` 对它返回 `None`，因为实例数在 GPU 上。

`TextureUsage::STORAGE` 把纹理标记为着色器存储图像，通过
`BindingType::StorageTexture { format, read_only }` 绑定。校验保证存储图像为单采样、单 mip、
非深度格式（与各原生 UAV/image unit 规则一致）；`read_only` 绑定拒绝写入。后端支持分阶段：
后端支持分阶段完成。**OpenGL 已完整接通**：GLSL 反射产出 image unit（`GlslShader::images`：
名字、逻辑槽位、纹素格式、只读标记），管线分配 image unit，bind group 发出 `glBindImageTexture`；
硬件测试通过 fragment shader 写入 image unit 并回读纹素验证。DX12 已接好同一能力但尚无端到端测试：资源带 `ALLOW_UNORDERED_ACCESS`、根签名把该绑定映射为
UAV range、bind group 创建 unordered-access view。Vulkan 直接把存储图像创建在 `GENERAL` 布局，因此其
`STORAGE_IMAGE` 绑定不需要录制期迁移；两者都还缺端到端测试。DX11 返回 `Unavailable`，因为它的 UAV
路径需要 dispatch API 提供 compute bind group 入口，因此存储绑定在该后端会明确报错，而不是被静默
当成采样纹理。

`ComputeBindGroupDispatch` 是渲染 bind group 在 compute 侧的对应物：携带 shader 模块、一个资源集
（纹理、存储图像、采样器）与 workgroup 计数，使存储图像可以从 compute shader 访问，而不受只支持 buffer
的 `ComputeDispatch` 限制。`ResourceDevice::dispatch_compute_bind_group` 默认返回 `Unavailable`，
因此无法绑定 compute 资源集的后端绝不会静默丢掉调用方的纹理绑定。OpenGL 已实现：compute shader 通过该资源集写存储图像，
并有硬件测试验证。

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
