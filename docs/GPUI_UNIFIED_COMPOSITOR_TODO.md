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

- 查看 `docs/GPUI_UNIFIED_COMPOSITOR_TODO.md`，从 **P0-A 的单一目标复用入口的编译/测试与 Windows 实机重建日志** 和 **P0-B 的同步 WM_SIZE Prepaint 时长** 开始。
- 确认下一步先处理 Root Color Cache 与 Blur Capture 的身份耦合，还是 Frame Scheduler 的统一调度；结论必须用同一份 GPUI 压力测试证明。
- 严禁直接执行“删除 Scene”而不建立 GPU Owner 保留内容、资源退休和有序绘制的正确性方案。
- 下一阶段主要交付物：为 **Scene → FrameUpload → DrawSteps → Present** 建立每一步拥有的全部状态与复制字节账本，确定哪些结构能合并/删除；然后以 GPUI 独立 Dropdown/Blur 实验完成单一合成通路的首次纵向切片。
- **最近更新（2026-10-15）**：在 `331ae0a4` 基线上，Nova Retained Quad Batch 选择由逐批全量扫描改为基于 Scene 既有排序的区间二分；Scene Quad 完整性前置检查跳过部分不必要的 paint-order 全表证明。新增对照测试，尚无 Rust/Windows 运行验证；没有删除 Scene，未证明 GPU 占用、Prepaint 或 Resize 已修复。
