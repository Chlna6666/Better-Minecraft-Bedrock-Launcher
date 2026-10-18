# GPUI 3D 架构与 API

本文档定义 `gpui-3d` 的工作区边界与当前契约，是 3D API 归属、场景准备、scene-view 与离屏渲染器生命周期、glTF 资源导入、示例、测试资源以及 GPU 验证范围的权威说明。

当前 crate 在同一套场景/资源/材质模型上提供两条互补的渲染路径：

1. **GPUI scene-view**：通过通用 `RendererExtension` 契约渲染在宿主合成 pass 内，是 Map Viewer 和 Skin Pack 使用的 UI 内路径。
2. **离屏渲染器**（`gpui_3d::OffscreenRenderer`）：由应用持有的多 pass GPU 帧，包含方向光阴影 pass、线性 HDR PBR 场景 pass、半分辨率泛光和 HDR 后处理 resolve（曝光、色调映射、FXAA、调色/颗粒），最终写入可回读或交给合成器的离屏纹理。它只使用调用方传入的 Nova device，从不创建第二个窗口渲染器、swapchain、缓存所有者或渲染队列。

glTF 2.0 / GLB 导入器（`gpui_3d::load_path` / `load_bytes`）把有许可证的测试模型与应用资源转换为两条渲染路径共用的 `Scene`、`Mesh`、`Material`、`TextureAsset`、灯光、相机与动画轨道类型。导入器是纯 CPU 实现，并针对畸形输入、资源上限、路径逃逸和不支持的必需扩展做了验证。

## 职责归属

```text
BMCBL 地图 / 皮肤预览数据
    -> src/ui 转换领域几何与材质
    -> crates/gpui-3d Scene, Mesh, Camera, Material, SceneView
    -> crates/gpui RendererExtension 生命周期
    -> nova-gfx indexed draw steps 与后端资源
```

- `crates/gpui-3d` 负责后端无关的场景对象、几何校验、相机、材质、动画采样、CPU 剔除、射线查询，以及基于 Nova 的 GPUI scene-view。
- `crates/gpui` 负责通用 element、窗口、输入、renderer-extension 调度与原生后端集成；不得定义 Minecraft 网格、BMCBL 预览材质或 3D 产品策略。
- `src/ui` 负责 BMCBL 数据到 3D 类型的转换和预览行为；`gpui-3d` 不得依赖 `src/`、BMCBL crate、Minecraft 资源名或页面状态。
- `crates/nova-gfx` 负责设备无关的 texture、buffer、pipeline 与 draw 资源契约。

[cradiy/gpui 的 gpui_3d 分支](https://github.com/cradiy/gpui/tree/main/crates%2Fgpui_3d) 是功能与边界参考，不是上游依赖，也不代表上游 GPUI API。本工作区维护自己的 crate 与 API 表面。

## 功能状态与完成门槛

当前 crate 是基础实现，并非参考分支的完整等价物。每个受支持功能只有在 API、实现、示例和针对性验证都存在后才算完成；在此之前必须保持下列缺口可见。

| 领域 | 当前本地 API | 尚缺的 3D 工作 |
| --- | --- | --- |
| 几何 | 不可变索引网格、材质 part、面积加权法线、MikkTSpace 切线、逐三角形边缘掩码、五种内置图元、bounds、BVH 查询、自动索引实例化；glTF indexed/non-indexed primitive、morph-target 基础几何、蒙皮 bind pose | GPU 蒙皮/morph 形变、GPU 几何局部更新、压缩网格扩展 |
| 场景图 | 带 generation 的层级节点、用于 retained frame 失效的 mutation version、渲染/阴影/拾取/bounds 均尊重的节点与子树可见性、按距离选择的渲染 LOD（渲染使用层级网格，bounds/raycast 仍用 authored mesh） | 屏幕空间/HLOD 选择、layer/culling mask、user metadata、gizmo/overlay 线 |
| 相机与拾取 | 透视/正交投影、orbit/pan/zoom、bounds framing、CPU 射线查询、glTF 相机、左上角 viewport pick/project helper、与提交帧一致的 GPU object-id 拾取 | 深度 GPU 拾取、异步回读 |
| 材质与光照 | metallic-roughness 与 unlit 材质、albedo/normal/occlusion/**metallic-roughness** 贴图、四种灯光、三种 alpha 模式；离屏路径增加方向阴影、等距柱状图 IBL（mip 预过滤的漫反射/镜面 + Karis 解析 split-sum BRDF）、环境天空背景、线性 HDR、曝光与 ACES/Reinhard 色调映射、泛光、FXAA 风格边缘过滤、vignette/film grain/dither 调色 | 预过滤 cubemap/irradiance probe 与真正的 BRDF LUT、clustered Forward+、点光/聚光阴影、MSAA/TAA |
| 渲染路径 | retained GPUI scene-view；应用持有 Nova device 的**离屏多 pass 渲染器**（阴影、HDR PBR 场景、半分辨率泛光、HDR 后处理），含网格/纹理缓存、资源集复用、回读、resize、trim/budget 驱逐、哈希跟踪增量上传、可复用 CPU staging buffer、有界 mesh-buffer 复用池、动态分辨率/超采样 render scale、capture 前的延迟 GPU 完成轮询和逐帧统计 | 离屏目标免 CPU 回读交给 GPUI compositor、GPU-driven indirect、时序效果 |
| 场景规模与复用 | retained draw list、视锥剔除、实例化 batch、静态帧复用、仅相机变化时免 scene walk 重新剔除、逐元素哈希跟踪上传、已验证单 indexed draw 渲染 200,000 实例场 | GPU-driven indirect、屏幕空间/HLOD LOD、遮挡剔除、GPU 侧场景遍历 |
| 动画与形变 | 绝对时间 TRS 轨道、glTF STEP/LINEAR/CUBICSPLINE 导入、CPU pose 求值、两段 clip 交叉淡入（`evaluate_blended` / `render_blended`）、动画离屏帧 API（`render_animated`）、实例化；clip 采样与 authored hierarchy 一致 | 骨骼蒙皮、morph 权重、多 layer/同步 clip graph、IK |
| 资源管线 | glTF 2.0 / GLB：buffer、data URI、外部文件、sparse accessor、材质、纹理 + 生成 mip 链、节点 TRS/matrix、KHR_lights_punctual、KHR_materials_unlit、KHR_materials_emissive_strength、相机、TRS 动画；含上限和路径逃逸检查 | KHR_texture_transform、clearcoat/transmission/ior/variants 着色、KTX2/Basis、Draco/meshopt |
| 后处理与质量 | RGBA16Float 线性 HDR 场景目标、半分辨率泛光、FXAA 风格边缘混合、曝光/色调映射/gamma resolve、vignette、film grain、有序 dither、Low/Medium/High/Ultra 预设、动态分辨率与超采样 render scale；debug 和 object-id 帧绕过调色 | MSAA、TAA、motion blur、SSAO/SSR、景深、LUT 调色 |
| 输出与捕获 | GPUI scene-view element；通过 `scene_lab` 离屏 PNG 捕获、回读像素、回读前 deferred submission 轮询、可脚本化 debug view（albedo/normal/depth/UV/shadow/object-id） | 真正的异步 GPU→CPU 回读与 compositor hand-off、多帧 trace 导出、GPU 分 pass 时间戳 |
| 测试资源 | `test_assets/manifest.toml` + SHA-256 下载脚本 + 许可证、离线缓存、ignored native 测试、GPU 像素矩阵 | 更多有许可证的大场景与动画资源 |

这些领域对应[参考分支的 crate 指南](https://raw.githubusercontent.com/cradiy/gpui/main/crates/gpui_3d/README.md)中的功能分类；出现在清单里不代表已经实现。只有通用 API 与渲染器在所有权、校验、生命周期和失败语义上达成一致，并且可运行示例走通公开路径后，功能才可供应用使用。

源码/构建迁移门槛已经关闭：Map Viewer 与 Skin Pack 使用新的 scene view，旧的 GPUI mesh 路径已移除，GPUI 与 `gpui-3d` 在通用 `RendererExtension` 边界下编译；没有 deprecated wrapper、alias 或新旧并行渲染路径。scene-view 示例已在原生 DX12 与 Vulkan 完成像素 smoke check；帧时间证据仍缺失，仅编译 shader 不能关闭性能门槛。Metal 在实现并验证纹理上传与真实窗口渲染前仍不可用。

## 场景与帧流程

1. 把应用几何转换为不可变索引 [`Mesh`](../crates/gpui-3d/src/mesh.rs) 资源，挂到 [`Node`](../crates/gpui-3d/src/scene.rs) 上。
2. 按 mesh-part slot 挂 `Material`，把灯光挂到节点；父子变换组合，场景节点不包含 GPUI entity 或应用状态。
3. 选择 [`Camera`](../crates/gpui-3d/src/camera.rs)，创建拥有不可变 `Arc<Scene>` 快照和纹理表的 `SceneView`。
4. 用 `gpui_3d::scene_view(Arc<SceneView>)` 渲染（填充可用布局 bounds），或从应用 canvas 调用 `window.paint_renderer_extension(...)`。
5. GPUI 通过通用 renderer 生命周期调度扩展；`gpui-3d` 准备可见 draw range、上传变化的网格/纹理、把相邻兼容 draw 合并为 indexed instanced Nova step，并保持 blended draw 的单独顺序。

`SceneView::with_scene`、`with_camera`、`with_textures`、`with_animation`、`with_animation_time`、`with_projection_region`、`with_projection_inset`、`with_blend_edge_feather`、`with_anisotropy` 保持 scene-view identity。更新预览时应保持该 identity 稳定，使 retained GPU buffer 可复用；每次 render 新建 scene view 会创建新的缓存所有者并破坏复用。替换场景会清除动画轨道（其中含旧场景拥有的 handle），需要显式重新挂载。

`with_projection_inset(max_pixels, fraction)` 在元素 bounds 上按轴内缩 `min(max_pixels, axis_length * fraction)`，再与 content mask 求交。`ProjectionRegion::ElementBounds` 为默认值。`VisibleContent` 把相机投影映射到扩展 bounds 与帧矩形 `content_mask.bounds` 的交集；Nova 仍会对每个 draw step 应用宿主 scissor。圆角 content-mask 不会转换为 3D coverage。`VisibleSquare` 在同一交集内居中一个正方形，用于需要稳定正方形相机区域的预览。`with_blend_edge_feather` 只对投影矩形边缘处的 blended draw 乘 fragment alpha；opaque/masked draw 保持原有 alpha 与深度行为。边缘 ramp 按像素线性，以匹配 Map 现有透明边缘淡出；宽度为 0 时关闭。

`Mesh::with_edge_masks` 按 index-buffer 顺序给每个三角形附加一个 [`TriangleEdgeMask`](../crates/gpui-3d/src/mesh.rs)，三个 bit 选择三角形顶点 0/1/2 对边。导入器可平滑外边缘而保留三角化接缝。至少一个 bit 选中的网格会扩展为每三角形三个本地 GPU 顶点以插值重心坐标；无掩码网格保持原始共享索引上传。CPU sidecar 每三角形 1 字节；启用掩码的网格每 index 上传 64 字节而非每共享顶点 64 字节；带切线网格另加每上传顶点 16 字节切线帧。公开 `Vertex` 布局和 CPU BVH/查询顺序不变。

## 几何与变换

`Mesh::new` 校验有限顶点属性、三角形对齐的 index、index 边界以及连续 material-part 覆盖；一个顶点/index buffer 包含多个材质槽时用 `Mesh::with_parts`。内置几何包括 cube、plane、UV sphere、cylinder、cone。`Mesh::with_vertices` 在保持顶点数、index、material part 和三角形边缘掩码不变的前提下返回新快照：重新校验属性、重建网格与逐 part bounds/射线查询 BVH、清除切线帧并分配新的 mesh identity；源网格仍可用。替换提交时 scene view 会整体上传几何，不会写入可能仍被 in-flight 帧使用的 buffer；`scene` 示例演示替换位置同时保留原始快照。

顶点包含本地 position、normal、UV 和线性 RGBA color。缺少法线的导入几何可调用 `Mesh::generate_normals`：按共享 index 计算面积加权法线，拒绝退化三角形或未引用顶点；重复顶点保留硬边，重新生成法线会清除切线帧，需要重新生成或重新挂载。`Mesh::generate_tangents` 为 UV set 0 计算 MikkTSpace 切线帧，在需要不同帧的 corner（包括镜像 UV 接缝）拆分顶点，保持三角形顺序、material part 和边缘掩码，并返回 output→source 顶点映射。它要求至少一个 position/UV 面积非零的三角形和可用的索引法线；MikkTSpace 为其余 corner 提供切线帧。已有 authored 切线的导入器可用 `Mesh::with_tangents` 挂载，但必须在切线接缝处拆分顶点，并提供每三角形一个 handedness 符号。两套 API 都把切线作为可选 CPU/GPU 流，不需要 normal map 的网格不保留也不上传切线。

`Node::with_pixel_offset` 在相机投影后平移光栅像素（X 正向右、Y 正向下）；`Node::with_depth_bias` 在投影后改变归一化 0..1 深度（正值靠近近平面）。两者拒绝非有限值，且不改变场景变换、世界 bounds 和 CPU 射线查询。选中的三角形边缘覆盖只作用于 blended 材质，使用 fragment 重心坐标和屏幕空间导数，并与顶点 alpha 独立存储。

`Transform` 由 translation、quaternion rotation、scale 组成；共享摆放和关节层级使用父节点。`Scene::evaluate` 采样绝对时间 TRS 轨道而不修改 authored 节点；`SceneView::with_animation` 校验并挂载轨道，`with_animation_time` 选择 clip 采样。scene-view 准备与 `SceneView::raycast` 使用同一采样 pose。当前重复 clip-time 变化在 CPU 上求值节点变换、复用 renderer 的 transform-map 容量、重建 prepared draw 记录并上传 draw 数据；该 API 适合静态 pose 和拖拽 scrub，但还不是独立的高帧率 presentation lane。

GPUI 可见动画采样必须使用当前帧的 `window.animation_time()`。启动或重定向动画时记录事件时间；不要在 render 路径内采样 `Instant::now()`。`gpui-3d` 有意不提供播放时钟或每帧唤醒 GPUI 的 task：应用拥有 clip time，GPUI/原生帧调度器拥有 presentation cadence。由于每次采样变化目前都会重建 prepared CPU 数据，连续高帧率运动需要 GPU 侧 retained transform 求值后才能满足 presentation-lane 契约。重复 CPU 采样可用 `Scene::evaluate_with` 传入可复用 `AnimationScratch`，`AnimationScratch::trim` 释放其保留的 map 容量。

## 相机与查询

相机使用右手 view matrix 和 0..1 clip-space 深度，与 Nova 的 DX12/Vulkan/Metal 投影约定一致。相机射线接受 NDC：左下角 `(-1, -1)`、右上角 `(1, 1)`；`SceneView::raycast` 接受左上角为原点的 viewport 像素并完成 Y 轴转换。

`OrbitCamera` 是后端无关的 Y-up 指针控制器：`orbit` 接受弧度 yaw/pitch 增量，`pan` 沿相机 right/up 平移 target（世界单位），`zoom` 改变透视距离或正交视图高度。`fit_bounds` 接受世界空间 [`Aabb`](../crates/gpui-3d/src/math.rs)，保持当前 orbit 方向、把 target 居中，并按给定 viewport aspect/padding 拟合八个角；透视移动 eye，正交调整 view height，均保留现有 near/far，若无法容纳所有角则返回 `BoundsOutsideClipRange`。用 `Scene::bounds` 或 `EvaluatedScene::bounds` 获取 authored/sampled pose 的完整场景 bounds，二者都包含所有非空网格（包括当前视锥外网格）。这些操作返回已校验的 `Camera` 快照；指针手势和替换相机由应用决定。`scene` 示例演示 framing、orbit、pan、zoom，但不把 crate 绑定到 GPUI 输入。

`Scene::bounds` 组合父变换并 union 每个非空网格的世界 bounds；`EvaluatedScene::bounds` 使用该不可变 pose 的采样本地变换，因此 camera framing 与 draw 准备可使用同一动画时间。空场景返回 `None`；非法变换或溢出有限世界坐标的 bounds 返回 `SceneError::InvalidTransform`。

`PreparedScene` 对视锥剔除 mesh part，并把 opaque/masked draw 排在 blended draw 之前，blended 按从后到前排序。`PreparedScene::draw_batches()` 不分配地遍历 prepared 顺序；仅当 mesh identity/generation、mesh part 和所有材质值一致时，连续 opaque/masked draw 才合并为一组，scene view 把每组提交为一个 indexed instanced draw。逐实例 world/normal transform、pixel offset 和 depth bias 位于 instance buffer。Blended draw 保持单独提交以避免改变合成顺序。`scene` 与 `scene_view` 示例演示 API 以及共享 mesh/material 场景。

`Scene::raycast` 与 `EvaluatedScene::raycast` 走同一 mesh bounds/BVH 查询路径；直接场景查询可复用 `RaycastScratch`。viewport 指针坐标用 `SceneView::raycast` 把左上角像素转换为相机 NDC，并使用 scene view 的动画采样；其 size/position 必须相对于渲染所用的同一 projection rectangle：`VisibleContent` 下先减去该矩形原点并使用交集尺寸，`VisibleSquare` 下使用居中正方形的原点与尺寸。跨指针事件复用 `SceneViewRaycastScratch` 保留遍历与动画 transform 存储。CPU 查询目前尚不考虑 alpha mask 或 blended 表面可见性。

## 材质、Alpha 与纹理资源

- `Material::base_color` 和 emissive color 是线性值；metallic/roughness 在材质校验时 clamp 到 `0..=1`。
- `SpotLight` 使用归一化本地空间射线方向、正的有限 range，以及弧度制内/外锥角（`0 <= inner < outer < PI`）。节点变换移动其位置/方向，并按最大世界轴缩放 range。shader 组合点光 range 衰减与外锥到内锥的平滑 cosine falloff：内锥满强度，外缘为 0。
- `Material::occlusion_texture` 用 mesh UV 采样线性 red channel；`occlusion_strength` clamp 到 `0..=1`，只衰减环境光，点光/聚光/方向光的直接光照不变；缺贴图时绑定白色 fallback，不增加 AO 采样。
- `ShadingModel::MetallicRoughness` 使用 scene view 固定 microfacet shader；`Unlit` 只用材质与顶点颜色，不参与灯光或色调映射。
- `AlphaMode::Opaque` 写颜色/深度；`Mask` 丢弃低于 `alpha_cutoff` 的 fragment；`Blend` 使用预乘 alpha 且关闭深度写入，场景按从后到前准备 blended draw。
- 正的 blended edge-feather 宽度仅对 `Blend` 材质在 projection rectangle 边缘对输出 alpha 应用线性像素 ramp；opaque/masked draw 保持正常 replace pipeline 与深度行为。
- 用 `TextureAsset::rgba8` 创建 sRGB 颜色贴图，用 `TextureAsset::linear_rgba8` 创建切线空间 normal map 等数据贴图；`*_mip_chain` 构造器接受 level 0 后由调用方生成的逐级减半（floor）并在 1 处 clamp 的 mip；不完整链合法。把资源加入 scene-view texture table，并用 asset ID 从 `Material` 引用；GPUI-3D 在一次 Nova batch 中上传所有 level，创建覆盖整条链的 view，默认线性 texel/mip 过滤与 clamp-to-edge。`SceneView::with_anisotropy(true)` 启用硬件各向异性过滤；Vulkan 使用 adapter 上报上限，不可用时回退到配置的过滤。图像解码、RGBA8 存储前的颜色转换、mip 生成和资源缓存仍由应用负责。
- 带 `normal_texture` 的 lit `MetallicRoughness` 材质要求 `Mesh::tangents()`；normal map 必须是线性色彩空间，renderer 拒绝 sRGB normal map。对 UV set 0 调用 `Mesh::generate_tangents()`，或用 `Mesh::with_tangents()` 挂载导入切线。生成器返回 seam-split 顶点与 output→source map 供应用自有属性使用；shader 在应用 normal-map RGB 前变换并正交化切线帧。
- DX11、DX12、Vulkan 和 OpenGL Nova device 实现纹理上传；`gfx-metal` 目前没有像素上传实现，Metal 能编译 scene-view WGSL，但带纹理 scene view 暂不支持。

| 后端 | GPUI 3D shader 源编译 | RGBA8 纹理/mip 上传 | 各向异性采样 | 本工作中的真实 scene-view 证据 |
| --- | --- | --- | --- | --- |
| DX11 | 是，构建期 SM5.0 bytecode | 已实现，带原生纹理/mip 回读门槛 | 支持，opt-in | GPUI glyph/image 像素已测；strict-backend 3D 示例完成原生 present；3D 像素检查待补 |
| OpenGL 4.5 | 是，构建期转译 GLSL 4.50 | 已实现，带原生纹理/mip 回读门槛 | 请求时需原生各向异性扩展 | Windows GPUI glyph/image 像素已测；strict-backend 3D 示例完成原生 present；Linux 与 3D 像素检查待补 |
| DX12 | 是 | 已实现，批量上传 | 支持，opt-in | 原生窗口中可见几何与共享资源球实例；帧节奏和 GPU draw count 未测量 |
| Vulkan | 是 | 已实现，批量上传 | 支持 `samplerAnisotropy` 时 opt-in，否则使用请求的 min/mag filter | 原生窗口中可见几何与共享资源球实例；帧节奏和 GPU draw count 未测量 |
| Metal | 是 | 未实现 | 未验证；纹理上传不支持 | 待补；仅编译证据 |

原生 `scene_view` 示例提供两级 sRGB albedo mip 链。DX12 与 Vulkan transfer 测试在一次 batch 中写 base image 与更小的 mip；其 `texture_write` benchmark 还包含 128x128 RGBA8 八级上传用例，测的是 transfer 成本而非帧节奏。

当前 scene view 没有 shadow map、环境贴图光照、骨骼形变、morph target、应用自定义材质 shader 变体或 headless renderer；这些能力不会由 BMCBL UI 静默模拟。应在 `gpui-3d` 中以后端无关场景/API 语义实现后再用于应用代码。

## 资源生命周期与错误

网格和纹理是以生成 ID 寻址的不可变 CPU 资源。`gpui-3d` 按 scene view 缓存其 GPU buffer/view，键为 asset ID 与 mesh generation；保持 scene-view identity 的快照复用兼容上传，变化资源只替换对应资源。每个兼容的 opaque/masked batch 拥有绑定到其首个 draw slot 的一个资源集；blended draw 保留独立 set 以维持合成顺序。batch 首个 slot 与场景 instance range 是资源集复用检查的一部分。

renderer 持有的 scene-view 缓存在 120 个未使用渲染帧后退休，或在 moderate trim 判定空闲至少 1 秒时、aggressive trim 清空所有 scene-view 资源时释放；light trim 保留 GPU scene 资源。shader/pipeline 资源跟随 renderer，因此 moderate trim 不会强制重新编译。

场景材质引用非法纹理时 scene-view 准备报错。GPU 分配、纹理上传、shader 编译和 draw-resource 创建错误经 GPUI renderer 错误路径传播；应用必须通过现有预览状态暴露失败，不能把失败的 3D 构建伪装成空白成功状态。

## 示例与基准

crate 示例是可执行的 API 指南。`scene_view` element 填充父 bounds，因此原生示例必须给容器有限尺寸（内置窗口示例用 `size_full()`）：

```powershell
cargo run -p gpui-3d --example scene
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx11
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-opengl
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx12
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-vulkan
```

无人值守原生 present 检查可加 `--auto-exit-ms=5000`。示例禁用后端 fallback，报告实际 backend 与观察到的 present 次数，并在 deadline 内无 present 时报错；成功 present 并不校验场景像素。

BMCBL 适配器展示产品数据跨越边界的位置：[Map Viewer preview 转换](../src/ui/window/map_viewer/preview_3d.rs) 与 [Skin Pack preview 转换](../src/ui/window/skin_pack/mesh.rs)。这些适配器留在 `src/ui`；crate 与通用示例不得导入 BMCBL 类型。

`scene` 演示共享 mesh/material 实例、可见 draw/batch 计数、从导入三角形位置生成 normal/tangent、authored/evaluated scene bounds、sampled pose framing、orbit/pan/zoom、正交投影、`AnimationScratch` 重复绝对时间采样，以及直接与 viewport-local 射线查询；其 scene-view 射线查询与渲染使用同一采样 pose 并复用 `SceneViewRaycastScratch`。`scene_view` 演示原生 GPUI 窗口、两级 albedo mip 链、三个共享资源球实例、PBR/unlit 材质、ambient/directional/point/spot 灯光、normal/occlusion map、alpha blending、可见内容裁剪前的响应式 inset、blended 边缘一像素线性淡出，以及固定采样动画 pose；当前窗口示例不是自动播放。两个示例都由 `cargo check --locked -p gpui-3d --all-targets --all-features` 覆盖。

Criterion benchmark 覆盖 CPU 图元网格与 MikkTSpace 切线生成、64x32 UV sphere 固定拓扑顶点快照重建、authored/evaluated scene bounds、场景准备/retained 容量复用、动画 transform 求值 + prepared-scene 更新、100/1,000/10,000 draw 的兼容 batch 发现，以及 BVH 射线查询：

```powershell
cargo bench -p gpui-3d --bench scene
cargo bench -p gpui-3d --bench scene -- scene/batching
cargo bench -p gpui-3d --bench scene -- scene/bounds
cargo bench -p gpui-3d --bench scene -- vertex-snapshot/uv-sphere-64x32
cargo bench -p gpui-3d --bench scene -- generate-tangents/uv-sphere-64x32
cargo bench -p gpui-3d --bench raycast
```

`gpui-3d` bench 只测 CPU 工作，不测 GPU 执行、原生 present 延迟、input-to-photon 或帧节奏。Nova 后端 bench 测 128x128 RGBA8 八级 mip 上传 batch，并在 GPU timestamp 可用时报告原生 copy 时间：

```powershell
cargo test --locked -p gfx-dx12 --test texture_transfer
cargo test --locked -p gfx-vulkan --test texture_transfer
cargo bench --locked -p gfx-dx12 --bench texture_write -- mip-chain
cargo bench --locked -p gfx-vulkan --bench texture_write -- mip-chain
```

这些 transfer 数据不代表 3D 场景帧节奏。动画场景用例包含逐采样 CPU 求值与 prepared draw 更新；在考虑 GPU retained transform 动画路径前，用它跟踪该成本。DX12/Vulkan 真实窗口对比需分别记录 GPU/backend/driver、窗口尺寸、刷新率、场景规模、warm-up 和逐帧时间；shader 编译只是格式检查，不能证明像素正确或呈现流畅。

## 离屏多 pass 渲染器

`gpui_3d::OffscreenRenderer` 每次公开调用渲染一帧，仅拥有该帧的 GPU 资源。所有权边界：

- 调用方创建并拥有 Nova `Device`；`gpui-3d` 只在 `render`/`render_animated`/`render_blended`/`capture` 内提交，绝不创建窗口 surface/swapchain，也不读取非自己创建的纹理 CPU 内存。
- renderer 创建的 GPU 对象：shadow color/depth、HDR scene color/depth、bloom color、post color（回读源）、十个 shader module、六个 pipeline（shadow、opaque、blended、bloom、post、sky）、四个 render pass、五个 resource-set layout、五个 pipeline layout、三个 sampler、fallback texture、逐 mesh vertex/index/tangent buffer、逐纹理 image、frame uniform/instance/light/draw buffer，以及缓存的逐 batch 资源集。pipeline layout 随 renderer 释放；早期版本因未纳入 pipeline bundle 而在每次 drop/resize 时泄漏三个 layout。
- `destroy` 释放上述全部对象，必须在 device 销毁前执行。`trim(Light|Moderate|Aggressive)` 丢弃缓存的 mesh/texture/资源集，但不触碰 target/pipeline 状态；`resize` 重建尺寸相关 target、pipeline 和资源集，同时保留 mesh/texture 缓存。

渲染图（pass 经 `render_step_lists_to_textures_deferred` 记录/提交；scene 与 bloom 目标使用内部 render resolution，由 render scale API 控制，post 目标与回读始终是请求的输出尺寸）：

| Pass | Color | Depth | Load | 内容 |
| --- | --- | --- | --- | --- |
| Shadow | `Rgba8Unorm` 打包 24-bit 深度 | `Depth32Float` | Clear white / 1.0 | 按场景 bounds 拟合的光空间正交相机渲染 opaque/masked draw |
| Scene | `Rgba16Float` 线性 HDR | `Depth32Float` | 调用方线性 clear color / 1.0 | indexed instanced PBR draw、按 softness 缩放的 3x3 PCF 阴影比较、线性 radiance、debug view；可选环境天空全屏三角形与 IBL |
| Bloom | 半分辨率 `Rgba8Unorm` | 无 | Clear | 从 HDR scene target 采样的阈值化 9-tap 亮部模糊 |
| Post | `Rgba8Unorm` + `COPY_SRC` | 无 | Clear | 全屏 HDR resolve：曝光、色调映射、泛光叠加、FXAA、vignette、grain、dither、gamma；回读源 |

逐帧更新与上次上传字节做 diff：静态场景且相机不变时上传 0 字节，warm-up 后不发起 mesh/texture 上传。仅相机变化时上传 368 字节 frame uniform（含 inverse view-projection 与环境参数）。逐 batch 资源集按 mesh id/generation、mesh part、draw index、instance count、材质纹理 id 以及 target/frame-buffer epoch 缓存，因此静态场景逐帧复用全部 set，无 descriptor 创建。

## glTF 2.0 / GLB 导入

`gpui_3d::load_path` / `load_bytes` 产出含 `scene`、`textures`、`animations`、`cameras`、`warnings`、`statistics` 的 `GltfScene`。支持：GLB 与 JSON 容器；内嵌/data-URI/外部 buffer；interleaved 与紧凑 accessor；sparse accessor；POSITION/NORMAL/TANGENT/TEXCOORD_0/COLOR_0；indexed/non-indexed 三角形 primitive；材质 alpha mode 与 double-sided；base-color/normal/occlusion/metallic-roughness 纹理经 `image` 解码并生成 box-filtered mip 链；节点 TRS 或 matrix；相机；KHR_lights_punctual（directional/point/spot）；KHR_materials_unlit；KHR_materials_emissive_strength；STEP/LINEAR/CUBICSPLINE 动画通道转换为绝对时间 `TransformTrack`。

导入上限由 `GltfLoadOptions` 控制：document bytes、decoded buffer bytes、node count、每 primitive 顶点数、纹理尺寸、是否解码纹理、是否生成 mip、外部资源访问。每次 accessor 读取都有 checked arithmetic 边界检查；节点环、重复动画时间、越界 index、非有限值、逃逸文档目录的外部路径都会以类型化 `GltfError` 失败。`extensionsRequired` 中超出 {KHR_lights_punctual, KHR_materials_unlit} 的扩展使导入失败；未知可选扩展记为 warning。

必须诚实说明的能力状态：skin 与 morph target 只解析到足以渲染 bind pose 并发出 warning；emissive texture 不采样（factor 被忽略并 warning）；KHR_texture_transform 的 UV 变换未应用；KTX2/Basis、Draco、meshopt 未解码；KHR_materials_clearcoat/transmission/ior/variants 记录 warning 并回退基础 metallic-roughness 模型。

## 测试资源

`crates/gpui-3d/test_assets/manifest.toml` 列出每个外部模型的 source URL、SHA-256、字节数、许可证、署名和 feature tag。`download.ps1` 下载缺失文件到 `cache/`，校验全部哈希，任何错误都会失败；缓存被 git ignore，首次准备后核心回归可完全离线运行。已验证资源：

| 资源 | 许可证 | 覆盖特性 |
| --- | --- | --- |
| DamagedHelmet.glb | CC BY 4.0 (ctxwing) | GLB、PBR、base-color/normal/occlusion/metallic-roughness 纹理 |
| AnimatedCube.gltf + .bin + .png | CC0 (UX3D / Norbert Nopper) | JSON 文档、外部 buffer、外部 image、节点旋转动画 |
| BoxVertexColors.glb | CC0 (Marco Hutte) | GLB、COLOR_0 顶点颜色、索引几何 |

需要缓存的测试标记 `#[ignore]` 并注明依赖 `test_assets/download.ps1`；未准备已验证资源时运行会失败而不是跳过。`tests/gltf_import.rs` 内生成的合成 GLB fixture 覆盖畸形容器、index 溢出、节点环、必需扩展和路径逃逸，不依赖外部文件。

## GPU 验证与实测结果

`tests/offscreen_gpu.rs` 创建真实 Nova device，经公开 API 渲染、回读像素并断言：着色输出非均匀且有光照；开启方向阴影改变超过 100 像素；resize 后按新尺寸渲染；重复静态帧上传 0 字节且不创建资源；aggressive trim 强制重新上传；DX11/Vulkan 与 DX12 参考像素误差在 1% 以内。显式运行：

```powershell
cargo test -p gpui-3d --test offscreen_gpu -- --ignored --test-threads=1
```

`scene_lab` 提供可脚本化捕获（`--backend=all --out=... --json=...`）。在本开发机（Windows 11 26100、AMD Radeon RX 7600M XT、1280x720、2048 shadow map、8 draw call / 7 shadow draw / 18,806 三角形 / 13 实例，首帧后场景缓存）实测：

| 后端 | 首帧（prepare / submit） | warm render p50 | 稳态帧（prepare / submit） | 稳态上传字节 | capture p50 |
| --- | --- | --- | --- | --- | --- |
| DX12 | 92.4 ms / 1.6 ms | 1.16 ms | 1.16 ms / 1.10 ms | 0 | 5.10 ms |
| DX11 | 2.9 ms / 0.1 ms | 0.27 ms | 0.09 ms / 0.05 ms | 0 | 2.85 ms |
| Vulkan | 40.4 ms / 1.5 ms | 0.31 ms | 0.30 ms / 0.25 ms | 0 | 5.09 ms |
| OpenGL 4.5 | 0.9 ms / 0.1 ms | 0.12 ms | 0.09 ms / 0.05 ms | 0 | 1.85 ms |

首帧差异来自 shader/pipeline 创建成本；warm-up 后每个后端均报告 0 mesh 上传、0 texture 上传、0 上传字节，draw/三角形/实例数一致。这些是 CPU 侧 prepare/submit 与回读时间，不是 GPU 帧时间：Nova 仍无跨后端 GPU timestamp 契约，因此不宣称 GPU pass 时间。捕获写入 `target/scene_lab/*.png`；DX11、DX12、Vulkan、OpenGL 图像视觉一致，GPU 像素测试对 DX11/Vulkan 与 DX12 做数值一致性断言。

| 后端 | GPUI scene-view shader | 离屏 shadow+PBR+post | 回读像素 | 跨后端一致性 |
| --- | --- | --- | --- | --- |
| DX12 | 构建期 DXBC | 已验证（scene_lab 捕获 + GPU 测试） | 已验证 | 参考后端 |
| DX11 | 构建期 SM5.0 bytecode | 已验证（scene_lab 捕获 + GPU 测试） | 已验证 | 与 DX12 误差 1% 内 |
| Vulkan | 构建期 SPIR-V | 已验证（scene_lab 捕获 + GPU 测试） | 已验证 | 与 DX12 误差 1% 内 |
| OpenGL 4.5 | 构建期 GLSL 4.50 | 经 scene_lab 捕获验证（WGL hidden-window bootstrap） | 已验证 | 视觉一致；不在自动矩阵中 |
| Metal | 构建期 MSL | 未实现（无纹理上传路径） | 不支持 | 待补 |

## HDR 后处理、泛光、FXAA 与质量预设

离屏渲染器在线性 HDR 中 resolve 光照。Nova 的 format set 已包含 `Rgba16Float`；DX12、DX11、Vulkan 和 OpenGL 4.5 创建并采样 HDR scene target（Metal 映射格式但未在本 Windows 主机验证）。链路：

1. Shadow pass：`Rgba8Unorm` 打包 24-bit 深度 + `Depth32Float`。
2. Scene pass：线性 radiance 写入 `Rgba16Float`，带 depth target；尚不做曝光/gamma。
3. Bloom pass（可选）：半分辨率 `Rgba8Unorm`，阈值化 9-tap 亮部模糊。
4. Post pass：全屏 resolve 到可回读 `Rgba8Unorm`。

post shader 依次应用曝光、ACES/Reinhard 色调映射、叠加泛光、FXAA 风格边缘混合、vignette、基于稳定像素哈希的 film grain、4x4 有序 dither 抗 8-bit 色带、gamma。`RenderOptions::post_process == false` 直接透传 scene 字节不做调色（object-id 拾取依赖此行为）；debug view 同样绕过 HDR resolve。后处理是确定性的：没有帧时间输入，也没有 TAA。

| 预设 | Shadow map | 泛光 | FXAA | 调色 |
| --- | --- | --- | --- | --- |
| `Low` | 512 | off | off | 仅 vignette，无 grain/dither |
| `Medium` | 1024 | strength 0.25 | on | vignette、grain、dither |
| `High` | 2048 | strength 0.35 | on | vignette、grain、dither |
| `Ultra` | 4096 | strength 0.45 | on | 更强 vignette/grain |

`RenderOptions::bloom` 控制 threshold/strength/radius，`RenderOptions::post_effects` 控制 vignette/grain/dither，`RenderOptions::antialias` 选择 `None` 或 `Fxaa`。`RenderQuality::render_options` 填充完整选项，调用方可覆盖单个字段。`ShadowOptions::softness` 只缩放 PCF 比较半径，不改变 shadow-map 内存。

本机实测（DX12、1280x720、10 帧 orbit benchmark；render 为 CPU 帧成本，capture 含 GPU 完成与回读）：

| 预设 | 常驻字节 | Render p50 | Capture p50 |
| --- | --- | --- | --- |
| `Low` | 18.1 MiB | 3.60 ms | 5.36 ms |
| `Medium` | 24.3 MiB | 3.02 ms | 4.38 ms |
| `High` | 48.8 MiB | 3.16 ms | 4.75 ms |
| `Ultra` | 147.1 MiB | 3.38 ms | 5.19 ms |

本 GPU 在 720p 下 bloom/FXAA 成本约在 capture 时间的 1 ms 驱动/队列噪声内，shadow-map 分辨率主导内存与 scene-pass fill。HDR scene target 比旧 display-referred target 每像素多 4 字节，半分辨率 bloom 每像素多 1 字节，720p 合计约 5.5 MiB。GPU timestamp query 仍缺失，因此不报告分 pass GPU 时间。

剩余后处理缺口：MSAA、TAA、motion blur、SSAO/SSR、景深、多尺度泛光、LUT 调色。FXAA 是亮度边缘启发式，不是解析的多重采样覆盖率。

## Render scale、可见性与动画交叉淡入

### Render scale（动态分辨率与超采样）

`OffscreenRenderer::with_render_scale` / `set_render_scale` 以 `render_scale * output_size` 渲染 HDR scene 与 bloom target，post target、回读和 viewport 保持请求的输出尺寸；允许 `0.25..=2.0`：

- 小于 1 降低 scene/bloom fill 成本与 GPU target 内存，post pass 用 four-tap footprint filter 上采样。
- 大于 1 超采样场景并在 post pass 降采样，无需 multisample render target 即可近似 SSAA 边缘、高光和阴影。
- 改变 scale 重建尺寸相关 target/资源集并失效 retained static frame；mesh/texture 保持缓存。debug view 与 object-id 拾取以单 texel 无过滤读取 scene，保证任意 scale 下 id 仍可解码。

实测 1280x720 studio、Medium、DX12（含 post/bloom/shadow）：

| Render scale | 内部 HDR 尺寸 | Target bytes | 常驻 |
| --- | --- | --- | --- |
| 0.5 | 640x360 | 14.4 MiB | 15.1 MiB |
| 1.0 | 1280x720 | 22.9 MiB | 23.7 MiB |
| 2.0 | 2560x1440 | 57.2 MiB | 58.0 MiB |

在 Nova 暴露 multisample target 前，render scale 是 MSAA 的实用替代：scale 2 比 MSAA 更耗 fill/内存，但不需要新后端契约。scale 1 默认仍启用 FXAA；scale 2 与 FXAA 组合合法但通常不必要。

### 节点可见性

`Node::visible` 默认 `true`，`Node::with_visible` 构建时设置，`Scene::set_visible` 切换并使 retained 快照失效。隐藏节点会隐藏整棵子树：场景准备（绘制与阴影）、CPU 射线拾取、场景 bounds 都跳过它，因此隐藏子树不会投射阴影或被拾取。可见性不改变层级、变换或可见兄弟节点的 prepared draw 顺序。

### 动画交叉淡入

`gpui_3d::blend_transform` 混合两个本地 pose（平移/缩放线性、旋转最短弧球面插值、权重 clamp）；`Scene::evaluate_blended` / `evaluate_blended_with` 以各自独立绝对时间采样两段 clip 并逐节点混合：`0` 为 primary pose，`1` 为 secondary pose，只被单侧 clip 驱动的节点向 authored rest pose 淡入淡出。renderer 以 `OffscreenRenderer::render_blended` 暴露同一操作。非法 blend 权重、同一 clip 内重复轨道、stale handle、非法 authored pose 返回类型化错误。这是 CPU clip blending；骨骼与 morph 形变仍未实现。

### 图像光照与天空（IBL）

`EnvironmentMap` 包装等距柱状纹理，并带 intensity 与 yaw rotation。`EnvironmentMap::new` 校验参数，`load_path` 解码 PNG/JPEG 全景图（sRGB、最大下采样到 4096、box-filtered mip 链），`EnvironmentMap::procedural_studio` 生成无外部许可证约束的 studio 全景图。用 `Scene::set_environment` 挂载、`None` 清除；变化会使场景版本递增，从而不复用 retained frame。

scene shader 用该贴图计算两个 ambient 项：diffuse 采样高度模糊的 mip（全环境平均），specular 沿反射方向采样 `roughness * (mip_count - 1)`，再结合 Karis 解析 split-sum BRDF 近似。全屏天空三角形在 scene pass 最先绘制，做深度比较但不写深度，因此几何会覆盖它；debug 与 object-id pass 跳过天空。这是等距柱状 LDR 纹理的 mip 预过滤，不是预过滤 cubemap + irradiance probe + BRDF LUT；高光金属因此使用近似，HDR 动态范围受纹理链限制。环境纹理按 asset ID 缓存、计入统计、在变化时失效，并在 aggressive trim 或 renderer 销毁时释放。

### 距离 LOD

`Node::with_lod` 接受从近到远排列的 `LodLevel`。每级包含 mesh、可选 materials 和 `max_view_distance`；prepare 选择第一个覆盖相机到节点距离的层级，否则用最后一级。节点可以只有 LOD 层级，但 `Node::mesh` 仍是 bounds/raycast 的表示，因此应把最高细节 mesh 放在节点上以保证查询准确。LOD 选择依赖相机，所以只要 prepare 使用了 LOD，camera-only retained 快速路径就禁用；相机移动会重新 walk 并重新选择。屏幕空间 LOD、HLOD、layer/culling mask 尚未实现。

### 延迟 GPU 完成轮询

Nova 的 `PresentationDevice` 新增 `render_step_lists_to_textures_deferred`：支持延迟离屏路径的后端返回 Nova `SubmissionId`，默认实现走同步兼容提交并返回 `None`。DX12 实现延迟路径。离屏 renderer 保存 handle 并暴露 `OffscreenRenderer::poll_render`：进行中返回 `Some(false)`、完成返回 `Some(true)`、后端无 handle 返回 `None`。capture 与维护等待消费该 handle，常见维护等待不再为了确认完成而额外全量回读 post target。像素回读本身仍是完成后的同步 copy；异步映射 GPU buffer、以及不经过 CPU 回读直接合成离屏目标仍属未来工作。

### 本批测试覆盖

- 库测试覆盖隐藏子树准备、拾取、bounds、transform blending、crossfade 端点/中点、fade-to-rest、重复/stale/权重错误以及 animation-scratch 复用。
- `renderer_memory` 现有 14 个原生测试：新增用例验证 blended pose 与可见性改变提交帧、render scale 保持输出尺寸同时改变 target 内存/采样且 object-id 拾取仍可解码、环境贴图照亮场景并绘制天空且具备缓存/trim 生命周期、延迟提交可在 capture 前轮询至完成。

## GPU 内存与缓存生命周期

renderer 通过 `RendererStatistics` 报告自己的分配账目。这些是创建 GPU 资源时测得（Nova 分配账目）的引擎逻辑字节，不是 OS 进程 RSS，也不是驱动上报的 residency；renderer 不把它们当作后者宣称。

| 字段 | 含义 |
| --- | --- |
| `resident_buffer_bytes` / `resident_texture_bytes` | renderer 持有的存活 GPU 字节 |
| `target_bytes` | shadow/scene/bloom/post 等尺寸相关 target |
| `cached_mesh_bytes` / `cached_texture_bytes` | 资源缓存（纹理含 fallback 与环境贴图及 mip 链） |
| `peak_resident_bytes` | 每帧后观察到的常驻峰值 |
| `upload_bytes_total` | 累计上传字节，含 uniform 与局部 range 写入 |
| `mesh_cache_hits/misses`、`texture_cache_hits/misses` | 累计的逐帧资源缓存结果 |
| `evictions` | 由 trim 或内存预算释放的资源数 |
| `tracker_cpu_bytes` | 检测上传范围变化所保留的 CPU 内存 |
| `memory_budget_bytes` | 配置的软预算 |

`submission_unobserved` 是内部状态：render 提交后若尚未被 capture/维护观察则为真。支持延迟完成 handle 的后端（当前 DX12）会在 capture 或维护操作中 `wait_submission`；不支持的后端回退为回读 post target 以排空提交。始终 capture 的调用方不会支付额外等待。没有 deferred-retirement 队列，因此 `pending_retirement_bytes` 有意不存在而非伪造。

内存默认值与控制：

- shadow map 默认 1024（打包深度颜色 + depth target 约 8 MiB）；2048 约 33 MiB，4096 约 134 MiB；只有阴影细节值得时才提高 `ShadowOptions::resolution`。
- glTF 导入默认纹理上限 2048，并在生成 mip 链前下采样超大图；`GltfLoadOptions::max_texture_dimension` 与 `GltfLoadOptions::downscale_textures` 控制上限与回退。
- 纹理缓存键是源 image index + color space，因此多个 glTF texture entry 共享同一 image 时只上传一张 GPU 纹理。
- `OffscreenRenderer::set_memory_budget(Some(bytes))` 启用软预算驱逐：每帧后按 LRU 驱逐该帧未引用的资源直到常驻总量符合预算；`trim_to_budget` 按需强制同一轮。
- `trim(Light|Moderate|Aggressive)` 释放空闲资源；Aggressive 还会收缩过度分配的 streaming buffer、清空 mesh-buffer 复用池并收缩 CPU staging buffer。

分配抖动与碎片控制：

- 逐帧 draw/instance/light/frame 编码复用四块 CPU staging buffer 容量，不在每个 dirty frame 重新分配（`RendererStatistics::cpu_staging_bytes`）。
- 被驱逐的 mesh buffer 进入有界复用池（16 MiB / 64 项，精确 usage 匹配，最多 2x 浪费）。后续同 size class mesh 免 `Device::create_buffer` 复用该分配，因此场景切换与预算裁剪不会因反复 create/destroy 碎裂 GPU heap。该池不参与软预算、独立有界（超过上限一半即释放），由 aggressive trim 清空。指标：`pooled_buffer_bytes`、`pooled_buffer_count`、`buffer_reuses`、`buffer_allocation_count`、`buffer_slack_bytes`。
- pipeline layout 纳入 pipeline bundle 并随 renderer 销毁；早期版本在每次 drop/resize 时泄漏。
- streaming buffer 几何增长，只在显式 aggressive trim 时收缩，避免反复 realloc 导致 heap 碎片。

本机实测 VRAM 影响（引擎逻辑字节）：

| 配置 | 常驻 | 备注 |
| --- | --- | --- |
| Studio，720p，`Low`（512 shadow、无 bloom/FXAA） | 18.1 MiB | HDR scene + bloom target 约 17 MiB |
| Studio，720p，`Medium`（1024 shadow、bloom、FXAA） | 24.3 MiB | 1024 shadow map +6.2 MiB |
| Studio，720p，`High`（2048 shadow、bloom、FXAA） | 48.8 MiB | shadow target 主导 |
| Studio，720p，`Ultra`（4096 shadow） | 147.1 MiB | shadow target 约 134 MiB |
| DamagedHelmet，720p，`High`，纹理 2048 | 114.7 MiB | 纹理 64 MiB + target 48 MiB |
| DamagedHelmet，720p，`High`，`--max-texture=1024` | 63.0 MiB | 纹理 16 MiB；VRAM 少 49 MiB |

缓存的 720p studio 场景报告 17 个 GPU buffer 分配、0 字节 buffer slack、7 KiB CPU staging 容量、全部缓存存活时 0 pooled 字节；预算裁剪后的场景经复用池循环 buffer 并报告真实 reuse，aggressive trim 后回落到 4 个 frame buffer。

上传粒度按元素以 64-bit 内容哈希跟踪 draw slot、instance、light 和 frame uniform，因此不保留这些 buffer 的 CPU 字节副本。64 个实例中移动一个实例的动画帧只上传 144 字节 instance record（`renderer_memory` 断言为 9 KiB instance table 中小于 512 字节），mesh、texture、draw slot 均不上传。

原生内存测试（`tests/renderer_memory.rs`，14 个 ignored 测试）断言：软预算驱逐释放空闲 mesh/texture 并让常驻字节回到预算内；8 场景资源循环保持有界，aggressive trim 清空全部缓存/资源集；2 MiB 资源预算下 60 帧三场景循环保持常驻包络、资源集数量有界并记录重复驱逐；两个 renderer 共享一个 device 且 target/cache 独立，另一个销毁后存活者继续渲染；resize 保持 mesh 缓存热且重定向 target；动画只上传变化的 instance range；CPU/GPU 拾取一致；10,000 实例场保持一个 draw slot、静态帧零上传复用、相机移动跳过 scene walk，单次 `node_mut` 只上传一条 instance record；HDR/bloom/FXAA 改变输出像素、质量预设生效、质量变化失效 retained frame；有界 mesh-buffer 池报告真实 reuse 且 aggressive trim 释放；blended pose 与节点可见性改变提交帧；render scale 保持输出尺寸同时改变 HDR target 内存/采样且超采样下 object-id 仍解码；环境贴图照亮场景、绘制天空、经历资源失效并被 aggressive trim 释放；延迟提交可在 capture 前轮询至完成。

## Retained frame 复用与场景规模

帧统计暴露两条额外 CPU 快速路径：

- `FrameStatistics::frame_reused`：场景 identity、mutation version、相机 pose、options 与 target 尺寸都与上次提交帧一致。renderer 直接返回 retained 统计，不 walk 场景、不编码 buffer、不上传、不提交 GPU；随后 `capture` 读取仍有效的 render target。静态帧因此零 CPU 准备、零上传字节。
- `FrameStatistics::camera_only_update`：仅相机移动。`PreparedScene::update_camera` 重新剔除/排序 retained candidate draw 而不重走场景图，且只上传 368 字节 frame uniform，因为 draw/instance/light 内容哈希未变。场景包含 LOD 时该快速路径禁用，确保重新选择 LOD。

`Scene::version` 是单调 mutation 计数器，insert、remove、reparent 和 `node_mut`（保守地）都会递增。renderer 同时校验场景的进程唯一 identity 与 version，因此被销毁重建的场景不会继承旧缓存帧。

实例化 batch 同时修正 draw-buffer 增长：离屏 renderer 每个 instanced batch 写一个 draw-parameter slot，而不是每 draw 一个。共享 mesh/material 的 200,000 节点场提交一个 200,000 实例的 indexed draw 和一个 512-byte draw slot，而不是 200,000 个重复 512-byte slot（节省约 100 MiB）。lab 用 `--instances=N` 暴露。本机实测（1280x720、shadow map 1024、静态相机）：

| 节点数 | Draw call | 实例 | 三角形 | Buffer bytes | Target bytes | 常驻 | 首帧 prepare | 稳态 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 5,000 | 1 | 5,000 | 60,000 | 704 KiB | 18.5 MiB | 19.7 MiB | 29.7 ms | 复用，0 上传 |
| 50,000 | 1 | 50,000 | 600,000 | 7.0 MiB | 18.5 MiB | 25.7 MiB | 44 ms | 复用，0 上传 |
| 200,000 | 1 | 200,000 | 2,400,000 | 28.1 MiB | 18.5 MiB | 47.1 MiB | 242 ms | 复用，0 上传 |

改为逐 batch draw slot 之前，200,000 节点场报告 128 MiB buffer、147 MiB 常驻、353 ms 首帧准备。剩余成本是初始 CPU scene walk 与完整 instance range 的 GPU 提交；仍无 GPU-driven indirect、clustered lighting 或屏幕空间 LOD，因此相机/场景变化时仍会重绘全部实例。

## 拾取与投影

`pick` / `pick_evaluated` 把左上角 viewport 像素转换为相机射线，返回 `RaycastHit`（稳定 node/mesh id、material part、距离、位置、几何 normal、UV）。`project` 把世界点映射回像素与归一化深度，供 gizmo/overlay 使用。`OffscreenRenderer::pick` 使用 renderer viewport 并保留遍历存储；`pick_animated` 在 raycast 前采样给定 clip 时间。

`OffscreenRenderer::pick_object` 以 object-id debug view 渲染场景、回读 resolve 后帧，并从 RGB 解码 one-based 24-bit instance id。该 id 映射到 GPU 实际提交的 prepared draw，因此剔除、混合顺序和动画 pose 都与该帧一致。它要求 device 实现 `TextureTransferDevice`，因为兼容提交 API 尚无异步回读路径；回读是同步的，且之后 renderer 的 last frame 仍是 object-id 帧。`renderer_memory` 中 CPU 与 GPU 拾取一致，scene_lab 用 `--pick=x,y` 打印两者结果。

## 验证

```powershell
# 本次改动的每个文件都经过 rustfmt 检查；仓库整体仍有历史格式差异，因此不把 --all 作为门槛。
cargo test -p gpui-3d
cargo test -p gpui-3d --test picking
cargo test -p gpui-3d --test renderer_memory -- --ignored --test-threads=1
cargo test -p gpui-3d -- --ignored --test-threads=1   # test assets + 原生 GPU 矩阵
cargo check -p gpui-3d --all-targets --all-features
cargo clippy -p gpui-3d --all-targets --all-features --no-deps
cargo run -p gpui-3d --example scene_lab -- --backend=all --width=1280 --height=720 --frames=4 --quality=high --out=target/scene_lab/studio.png --json=target/scene_lab/studio.json
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --frames=8 --quality=high --orbit-degrees=2.0 --json=target/scene_lab/bench.json
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --quality=low --bloom=off --fxaa=off --out=target/scene_lab/plain.png
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --model=crates/gpui-3d/test_assets/cache/DamagedHelmet.glb --quality=high --exposure=2.0 --max-texture=1024 --budget-mb=160 --pick=640,360 --out=target/scene_lab/helmet.png
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --instances=200000 --frames=4 --quality=medium --out=target/scene_lab/instances.png --json=target/scene_lab/instances.json
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --quality=high --render-scale=2.0 --out=target/scene_lab/ssaa.png
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --quality=high --env-map=studio --out=target/scene_lab/ibl.png
```

后端改动还需在真实窗口启动对应 `scene_view` 示例，检查像素与帧行为。在上述 Metal 纹理限制改变并重新验证前，该限制继续成立。

## 剩余缺口审计与优化 backlog

以下是仍缺失或未优化的高价值项目及具体阻塞点，按用户可见收益优先、其次是 API/后端成本排序。

| 领域 | 缺失或未优化 | 阻塞点 / 工作量 |
| --- | --- | --- |
| 渲染效果 | MSAA、TAA、motion blur、SSAO、SSR、景深、多尺度泛光、LUT 调色 | MSAA 需要 Nova 的 multisample render-pass 状态；其余是新 pass/target 加 history buffer。render scale 2 已提供近似 SSAA。 |
| 光照 | 预过滤 cubemap/irradiance probe 与真正的 BRDF LUT、点光/聚光阴影、clustered Forward+ | 当前等距柱状 mip 近似已覆盖 IBL；预过滤 probe 需要 cube target 与卷积 pass，点光/聚光阴影需要 atlas target 和多光源剔除。 |
| 形变 | GPU 骨骼蒙皮、morph target、pose graph、IK | 导入器已识别 skin/morph primitive 但只保留基础几何；需要 joint/weight 顶点属性、bone buffer 和蒙皮 pipeline。 |
| glTF 完整性 | `KHR_texture_transform`、`TEXCOORD_1`、clearcoat/transmission/ior/variants、`KHR_texture_basisu`、Draco/meshopt | 材质 UV 变换与 UV1 需要额外 draw 参数；压缩扩展需要新解码器。 |
| 场景图 | 屏幕空间/HLOD 选择、layer/culling mask、user metadata、gizmo/debug 线 | 距离 LOD 已实现；屏幕空间尺寸选择需要投影感知的度量，线段需要独立 vertex/pipeline 路径。 |
| 异步/延迟 | 真正的异步 GPU→CPU 回读、免 CPU copy 的 compositor hand-off、GPU 分 pass 时间戳、帧节奏 | DX12 延迟提交已提供完成 handle 与 poll/wait 路径，但回读 copy 仍是同步的；Nova 有 transfer timestamp 但尚无 per-pass query handle。 |
| GPU-driven 管线 | 间接绘制、GPU culling、GPU 侧场景遍历、mesh shader | Nova phase-1 API 没有 indirect draw descriptor 或 mesh pipeline。 |
| 内存/性能 backlog | mesh arena/压缩、纹理流式加载与压缩、bindless 风格 descriptor 复用、并行场景准备、阴影视锥剔除、pipeline cache 预热、异步上传 | 主要是引擎侧工作；并行准备不能破坏确定性顺序，arena/共享 buffer 方案在增长时会使逐 batch 资源集失效。 |
| 平台覆盖 | Metal 运行时验证、WebGL 后端 | 本 Windows 主机无法验证 Metal；WebGL 缺少 renderer 使用的 storage buffer/indirect 特性。 |

本阶段已优化并验证：HDR target 内存、shadow 默认值、纹理上限与下采样、逐元素增量上传、静态帧复用、camera-only re-cull、逐 batch draw slot、staging buffer 复用、有界 mesh-buffer 复用池、预算驱逐、render scale、IBL 环境缓存、距离 LOD、延迟完成轮询和 pipeline layout 生命周期。下一步最高收益是预过滤 cubemap/BRDF LUT 与点光/聚光阴影（视觉收益），然后真正的异步回读（延迟）与屏幕空间 LOD（大场景成本）。

## 完成记录

本记录关闭当前阶段已在 `master` 落地的 GPUI-3D 工作。提交刻意不带 `[skip ci]`，让仓库 Compile Check workflow 校验推送的树。

本阶段交付：

- `gpui-3d` 公开 API：场景图、网格/切线、材质、相机、射线查询、glTF 2.0/GLB 导入、retained `PreparedScene`、离屏多 pass 渲染器、动画采样、project/pick helper、GPU object-id 拾取、统计与资源生命周期 API。
- `nova-gfx`：DX11/DX12/Vulkan/OpenGL 的线性 HDR `Rgba16Float`（Metal 仅增加映射，未在本机运行时验证）。
- 离屏渲染图：打包深度 shadow pass、线性 HDR PBR scene pass、半分辨率 bloom pass、带曝光/色调映射/FXAA/vignette/grain/dither 的 HDR post resolve、回读 target。
- 内存与性能：逐元素哈希跟踪增量上传、可 trim 的几何增长 streaming buffer、带 LRU 驱逐的软资源预算、纹理缓存/下采样、静态帧复用、camera-only re-cull、逐 batch draw slot、可复用 CPU staging buffer、有界 mesh-buffer 复用池、分配/碎片指标、pipeline-layout 泄漏修复、动态分辨率与超采样 render scale。
- 场景图与动画：渲染/阴影/拾取/bounds 尊重的节点与子树可见性、按距离选择的渲染 LOD，以及 CPU 两段 clip 交叉淡入求值与 `render_blended`。
- 环境与提交：等距柱状 mip 预过滤 IBL、程序化天空背景和环境纹理生命周期，以及 Nova 延迟离屏提交 handle 与 DX12 在同步 capture 前的完成轮询。
- 验证资源与工具：SHA-256 pin 的有许可证测试资源 manifest 与下载器、四后端像素矩阵、14 个原生内存/生命周期测试、带质量预设/render scale/orbit benchmark/debug view/JSON 统计的 scene lab，以及本文档。

本记录前的最近一次本地验证：

- `cargo test -p gpui-3d`：107 个库测试、8 个 glTF 导入测试、5 个 CPU 拾取测试、2 个文档测试通过。
- `cargo test -p gpui-3d -- --ignored --test-threads=1`：缓存真实模型导入、DX12→DX11→Vulkan 像素一致性矩阵、14 个原生 renderer 内存/生命周期测试通过。
- `cargo check --lib`（仓库默认 features）与 `cargo check -p bmcbl --no-default-features` 通过；改动 Rust 文件 rustfmt-clean，未新增 clippy 警告（仓库存在与本工作无关的历史格式基线）。
- scene lab 在 DX12 上重新生成 HDR + bloom + FXAA 质量预设截图，并用 DX11、Vulkan、OpenGL 4.5 生成像素矩阵与 IBL 截图。

本阶段明确未实现（文档说明，不作宣称）：GPU 蒙皮/morph 形变、预过滤 cubemap irradiance probe 与真正 BRDF LUT、clustered Forward+、点光/聚光阴影、MSAA/TAA、SSAO/SSR/motion blur/景深、多尺度泛光、LUT 调色、GPU 分 pass 时间戳、GPU-driven indirect、屏幕空间/HLOD 选择、layer mask、遮挡剔除、真正的异步 GPU→CPU 回读、免 CPU 回读的 compositor hand-off，以及 Metal 运行时验证。render scale 2 近似超采样，等距柱状 mip IBL 覆盖环境光照；距离 LOD 与 DX12 延迟完成轮询已实现。
