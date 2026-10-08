# gpui-3d

`gpui-3d` 提供可复用的 3D 场景、几何、相机、材质、动画、查询和 GPUI 场景视图。应用层把
领域数据转换成这些通用类型；GPUI 负责通用 renderer extension 调度和 Nova 接入，`gpui-3d`
负责 3D 场景准备与场景视图资源，BMCBL 的世界数据和预览策略留在 `src/ui`。

该 API 仍在开发中，crate 暂不发布。Map Viewer 和 Skin Pack 已使用本 crate 构建 3D 预览场景。
GPUI 负责通用 `RendererExtension` 生命周期；GPUI 专用 mesh primitive 及 Nova 上传、缓存和绘制路径
已移除，不保留兼容 wrapper。`gpui-3d` 与 BMCBL 检查通过，CPU scene 示例已实际运行；修复场景视图元素填满父布局后，
DX12 和 Vulkan 原生窗口均已看到场景几何与三个共享资源的小球实例。帧时间和 GPU draw 数尚未测量。

## 当前能力

- 带材质分段的 indexed mesh、按面积加权的法线生成、MikkTSpace 切线生成与镜像 UV 接缝拆点、可选三角形边缘 mask、通过 `Mesh::with_vertices` 更新固定拓扑快照，以及 cube、plane、UV sphere、cylinder 和 cone 几何。
- 支持平移、四元数旋转、缩放的父子场景节点。
- 环境光、方向光、点光和软边聚光灯；metallic-roughness 和 unlit 材质；sRGB albedo、线性 normal map 与 R 通道 AO 贴图；
  opaque、mask 和 premultiplied blend 透明模式。
- 相机视锥裁剪、透明面从后向前排序、每个场景视图复用 GPU 网格资源，以及 BVH 加速的射线查询。
- 相邻且 mesh part、完整材质值相同的 opaque/mask draw 会自动合并为 indexed instanced draw。
  `PreparedScene::draw_batches()` 无分配地返回相同分组；Blend draw 始终单独提交，以保留后向前顺序。
- 绝对时间的平移、缩放和旋转轨道。`Scene::evaluate` 不修改源场景；重复 CPU 采样可通过
  `Scene::evaluate_with` 和 `AnimationScratch` 复用变换表容量。
- 基于 Nova indexed rendering、并填满可用父布局尺寸的 GPUI 场景视图元素。
- 相机可投影到元素 bounds、可见矩形交集或交集内的居中正方形；支持按轴响应式内缩和 blend 边缘线性渐隐。
- 透视与正交相机，提供经过校验的 orbit、pan、zoom 和场景 bounds 取景。`Scene::bounds()` 与
  `EvaluatedScene::bounds()` 分别查询 authored 和 sampled 姿态的世界空间网格范围；
  `OrbitCamera::fit_bounds()` 会把相机 target 移到范围中心。场景视图的视口像素拾取通过
  `SceneViewRaycastScratch` 复用动画和遍历缓冲。

贴图由不可变 RGBA8 `TextureAsset` 提供，材质通过 asset ID 引用。场景视图会上传调用方提供的
mip 链，默认使用线性纹素/级间过滤和边缘钳制。调用 `SceneView::with_anisotropy(true)` 可请求硬件各向异性过滤；Vulkan 在设备支持时使用其报告上限，否则使用已配置的过滤模式。Nova 的 DX11、DX12、Vulkan 和 OpenGL 4.5 支持贴图上传。`gfx-metal` 尚未实现像素上传；
shader 编译通过不代表 Metal 贴图可用。

AO 贴图通过网格 UV 采样线性 R 通道，只衰减环境光。使用 `TextureAsset::linear_rgba8` 创建，
渲染器会拒绝 sRGB AO 贴图。`occlusion_strength` 限定在 `0..=1`；方向光、点光和聚光灯不受影响。

法线贴图也必须使用线性 RGBA8，渲染器会拒绝 sRGB 法线贴图。使用 normal map 的 lit metallic-roughness 材质要求网格包含切线数据；可通过
`Mesh::generate_tangents()` 按 UV set 0 生成 MikkTSpace 切线，或用 `Mesh::with_tangents()`
附加导入器提供的切线。生成器会在切线接缝处分割顶点，并返回源顶点映射。

当前支持调用方生成并提供的 mip 链，但不负责生成。尚无 shadow map、HDR 环境光、自定义材质程序、
mesh pass、骨骼或 morph 变形、约束与 IK、headless capture 和异步 readback。shader 编译与 CPU
基准不能证明真实窗口像素正确或帧时间达标。

`TextureAsset::rgba8` 和 `TextureAsset::linear_rgba8` 创建单级贴图；`rgba8_mip_chain` 与
`linear_rgba8_mip_chain` 接收从 level 0 开始、每级宽高向下折半并至少为 1 的像素数据，允许不完整链。
纹理解码、颜色空间转换和 mip 生成由调用方负责。Nova 在 DX11、DX12、Vulkan 和 OpenGL 上支持 mip 链上传；Metal 上传尚未实现。

聚光灯内外锥角使用弧度，且必须满足 `0 <= inner < outer < PI`。节点变换会移动灯光，并按最大轴缩放影响距离。
内锥范围保持完整强度，向外锥平滑衰减；距离衰减仍按影响范围计算。

## 示例

运行 CPU 场景、动画、绘制准备和拾取示例：

```powershell
cargo run -p gpui-3d --example scene
```

运行原生场景视图示例，可选择已编译的原生后端：

```powershell
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx12
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-vulkan
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx11
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-opengl
```

CPU scene 示例通过 `Mesh::with_vertices` 替换顶点属性，保留源快照并重建 bounds 和查询数据；它还会生成导入三角面的法线和切线，跨绝对时间样本复用 `AnimationScratch`。示例读取动画姿态的世界空间场景 bounds 来调整正交相机取景，再演示 orbit/pan/zoom、场景射线和 viewport 像素拾取。

原生窗口示例展示使用两级 sRGB albedo mip 链、法线贴图和生成切线的金属 cube、带 AO 贴图的 PBR sphere，以及共享同一 mesh 与材质的三个小球实例。
这些小球演示 indexed instancing。示例也展示点光源与聚光灯、unlit 标记和
半透明球体。它还演示 edge-mask quad、像素偏移、depth bias、响应式内缩、可见区域投影和
blend 边缘渐隐。渐隐仅作用于 Blend 材质。圆角 content mask 仍由宿主矩形 scissor 处理，当前
不会生成 3D 圆角 coverage。皮肤预览可选
`ProjectionRegion::VisibleSquare`，在同一可见交集中居中适配正方形。`SceneView::with_animation`
为场景节点设置经过校验的绝对时间轨道，
`SceneView::with_animation_time` 选择另一个采样时间；`SceneView::raycast` 与绘制准备使用同一姿态，
`SceneViewRaycastScratch` 在重复拾取时复用变换和遍历缓冲。原生示例使用固定动画样本，不驱动自主播放。
`with_camera`、`with_scene`、`with_textures`、`with_projection_region`、`with_projection_inset` 和
`with_blend_edge_feather`
更新不可变快照并保留 renderer identity 与兼容 GPU 资源。`VisibleContent` 模式下，拾取坐标和尺寸
必须相对同一个投影矩形。替换 scene 时会清除绑定旧节点句柄的轨道。

`Mesh::with_vertices` 用于替换固定拓扑的 CPU 几何快照：顶点数量保持不变，indices、材质分段和
edge mask 会保留，mesh/part bounds 与射线查询 BVH 会重建。更新会清除切线；使用法线贴图前需重新生成或附加切线。
每个替换快照有新的 mesh identity，提交后会完整上传几何；当前不支持局部原位 GPU 更新。

## 基准和检查

```powershell
cargo bench -p gpui-3d --bench raycast
cargo bench -p gpui-3d --bench scene
cargo test --locked -p gpui-3d
cargo check --locked -p gpui-3d --all-targets --all-features
```

Criterion 基准测量 CPU BVH 射线查询、authored/evaluated 场景 bounds、场景准备与容量复用、
动画采样及绘制准备、100/1,000/10,000 个重复 draw 的合批查找、内置几何生成，以及
固定拓扑顶点快照重建和 64×32 UV 球体的 MikkTSpace 切线生成。单独运行某个基准组：

```powershell
cargo bench -p gpui-3d --bench scene -- scene/batching
cargo bench -p gpui-3d --bench scene -- scene/bounds
cargo bench -p gpui-3d --bench scene -- vertex-snapshot/uv-sphere-64x32
cargo bench -p gpui-3d --bench scene -- generate-tangents/uv-sphere-64x32
```

`gpui-3d` Criterion 基准测量 CPU 工作，不测量 GPU 呈现延迟。Nova `mip-chain` 基准测量
128×128、8 级 RGBA8 贴图的批量上传成本；设备支持时间戳时同时报告 GPU copy 时间，但不代表帧时间。

```powershell
cargo test --locked -p gfx-dx12 --test texture_transfer
cargo test --locked -p gfx-vulkan --test texture_transfer
cargo bench --locked -p gfx-dx12 --bench texture_write -- mip-chain
cargo bench --locked -p gfx-vulkan --bench texture_write -- mip-chain
```

参阅[GPUI 3D 架构与 API 指南](../../docs/GPUI_3D.md)，了解职责边界、坐标约定、材质、资源生命
周期、示例、基准解释和后端验证矩阵。

应用侧转换示例见 [Map Viewer](../../src/ui/window/map_viewer/preview_3d.rs) 和
[Skin Pack](../../src/ui/window/skin_pack/mesh.rs)；Minecraft 专用数据转换应留在这些应用模块。
