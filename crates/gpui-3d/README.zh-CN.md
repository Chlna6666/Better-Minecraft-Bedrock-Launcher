# gpui-3d

[English](README.md)

`gpui-3d` 提供可复用的 3D 场景、几何、相机、材质、动画、查询与 GPUI scene-view 层。
应用把领域数据转换成本 crate 的类型；GPUI 负责通用 `RendererExtension` 调度与 Nova 集成，
`gpui-3d` 负责 3D 场景准备与 scene-view 资源；BMCBL 专属的世界/预览策略留在 `src/ui`。

## 渲染路径

GPUI scene view 根据 `Material::double_sided` 与 `front_face` 选择光栅剔除管线；
单面材质剔除背面，双面材质保留两面。镜像变换会反转正面绕序，并在实例分批时单独分组。
`front_face` 默认为 `FrontFace::Ccw`；顺时针创作的网格应显式使用 `FrontFace::Cw`。
此管线选择目前仅接入 scene view，离屏 PBR 渲染路径仍保持原有行为。

- **GPUI scene view**：通过 `RendererExtension` 在宿主合成 pass 内绘制。
- **Offscreen renderer**：`OffscreenRenderer` 渲染方向光阴影、线性 PBR 场景、可选 IBL/天空、
  显示参考后处理到离屏目标，支持 readback、resize、trim、调试视图、延迟完成轮询与逐帧统计。
  使用调用方提供的 Nova device，不创建第二个窗口渲染器、swapchain 或缓存所有者。

`Mesh::from_voxel_quads_without_raycast` 按顺序从四边形顶点和材质分段直接构造紧凑体素网格，
无需临时完整顶点或索引缓冲；保留每面两个三角形的拓扑，并跳过网格射线查询。GPUI scene view
仅上传其紧凑顶点，由顶点着色器将每个四边形展开为两个三角形，无需上传索引；其他网格拓扑继续使用索引绘制。
`Mesh::same_voxel_quad_geometry` 可精确比较该紧凑表示，供更新时复用网格。

在声明缓冲计算及间接绘制能力的设备上（DX11、DX12、Vulkan 及满足条件的 OpenGL 驱动），体素 scene view 保留不随相机变化的
不透明绘制记录。兼容的不透明四边形分段按驻留页、材质和顶点数量级合为间接实例批次。
计算着色器判断包围盒并压缩可见实例索引；每个工作组共享视锥平面，在组内汇总计数后统一
写出间接绘制记录；单实例批次跳过工作组屏障。GPU 生成的网格将实际绘制计数接入同一剔除
输出，不再在 CPU 重复判断视锥。间接顶点数取实际可见分段的最大值，避免已隐藏的
较大分段增加补齐开销。体素顶点着色器展开四边形，并裁掉不同长度分段的
补齐部分。相机变化更新帧 uniform 并派发计算，无需重建不透明部分的 CPU 可见列表。
DX11/DX12/Vulkan 的场景参数缓冲采用设备本地有序写入，避免在途 GPU 帧读到后续 CPU 映射更新。
裁剪范围不变时跳过对应 uniform 上传。
透明绘制继续保留 CPU 深度排序与独立批次；其他后端沿用 CPU 剔除和直接绘制。
仅相机引起透明顺序变化时，scene view 排序紧凑索引，并重排驻留绑定与绘制地址。
已有深度顺序未变化时保留恒等映射，跳过排序和绘制置换。
材质、实例及剔除输入表保持驻留，仅更新帧 uniform 和计算输出；索引回退绘制也保留原实例地址。
非程序化网格分段在直接索引提交前仍执行 CPU 视锥测试。
`Mesh::from_gpu_voxels` 接收带邻接层的 `VoxelMeshSource` 网格和共享六面模板。
在具备计算和间接绘制能力的 DX11、DX12、Vulkan、OpenGL 设备上，计算工作组剔除邻接遮挡面，
生成紧凑面描述和各材质顶点计数；顶点着色器直接展开描述，生产渲染无需将计数读回 CPU。
只有网格几何变化时才重新构网，相机移动不触发构网。
`VoxelMeshSource::with_instances` 接收可复用的任意面模板、单元位置、垂直缩放和邻接类别掩码；
并行计数、各材质前缀扫描和输出 pass 压紧模型与流体可见面，并为透明材质保留实例顺序。
构网只上传一次不可变参数，并通过有序计算批次排入各阶段。生成面描述为 16 字节；常用的
8 位邻接掩码在 CPU 与 GPU 按每字四格打包，更宽掩码仍保留完整 32 位值。
CPU 保留的方块模型索引在编号小于 255 时使用字节，并保留专用的 halo 遮挡哨兵；更大的
编号使用 32 位存储。上传编码与 CPU 回退保留原始数值。场景及剔除输入表更新时批量上传。
绘制映射保留每个活跃源的缓冲，不变映射跳过上传与计算；输出缓冲被替换时，即使映射内容
不变也会重新复制，源移除时释放缓存。
可选静态面在生成的方块面之后保留各材质内部顺序。
网格计数和分段表示输出容量，而非 GPU 最终可见面数。其他渲染路径按需生成补齐的 CPU 几何；
体素网格不支持射线查询。世界解码、单元和模板提取、连接模型状态解析、LOD 选择及材质解析仍在 CPU；
应用可继续用 CPU 生成实体几何。当前不包含 GPU Hi-Z 遮挡剔除或独立异步计算队列。
`GPUI_3D_TRACE_TIMINGS` 在原有绘制统计旁输出 `gpu_cull_batches_max`、`gpu_resident_draws_max`。

`SceneView::with_culling_bounds` 可在不替换场景的情况下更新世界空间裁剪范围；节点通过
`Node::with_culling_origin` 参与判断。范围最小值包含、最大值不包含；射线查询及其他节点不受影响。

体素网格池在 GPU 可能仍读取时保留已退役区间。宿主内存整理时，DX11、DX12 和 Vulkan 会等待队列
完成信号后才使区间可复用；当前尚不支持逐帧回收或 GPU 内存预算。

`load_path` / `load_bytes` 支持 glTF 2.0 与 GLB（PBR 材质、mip 纹理、节点层级、灯光、
相机、变换动画）。

## 边界

- 依赖 `gpui` + `gfx-core`；不拥有窗口、swapchain、后端或应用策略。
- 不提供播放时钟；动画采样使用调用方提供的时间。
- 离屏渲染能力与后端状态随实现推进，具体未实现项以 crate rustdoc 为准。

## 构建与测试

```powershell
cargo check -p gpui-3d
cargo check -p gpui-3d --tests
```

`test_assets/` 是有许可证与哈希固定的测试模型资产，不是文档或运行时依赖。

## 文档

- [工作区架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
- [构建与测试](../../docs/BUILD_AND_TEST.md)
- [English README](README.md)
