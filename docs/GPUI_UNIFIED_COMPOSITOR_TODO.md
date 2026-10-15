# GPUI 统一构帧与合成链路：持续执行 TODO

> 状态：进行中。此文件是独立维护 GPUI/Nova-GFX 渲染重构的唯一进度入口；不是 BMCBL 业务 UI TODO。
> 更新日期：2026-10-14（UTC）。初始依据：master `405a7133eec7e093ad93681ed725de6a69a89cfb`、GPUI/Nova 源码及 2026-10-14 DX11 本地调试日志。
> **所有后续对话/代理在编辑前必须重新读取 master HEAD、本文和相关源码，不得使用本文件的历史 HEAD 直接写入。**
> 本文是唯一执行状态源；[GPUI P0 历史依赖审计](GPUI_P0_RENDER_DEPENDENCY_AUDIT.md)仅作已发生事实与当时源码快照参考，其旧 RenderTree/RenderCommit 方案不再是目标。

## 不可更改的工程目标

1. **统一 GPUI 的 UI 构帧、增量绘制、GPU 合成、Surface Present 语义**，降低端到端延迟、内存复制和显存/纹理反复分配。框架必须独立于 BMCBL，BMCBL 只是一个真实负载。
2. **采用破坏性替换，无旧链路兼容层**：不建立第二棵 Render Tree，不形成 RenderTree → Scene → RenderCommit → RenderPipeline 链式全量中间表示。保留已有 GPUI Element/布局/输入身份，最终移除旧全窗口 Scene 主链路；新增结构只能承担不可替代的职责，并应替换而非叠加旧所有者。必要 Property/Filter DAG 需用依赖与性能证据说明。
3. **失效分类**：UI/Layout、Paint/Content、Composite/Property、Surface/Presentation。纯合成变化不得误触发 UI Prepaint；不变内容不得重复编码与上传。
4. **GPU Owner 保留最新有效可呈现内容**；UI 任务、GPU 提交、Surface Resize、Present 各自有清晰版本和资源生命周期。禁止 Win32 WM_SIZE 路径承担重型同步构帧，也禁止以增加帧请求频率掩盖黑边。
5. **Root Color Cache 不应永久伪装成零半径 Blur**；Filter Capture/Target 需要稳定资源身份，避免弹窗变化令纹理全局失效。改造应包括预算、资源回收、Fence 生命周期。
6. **四后端等价性**：DX11 / DX12 / Vulkan / OpenGL；保留受支持的 Metal 路径，不以某一后端的专有假设污染统一合成层。
7. 不采用全窗口强制重绘、每帧 GPU idle、无上限纹理缓存或禁用滤镜作为所谓修复。

## 已确认的基线与原因边界

2026-10-14 用户的 DX11 Debug 日志（非四后端性能结论）：

- 5 秒：806 requests / 474 Draw / 446 Present。存在频繁请求与合成；持续动画时不应将请求数直接等同浪费。
- 一次 91.6202 ms 构帧：**Prepaint 86.753 ms**、Paint 3.829 ms、Scene Finish **0.028 ms**。不能断言 Scene Finish 是长尾主因；首先查 layout/text/CachedView/依赖失效。
- Retained Target Layout 多次在 2～6 个 active variants 间重建。日志报告的目标容量变化从约 4.67 MB 至约 27.99 MB；**不等于实际分配额或 GPU Stall，需资源事件/时间戳确认**。
- 最新一帧 scene_primitives=46, scene_batches=12, encoded_primitives=109, encoded_batches=31, gpu upload=6144B, composite_passes=2, retained_copy_pixels=583200。局部逻辑 Scene 很小，但编码/合成布局可能额外扩张；继续查独立 Color/Filter 语义。
- `crates/gpui/src/platform/nova/frame_upload/encode.rs::append_retained_root` 使用零半径 Blur/Render Target 表示根颜色驻留；`renderer.rs::ensure_backdrop_blur_targets` 在布局不兼容时会重建并可能等待已提交 GPU 工作。
- `crates/gpui/src/platform/windows/window.rs::dispatch_native_resize_frame` 仍在原生 WM_SIZE 栈内执行 `dispatch_pending_update_with_vsync(None)`，须测量 UI 长尾和新旧 HWND / Layout / Surface / Present 尺寸代次。
- DXGI opaque HWND 当前 Scaling=None；这可能导致 Resize 期间旧画面未覆盖扩大的客户区，**须按窗口类型进行实机 A/B，尚非最终根因**。


## 破坏性重构决定：消除多重表示与技术债（2026-10-14 增补）

### 真实模块盘点：先判定“存在”再决定删除

基于远程 master 的当前源码目录和入口核对：

| 名称 | 当前事实 | 最终决定 | 严禁误删 |
| --- | --- | --- | --- |
| render_tree / RenderTree | crates/gpui/src 下**不存在独立 render_tree 模块** | 不新增完整树；不把 Scene 更名为 RenderTree | 已有 Element、Taffy、Dispatch Tree |
| render_commit / RenderCommit | **不存在独立 render_commit 模块** | 不新增与既有提交协议并行的全量持久对象；统一帧变更通知 | 必要的线程安全、版本化消息 |
| scene.rs + scene/ | 确实存在 Scene、paint_operations、Replay、Finish、Batch、动画边界、Blur 等 | **删除旧整窗口 Scene 作为每帧输入/输出边界的全部职责与模块**；必要几何图元定义归入各自所有者 | 真实 Quad、Shadow、Text、Path、Blur、Surface 功能 |
| render_pipeline.rs + render_pipeline/ | 不是另一套正在运行的渲染管线；实际为 renderer_backend.rs、shader.rs、svg_renderer.rs | 最终**拆散/删除含糊的 render_pipeline 目录及聚合 re-export**：RendererBackend/Options/Policy 归入平台渲染配置；WGSL Shader 类型归入 Shader/Resource 所有者；SVG Renderer 归入图像/资产渲染所属模块；更新全部调用方 | 不得因目录改名就丢掉公开功能、设备选择、WGSL、SVG |
| Frame / SceneSlot / CachedView | UI Frame 包含输入、布局与 Scene；SceneSlot 通过 Arc 共享 immutable Scene | 保留 UI 输入状态与必要 scratch；删除独立 SceneSlot、Scene Index、基于 Scene Range 的 Paint Replay | Dispatch、Hitbox、IME、Focus、TabStops、Listeners |
| PresentationSnapshot/Packet、RenderOwner | 目前按完整 Scene Snapshot 进行 pending/active/latest-wins 和 GPU owner 排队 | **只保留一种 GPU 消费的权威绘制状态和版本化更新事件**；合并重叠的 Frame / Presentation 数据所有权 | Native Size / Layout / GPU Fence / Presented 语义 |
| Nova FrameUpload / retained_upload / draw_steps | 目前含重复 pack/encode/signature/cache 与 GPU FrameSlot staging | 以 GPU resident 编码和变化范围复用替换多层全场景中间副本；DrawStep 仍保留必要的 GPU 命令编排，但不因一次局部变化重建全部记录 | 有用的 GPU Buffer、Atlas、Mask、Barrier、Frame Resource |
| gpui-3d::Scene | crates/gpui-3d/src/scene.rs 下独立 3D 领域类型 | **不属于本次删除** | 3D 模型、地图与皮肤预览 |

注意：删除 render_pipeline/ 是**文件与职责归属清理**，不应混同消除 GPU 真实编码/Render Graph/Draw/Present 功能。中间阶段可以有函数，但不能各自拥有第二套全窗口状态与生命周期。

### 最终目标：一条唯一权威的渲染合成链路

~~~text
GPUI View / Element / Layout / Input （唯一 UI 状态所有者）
  ↓ 依赖失效（Layout / Content / Composite / Surface）
局部计算 + 已有 Element/CachedView 的有效性校验
  ↓ 仅变化片段
持久化 Paint Artifact （有序 Chunk / Property / Filter 依赖；不含额外全量树）
  ↓ 同一条版本化变更通知（不再包裹完整 Scene）
GPU Owner 持有的唯一权威 Compositor 状态
  ├─ 稳定 GPU Draw Range / Buffer / Atlas
  ├─ Color Layer 与 Filter Target（分别管理身份、内容、回收）
  ├─ Animation / Clip / Transform / Damage
  └─ Surface Geometry / Fence / Frame Resources
  ↓ 根据现有资源版本生成必要 GPU 命令
Nova-GFX DX11 / DX12 / Vulkan / OpenGL / Metal
  ↓ Native Swapchain Present（版本验证与窗口覆盖）
~~~

这里的 Paint Artifact 是**同一份持久绘制内容**，不是另一个全量 Scene、RenderTree 或逐帧重新生成的全量 FrameUpload。GPU Owner 根据它维护已驻留的后端资源；允许为 GPU 并行性保留多 FrameSlot、Staging Ring、已提交旧纹理直到 Fence，但这些是不同生命周期的硬件资源，**不能复制第二套权威逻辑绘制状态**。

### 统一所有权：每份数据只能有一个权威修改者

| 数据 | 权威拥有者 | 允许复制/保留 | 禁止 |
| --- | --- | --- | --- |
| Element、布局依赖、输入命中 | GPUI UI Thread | 不可变已提交布局/命中版本 | GPU 线程操作 View/Context/RefCell；重建第二棵等价 UI 树 |
| Paint Artifact / Chunk 身份与顺序 | 单一持久绘制内容管理器（可由 UI 编排、GPU 镜像索引） | 引用、版本号、局部不可变内容 | Scene + RenderTree + RenderCommit 全量三份并行维护 |
| 局部变化与 Damage | 单一版本化更新流 | 未呈现 Damage 的合并结果 | 直接丢弃 Create/Remove/Resource 操作造成状态不完整 |
| GPU Buffer/Texture/Atlas/Layer | GPU Owner | Frame-in-flight 和 Fence 退休队列 | RenderGraph、FrameUpload、FilterPool 各自独立拥有同一物理资源 |
| Presentation 状态、Native Surface | 平台 Surface Controller + GPU Owner | last-valid image / epoch 快照 | 把 UI Layout 完成、GPU Submit、Present 和屏幕显示混为一谈 |
| Filter DAG、Property/Animation | 对应 GPU Compositor 数据所有者 | 小型依赖索引和变更值 | 无变化重建完整 DAG 或让根颜色缓存依附 Blur 插槽 |

**最大化减少传输层**：不改变内容的帧，UI → GPU 传输的静态图元字节数应为零；变更仅传局部内容或属性值，不复制全部子树、不进行全局 Scene equality/Hash/Batch 重建。只有资源依赖、硬件缓冲复用、裁剪/滤镜必须时才产生新的编码和上传。

### 强制删除/替换清单（每个条目必须有代码证明）

- [ ] 删除 crates/gpui/src/scene.rs 与 scene/ 的旧全窗口容器/操作记录/排序/Replay/Finish；必要基础 Primitive/Bounds/Geometry 定义**迁移并更新全部调用方**，不保留只转调旧 Scene 的适配器。
- [ ] 删除 crates/gpui/src/window/frame.rs 中 SceneSlot、Frame.scene、PaintIndex.scene_index、retained_scene_index，以及相关无效范围修补；输入状态/布局生命周期保留在 UI。
- [ ] 改写 window/draw.rs、draw_reuse.rs、retained_identity.rs、element/div/、element/cached_view.rs：Retained 子树可以复用真实有序绘制块，不再在下一帧复制整个旧区间，且匿名 ID、父自身与子树交错的 painter order 保持正确。
- [ ] 改写 window/paint.rs、window/composite_layer.rs：图元、Clip、Blur、Opacity、Surface、RendererExtension 的变化直接作用于持久绘制内容；不产生嵌套 Arc Scene。
- [ ] 消除 platform/frame.rs::PresentationPacket.scene 与 window/state.rs::PresentationState 的全量 Scene 快照双份协议，统一状态变更/版本/合成属性，并保持 UI 与 GPU 的线程隔离。
- [ ] 改写 platform/render_owner/queue.rs、worker.rs：增量数据基于已应用版本；队列压缩不能遗失 Create/Update/Remove 与资源、Resize、Shutdown 的依赖，不需要每个提交另存一个完整绘制快照。
- [ ] 删除 NovaRenderer::pack_scene 与 scene_revision 主导的全部缓存键；重用实际未变化的 GPU Buffer/DrawStep/PathMask/Atlas 与渲染命令，同时不要让一个变更导致全局 Encode/Upload。
- [ ] 移除 FrameUpload 的**全场景 CPU staging 容器职责**，保留必要的小块编码器、紧凑动画参数、FrameSlot 上传 Ring 与一次性 Backend Batch；不能再新增等价的 RenderPacket 中转。
- [ ] 拆开零半径 Root Color Cache 与真实 BackdropBlur Target：分别拥有尺寸/内容/资源代次、纹理池与 Fence 退休；Popup 频繁展开不得无故抖动整条 Target Chain。
- [ ] 删除/拆散 crates/gpui/src/render_pipeline.rs 和 render_pipeline/ 的错误聚合职责，按上述真实所有者搬迁三个现存模块并全量更新编译/测试/文档引用。这里是**整理功能归属**，不是删掉 Shader、SVG 和 RendererOptions。
- [ ] 同步更新 platform/nova、windows、linux/x11、linux/wayland、mac/metal、Blade、StatusItem、gpui examples、benchmarks、测试；不保留 GPUI 的旧 Scene 后端入口。
- [ ] 删除原 Scene 专属日志、指标与迁移期死代码，修复过时文档和多套互相冲突的架构图；只保留本文作为下一次对话状态源，历史审计文档明确标记 snapshot。

清理实现时：以**源码符号引用**验证，不能只执行文本关键字删除。Rust 中合法存在的 3D Scene、设备后端 Shader、渲染命令和 Render Graph 不因为命中关键字就删除。

### 必须解决的七个端到端链路问题

1. **元素计算 → 局部绘制**：谁生成 Dirty？谁向祖先传播？为什么未变化 View/Render 被重构？Taffy / Text / CachedView / Prepaint 是否仅处理必要节点？优先消除 86ms 等长尾，而不是仅删除几十微秒的 Scene Finish。
2. **绘制内容 → GPU 数据**：一次按钮/Dropdown 更新带来多少 RenderRange 变动？Quad、Glyph、Image、Path 是否多次序列化/Hash/Copy？连续帧的传输字节必须可追踪，不变资源应直接驻留。
3. **合成属性 → GPU 动画**：纯 Transform/Opacity/Color 不应重新 Layout 与 Paint；动画结束停止唤醒。过滤属性、文本排版和实际命中保持正确，不能以 GPU 动画造成交互错位。
4. **颜色缓存 → Filter DAG**：Root Color、Background Blur、Element Blur、Clip/Mask 的内容/分配生命期独立。拓扑数量变化尽量复用目标容量，但不复用错误的旧像素；不得无上限堆积纹理。
5. **队列延迟 → Frame Pacing**：UI Update / compositor tick / resource ready / Native Surface 分别调度、合并重复请求；报告从 Dirty 开始到**真正成功 Present** 的延迟及 p50/p95/p99，不拿 GPU owner CPU Busy 当实际 GPU Shader 时间。
6. **Win32 Native Resize → GPU 覆盖**：WM_SIZE 与重型 UI 构帧解耦；记录 HWND client、Layout、GPU Drawable、成功 Present 的尺寸/epoch；新 UI 未就绪时让旧有效画面以适合各窗口类型的方式覆盖全客户区。不能把失配 Scene 当作新尺寸内容。包括 DX11 不出帧、DX12 子窗口崩溃、Vulkan 拉伸、GL 卡顿的专项回归。
7. **资源提交 → 回收**：所有后端保护 in-flight Buffer、Texture、Descriptor、FBO、Swapchain；资源分配/复用/退休由单一所有权控制，有显存预算、内存碎片统计和关闭窗口后的回收证据。严禁通过每帧 wait_idle 掩盖生命周期。

### 反技术债验收合同（未通过不得标记“完成”）

| 验收项 | 必须提供的证据 |
| --- | --- |
| 删除旧结构 | ripgrep 搜索及新 GPUI 主干源码证明：无 Frame.scene / SceneSlot / Scene::replay / Scene::finish / pack_scene / PresentationPacket.scene / Scene revision cache；无二次全窗口容器伪装 |
| 单一数据权威 | 每份画面内容的所有权、变更源、GPU resident handle、资源退休路径可追溯；不允许全量 Frame → 全量 Scene → 全量 Upload → 全量 DrawStep 多份副本 |
| 局部事件效率 | Dropdown Hover/展开/回收、单卡片动画、单字输入、列表滚动的 Layout Nodes / Paint Chunks / Encoded Bytes / GPU Upload / GPU Composite Pixels 有量化记录 |
| 串行延迟 | UI invalidation → Layout → Paint → GPU data ready → Submit → Present 的阶段直方图与 p95/p99；不允许输入延迟改善仅靠延迟 UI 事件 |
| GPU 时间与实际占用 | DX11/DX12/Vulkan/OpenGL 的 GPU timestamp/query 或可比的原生测量；禁止把 CPU 调用时长等同 GPU 花费 |
| 纹理生命周期 | 重复 Popup 与 Blur 变化：物理分配、池命中、失效、Fence 退休、显存峰值/回落都可证明；没有 target_layout_count 持续抖动导致全链重建 |
| 静态场景 | 静态、无动画窗口没有不必要的 Draw/Present/资源分配；纯合成动画无 UI 全量构帧 |
| Resize | Native/UI/Surface/Presented 代次在静止后收敛且像素实际覆盖；Opaque、透明、DComp、多窗口、DPI、最大化、缩小、移动与 Resize 分别验证 |
| 跨后端 | DX11 / DX12 / Vulkan / OpenGL 共用相同的局部绘制/滤镜/Alpha/资源语义；不能通过关局部裁剪或强制全窗口绘制取得“通过” |
| 内存与维护 | Allocations/frame、RSS、GPU residency、peak、pending retire、idle trim、Frame/Resource owner 数量；删除重复所有权与未使用的 Feature Flag / adapter / obsolete doc |
| 工程质量 | 实际 cargo fmt/check/test 命令和结果、没有未经验证的性能提升宣传、Windows GPU 实机与非 Windows 状态分开标记 |

**注意**：允许特殊后端或滤镜为正确性在最终 Swapchain 执行整目标合成，但不等于允许 CPU 再构造、上传全部不变原语。完整 Present、GPU 栅格化、CPU Encode 与 UI Prepaint 是不同成本，需分别统计。

### 执行排序：端到端纵切，允许最终删除时一次性破坏内部 API

- S0（当前）：依赖与成本审计、完整基线、确定职责所有者；本文和历史审计一致，禁止再扩展新 RenderTree 原型。
- S1：在至少一个原生后端实现单一持久绘制内容、局部绘制与 GPU resident 纵向通路；不改完所有后端不允许宣布完成。独立 GPUI 最小测试必须验证静态、Dropdown、Blur 与 Input。
- S2：在共享 Nova 层统一 Filter/Root Color/Target 资源与 Present 调度；并行独立解决 Win32 Resize 同步构帧与代次覆盖，不能等待 S4 才修黑边。
- S3：将 DX11/DX12/Vulkan/GL 接到**同一个**通用合成语义和提交接口；对于 Metal、Blade、Linux consumers 同步迁移或明确可编译地退出支持范围，不保留老 Scene 后门。
- S4：彻底清理 Scene / render_pipeline 旧模块与所有引用、重复临时结构、兼容适配器、过时文档；保持 GPU Barrier 和线程所有权安全。
- S5：全场景 Correctness + GPU 时间/内存/显存/帧延迟回归，以旧基线对比；所有测试不执行则明确未完成。

最终完成 = **旧全窗口 Scene 真实删除 + 不存在另一个等价全量 RenderTree/FrameUpload + UI 局部计算可量化 + GPU 资源跨帧复用可验证 + Windows Resize 正确 + 四后端等价 + 不留兼容/遗留文件技术债**。不得仅以编译通过、减少一处分配或新增 TODO 证明整项工作完成。

## 阶段与状态（勾选代表代码/文档已写，不代表四后端实机验证）

### P0-A 共享 GPU Filter / Color Target 生命周期

- [x] **本轮代码（2026-10-14，单一路径压缩）**：删除重复的 `reuse_stable_prefix_layout` 入口，合并进 `reuse_layout`；实现按稳定 Capture 槽身份一对一重排（包括非前缀顺序、缩减活动数），直接搬移 GPU target descriptor/resource-set handle 而不重分配 GPU 纹理；不兼容时保留原有根颜色特殊映射 / zero-radius rebind 路径。新拓扑复用后统一强制刷新滤镜内容与 DrawStep，杜绝错误采样旧像素。
- [x] 新增模拟 GPU TextureView 身份重排、失败不修改旧状态的单元测试；**进一步改为原地 Vec::swap 重排**，防止复用物理 GPU 纹理时却反复分配 CPU Target Descriptor Vec。测试检查 Vec backing pointer 在变更前后不变；单元测试尚未运行。
- [ ] **本轮测试未执行**：GitHub 远程编辑环境不提供 Rust/Cargo；需本地运行 `cargo fmt --all -- --check` 和 `cargo test --manifest-path crates/gpui/Cargo.toml --lib gaussian_capture`，再做 Windows 4 后端高频弹窗 Native GPU 回归。不得将代码已提交视为性能证据。

- [x] 审计 Zero-radius Capture、Root Retained Color、目标布局复用路径和最新日志。
- [x] 增加**稳定前缀布局**复用：在现有 GPU 纹理规格/目标身份均不变且只改变活动 Capture 数时，保留已有分配，不再走完整目标链重建；覆盖 Gaussian Source 情况。变更后保守刷新 Filter Cache 和 DrawStep。
- [x] 为上述稳定前缀路径增加无实际 GPU 依赖的单元测试，确认 TextureView 身份保留及不兼容时拒绝复用。
- [x] 为无法复用而需要 Recreate 的情况补充拓扑、Root Source、Variant 计数和 in-flight 数量诊断。
- [ ] Windows 真机确认新增路径命中率、纹理重建次数与 GPU/CPU 耗时（DX11/DX12/Vulkan/OpenGL）。
- [ ] 分离 Root Color Residency 与 Gaussian/Backdrop Filter Target；取消零半径 Blur 伪合成的结构性耦合。
- [ ] 过滤布局以稳定 Capture ID/Generation 表示，避免扁平索引扰动带来的全链路失效；处理跨帧 Fence 与资源身份。
- [ ] GPU 纹理预算、空闲 Target 复用与高频弹窗的压力/泄漏测试。

### P0-B 帧请求、UI 计算与 Windows Resize

- [x] **2026-10-15 Prepaint 选择性重放正确性修复（代码层）**：将 `reuse_prepaint_fragment` 的 DispatchTree parent-map 验证/节点转移置于 Hitbox、Tooltip、ElementState、TextLayout 变更之前，避免因无效父路径返回 false 时已消费旧 Tooltip 或残留输入命中状态。依赖 `DispatchTree::reuse_fragment` 已有的全范围预验证保证其失败时不搬运 listener；没有新增双缓冲或回退协议。
- [x] **2026-10-15 元数据搬运临时数组收敛（代码层）**：`replay_retained_element_metadata_fragment` 移除 `SmallVec<[bool;64]>`，由唯一 `included_prefix` 的相邻差分判定纳入集合，同时提供重排后的 offset；添加 256 种 8 节点位图的计数与区间完整性单测（未运行）。这是一次临时数据结构删除，不等于旧 Scene/Replay 已删除。
- [ ] **待执行**：本地 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib compact_inclusion_prefix` 和 CachedView 嵌套 Prepaint/Focus/Tooltip/IME 验证，尤其是失效后 fallback 的无副作用性。当前无 Rust 工具链，尚未实机确认延迟收益。
- [x] **代码修改（2026-10-14，Win32 消息栈解耦）**：删除 `dispatch_native_resize_frame` 及从 `WM_SIZE`/`WM_WINDOWPOSCHANGED`/`WM_EXITSIZEMOVE` 直达 `dispatch_pending_update_with_vsync(None)` 的同步构帧分支；原生事件只对齐 HWND Client 尺寸/epoch、排入最新 Resize/UI 请求，交给现有 modal VSync/foreground-task/fallback timer 消费。不新增 Frame/Scene/RenderCommit 状态或兼容入口。
- [x] **代码诊断（未运行）**：在 `dispatch_pending_resize` 的消费路径记录达到 16ms 的 resize callback 耗时与尺寸代次；该耗时是 CPU callback 墙钟时间，**不是 GPU 时间，也不能代替 GPUI Prepaint 分项计时**。
- [ ] **仍须验证**：Rust fmt/check、WM_SIZE→modal VSync 消费与 Windows 四后端实机 Resize，重点检查扩大窗口时旧图覆盖、DX11 不出帧、DComp/透明子窗口、失活/无 VSync fallback；此项不能被当作黑边已解决。
- [ ] 详细计数与标注每次 Frame Request 的 UI/Paint/Composite/Surface 原因，消除同一时钟重复唤醒。
- [ ] 剖析 Prepaint 长尾：Taffy Bounds 缓存失效、Text Layout、CachedView Selective Splice、Overlay/Dropdown 的具体触发者。
- [ ] 将 Window Resize Native Message 快路径与可能耗时的 Layout/Prepaint 解耦；确认 Win32 modal event loop 仍可及时调度 UI。
- [ ] 建立 Native/UI/Surface/Presented 代次与最终像素覆盖诊断；区分窗口 Move 与 Resize。
- [ ] 复现并定位 DX12 子窗口关闭异常、Vulkan Resize 拉伸、DX11 不出帧和 OpenGL 交互卡顿。

### P1 统一帧内容表示与合成提交

- [x] **2026-10-15 自动背景 Source Scene Replay/Finish 全部删除（仅代码实现，未经 Rust 编译和 GPU 实机验证）**：原来的 `auto_capture_static_background` 不再创建 Background `Scene` 或 `Arc<Scene>`。使用现有唯一的 `FrameUpload` 直接从已编码、排除 Clip 退化的 Quad/Shadow 有序 Batch 中计算源内容指纹（类型、实例数、打包字节和顺序），原位插入已有 Blur Marker；新源只保存 Bounds/ContentMask/Hash/Bytes，而非第二套 Paint 所有权。
- [x] **统一 Filter Source 输入类型（仅代码实现）**：`FilterSource::Element(PaintBlur)` 保留嵌套 Element Blur 的原有 Scene/动画语义，`FilterSource::AutoStatic` 仅保存 GPU 静态源内容身份；同一个 `FilterRegistry` 依据成功 Present 的离屏目标刷新单调 `TextureContentVersion`，新旧 Source 类型变化不可复用旧目标。自动静态源内容指纹改变时强制整层刷新，避免脏矩形为空或不足导致旧纹理被误保留，原有 Filter 创建/退休机制不变。
- [x] **单测源码（未运行）**：静态内容变动失效、前景动画变动不失效、Filter 内容版本只在成功刷新时推进、AutoStatic 与真实 Element 类型隔离、Quad/Shadow Batch 切分与空 Clip 退化回退。
- [ ] **未完成验收**：本地 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib auto_static`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib retained_filters`，Windows DX11/DX12/Vulkan/GL 的 Dropdown/Blur/动画/Resize/切换页面及目标 GPU Timing。无 Rust 工具链，不能声称测试通过或性能改善；Scene/SceneSlot/全局 Replay/FrameUpload 主链路仍未删除。
- [x] **2026-10-15 自动背景捕获删除整份 Foreground Scene（代码层，未编译）**：`auto_capture_static_background` 不再 `Scene::replay(prefix_end..)` 构建动态前景、副本 `Scene::finish` 后再次编码；只构建一份静态背景 Source Scene 用于原有 Filter 内容版本/失效证明。`pack_scene` 直接对唯一原始 Scene 编码一次，再由 `FrameUpload::capture_encoded_static_prefix` 按原始 DrawOrder、排除零面积 Clip 后的 Quad/Shadow GPU 实例索引，原地切分最多一个混合 Batch，插入 BeginBlur/EndBlur/CompositeBlur，重建现有 FilterConfig。动态数据的 Source Batch 无额外完整 Scene 或二次编码。不能安全切分时不修改编码结果，回退已有直接绘制/Root Color 策略，而不是保留原 Foreground Scene 兼容路径。
- [x] **2026-10-15 测试源码（尚未运行）**：静态100 Quad+动态1 Quad（两段 Batch）、静态100 Shadow+动态1 Shadow（Shadow 单 Batch 内切分）、裁剪掉动态后的拒绝/无副作用、旧/新 Source Content 改变正确失效、Blur marker/source 范围。必须测试 Layer 嵌套、不同 Solid 混合、滚动、DPI/Resize、动画和 Filter Target Fence。
- [ ] **仍未完成**：最终删除静态 Background 的 `Scene::replay/finish` 与 `Arc<Scene>` 内容身份依赖。应让持久 Paint Chunk/Source ContentVersion 取代 Source Scene 的深度对比，在 FrameSlot 和 GPU Filter Owner 同一代次下管理。避免新建 RenderTree/RenderCommit 或 FrameUpload 第二份全场景容器。
- [x] **2026-10-15 直接编码 GPU Root Color 合成记录（代码已提交，单测待执行）**：`append_retained_root` 删除只为序列化零半径 Root Blur 而分配的 `Arc<Scene::default()>` 与临时 `PaintBlur`。生产编码共用 `write_paint_blur_fields`，实 Element Blur 包装器转调相同字段 ABI，保持原 136 字节缓冲布局、合成顺序与四后端 Shader 语义。该改动只移除**空 Scene 的错误构造**，不应声称真正静态背景的两次 Scene Replay 已删除。
- [ ] 针对 Root Color 合成 ABI 的 Rust 单测/性能计数、四后端静态/Blur/弹窗实机仍待执行。
- [x] **2026-10-15 Retained Quad 当前布局校验与描述符分配去重（仅静态代码）**：`QuadResidentLayout::from_upload` 从原先的 `Vec::clone() → Arc::from(Vec)` 双重分配，改为从 `&[RetainedResidentSpan]` 直接构造唯一不可变 Arc。局部上传前同时校验 old/current Chunk byte range 的非空、排序、无重叠和当前 buffer 上界；当前布局损坏即回退 Full，避免错误跳过上传。允许旧布局长于缩小后的当前 buffer，并对这些边界添加单测（未运行）。
- [ ] **本地验证**：`cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib chunk_upload`、Windows Native 多 FrameSlot/Resize；此改动不意味着底层 Scene/FrameUpload 的重复权威状态已被清理。

- [x] **2026-10-15 Scene Finish 候选隔离证明去重复扫描（源码已改、测试未运行）**：`Scene::prepare_retained_quad_chunks` 在候选数≥8 且 PaintOperation 数≥128 时，惰性构建一次仅含 `DrawOrder` 的临时排序索引，用二分统计候选范围内的所有 Primitive 操作数，替换多候选的重复全量 `paint_operations` 顺序扫描。候选操作已经全部证明为 Quad 且其最小/最大 DrawOrder 正是范围端点，所以全局计数等于候选 Quad 数与原有“范围外没有同 Order 操作”条件等价；重复 Order 与嵌套 Blur 子操作仍参与计数。小场景原扫描保留且不建立索引。新增 32 种长度下所有连续区间的等价性单测源码。
- [ ] **仍须实测**：比较多个 CachedView/Dropdown 同时重放时 `scene_finish_us` p50/p95/p99、内存分配与短片段阈值，不得直接归因于 Prepaint 86ms；CI 不替代 cargo 本地执行。
- [x] **代码提交（2026-10-15，Retained Chunk 区间化编码）**：`FrameUpload::encode_retained_quads` 不再为每一个 Quad Batch 全量扫描所有 Retained Chunk；直接使用 `Scene::finish` 已建立的**按 Quad 索引排序、互不重叠**的有序片段，经两次 `partition_point` 截取相交部分。保留 Chunk 相邻范围、Solid/Non-solid、Cache 命中、Painter order 的原有检查。新增与旧全量扫描结果对照的覆盖测试（仅编写，尚未执行）。
- [x] **Scene Finish 低成本剪枝（未实测）**：对候选 Retained Chunk 先检查已排序的 Quad Buffer 范围完整性，再执行全 `paint_operations` 顺序隔离证明，避免无法表示为单个 GPU 范围的候选触发昂贵的全表遍历；不修改正确性判定，不新增 Scene 索引/RenderTree。
- [ ] **仍未完成**：持久 Chunk 唯一所有权、Paint/Prepaint 粒度失效与 Scene/FrameUpload 删除；需运行 `cargo fmt --all -- --check`、GPUI retained scene 与 nova static_dynamic_batch_tests，以及 Windows GPU 实机。性能收益尚无基准数据。
- [x] **代码修改（2026-10-14，Retained Quad 描述符去重）**：复用完全相同的有序 `QuadResidentLayout` Arc backing（按 Chunk ID、范围、Generation、内容 hash 完整比较），减少新 Scene revision 引起的重复 CPU 描述符分配；Quad 局部上传范围规划以已排序的 GPU byte offset 做线性匹配，不再每次构造临时 FxHashSet；乱序旧布局保守全量上传。**这只消除描述符复制/临时哈希表，不代表 Scene/FrameUpload 已删除。**
- [x] **单测源码（未运行）**：添加复用 Arc、内容变更后拒绝复用与乱序回退验证；既有兄弟 Chunk 局部上传覆盖保持。
- [ ] **待验证**：`cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib chunk_upload`、GPU Buffer 多帧槽/多窗口内容等价；本轮环境无 Cargo/Rust。
- [ ] 明确目前 Frame → SceneSlot → PresentationPacket → pack_scene → FrameUpload → DrawSteps 的职责重复点及每帧字节数，并按上方“强制删除/替换清单”逐项确认**删除落点和唯一所有者**。
- [ ] 设计**唯一一份权威的持久绘制记录**：稳定 Chunk/绘制顺序/属性依赖，避免复制整 Scene 或再建一棵冗余 Render Tree。
- [ ] 用增量变更通知 GPU Owner，旧静态 Chunk 不重复编码/上传；正确处理旧变更丢弃、资源创建/释放顺序和 damage 累积。
- [ ] 删除被替代的 Scene Replay/Scene Finish/Pack 主链路及其旧缓存规则，而不是只改名。
- [ ] Element 命中、IME/Focus、CachedView、动画、Blur/Clip 在新旧视觉代次之间保持正确性。

### P2 Nova Compositor / Resource Lifetime

- [ ] 长期驻留的可复用 GPU 编码、DrawStep、Buffer Range 与 Texture 资源；预算和碎片回收。
- [ ] GPU 原生 Pass 时间戳和关键上传/资源创建/销毁/Fence 数据。
- [ ] 独立合成属性动画、按需 Present，静态窗口不轮询；留意 240Hz 与后台限帧。
- [ ] 四后端统一的 Barrier/Load/Clear/Scissor/Texture Alias/Resize 首帧正确性测试。

### P3 删除旧数据结构并验证

- [ ] 清理旧 SceneSlot / Frame.scene / PresentationPacket.scene / pack_scene / Scene::replay / Scene::finish / 全量 FrameUpload / 旧统计（**不保留双路径**）；拆散名称误导的 render_pipeline，**不删除其真实 Shader/SVG/BackendOptions 能力**。
- [ ] 独立 GPUI 性能实验：Dropdown 高频展开、Blur 切换、静态页面、Text/Image、动画、窗口 Resize/多窗口。
- [ ] BMCBL 实机回归，尤其页面切换、弹窗、地图、透明/模糊与输入法。
- [ ] 对比 CPU 帧长尾、GPU Present 频率、显存、上传量、Filter Target Recreate、资源回收；不可编造提升百分比。

## 本次改动的边界与验证状态

- **代码层面（累计）**：稳定 Filter Target 复用、Win32 WM_SIZE 同步构帧入口删除、Retained Quad 布局共享/局部上传、按排序 Chunk 区间编码和 Scene Chunk 完整性前置检查。均是已有单链路的局部纵切，尚未删除 Scene/FrameUpload，也不能宣称统一合成器已完成。
- **本地编译/测试**：当前运行环境未提供 Rust/Cargo、Windows GPU，也不能通过容器连接 GitHub 克隆仓库；已做源码静态审查与 2695 例 JavaScript 区间算法等价检查。Rust 单元测试已加入源码，**未运行**；没有 Cargo fmt/check/test 或 Windows 四后端实机结果。
- **不得宣称**：Windows 黑边已修复、四后端 GPU 占用已改善、Scene 已删除、Prepaint 长尾已解决。

## 2026-10-14 继续重构：原生 Resize 的同步构帧去重

- **变更文件**：`crates/gpui/src/platform/windows/window.rs` 与本文。基线 master `0e45ca39d4cb79dbb184e09a4c1becdc7058272f`；本次提交以 GitHub commit 记录为准。
- **具体删除**：旧 `dispatch_native_resize_frame`（包括一次 `request_frame` 后紧接同步 `dispatch_pending_update_with_vsync(None)` 的二次入口），去掉 WM_SIZE/WM_WINDOWPOSCHANGED 同步调用与 EXITSIZEMOVE 同步调用。原有 modal VSync、foreground-task 和 watchdog 仍负责实际投递更新。
- **保留约束**：`pending_resize` 最新值、尺寸 epoch、Scene/Surface 匹配、in-flight GPU 资源与 UI 输入状态均不被重写；`queue_resize` 自身已发出 UI request，WM_SIZE 不应再重复强制消费。
- **风险与验证**：当前容器没有 Rust/Cargo 和 Windows 显卡设备，不能运行格式检查、编译或 GPU 原生验证；必须本地复现 WM_SIZE 消息嵌套、DWM 暂停时的 watchdog、退出 modal 的最后一帧和四后端 pixel coverage。未解决 Root Color 与 Blur 资源耦合、SceneSlot/Replay/FrameUpload 冗余以及 Prepaint 86ms 长尾。
- **后续优先级**：以 Windows 原生日志确认新的 Resize 消费源和 p95，再按 P1 逐段替换 Scene → FrameUpload 的权威数据重复，禁止新增 RenderTree/RenderCommit。

## 2026-10-14 继续重构：Quad 局部上传索引去重

- **变更文件**：`crates/gpui/src/platform/nova/renderer/chunk_upload.rs`、`renderer/retained_upload.rs` 与本文，接续 Windows Resize 解耦提交 `74088d480c062a8207eeb5ea374f7fa0d72c1619`。
- **唯一所有者不变**：GPU Owner 的 FrameSlot 仍保存每个 slot 的驻留布局身份，当前 Quad Span 作为不可变 Arc 与上传槽共享；不建立第二份权威 Paint 内容，也不变更资源 Fence。
- **可观测事实**：相同 Span 描述符无需重复分配新 Arc；局部上传规划不再为 previous spans 单独分配 HashSet。尚未取得 CPU p95/Allocations/frame 或 GPU 上传量实测。
- **尚未完成**：消除 Scene/Replay/pack_scene、Root Color 与 Blur 分离、四后端正确性及本地编译。上述条目仍是 P0/P1/P3 主线，不得作为完成勾选。

## 2026-10-15：Retained Chunk 编码遍历与隔离校验纵切

- 源码审查基线：`331ae0a4b41873f03514958aebc1f9d19e1bd1c0`；提交期间 master 持续接收其他 UI 提交；每次重新读取 HEAD 与本次目标文件 Blob/Tree，确认无 GPUI 冲突后采用 expected_sha 的非强制快进提交，不覆盖并行变更。
- 生产代码：`crates/gpui/src/platform/nova/frame_upload/encode/retained.rs` 与 `crates/gpui/src/scene/display_list.rs`，另同步本文。
- **计算复杂度变化**：当同一 Scene 有 B 个 Quad Batch、C 个保留 Chunk 时，原编码遍历按 Batch 扫 C 个槽位；新路径在 Scene 已排序、互不重叠的不变量下每 Batch 二分相交区间（`O(log C + overlap)`），没有新增第二份索引。不能用估算复杂度代替实机加速结论。
- **保守正确性**：`Scene::prepare_retained_quad_chunks` 的 `paint_operations` 外部顺序扫描仍保留，因为嵌套 Blur 捕获会有未展平到当前 Scene primitive vector 的子绘制操作；优化仅提前进行不影响最终判定的 Quad 完整性检查。
- **尚未验证**：无 Rust 工具链／Windows GPU，本次只是静态补丁和测试源码；下一次重点核查 Compiler/Rustfmt、Scene Blur、Retained Chunk 复用、Dropdown CPU Encode 与四后端 Present。
- **尚未清除技术债**：Scene/SceneSlot/Replay、GPU root zero-radius filter 与 FrameUpload 的全场景 CPU staging 均存在；本条不得算 P1 完成。

## 2026-10-15 续改：Prepaint 事务性重放与元数据合并

- 基线 HEAD：`446defcff320895034e7d500dc32666d29b8c7f5`。代码文件 `crates/gpui/src/window/draw_reuse.rs`，唯一进度文档为本文。
- 验收语义：`DispatchTree::reuse_fragment` 首先验证全片段父路径映射，失败直接返回 None；`Window::reuse_prepaint_fragment` 仅在其成功后搬运辅助 Prepaint 状态，避免旧 Tooltip 提前被 take() 造成 UI 输入状态损失。
- 内存路径：片段元数据重放从两份 SmallVec 标记/前缀表减少为一份 prefix；仅改变选择/偏移的表示，原先关于完整子树、paint/prepaint/child Scene 代次和保守回退的判断保持不变。
- 测试分层：静态审查及代码内用例并不等于通过 cargo、Windows DX11/DX12/Vulkan/GL 原生验证；Prepaint p95、GPU 使用率、Scene/FrameUpload 清理和 Filter Root Color 解耦仍未完成。
- 下一阶段：核对 CachedView fragment 复用的边界与退化日志，并逐步迁移唯一 Paint Chunk 所有权，避免新增 RenderTree、RenderCommit 和重复 staging。

## 2026-10-15：Scene Finish Retained 候选隔离优化

- 基线：`3f21b67fc72e3027ef1b62c6bea099518b2cb83d`；修改 `crates/gpui/src/scene/display_list.rs` 与本文，上一轮 Prepaint 事务性重放提交 `3f21b67f` 已在 master。
- 确认保留全 `PaintOperation`（包括嵌套 Blur）Order 语义，不能只索引扁平 `Scene.quads`；在大候选场景才惰性构建临时 DrawOrder 数组，排序后以二分计数，避免 C 次完整扫描。该数组不是持久绘制状态或第二棵 RenderTree。
- 正确性验证：新增排序计数与原范围外检查的纯逻辑对照单测；真实窗口 Blur/Dropdown、运行时间、显存和四后端原生测试仍未执行。
- 重要未完成项：Prepaint 真正耗时贡献、Paint Chunk 单一所有权、Scene/FrameUpload 删除、Root Color 与 Filter 拆分、DX11/DX12/Vulkan/GL 回归均未完成。

## 2026-10-15：Nova GPU Retained Quad 上传安全边界

- 代码基线：`898eeae62ac6810ec817d86523b54c664dc57bb2`。文件 `crates/gpui/src/platform/nova/renderer/chunk_upload.rs`，同步唯一进度文档。
- 改进：避免从 Vec 再拷贝到 Arc 的中间暂存所有权，局部上传前验证当前与旧布局均为合法有序片段；不新增任何 RenderTree/RenderCommit、GPU Resource Owner 或兼容层。
- 风险隔离：过时的大尺寸 old FrameSlot 仍可能在尺寸变化后保存有效布局，因此不按新 byte_len 校验旧布局尾部；current 必须满足新的 GPU Buffer 上界，损坏时回退全量上传。
- 尚未验证：无 Cargo/rustc/四后端显卡，已添加单测但尚未执行；未知收益不得写为已实测。
- 下一步：继续减少 FrameUpload 对完整 Scene 的打包/签名扫描，验证 CachedView 局部失效对应实际 GPU bytes，并进一步拆分 root color / Filter DAG 资源所有权。

## 2026-10-15：直接 Root Color GPU 记录，移除空 Scene

- 基线：`5d349bc50661389cd053632433ae42e1495e2ce3`；代码文件 `crates/gpui/src/platform/nova/upload_packing.rs` 与 `frame_upload/encode.rs`，另同步唯一 TODO。
- 生产变更：`write_paint_blur` 保留真实 Element Blur 的原有调用契约，委托共享 `write_paint_blur_fields` 写同样的 136-byte packed ABI；`append_retained_root` 直接传递零半径、尺寸和 mask，删除 `Arc::new(Scene::default())`。新字段编码不触及 Filter DAG、GPU Barrier、资源退休或 Shader。
- **必须继续删除**：`auto_capture_static_background` 仍然会为静态前缀和动态前景分别 `Scene::replay` / `finish`；处理它必须先证明自动捕获的 Prepared Batch 边界、Drawable extent、Blur 配置及跨帧 source-content version 不变量，不能随意丢失 GPU retained source。
- 本轮 GitHub 编辑没有 cargo/rustc/Windows GPU 测试环境，性能和四后端等价未确认。

## 2026-10-15：自动背景捕获原地编码与真实资源范围

- 本次连续改动承接 `c68363a37db169814f2798033adb7b5a628c1088`（Root Color 空 Scene 删除），基于其后 HEAD 追加 `renderer/retained_upload.rs`、`frame_upload/encode.rs` 与本文的纵向修改。
- 新链路：`原始 Scene.prepare_batches → FrameUpload::encode(原 Scene) → capture_encoded_static_prefix(在原有 GPU 实例批次内切开) → Filter Source/DrawStep/Present`。不再创建中间 Foreground Scene；不再对动态前景再做 `replay/finish`。
- 旧前景 Scene 会变更上传 Primitive instance 排序和 Painter-order；因此严格要求录制次序的 DrawOrder 分界：前缀操作 Order 小于动态第一个 Order，后缀不小于；编码后以**裁剪后的有效实例计数**确认已分组 Batch，且仅对 Quad/Shadow 合法批次切开。任一条件不成立原地回退，而非部分提交 Batch。保留单一 Source Scene 用于 `FilterRegistry::source_unchanged` 的原内容版本证明，不能简化成只看 revision。
- GPU Target：不改变 Shader 136 字节 blur ABI、原有元素模糊目标分配及 Fence 退休；已复用 `FrameUpload::refresh_backdrop_blur_configs/rebuild_backdrop_blur_passes`。DX11/DX12/Vulkan/GL Native Buffer、重合阴影、Clip、零面积、Live Resize 均**没有**实测。
- 下一步：Source ContentVersion 脱离背景 Scene，并对 auto source Texture Residency 接入持久 Paint Chunk 原有逻辑，最终从代码删除所有自动背景 Scene Replay；在此之前 P1/P3 保持未完成。

## 2026-10-15：自动静态背景内容身份从 Scene 移至 GPU 编码/Filter Owner

- 起点 HEAD `80e17ecdfe52f672a5c7d7e24f39285a397ce07b`，生产代码：`crates/gpui/src/platform/nova/frame_upload/frame.rs`、`frame_upload/encode.rs`、`renderer/retained_upload.rs`、`renderer/filters.rs`、`renderer/draw_steps.rs`；修正 `renderer/present.rs` 单测中旧 Source 类型。
- **本次真正删除**：`auto_capture_static_background` 的 `Scene::default` / `Scene::replay(0..prefix_end)` / `Scene::finish` / `Arc::new(background)`；自动背景不会再复制任何子 Scene。Encoder 的源指纹按实际渲染的有效 Quad/Shadow Batch 次序读取打包字节，不能仅凭 Scene revision/ElementID 跳过内容变更。
- **无新的所有权层**：唯一 `FrameUpload` 仍持有本帧临时批次，唯一 `FilterRegistry` 仍持有在成功 GPU 提交后才推进的内容版本；`FilterSource` 替换原有 `Vec<(u32,PaintBlur)>` 语义类型而非建立并存缓存；AutoStatic 不持有 Scene。真正的 Element Blur 保持旧的深度匹配与动画区域计算。
- **正确性边界**：AutoStatic 仅接受原来已证明的 96+ 个纯 Quad/Shadow 静态前缀；Layer 必须平衡，不能有 Retained Chunk/Atlas/Shader Effects/混合跨顺序。已编码 Batch 不合法仍原子回退；指纹差异或未验证目标强制重绘完整源，不依赖窗口 DirtyRect 的覆盖范围。Blur Target 创建/更新/提交时序不变。
- **下一步起点**：验证新 Source Type 的所有消费者、单元测试和 GPU 原生渲染；补齐单一 Paint Chunk 的真实版本/所有权并继续消除 SceneSlot/Scene Replay 和双重 CPU FrameUpload，在验收前 P1/P3 未完成。

## 2026-10-15：FilterRegistry 退役源快照与有序索引

- 起点 HEAD：`90ea49cc54e3fd4cd40d5664b4a3814d57ad08b5`；修改 `crates/gpui/src/platform/nova/renderer/filters.rs`，同步本唯一进度文档。
- **代码实现，未编译验证**：成功 Present 后，仅移除本帧已不存在的 FilterSource 驻留快照；仍在帧内但未执行离屏刷新者保持原 TextureContentVersion，不会凭普通 Present 伪造源更新。消失的 Element FilterSource 不再长期强持有旧子 `Arc<Scene>`。
- 保留唯一 GPU FilterRegistry 所有者，源列表采用已存在的 `Vec` 按 index 有序插入，查找和活动源裁剪使用二分；不创建 RenderTree/RenderCommit/第二份缓存。依赖 `FrameUpload::encode_scene` 按 packed backdrop index 单调分配 source 的事实，加入 debug 断言及测试覆盖反序刷新、未刷新版本延续、旧 Scene Arc 释放。
- **尚未验证**：无本地 Rust/Cargo 及四后端 GPU，本改动未执行 rustfmt/check/test、Windows Render Graph/Fence 回归；不能宣称显存或 GPU 占用已下降。缓存复用仅代表 CPU 侧元数据/Arc 生命周期，不代表实际 GPU Target 已退休；目标资源仍由原有 Fence/TargetPool 路径管理。
- **下一步**：验证 FilterSource 迁移后的原生四后端编译与动态页面过滤器数量回落，再推进 CachedView 持久 Paint Chunk 唯一所有权、SceneSlot/Replay/pack_scene 的有依赖删除；仍须验证 Dropdown、Blur、DX11 Resize、DX12 子窗口、Vulkan/OpenGL。

## 2026-10-15：Element Filter 动画快照同代共享

- 接续 master `c6398104e513932b91cec36590d919fd9e9e34b3`。在 `crates/gpui/src/platform/nova/renderer/filters.rs` 中将各 Element Filter 离屏刷新时的动画快照，从逐滤镜独立 `Vec<SceneAnimationValue>` 复制改为同一次成功 Present 的共享不可变 `Arc<[SceneAnimationValue]>`；无动画与 AutoStatic 不分配此快照。不同刷新代次独立持有，未重绘的 Element 继续保存原始采样值，不受其他目标的刷新影响。
- 正确性：只优化 CPU 样本所有权和复制次数，保持 TextureContentVersion、Source Damage、GPU Shader 采样协议、Filter Target/Fence 和真实动画轨迹语义。新增同代 `Arc::ptr_eq`、跳过目标后的旧采样保留、清除失效版本等源码单测。
- **未通过验证**：容器仍无 Rust/Cargo 与四后端原生 GPU；没有执行 cargo fmt/check/test 或实机测试。性能效果只可称潜在减少分配，不声称现有瓶颈已经实测解决。下一步需要继续将 FrameUpload 与 retained Paint Chunk 归为单一所有权，按既定删除清单清理 SceneSlot/Replay。

## 2026-10-15：Retained Quad 持久编码缓存迁入 GPU Owner，统一嵌套 Chunk 存活范围

- 起始 master：`363195999e129e64fcd34176e6b5e794b725748c`。本次修改 `crates/gpui/src/platform/nova/frame_upload/{frame.rs,encode.rs,encode/retained.rs,capacity.rs,quad_stream.rs}`、`platform/nova/renderer/{retained_upload.rs,memory.rs}`、`platform/nova/renderer.rs`、`platform/nova/renderer/shared_buffers/native_tests/chunks.rs`、`platform/nova.rs`、`platform/nova/tests.rs`；本文是唯一进度文档。
- **真实所有权迁移（代码实现，未编译）**：`RetainedQuadChunkCache { retained_quad_chunks, active_retained_chunk_ids_scratch }` 是 GPU Owner 下 `RetainedUpload.quad_chunks` 的独占持久成员；这两个跨帧容器从 `FrameUpload` 删除。本帧编码器只借用 `&mut RetainedQuadChunkCache`，`FrameUpload.quads` 只持有相同不可变 Arc 源的分段引用、增量 dirty gaps 与本帧 `resident_quad_spans`。不引入 RenderTree、RenderCommit、另一份 packed byte 缓存或 Scene 兼容层。
- **嵌套失效修复**：原 `FrameUpload::prune_retained_quads(scene)` 只采集根 Scene Chunk，递归 Element Blur Scene 编出的保留 Chunk 即使已进缓存也会被误删。现在编码入口先 `cache.begin_frame()`，每个实际准入的 Quad Chunk 在递归编码中标记存活，根编码结束时对唯一 GPU 缓存进行一次 `prune_retained_quads()`。完成纹理提交前不修改 GPU Fence/Target/FrameSlot；已借入当前帧的 Arc 在缓存 Trim 后仍有独立生命周期。
- **内存/压力路径同步**：`retained_quad_memory` 与 `retained_byte_capacity` 接受 GPU 缓存所有者并按 Arc 地址去重，`NovaRenderer::trim_retained_capacity` 对临时 FrameUpload 和持久 Quad 缓存分别执行裁剪；两套 bench 明确持有自身测试用缓存。内存统计仍不能替代驱动 VRAM 实测。
- **测试源码（未运行）**：更新全部已定位的 `FrameUpload::encode`、内存统计、bench、Native GPU 测试消费者；新增释放/重新构造 FrameUpload 后仍能命中驻留 payload，以及嵌套 Element Blur 下的 Retained Chunk 不被根帧错误裁剪的对照测试。需要 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib retained_quad`，Windows DX11/DX12/Vulkan/OpenGL 及 Live Resize/Fence 实机验证，当前环境未运行。
- **不能标记已完成**：SceneSlot、`Scene::replay`、`Scene::finish`、`NovaRenderer::pack_scene`、全场景 FrameUpload staging、完整持久 Paint Chunk、GPU property compositor lane 仍存在。下一次必须先编译/验证此所有权迁移，随后推进直接以 CachedView Chunk 变化集驱动 GPU resident encode/DrawStep，移除更多 Scene Replay 与全帧打包链路，不添加新包装中转。

## 2026-10-15：修复迁移后测试并用 Chunk Epoch 清理存活集合

- 基线 HEAD：`005d3f7551f016aaa3eb8273e9e4d8879be87eea`。源码追踪确认上轮 `crates/gpui/src/platform/nova/tests.rs` 的独立 FrameUpload 生命周期测试漏声明 `chunk_cache`，另有两处旧 `upload.retained_quad_chunks` 访问；本次一并修复。前一版**尚未通过 Rust 测试**，不能将其描述为已编译。
- 以 GPU Owner 的 `RetainedQuadChunkCache.frame_epoch` 和各 `PackedRetainedQuadChunk.last_used_epoch` 替代每次完整编码复制所有 `RetainedChunkId/GlobalElementId` 的临时活跃 HashSet。只有实际完成打包/复用的 Chunk 写入本轮 epoch；包括递归 Element Blur 的 CachedView；根层完成后对唯一缓存按 epoch 原地 retain。u64 epoch 溢出时清空旧缓存，避免错误命中。此变更不产生第二份持久 Paint 状态，不能推导出 GPU VRAM 降低。
- 修改范围：`frame_upload/frame.rs`、`frame_upload/encode/retained.rs`、`frame_upload/capacity.rs`、`platform/nova/tests.rs`、本 TODO。同步删除 HashSet scratch 的统计、Trim、测试假设，增加过期 Chunk 释放时仍由外部 `Arc` 持有旧字节的测试。
- 当前仅静态检查，**未运行 cargo fmt/check/test，也无四后端实机验证**。下一步必须优先本地编译上轮及本轮 GPU Owner 迁移，然后推进真正 CachedView → Paint Chunk 唯一记录，清除 SceneSlot/Replay/pack_scene；不要仅增加旁路缓存。

## 2026-10-15：大型 Scene 的 CachedView 结构隔离校验去重复前缀扫描

- 起点 `1e7ddc3261688a6a3a704556d1645fe85f0fa13c`，代码变更 `crates/gpui/src/scene/display_list.rs`；保持本文件为唯一进度记录。
- **代码实现（未编译）**：`Scene::range_is_independently_replayable` 原本每次验证都从全场景 `paint_operations[0..range.start]` 逐项计算 Blur/Layer 深度，再扫描目标范围；多个 CachedView、Selective Splice、普通 Retained Element 重复判断同一已完成 Scene 时，可能产生重复 O(N) 前缀扫描。对已完成且 PaintOperation >=256 的 Scene，第一次查询惰性建立仅含 Blur/Layer Begin/End 的稀疏结构边界索引，后续以 `partition_point` 求边界前缀状态，复杂度按结构屏障数为 O(log B) 每次查询。静态 Quad/文本操作不在索引中，不构建新的绘制树或双份权威内容；小场景及正在构建/回放的 Scene 继续按原代码扫描。
- **保守正确性**：空区间、越界、区间起点处于未闭合 Blur/Layer、前缀出现不匹配关闭、区间内部不匹配关闭，均保留原语义。索引由唯一 `paint_operations` 派生，`Scene::clear`、`finish_with_previous` 取消旧计算结果，未完成 Scene `revision=0` 不读取旧索引。新增正常嵌套与错误结构下逐区间对照旧扫描单测。稀疏索引仅属于过渡期 Scene 的加速元数据，长期目标仍是单一持久 Paint Chunk/依赖索引；不将 Scene 标记完成。
- **验证**：已静态检查算法和调用关系；无 Rust/Cargo、Windows 原生 GPU，未执行 rustfmt/check/test，不声明 Prepaint p95 改善。须重点测多个嵌套 CachedView 的命中率/Prepaint p50/p95/p99、内存增量和普通小页面冷启动。后续仍须移除 SceneSlot/Replay 与重复 FrameUpload staging，并测试 Blur、Dropdown、DX11/DX12/Vulkan/GL。

- **2026-10-15 新增测试静态类型修正**：上条结构索引提交 `c4c9346f` 的测试错误假设 `PaintOperation: Clone`；实际 `PaintOperation` 未实现 Clone，已改为按需构造普通 Quad 操作的闭包，不修改生产逻辑。仍未执行 Rust 编译/单测；本修正不等于验证通过。

## 2026-10-15：FrameUpload 编码接口迁移后的测试作用域回归修复

- 起点 HEAD `953db6be081650e38aaaded60e44d4511b7f6d88`。对上轮迁移后的全部 `platform/nova/tests.rs` 和 `platform/nova/frame_upload/encode.rs` 单测函数做静态变量作用域审查，确认另外 6 处 `&mut chunk_cache` 调用缺少对应声明（分别 4 处、2 处），现已在各自测试作用域创建 `RetainedQuadChunkCache::default()`，不修改生产函数、GPU ABI 或资源生命周期。
- 静态逐测试函数检索中已无未声明 `chunk_cache`；此结论不等同 Rust 编译/测试通过。仍须本地运行 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib retained_quad` 及普通 GPUI 单测，禁止用 CI 替代。

## 2026-10-15：补齐 FrameUpload 子模块剩余调用点

- 起点 HEAD `118d1cadd0a68f163d2f2177f5514be07d7b9ef1`。通过 GitHub 代码搜索审计 `upload.encode(`，发现上轮 GPU Owner 缓存迁移尚遗漏 `platform/nova/renderer/retained_upload/sharing.rs`、`platform/nova/tests/sprite_gpu.rs`、`platform/nova/tests/sprite_gpu/relocation.rs` 三处测试/硬件验证子模块，仍按旧版 `FrameUpload::encode(scene,...)` 调用。现均改为独立 `RetainedQuadChunkCache` 可变借用并保持原场景数据和 GPU 资源流程不变。
- 这属于**编译接口完整性修复**，不是新增功能，尚无 Rust 编译或四后端实机验证。迁移是否真正通过以本地 `cargo check` 与 `cargo test` 为准，不能以静态审查替代。

## 2026-10-15：CachedView Scene Replay 候选范围二分，移除每片段全量扫描

- 基线 master `faa7f4725cb7ef5c0c895757874d7fc2181542ef`；修改 `crates/gpui/src/scene/display_list.rs` 与本唯一进度文档。保留原始 `Scene::paint_operations` 和 `retained_chunk_candidates`，**不创建第二份 RenderTree/Chunk ID 索引**。
- **代码实现，未 Rust 编译**：`Scene::finish_with_previous` 原地按候选 `scene_range.start` 排序（相同起点优先较大的结束位置），令完成帧候选区间可二分。`Scene::replay` 对已完成上一帧通过两个 `partition_point` 收窄起点落在重放片段内的候选，再保留既有 `scene_range.end <= range.end` 的完整包含证明。这样多个独立兄弟 CachedView 局部重放不再为每个片段线性遍历全部候选；复杂度由每片段 O(C) 降至 O(log C + K)，K 为该片段起点内候选（包括与片段终点重叠但需要过滤的候选）。多层嵌套父范围、断开的 Replay 片段不被错误继承。
- **正确性边界**：构建中的 Scene（`revision == 0`）候选可能仍是 append 顺序，继续使用原全扫描；`record_retained_chunk` 写入新的候选时将 `revision` 标回 0，不允许在再次 `finish` 之前消费过时的有序证明。候选排序在 `prepare_retained_quad_chunks` 前完成；Prepared Chunk 的保守 painter-order/独占验证、GPU Arc/Fence、离屏滤镜不变。
- **测试源码**：16 个兄弟 Cache Chunk、跨越两次 Replay 的父 Chunk、排序与跨片段保留、未完成 Scene 回退以及后置候选更新后的 revision 失效；另用独立随机区间脚本对照原始逐项扫描，36,000 组结果一致。**没有执行 Rust 编译、rustfmt、单测及 DX11/DX12/Vulkan/OpenGL 实机测试**，不宣称 Prepaint 或 GPU 占用已实测下降。
- **后续删除清单**：`SceneSlot`、`Scene::replay`、`pack_scene`、全量 `FrameUpload` staging、全类型持久 Paint Chunk 尚未删除。本改动优化现存 Replay 热路径，而非以额外长期层阻碍后续破坏性迁移。下轮先运行 Cargo 验证，并从 CachedView 的纯 Quad/Shadow 可持久段扩展向单一 GPU resident 变更集。

## 2026-10-15：移除仅局部平衡的弱 Blur Replay 校验

- 基线 master `f252280cdb26b9f426f6a29765620ae7dcf6db8e`；删除 `crates/gpui/src/scene/display_list.rs` 的 `range_has_balanced_element_blurs`，同步清理 `crates/gpui/src/scene/tests.rs` 中仅测试该弱接口的断言。源码搜索确认无生产消费者。
- 该旧方法只检查候选片段内部 `StartBlur/EndBlur` 配对，不能识别片段起点位于外层 `StartBlur` 或 `StartLayer` 内的情况，返回 true 不代表可单独 Replay。生产链路原已使用更严格的 `range_is_independently_replayable`；现在消除可能被未来调用者误用的重复入口，保留嵌套捕获的拒绝回归测试。无新兼容层。
- **状态**：代码实现及静态依赖检查，未 Rust 编译/测试、未四后端原生验证；不以删除弱接口声称完整 Scene Replay 已迁移。

## 2026-10-15：修复 CachedView Debug 重放退化，验证 GPU Cache 实际长度

- 起点 master `72c97104f8f8a5a8f164be82487583521b9cad96`。修改 `crates/gpui/src/scene/display_list.rs`、`platform/nova/frame_upload/encode/retained.rs`、`platform/nova/tests.rs` 与本文。
- **已代码实现、尚未编译**：上轮 Replay 候选二分逻辑在 Debug 模式下仍在每一次重放时执行 `debug_assert!(candidates.windows(2).all(...))`，导致每个 CachedView 片段隐含 O(C) 全表扫描，抵消所声称的热路径优化。将这个排序不变量检查移至 `Scene::finish_with_previous` 完成排序后，只有每帧一次 Debug 校验；生产逻辑继续使用上一帧的有序候选二分，没有新增结构、缓存或场景快照。
- **GPU resident 防错边界**：同一 `RetainedChunkId` 语义身份不代表可忽略 Packed Quad 数量。缓存复用时额外校验 `quad_count == 本次 chunk.quad_range.len()` 且 `bytes.len() == quad_count * PACKED_QUAD_BYTES`；不一致回退当前帧重新编码，避免旧片段用错误实例数覆盖新布局。新增在保留相同 ID/generation 但真实 Quad 数从 32 增至 64 的异常回放对照测试，要求完全重编码并更新唯一缓存；不会伪造 chunk hit。
- **验证与后续**：尚无本机 Cargo/rustc、Windows 原生 GPU，未运行 `cargo fmt --all`、GPUI 单测及四后端验证；Debug O(C) 消除为静态代码事实，真实 Prepaint p95/GPU 占用未测。下一步继续检查 Scene Replay → Packed Chunk 身份、FrameUpload 全量实例流与独立 GPU Compositor Lane，待编译验证通过后逐步移除 SceneSlot/Replay，而不是继续增设兼容层。

## 2026-10-15：Quad 可见性边界按实际 Packed Batch 统一计算

- 起点 HEAD `e9fcc8aa4ebfd0fb074a6093526245681865a0fe`；修改 `crates/gpui/src/platform/nova/frame_upload/encode.rs` 和唯一 TODO。目标是减少 Scene→FrameUpload 重复 CPU 遍历，不引入新 Scene/RenderTree/额外 FrameUpload。
- **实现**：原 `encode_scene` 对每一个 Quad Prepared Run 先调用 `static_quad_run_visual_bounds` 遍历原始 Quad，再执行 packed encoding；仅当任一动画令预判失效时，才再次调用 `static_packed_quad_batch_bounds` 按真实上传 Batch 重扫。改为所有 Quad Run 编码完成后统一按已打包的 `UploadedBatch::{Quads,SolidQuads}` 及有效可见 Quad（跳过零面积 clip）计算一次每批可见边界。源 Run 中有 GPU-indexed 动画的 Batch 仍返回无边界并禁止错误裁剪，未含动画的兄弟 Batch 保留独立裁剪，嵌套 Element Blur 子 Batch 的 metadata 保持原有增量处理；Sprite/Underline 仍用各自先验计算，不调整 Shader ABI。
- **价值和风险**：静态场景不再对每个 Packed Chunk 套用整段 Run 的宽泛联合边界，提高按批次进行 Scissor/DirtyRegion 裁剪的机会；混合动画场景不再进行一次必然失败的整段 Quad bounds 前置遍历。仍会在编码后遍历有效 Quad 一次生成原有 metadata，不能宣称避免整个 FrameUpload、GPU copy 或已测量 GPU 占用下降。
- **测试源码（未运行）**：添加 32 Quad 保留块 + 1 Quad 空隙 + 32 Quad 保留块的真实 encode 对照，验证 3 个 Packed Batch 的独立非重叠边界；现有静态、动画、退化 clip、安全回退测试保持。当前环境无 `cargo`/`rustc`/`rustfmt` 和 Windows GPU，尚未编译、测试、截图或验证 DX11/DX12/Vulkan/OpenGL，仍需以本地 `cargo fmt`、`cargo check`、`cargo test` 和真实 Profiler 验收。
- **未删除**：`SceneSlot`、`Scene::replay`、`NovaRenderer::pack_scene`、全场景 FrameUpload staging；下一步分析 Quad batch bounds 是否应直接由 Scene 已验证的 retained Paint Chunk 维护，以便进一步删除同一场景遍历，而非新增身份缓存。

## 2026-10-15：Monochrome 字形边界裁剪收敛为单次遍历

- 起点 master `b7cd8eaa40920b32d4af974065769e7d817b0160`；修改 `crates/gpui/src/platform/nova/frame_upload/encode.rs` 与本 TODO。完成上轮 Quad Packed Batch 边界统一后的第二项直接热路径收敛，未创建新 RenderTree、状态缓存或 Upload Stage。
- **代码实现，尚未编译**：原 MonochromeSprite 预先 `sprites.iter().all` 检查是否有非单位 TransformationMatrix，再 `sprites.iter().map` 逐字形聚合裁剪边界；普通文本页面每一个无变换 glyph batch 都会重复扫描一遍。现在把 `has_unbounded_transform` 作为已有 `static_raster_run_visual_bounds` 的第四个输入，与动画安全检查合并，同一遍历完成矩阵证明和可见 bounds union。Quad、Polychrome、Underline 使用 `false`，保留原判定；对真实变换过的 Mono 字形继续返回 None，拒绝不安全的包围盒裁剪。
- **验证**：新增通用静态 sprite bounds 的非单位变换拒绝用例；现有动画 rejection、Quad Packed Batch 测试继续覆盖。静态检查全部 `static_raster_run_visual_bounds` 调用点并核对调用参数数量，未运行 `cargo fmt/check/test`、未在 Windows DX11/DX12/Vulkan/OpenGL 测试、未量化高频 Dropdown 的 CPU/GPU 效果。下一步应测试文本密集页面的 Encode p95，并检查预期的保守裁剪与实际 Render Graph Scissor 是否一致。
- **未完成**：彻底取消 Scene Replay、SceneSlot、FrameUpload 全量 staging、持久 Paint Chunk 类型统一和 Presentation Lane，不能因为局部减少一次字形遍历便宣布阶段结束。

## 2026-10-15：Quad 可见边界与 Packed Chunk 编码合并（移除生产二次扫描）

- 起点 master `bb2f705aa6d9d6519f32f2b2ea97d246ab9b3a9c`。纵向改动 `crates/gpui/src/platform/nova/frame_upload/encode.rs`、`frame_upload/encode/retained.rs`、`frame_upload/frame.rs`、`platform/nova/tests.rs` 及本文件；没有添加 Scene、RenderTree、RenderCommit、额外 FrameUpload 或第二个 Quad Payload Owner。
- **生产实现（未编译）**：原 `encode_scene` 即使 Retained Chunk packed bytes 直接从 GPU Owner 的 `Arc<Vec<u8>>` 复用，也调用 `static_packed_quad_batch_bounds` 重新遍历此帧全部源 Quads，导致复用场景仍付出大段 CPU bounds 扫描。现在 `encode_quad_range` 在写入可见 Quad 的同一循环累计当前 Batch 的保守可见边界；静态/动画分组切换时一并提交对应 `batch_visual_bounds`。GPU Owner 的**同一条** `PackedRetainedQuadChunk` 缓存记录增加 `visual_bounds`，在首次/脏内容打包时顺路计算，与 packed bytes、QuadCount、hash 同步失效；重放命中直接提交此前已证明的边界，不扫描旧 Quad 数据。动画、非有限几何返回 None 禁止错误裁剪；零面积 clip 继续被忽略，Count/ABI 未变。
- **嵌套所有权/批次顺序**：`encode_scene` 不再调用原生产二次扫描（旧算法仅供 `#[cfg(test)]` 测试对照）。在递归 Element Blur 的 `BeginBlur` 与子 Quad 编码之间先将 `batch_visual_bounds` 补齐到当前全局 `batches.len()`；同时覆盖同一 Prepared Batch 连续多个 sibling Blur 的 End/Composite 占位，避免边界与后续 GPU DrawStep 批次错位。其他 Sprite、Underline、Backdrop 和 RenderExtension 继续保留保守判断及原有 Fence/Target 协议。
- **测试源码（未运行）**：保留两个 32-Quad Chunk 与中间 gap 的独立边界用例；新增 12+1 动画+12 Quad 的分组边界测试；Nova 测试核对 cached/replayed Quad 的边界相同、单个与两个 sibling Element Blur 的 Begin/Quad/End/Composite 全局 metadata 下标对齐。新增字段由测试手动构造的 `PackedRetainedQuadChunk` 同步初始化。
- **验证限制**：当前环境无 Cargo/rustc/rustfmt 或 Windows GPU。仅已审计调用点和代码依赖；未运行 `cargo fmt/check/test`、DX11/DX12/Vulkan/GL native draw/resize，也未证明 Prepaint p95/CPU/GPU/VRAM 下降。须先本地编译并优先跑 `retained_quad`、`batch_visual_bounds`、`element_blur` 对照后再进一步破坏性改动。仅多出每条 CPU Chunk 缓存记录一个保守 Bounds 元数据，不修改 GPU Shader ABI。
- **剩余**：SceneSlot、Scene Replay、统一全类型 Paint Chunk、全部 FrameUpload Staging、独立 GPU Compositor Lane 未完成。下一步让 DirtyRegion/DrawStep 使用统一的 Chunk 变更代次并进一步减少不必要的 per-frame DrawStep/Upload 重建，不得引入平行 Scene/RenderTree 中转。

## 2026-10-15：Quad 打包与动画分段决策合并到单次遍历

- 起点 master `d584eb4f525e8e4fbf8445131004fb7724cf5d62`。修改 `crates/gpui/src/platform/nova/frame_upload/encode/retained.rs` 与本唯一 TODO；不创建并行场景、DrawStep、批次缓存或新增 GPU Owner。此前已实现 Retained Chunk Packed Bytes/Bounds 共同所有权，继续减少未保留 Quad Gap 的 CPU 工作。
- **【代码实现，未编译】**：删除编码前的 `should_split_static_dynamic_quads(quads)` 全量预扫描。现在 `encode_quad_range` 打包有效 Quad 时顺带统计动画状态切换、最长静态连续段与保守边界；最多在前 8 次切换处暂时追加批次。编码结束后，仅当动画切换 >0 且 <=8、最长静态段 >=8 时保留拆分；否则将本次局部新增批次和边界**原地**截断为一个连续 GPU Batch，无额外 Vec 或重复源 Quad 扫描。超过阈值即停止继续产生候选 Batch，防止动画高频交替造成大量临时提交。
- **【正确性收敛】**：`append_quad_batch` 只记录 `UploadedBatch` 与索引对齐的 `batch_visual_bounds`，`encode_quad_range` 按实际发出的 Quad 总数更新 `summary.quad_count` **一次**，合并候选后不会二次计数；`encode_retained_quad_chunk` 同样直接按编码实例数更新 summary。缓存命中原路径不变，仍使用唯一 GPU Owner 的 `PackedRetainedQuadChunk`。合并静态/动画混合 Batch 设 Bounds=None，避免危险的局部裁剪；不可见零面积 Quad 不计入切换与 GPU 实例。
- **【测试源码，未运行】**：原有 10+1+10 和高频交替两个 Batch 测试保持，增加短静态段自动合并与计数断言；新增 400 轮包含静态、动画、零面积裁剪的随机 Quad Pattern，对照旧阈值语义、Batch 数量、first/count、bounds 是否可裁剪以及 `summary.quad_count`。本次还使用独立脚本对 100,000 条随机 trace 比对旧/新拆分判定一致性；这不是 Rust 编译或运行测试。
- **【待验证】**：当前环境无 cargo/rustc/rustfmt，不能执行 `cargo fmt --all -- --check`、`cargo check`、`cargo test -p gpui --lib` 与四后端实机测试；应重点关注极高动画频率时批次合并、超出 MAX_QUADS 的截断帧、Canvas 自绘数据，及不匹配 Scene/Batch Bounds 的异常。只有本机 profiler 才能确认 CPU Encode p95 与 GPU DrawCall 效果。
- **【仍未完成】**：SceneSlot、Scene Replay、根 Scene/Filter 拆分、全量 FrameUpload Staging、跨类型 Paint Chunk、独立 GPU Presentation/Compositor Lane。下一纵向切片应减少 Scene→FrameUpload 的其他源数据重复复制并验证 GPU resident 修改代次，而不是再添加新的并行表示层。

## 2026-10-15：Quad Resident 上传脏区间内联规划，删除常规逐帧 Vec 分配

- 基线远程 master `28473c9519d9f1afb6371ebf1ddc08967c77369d`。修改 `crates/gpui/src/platform/nova/renderer/chunk_upload.rs`、`renderer/retained_upload.rs`、`renderer/retained_upload/sharing.rs` 和本唯一 TODO。遵守 GPU Owner 中 `QuadResidentLayout`/跨帧 slot residency 为单一依据，不引入第二套上传状态或额外 Scene/Chunk 所有者。
- **【代码实现，未编译】**：`QuadUploadPlan::Ranges(Vec<Range<usize>>)` 改为 `Ranges(SmallVec<[Range<usize>; 4]>)`。多 Chunk 局部修改中常见的 1～4 段脏字节区间可留在计划本身的内联存储，不再为此单独分配堆 Vec；超过四段仍自动 spill，完整保留顺序、半开区间、字节数和原生 API 接收的散射写入语义。
- 完整上传、空计划、合法性验证、原旧 Slot Range、Chunk ID/generation/hash、资源替换后强制 Full、物理 GPU Buffer 和 `BufferUploadBatch` 都不变。`dirty_complement` 的唯一区间列表仍按全局字节顺序产生，不会与动画局部更新重新排序；不假设 Backend 具备跨 Buffer 的无序执行能力。
- **【测试源码，未运行】**：将所有已知 `QuadUploadPlan::Ranges(vec![...])` 测试改为 `smallvec!`；新增三个脏区间全部内联、七段碎片自动转堆且正确计算 72 字节上传量、完整保持物理顺序的回归测试。注意本变更在最坏大量碎片情况下依然会分配堆内存，并非消除全部 BufferUploadBatch 阶段 CPU 开销。
- **【验证】**：仅对 GitHub 源码全局调用点进行了静态审查，**无 Rust Cargo/ rustc / Windows native GPU**，未运行 fmt/check/test/四后端。真实堆分配下降、DX11/12/Vulkan/GL 上传性能和 p95 延迟仍须本地 Profile 测量；不要以此推断 GPU/VRAM 降低。
- 后续重点：验证 Resident Upload Plan 与 DrawStep/DirtyRegion 的版本一致性，清理冗余 `SceneSlot`、`Scene::replay`、全量 `FrameUpload`、并完成独立 GPU Compositor/Present lane（尚未完成）。

## 2026-10-15：共享 Arc 的 GPU Quad Resident Layout 单遍脏区间验证

- 起点 `34a2091b5de771208bf5ee316265086ad75c0243`；变更 `crates/gpui/src/platform/nova/renderer/chunk_upload.rs` 与唯一 TODO。未引入第二套 GPU Chunk/Scene/FrameUpload 所有者。
- **【代码实现；未 Rust 编译】**：`QuadResidentLayout::from_upload_reusing` 已在完全相同的 Chunk ID/generation/hash/位置序列时直接共享 `Arc<[RetainedResidentSpan]>`；原 `upload_plan` 对这类相同对象仍各遍历旧布局、新布局并执行逐项归并。新增 `Arc::ptr_eq` 精确快路：利用现有 `dirty_complement` 在**单次遍历中同时验证范围顺序、是否越界、是否为空**以及生成脏字节间隙，不再扫两份等价元数据。Arc 指针不相同则保留原严格比较逻辑，不使用指针相等作为跳过范围合法性校验的理由。
- **【消除空工作】**：若当前被确认共享的保留 Span 覆盖全部 GPU Quad 字节，虽然静态签名可能由于动画拓扑发生变化，但本轮物理 Quad Bytes 没有缺口，`dirty_complement` 返回 `QuadUploadPlan::None` 而非 `Ranges(SmallVec::new())`；两者原生写入字节均为 0，后者仍要保留无用的区间状态。零 span、异常 Range、被替换 Slot、新旧内容不同仍走 Full 或既有安全局部上传。
- **【测试源码，未执行】**：同 Arc 与深拷贝相等 Span 的上传间隙必须一致；满驻留时返回 None；同 Arc 元数据倒序、重叠、越界、空范围均强制 Full。使用独立 JS 模型生成 60,000 组随机区间（含不合法区间和非同 Arc 情况），新旧真实上传字节区域对照一致；这不是运行 Rust 单测。
- **【风险与限制】**：无 rustc/cargo/rustfmt 和 Windows 原生 GPU，未验证 rustfmt、编译、GPUI lib tests、DX11/DX12/Vulkan/OpenGL。该提交只减少 CPU 上传规划扫描和空计划表达，不改变底层 GPU Buffer、Fence、Upload ABI 或跨 Slot 生命周期；实际帧耗时/GPU 占用未测。下一步继续减少 FrameUpload 静态流重复 hash/上传和复用 DrawStep 的有效性检查，持续删除 SceneSlot/Replay 技术债而非添加旁路缓存。

## 2026-10-15：彻底跳过零写入帧的 GPU Buffer 提交

- 基线远程 master `d2154ca382d73c9a222037f5aef2200e12e44788`；修改 `crates/gpui/src/platform/nova/renderer/buffer_upload.rs` 和本唯一 TODO。纵向范围仅涵盖静态/动画 FrameUpload → Borrowed BufferUploadBatch → Native ResourceDevice；无新增 FrameUpload、RenderTree、GPU 缓存或兼容适配层。
- **【代码实现、未 Rust 编译】**：`upload_frame_buffers` 原先即使全部静态流干净、`QuadUploadPlan::None`、没有 CPU animated primitives 或 animated backdrop-blur pass，仍执行全静态流/Quad/逐类动画的规划遍历。现在通过纯函数 `requires_buffer_upload_planning` 跳过无写入帧的规划步骤，保留原计时入口。
- **【原生 GPU 快路】**：`upload_buffer_batch` 曾在 `BufferUploadBatch::writes().len() == 0` 时仍调用 `ResourceDevice::write_buffer_batch(device, [])`。新版在此情况下直接采用 `gfx_core::BufferUploadStats::default()`，不进入任一后端 Native Driver 写 Buffer 路径，仍提交 `BufferUploadMetrics` 全 0 和现有计时样本。gfx-core 各后端零批次语义已按代码确认是零调用/零上传；非空情况下完全保留原顺序、Barrier/Fence/Slot 与错误返回。
- **【测试源码，未执行】**：增加零写入帧判定、静态 dirty Full、Quad Full/局部 Ranges 均须规划，以及空 Batch 统计契约的单测；代码审查确认 GPU-indexed animation tables 的更新并不经此 Native BufferUpload 路径，保留其独立原有更新流程。没有模拟 BackendResources 或运行测试。
- **【风险与验收】**：当前环境未提供 rustc/cargo/rustfmt，也没有 Windows GPU；尚未执行任何原生后端/单测和性能记录。此改动只会减少无写入帧 CPU 规划与无效 Native API 进入次数，并不能减少实际的 RenderPass/Draw/Present 或证明 GPU 20%+ 占用问题解决。后续应给静止页面、纯 GPU 动画与小 Dropdown 创建独立 profiling 基准，验证每帧写数/绘制数/Present 数，并持续推进 SceneSlot/Replay → 唯一 Paint Chunk 和独立 Compositor Lane。

## 2026-10-15：按唯一 Scene 编码代次失效 DrawStep 与 Path Mask 保留描述符

- 起点 master `11acc281b828b0227be9ac0e7461a5f76f84e0c6`。代码修改 `crates/gpui/src/platform/nova/frame_upload/{frame.rs,encode.rs}`、`renderer/{draw_steps.rs,draw_step_scratch.rs}` 和唯一 TODO；原生 DX11/DX12/VK/GL 的 Buffer ABI、Fence、Target/Present 提交顺序均不改。不新增第二套完整 Scene 或跨帧 Chunk 缓存。
- **【确认的源码风险】**：`NovaRenderer::pack_scene` 的 `UploadKey` 除 Scene revision 还考虑 Atlas placement generation、drawable size、blur quality、输出 Alpha，且动画结束时可强制重编码。之前 `DrawStepCacheKey` 仅在 RendererExtension 非空时带 `dynamic_frame_id`，否则相同 scene revision/size/Atlas Texture generation 可能重用旧 `FrameUpload.batches` 对应的 RenderStep；即使完整重编码后批次拓扑、root filter capture、资源绑定已经变化。PathMask 描述符缓存同样只由 scene revision 与 raster resource set 决定，无法显式区分一次完整重编码。
- **【代码实现，未编译】**：将现有 `FrameUpload.renderer_extension_frame_id`（实际上每次 root `encode_scene(reset=true)` 都递增）统一重命名为 `FrameUpload.encode_generation`，避免给 renderer extension 单独维护错误概念。该唯一代次直接进入 `DrawStepCacheKey` 和 `PathMaskCacheKey`；重新编码即使 producer Scene revision 不变，也失效两个描述符缓存。递归子 Blur 的 `reset=false` 不增加代次；普通 `refresh_retained_animation_values` 只刷新动画值，不会递增代次，继续保留 per-slot DrawStep cache hits。现有 frame slot/size/atlas/alpha/resource-set 判据和主动失效逻辑都保留，避免跨 Slot 缓存指向错误 ResourceSet。
- **【测试源码，未执行】**：新增 same Scene revision + new encode generation 时的主 DrawStep 与 PathMask rebuild，重复相同代次时应命中；FrameUpload 根 `encode` 两次代次 1→2，动画值 refresh 不增加代次。已全局核对旧字段的其他具体调用点仅在上述文件中。该修改属于缓存正确性修复，可能增加过去错误命中情况下必要的 DrawStep 重建，并非承诺所有页面 CPU 下降。
- **【验证门槛】**：环境无 cargo/rustc/rustfmt、没有原生 Windows GPU；未编译、未跑 Rust 单测或四后端。需本地 `cargo fmt --all -- --check`、`cargo check -p gpui`、`cargo test -p gpui --lib draw_step_cache` 和 `path_mask_cache`，通过后用动画完成、Atlas repack、Element Blur Root Capture、resize 场景截图与 GPU trace 验证无陈旧 RenderStep。
- **【未完成】**：该代次是当前 FrameUpload 的**同一个根编码序列**，并非长期统一 Paint Chunk version；SceneSlot、Scene Replay、全场景 FrameUpload Staging、DrawStep/DirtyRegion 跨类型持久增量和独立 GPU Compositor Lane 仍待破坏性重构。下一步基于真实 Present / DrawStep 请求计数确认静态页面的重复提交来源。

## 2026-10-15：原生 RendererExtension 动态 DrawStep 的跨帧正确性

- **起点**：master `8f4f0f0da7d475ca7e160ad788491521f076dc4c`；检查 `crates/gpui/src/platform/nova/renderer/draw_steps.rs`、`renderer/draw_step_scratch.rs`、`platform/nova/draw.rs`、`renderer.rs`。本次代码仅修改 `draw_steps.rs`，同步本 TODO。不增加第二套 Scene、FrameUpload、缓存 Owner 或功能配置开关。
- **【定位的真实风险】**：此前新统一的 `FrameUpload.encode_generation` 仅在完整根 Scene 编码时变更，而 `prepare_renderer_extensions(frame_time)` 每帧重新调用扩展，生成可变的原生 `RenderStepDescriptor`。主 `DrawStepCacheKey` 在同一 revision/encode_generation/size 下仍可能命中旧描述符：GPU Buffer、管线、资源绑定或扩展输出变动均可变成错误绘制。另 `prepare_renderer_extension` 原先只有进入 native prepare 后才 `steps.clear()`；若上一帧可见、本帧 Scissor 不可见而提前返回，则 `renderer_extension_steps` 仍包含上一帧命令。
- **【代码实现，尚未 Rust 编译】**：凡根 FrameUpload 含 `renderer_extensions`，现有 `DrawStepScratch.prepare_steps` 使用 scene revision=0 的**现成非缓存路径**，每帧读取本帧原生扩展 DrawSteps；纯 GPUI 场景则继续以 `encode_generation` + Scene Revision/Frame Slot/Atlas/Alpha/Size 保留并复用 DrawStep。新增泛型 `prepare_renderer_extension_step_storage` 在每帧遍历扩展前一次性 resize/truncate 并 clear 全部步骤 Vec，沿用其 capacity；任一扩展因 bounds/mask/空 scissor 提前返回时已无历史命令；成功 prepare 后原有顺序、Scissor 及 `draw_steps_for_upload_into` 的输出不变。
- **【测试源码未运行】**：新增真实 `DrawStepScratch` 的扩展帧不命中、扩展移除后恢复稳定缓存测试；以及扩展命令在被遮挡前清空、Vec capacity 复用和数量缩减的纯函数测试。当前环境检查仍无 `cargo/rustc/rustfmt`，没有 GPU，四后端未验证；提交只证明源码层面规避旧描述符，并未实测 DrawStep 耗时或 GPU 20% 占用下降。应先在 Windows/Linux 用动态 RendererExtension 与窗口遮挡/移动对照源缓冲和 GPU Trace，核查空命令不造成意外空帧。
- **【继续任务】**：向源代码中对齐 `Render Graph → Present` 的帧请求源与静态场景跳过策略，不能仅因 Buffer Upload/DrawStep Cache 正常就认为 Present 不再被唤醒。统一 Paint Chunk → GPU Compositor Lane、SceneSlot/Scene Replay/FrameUpload 老路径的安全迁移仍未完成。

## 2026-10-15：合并 Element Blur Source 查找入口，移除每层重复线性扫描

- 起点 master `3120dbc3f91f5d3e5a21851f4d30f0ce4536f95f`。源文件 `crates/gpui/src/platform/nova/frame_upload/layers.rs`、`renderer/present.rs`、`renderer/draw_steps.rs` 和本唯一 TODO。GPU Filter Registry、Quad Buffer、DrawStep 顺序、Render Graph/Present API 均无改动；不新增 Source HashMap 或长期第二缓存。
- **【审计确认】**：`FrameUpload.element_blur_inputs` 按 packed GPU Blur Index 严格递增写入（ElementBlur 先分配 index、记录源，再递归 child；AutoStatic 只在编码后追加最新索引），而 `blur_content_ranges_cache` 按 depth 倒序、content_start 正序供滤镜父子依赖绘制。两种排序不能混用，原 Present `dirty_element_blur_indices` 每层 `iter().find`、DrawStep `prepare_element_blur_layers` 每层 `find` 和 `any`，嵌套多层时造成重复 O(L×S) source 扫描。
- **【代码实现，未 Rust 编译】**：`FrameUpload::element_blur_source_for_index(u32)` 作为唯一查找入口，利用**既有**按 packed index 排序的源 Vec 做 `binary_search_by_key`；Present 和 DrawStep 三处查找全部迁移。不复制 Source，不制造新索引或新生命周期。源拓扑在 `refresh_blur_content_ranges`（仅重新编码时执行）增加递增顺序的 `debug_assert!`，捕获未来编码插入顺序错误；每帧 Presenter 不再遍历 Source 前缀。理想静态过滤器集合查找复杂度由每层 O(S) 降为 O(log S)，整体常见帧成本需真实测量。
- **【测试源码，未运行】**：增加稀疏 packed Blur Index [1,4,7,13,21] 与反向/乱序查询、缺失值/空表的回归测试。该 Lookup 不改变 Deepest-First GPU pass 排序，避免嵌套滤镜父子顺序倒置。
- **【验证及风险】**：没有在容器执行 cargo/rustc/rustfmt、DX11/DX12/Vulkan/OpenGL 本地编译与原生绘制；静态审查仍不足以证明 GPU 占用或静态 Present 问题得到解决。本轮保留原有 Frame Scheduling 空动画 tick 跳过语义，没有引入旁路静态 Present skip（其 WSI、GPU 动画和待提交 Scene 关系尚需实机追踪）。后续聚焦 View/Window Frame Request → GPU Render Graph/Present 的实际 5 秒计数和 DirtyRegion 证据，再考虑独立 Compositor Lane。

## 2026-10-15：追踪静态窗口 Frame Request → Present，分离空动画 Cadence 与显式 WSI

- 起点远程 master `05443561392c97f82ca1e0725fb3d842b127696f`。本阶段修改 `crates/gpui/src/window/frame_lifecycle.rs`、`crates/gpui/src/platform/frame.rs` 与本唯一 TODO；不新增帧状态所有者、不变更原生 Swapchain/DrawStep/Render Graph 语义、不添加不受证明的「静态强制跳 Present」分支。
- **【静态审查】**：`run_presentation_phase` 已用 `PlatformFrameRequest::resolve_animation_presentation(presentation_ready)` 在空动画 Cadence、没有待激活 Scene/未完成 Present 时屏蔽 Swapchain 提交；Required（WSI/外部渲染/显式 Present）不能因为 UI 看似静态而取消。既有 per-window 指标统计 redraw/draw/present，但 `log_frame_work_decision` 在 Trace 中输出常量 `request_id=0`，也未显示已经有的 `DirtyFrameDiagnostics.frame_request_reasons` 与首次请求 caller，使 5 秒高占用追踪无法区分动画、notify 与外部原生请求。
- **【代码实现，未编译】**：新增不分配内存的 Trace 诊断 `gpui presentation gate`：分别输出 pending Scene、未完成 Present、animation_visual、ready、原始/resolve 后的 Presentation Request、UI Commit、已提交 Scene、窗口可见/最小化与实际 early submit。现有 `gpui frame work` Trace 去除始终为 0 的假 request_id，输出已收集的 request reason bitset 和首次 reason/source(file:line)，并保留 dirty / render provenance、draw/present/skip 标志。诊断仅在 `log::Level::Trace` 打开时执行，**不**额外派发平台帧或插入渲染 Thread/Queue，Trace 内状态仍保持原生命周期，不误称来源掩码代表全部 WSI 事件。
- **【测试源码，未运行】**：新增 `PlatformFrameRequest` 三组调度契约测试：无视觉变化的动画 Tick 取消 Present 但保留 UI Tick；与 WSI/Required 合并后的 Present 不可吞掉；有损伤动画保留 Present。行为生产代码未改，避免未验证的帧跳过造成 DX11 拖拽、Vulkan live resize、最小化恢复黑屏回归。
- **【验收与下一步】**：当前容器缺少 cargo/rustc/rustfmt，Windows/Linux 四后端实机、GPU trace 与耗电/吞吐尚未验证。需本地运行 FrameRequest/Window 测试并用 `RUST_LOG=gpui=trace` 对静态 5 秒、Dropdown 展开/收起、Blur on/off、动画结束 5 秒分别收集 `gpui presentation gate`、`gpui frame work` 和 Nova draw_step_cache_hit / upload_bytes / actual_present，对比帧请求源；若发生纯 Presentation 多余提交，再修改**真实发起者**，而非添加第三套节流或缓存。统一 Paint Chunk/Compositor Lane、SceneSlot/Replay 删除仍未完成。

## 2026-10-15：简化四阶段 Nova Render Graph，删除每帧动态拓扑搜索

- 基线 master `328f513332b7fb214f22ce821fdda5ec0cbd5258`；本阶段修改 `crates/gpui/src/platform/nova/renderer/frame_graph.rs` 和本唯一 TODO。未改动 GPU API、RenderPass 实际提交、跨 Slot 纹理缓存、Fence、外部 WSI Request/Present 判据。所有四类 Pass 仍用同一 `FrameGraphPlan` 传给现有 Present 执行入口。
- **【已实现，未 Rust 编译】**：之前 `FrameGraphPlan::compile` 每次创建 `SmallVec<Node>`、节点 `dependencies` bitset、顺序查找可执行节点并生成已固定的拓扑序，还在执行图生成时再次按 Pass 逐项查找 Node。当前硬编码的四节点资源依赖始终单向、顺序固定（PathMask → ElementLayers → BackdropBlur → MainPresent），不是用户插件可扩展 DAG。删除 `Node`、`ResourceUse`、`Pass::bit`、运行时 topo-sort 和所有临时 Node Access Vec；根据本帧确有的 Mask/Layer/Blur 直接产生执行序列，再按该序列生成现有 `BarrierIntent` 和 `ResourceLifetime`。Native Barrier 仍由 Backend 执行，不开启跨帧/跨纹理别名复用。
- **【静态帧快路】**：当本帧三个离屏阶段都不需要刷新时，`FrameGraphPlan::compile(false,0,0)` 直接返回唯一 `MainPresent`、空 Hazard 和 Swapchain ExternallyOwned 的 `first=last=0` 生命周期，不做通用依赖扫描。Render Graph 仍保留 MainPresent 的真实 WSI 提交，**没有**错误取消窗口暴露/恢复/显式 Present。
- **【测试源码，未运行】**：新增 8 种三阶段有效/无效组合全排列的固定顺序、Barrier 数、资源 Count、纹理 first/last/Residency 验证；另验证多个只读消费者不产生 Read→Read Hazard。使用独立 JS 逻辑参照逐个比较旧/新 8 种拓扑的 Pass、Hazard 顺序/访问类型及生命周期数组，结果一致；**不能替代 Rust 测试、native GPU Barrier Test 或本地性能测量**。
- **【后续风险与目标】**：本改动减少即使静态 DrawStep 缓存命中时仍执行的 CPU Graph Planning；并非证明实际静态窗口不会重复 Present，也不能证明 20–30% GPU 占用下降。下一步应拿刚加入的 `gpui presentation gate` / `gpui frame work` 与 Nova Pass/Present 实机 5 秒 Trace 确认帧请求源和 Graph 次数。依然未完成跨类型持久 Paint Chunk、GPU Resident Buffer/DrawStep/DirtyRegion 内容版本统一、SceneSlot/Replay/FrameUpload 淘汰、独立 Compositor Lane；不得仅凭固定 Graph 声称全链路统一。

## 2026-10-15：单遍动画 Stream 共享判断与 GPU Slot 签名差异复用

- **远程基线**：master `cf69dacaba2aff331fa80aa22473580a42463b60`。修改 `crates/gpui/src/platform/nova/resources/buffers/streams.rs`、`renderer/retained_upload/sharing.rs`、`renderer/retained_upload.rs` 和本唯一 TODO。只整合现有 BufferStream/RetainedUpload 的元数据工作；不创建第二套 GPU owner、FrameUpload 或重叠资源缓存。
- **【代码实现，未编译】**：`BufferStream` 使用显式 `#[repr(u8)]` 和 `index()`；10 个成员与 `ALL` 的 canonical 顺序相同，测试校验映射，避免 per-stream `ALL.iter().position()`。对于每帧 `RetainedUpload::update_shareability`，删除原来每个 Stream 重复扫描全部 `animated_primitives` 的逻辑：初始化可共享位，一次遍历全部 CPU-patched Primitive 按原有 AnimatedPrimitiveKind 标记 Quad/Shadow/Mono/Poly/BlurRecords 不可共享；`BlurPasses` 独立继续按 `has_animated_backdrop_blurs()` 判定，未改 GPU Indexed Animation promotion。
- **【Slot 生命周期正确性】**：原 `mark_uploaded(slot)` 对每个 10 流调用 `stream_state`，重新取 Slot 签名、复制签名并匹配。现在从同一权威 `StaticUploadSignature::diff(previous)` **一次**计算各流 dirty 位并与此前 `uploaded_shareable` provenance 合并。保留「CPU Patch 后即使之后不再动画、签名相同，也不能在没有静态基础字节重新上传时错误共享」的语义；新版本真正上传或 Buffer 重建后恢复共享。每个 Slot 独立，FrameSlot/COW Buffer/Fence/Barrier 不变。
- **【回归验证】**：新增顺序映射、全部 10 种 Stream 的 CPU Patch → 静态签名相同保持非共享 → invalidate/re-upload 恢复共享，以及 PolySprites 单流改变不污染 Quad 的测试源码。另独立 JavaScript 模型比较 100,000 组随机旧/新 Slot 签名与共享状态、2,048 组动画 Primitive 类型序列，结果完全一致。**这不是 Rust 编译/测试**。
- **【未验证/下一步】**：当前容器仍无 `cargo`、`rustc`、`rustfmt`，没有 Windows/Linux 原生 GPU。需本地编译、运行 `gpui` 单测及四后端回归，确认 Scene revision 切换、CPU Patch 尾帧、Slot 轮换、Buffer Replace、live resize 均不发生错误共享。性能收益仅限减少元数据查询与 CPU 扫描；真正重复的 Frame Request → DrawStep → Native Present 和 Paint Chunk/SceneSlot 重构尚未解决。后续应针对重复的 `AnimatedPrimitive` 编码与 `FrameUpload` 全场景 staging 开始纵向迁移，不能把简单循环减少误认为统一 Compositor 已完成。

## 2026-10-15：补齐 Retained Quad 局部上传时 CPU 动画字节的覆盖证明

- **基线**：远程 master `c749d1c0726dbb6158ebd7c67cc6516f013494c7`。修改 `crates/gpui/src/platform/nova/renderer/buffer_upload.rs` 和本唯一 TODO；直接修复旧 `FrameUpload → QuadUploadPlan → BufferUploadBatch` 同一条规划路径，不引入旁路动画缓存、新 FrameUpload 所有者或兼容层。
- **【已定位正确性风险】**：`FrameUpload::sample_primitive_records` 每帧就地修改 CPU-animated packed Quad 字节，而过去 `plan_animated_buffers` 使用 `if !dirty.quad` 决定是否对 Quad 动画进行 GPU patch。静态上传 `dirty.quad == true` 并**不**必然表示 Full 上传：QuadResidentLayout 可能生成局部 `Ranges`，甚至静态签名只因 animation topology 改动而产生 `None`；因此动画 Quad 可能落在其未覆盖的 Gap，产生陈旧 GPU 数据。
- **【代码实现，未编译】**：新增纯范围算法 `for_each_uncovered_animated_quad_range`：`Full` 表示已覆盖无需动画额外写入，`None` 必须提交全部 CPU 动画字节，`Ranges` 则对每个动画 Quad 只规划未被静态脏区间覆盖的补集。静态范围保持有序/不相交，使用 `partition_point` 定位首个相交区间，避免从第一个 retained span 反复扫描；补传通过唯一 `plan_quad_range` 和 `PackedQuadStream::slices` 完成，不复制数据，不重写 Buffer/Fence 生命周期，不对已经覆盖的字节重复提交。
- **【测试源码未运行】**：新增 Full/None/空范围、动画跨多个 retained islands、部分重叠与完全覆盖测试。独立随机模型比较 120,000 组不同顺序的有效静态范围与动画范围，逐字节判断计算出的补集准确（不是执行 Rust 单测）。
- **【正确性边界】**：当 `QuadUploadPlan` 不合法时上游应降级 Full，本辅助函数仅处理上游已验证的有序计划；其他 Shadow/Mono/Poly/BlurRecords 仍按原有静态 Full 与动画 patch 规则。所有非空 Backend Buffer Upload 仍遵守已有提交顺序，WSI Present 与 Render Graph 未改；本次主要是规避遗漏及重复写入，不承诺 GPU 使用率必降。
- **【未验证与下一步】**：当前环境没有 cargo/rustc/rustfmt，尚无 gpui 单测、DX11/DX12/Vulkan/OpenGL 真实 Buffer/Fence 测试或显存/帧延迟证据。必须本地重点验证“CPU Quad 动画 + Retained Static Chunk + FrameSlot 轮换 + Scene Revision 变化 + GPU COW Resize”组合。之后继续收敛局部 Paint Chunk/DirtyRegion/DrawStep 的权威内容版本，并逐步替代全场景 Scene/FrameUpload staging。

## 2026-10-15：停止完全静态 Present 的 Presentation Clock GPU 写入与空批次 GPU Owner 设备锁

- 基线 master `2b5428b957acb33e185f9dabb6d4aeb3178011fa`。修改 `crates/gpui/src/platform/nova/renderer/submission.rs`、`renderer/buffer_upload.rs` 与本唯一 TODO。不增帧时钟/状态权威所有者，不引入新的 Upload/Scene/FrameGraph Cache，不更改 Native Present 或 FrameSlot/Fence 行为。
- **【已确认的重复工作】**：此前 `upload_presentation_buffers()` 在每个已被请求的 Native Present 帧都对 `global_buffer[16..24]` 提交 8B Presentation Clock，即使静态 Scene 没有采样动画表、CPU 动画或 RendererExtension；随后空 GPU Indexed Table 的上传被跳过，但仍通过 SharedBackend Mutex 进入 Buffer 上传入口，静态画面「0 静态上传」不能代表整帧 0 GPU Buffer 写入。检索内置 Nova WGSL 显示 `globals.presentation_time_seconds`、`presentation_tick_60hz` 目前仅声明于 Core GlobalParams，没有被内置绘制 Shader 消费（自定义 RendererExtension 是独立边界，仍保留 Clock）。
- **【代码实现；未 Rust 编译】**：以现有 `FrameUpload` 动态状态判断是否上传 Clock：`renderer_extensions`、`sampled_animation_values`、`gpu_indexed_animation_slots`、`animated_primitives` 中任一非空都必须保留由 `frame_time` 单一帧时刻采样的 Uniform Clock；仅全部为空的内置静态场景跳过更新 `globals[16..24]` 和 8B Native 时钟写入。保留 `rebuild_indexed_animation_values`、`prepare_static_buffers`、`ensure_stream_capacity` 的顺序与所有 GPU 资源变更逻辑。
- **【原生锁/统计统一】**：`upload_buffer_batch` 与 `submit_presentation_buffers` 使用同一个 `record_empty_buffer_batch` 统计入口；完全空的借用批次在 SharedBackend Mutex **外**立即记录 requested/actual writes、backend calls/bytes 均为 0 与时间后返回。非空路径仍按原 DX11/DX12/Vulkan/OpenGL/Metal 调用、原顺序上传，不跳过 Native WSI Present、不忽略 FrameSlot 资源安全。
- **【测试源码，未执行】**：新增内置静态场景零时钟上传、GPU indexed slot 存在但 timeline 空时仍保留 Clock、CPU 动画 sample 保留 Frame Clock、相同 frame_time 的 Timestamp 确定性测试；原 `BufferUploadBatch` 空写入契约测试保留。需要本地复核存在 RendererExtension 时的 clock、静态曝光/resize、动画末帧、slot 替换四后端结果。
- **【未验证与下阶段】**：容器仍无 cargo/rustc/rustfmt；本次未完成 Rust 编译、单测、GPU Trace、DX11/DX12/Vulkan/OpenGL 实机。减少了静态帧逐帧 GPU Buffer 更新，但静态页面若仍不断 Present，DrawStep/RenderPass/Swapchain 仍消耗 GPU；必须基于 `gpui presentation gate` 对照实际 Native Submit 原因，再改 Frame Request 的真实来源。独立 GPU Compositor Lane、持久 Paint Chunk 与 SceneSlot/Replay/FrameUpload Staging 破坏性替换仍未完成。

## 2026-10-15：Root Color 物理身份验证与内容版本的保守独立保留（P0 阶段切片）

- **基线**：远程 master `a585dc4a143d1e3f829d58144e72c6c6e3ab44e3`。修改 `crates/gpui/src/platform/nova/{targets.rs,tests.rs}`、`renderer.rs`、`renderer/{filters.rs,draw_steps.rs,present.rs}` 与唯一 TODO。此前 Root Color 由 `retained_root_blur: Option<u32>` 指向零半径 BackdropBlurVariantTargets，`ensure_backdrop_blur_targets` 即使物理 Target 在 Capture reorder/数量减少中复用成功仍一律 `invalidate_backdrop_blur_cache` + 全部 Filter 捕获重新刷新，这与根窗口静态颜色驻留目标相悖。
- **【实现，未编译】**：`BackdropBlurTargets::retained_root_texture_view` 读取**实际 GPU TextureViewId**，而不是以扁平 Blur index 当身份；`can_preserve_root_color_capture` 只允许无共享 backdrop source、无 isolated source、旧 Capture 均零半径的目标链。GPU FilterRegistry 独立维护 `root_color_version: Option<TextureContentVersion>`，仅在**成功 Present 且 Root source pass 确实刷新**后递增；跳过 Root Pass 保留版本，Root layer 消失或原生资源失效则清除。该内容版本不再依赖 Element Source 的索引快照，也不因为其他 Capture 重新编码自动增加。
- **【复用正确性】**：`ensure_backdrop_blur_targets` 在 `reuse_layout` 和 `rebind_zero_radius_layout` 两条现有 GPU 目标重排路径中，只有**原 Target 已提交过有效内容**、此前 Filter 状态有效、旧新 Capture 均无 Gaussian、没有 Root BackdropSource、且重排前后 Root TextureViewId 完全一致时，才调用 `invalidate_element_sources_preserving_root_color`：保留根颜色 GPU 内容版本，失效所有 Element 捕获快照及它们的前帧动画样本。新 Capture/重映射 Capture 在 `prepare_element_blur_layers` 没有已验证 source content version 时，强制**完整 Capture 初始化**，不得用 Popup 的小 DirtyRect 填充未初始化纹理。其余路径继续原有全量 Filter 失效，目标重建继续在旧 GPU Fence 完成后销毁旧资源，绝不在资源代次不匹配时复用旧纹理内容。
- **【测试源码，未运行】**：Root 版本初始为空、实际 refresh 后建立、未 refresh 时不变、其他 Element source 失效不影响 Root、Root 消失/全失效重置；已在 `warmed_zero_radius_popup_targets_rebind_without_reallocating_textures` 增加多种弹窗数量和根索引轮换前后 TextureViewId 完全一致的断言；补充新/重映射 Capture 必须 full 初始化的测试。
- **【本阶段严格限制】**：此提交仅把 **Root Color 失效决策与内容 epoch** 从全局 Filter snapshot 分离，尚**未**将 Root Color 的原生 Texture/ResourceSet 从 `BackdropBlurTargets.variants` 独立为物理 `RootColorTarget`，packed 合成命令也仍使用零半径 Blur ABI，故 P0「真正资源所有权拆分」仍为 **未完成**。本阶段不会假称 Capture 已有稳定 `layer_id + GPU generation`，也没有实现完整静态背景与动态前景独立合成；当前无 Rust/Cargo/Windows GPU，Rust 单测、四后端提交/Fence、GPU 使用率收益均未验证。
- **【下一步 P0 资源拆分】**：把根颜色 `Texture+View+per-slot ResourceSet+content version+size/format generation` 全部放到唯一 RootColorTarget owner，真正移除 Root 对 FilterVariant 的物理依赖；以 Scene `PaintBlur.layer_id` 作为明确稳定 Capture ID（无 ID 必须保守处理），`CaptureId → Target/Generation` 单一索引替代当前 `BackdropBlurConfig.member_first/last` 作为资源身份，重排时保留稳定 Target，淘汰者由提交 Fence 完成后退休；建立 popup 插入、删除、重排、缩放动画、resize 和 in-flight 多帧测试。完成这一步后再复核一层局部 GPU Composite：背景像素与子 Capture 的 Dirty 必须分离，不允许 popup 的透明度/平移/缩放样本重放 Root static Scene。
- **【P1 GPU Timestamp 评估】**：仓库已有 Vulkan `vk::QueryType::TIMESTAMP` 及 DX12 `D3D12_QUERY_HEAP_TYPE_TIMESTAMP` 用于局部 GPU 上传计时，但 `gfx-core` 尚无跨 DX11/DX12/Vulkan/OpenGL 的通用 RenderPass 时间戳契约；不能把 CPU `Instant::elapsed` 记作 GPU Pass 耗时。后续需统一 `GPU_PASS_ROOT_REFRESH`、`ELEMENT_SOURCE`、`GAUSSIAN_H`、`GAUSSIAN_V`、`FINAL_COMPOSITE`、`SWAPCHAIN_PRESENT` 标签与每 FrameSlot QueryRange；在提交栅栏完成后延迟无阻塞 Resolve/Readback，DX11 disjoint、DX12 frequency、VK validBits/timestampPeriod、GL availability 单独校准，不支持时标记 unavailable，防止 UI 线程/共享 GPU Owner 同步等待。这项 P1 仍**未实现**。

## 2026-10-15：公共 Dropdown 单 Capture Reveal（待 GPU Trace 验收）

- 远程起点 `f24fe7cd88b01a817ed2e48f3480ca05d8307aa8`，修改 `src/ui/components/dropdown.rs` 与本唯一进度文档；仅影响公共 Dropdown 弹层，不修改首页独立版本下拉框、不改变 GPUI/Nova-GFX Shader ABI。
- **【源码证据】**：公共弹层原先嵌套 `composite_layer_with_id(active.id)`、Opacity 动画、ClipReveal 动画。`ClipReveal` 在 `paint_scene_animation` 必定经 `with_scene_composite_animation → paint_composite_layer` 捕获整个菜单，内部 Opacity 对显式 Composite Layer 再做一次独立零滤镜捕获；两级源目标可能扩大 Render Graph 和纹理写入。`.occlude()` 仅调用 `occlude_mouse()`，是命中测试而非 GPU 像素遮挡或 DirtyRegion 操作。
- **【代码实现；未编译】**：公共菜单只保留同一个 `active.id` 标识的 GPU ClipReveal 子树捕获，删除重复显式 Composite 和独立 Opacity Track；不引入另一套缓存、动画时间循环或条件性全页 Render。固定边缘、原有 220/160 ms 曲线、反向中断与菜单内容/滚动/输入事件不变。**视觉取舍**：移除了菜单展开/收起时 0.78↔1.0 的轻微整体渐隐，保留 Reveal；不是无损像素等价变更。首页独立下拉框不受影响。
- **【实例隔离审查】**：`DropdownState` 使用 `window.use_keyed_state(id)`，每个触发器持有独立选项/滚动/phase；全局 `DropdownOverlayState.active_by_window` 当前每个窗口最多一个活动菜单，其他实例的内容不会自动进入此菜单。切换 phase 仍会通知拥有该 Dropdown 的父 View，全窗口 Root/Paint 及 Root Color GPU Target 是否在动画每帧重绘仍需由现有 Trace 验证，不能仅凭源代码宣称已彻底局部合成。
- **【验收阻塞】**：当前环境无 cargo/rustc/rustfmt/Windows 原生 GPU，未执行编译、单测及 DX11/DX12/Vulkan/OpenGL 实机。必须对比相同窗口/刷新率、Blur on/off、单/多个 Dropdown 连续快速点击：`gpui frame work`、`gpui presentation gate`、Capture 数量/Pass 次数、Root target version、retained_copy_pixels、GPU Draw/Present/实际占用与动画完整性。期望单个公共菜单 Capture 从嵌套目标收敛至一个，**GPU 40%→多少尚未知**；RootColorTarget 物理所有权分离、Fence 退休和异步 GPU Timestamp 仍未完成。

## 2026-10-15：稳定 Element Capture 跨索引保留真实纹理版本（未编译）

- 起点 master `3e1324b6084a2e45530d06f206c23129b3a7031b`；改动 `frame_upload/blur.rs`、`targets.rs`、`renderer/filters.rs`、`renderer.rs`，并更新本唯一 TODO。公共 Dropdown 仍使用上一提交的单 ClipReveal Capture，首页独立版本下拉框不变。
- **【已定位】**：`ensure_backdrop_blur_targets` 在 `reuse_layout` / `rebind_zero_radius_layout` 复用 GPU TextureView 成功时，原先仍清除全部 `FilterRegistry.element_blur_inputs`，可能导致与弹层无关的保留图层在后续 Present 被重新离屏刷新。
- **【实现；未验证】**：仅当 Root Color 的原生 TextureView 和已成功提交的内容版本保留时，从现有 GPU Target 提取临时 `(packed index, TextureViewId)` 列表；在唯一 `FilterRegistry` 中按非空稳定 `PaintBlur.layer_id`、前后完全相同的 ViewId、零滤镜/单成员独立目标三重验证后迁移 Snapshot 的 flattened index。其他目标释放旧快照，依旧走强制初始化；不增加持久缓存、额外 RenderTree/Scene/兼容层，不改变目标分配、Fence 等待、Shader 和 Native Present。
- **【边界】**：新内容仍经过现有 `FilterRegistry::source_unchanged` 的 Source/BBox/Mask/Animation 验证；物理 View 变更、无稳定身份、重复旧身份、正半径 Gaussian、合并目标不复用旧像素。旧 Root 失效继续全失效；未实现 RootColorTarget 物理资源独立化。
- **【测试源码未执行】**：添加 FilterRegistry 层身份变化、纹理变化、扁平索引重排、Gaussian 与源 bounds 变化测试。环境没有 Rust 工具链，也没有四后端原生 GPU：需 `cargo fmt --all -- --check` / `cargo check --workspace --no-default-features`，以及 DX11/DX12/VK/GL 单/多个 Dropdown、弹层重排、尺寸变化、切页的实际像素和 Trace 校验。GPU 40% 的变化未知；下一步聚焦 Root 源 DirtyRegion 与独立 Property Composite 的实际重复 Pass。

## 跨对话恢复协议（必须执行）

1. 读取本文件和 `docs/AI.md`，再读取**最新远程 master SHA、Tree SHA**。检查最近 Git 历史是否已改动相关模块。
2. 阅读本文件中最后完成项、未完成项和验收证据。逐条检查实际代码，**不能仅依据 TODO 勾选状态**。
3. 选取可以独立验证的一条纵向切片，先定位真实依赖和正确性约束，避免新增更多冗余生命周期状态。
4. 修改后至少静态检查；能运行 Rust 时优先 `cargo fmt --all -- --check`、针对性测试、必要的 Windows 4 后端截图/GPU 测试。**CI 不得作为本地测试替代品**。
5. **每次提交同步更新本文件**：变更路径、commit SHA 或历史指针、已验证事实、未验证风险、下一步起点。避免一边提交代码，一边让状态过期。
6. 提交前重新读取 master HEAD，比较基线，使用 blob → tree → commit → compare → update_ref(expected_sha) **非强制快进**；HEAD 发生变化则停止并重新对照，禁止覆盖他人提交。
7. 提交消息采用**中文 Conventional Commits + [skip ci]**，例：`perf(gpui): 复用滤镜目标并减少纹理布局重建 [skip ci]`；不要把未通过的测试写成“通过”。
8. 更新 README 或引用入口应始终指向**本文件的固定路径**，禁止创建第二套互相冲突的重构进度文件。
9. 破坏性修改时同步更新**删除清单中的符号搜索结果**与真实消费者；若存在多个等价的 Frame/Scene/Chunk 权威所有者，视为技术债未清，不能标记阶段完成。
10. 此文档的勾选状态必须区分【代码实现】【编译通过】【单测通过】【DX11/DX12/VK/GL 原生验证】；不可因为静态审计与提交创建就视为已验证。

## 下一次对话的直接起点

- **最新待验证起点（2026-10-15）**：本轮将 Nova Retained Quad 持久 packed 缓存唯一所有权迁到 `RetainedUpload.quad_chunks`，同时递归收集嵌套滤镜内 CachedView Chunk 存活；优先本地编译及 retained-quad 单测，修复任何类型/可见性/接口不一致后再扩大 P0 架构替换。
- 查看 `docs/GPUI_UNIFIED_COMPOSITOR_TODO.md`，从 **P0-A 的单一目标复用入口的编译/测试与 Windows 实机重建日志** 和 **P0-B 的同步 WM_SIZE Prepaint 时长** 开始。
- 确认下一步先处理 Root Color Cache 与 Blur Capture 的身份耦合，还是 Frame Scheduler 的统一调度；结论必须用同一份 GPUI 压力测试证明。
- 严禁直接执行“删除 Scene”而不建立 GPU Owner 保留内容、资源退休和有序绘制的正确性方案。
- 下一阶段主要交付物：为 **Scene → FrameUpload → DrawSteps → Present** 建立每一步拥有的全部状态与复制字节账本，确定哪些结构能合并/删除；然后以 GPUI 独立 Dropdown/Blur 实验完成单一合成通路的首次纵向切片。
- **最近更新（2026-10-15）**：在 `331ae0a4` 基线上，Nova Retained Quad Batch 选择由逐批全量扫描改为基于 Scene 既有排序的区间二分；Scene Quad 完整性前置检查跳过部分不必要的 paint-order 全表证明。新增对照测试，尚无 Rust/Windows 运行验证；没有删除 Scene，未证明 GPU 占用、Prepaint 或 Resize 已修复。
