# GPUI 统一构帧与合成链路：持续执行 TODO

> 状态：进行中。此文件是独立维护 GPUI/Nova-GFX 渲染重构的唯一进度入口；不是 BMCBL 业务 UI TODO。
> 更新日期：2026-10-14（UTC）。初始依据：master `405a7133eec7e093ad93681ed725de6a69a89cfb`、GPUI/Nova 源码及 2026-10-14 DX11 本地调试日志。
> **所有后续对话/代理在编辑前必须重新读取 master HEAD、本文和相关源码，不得使用本文件的历史 HEAD 直接写入。**

## 不可更改的工程目标

1. **统一 GPUI 的 UI 构帧、增量绘制、GPU 合成、Surface Present 语义**，降低端到端延迟、内存复制和显存/纹理反复分配。框架必须独立于 BMCBL，BMCBL 只是一个真实负载。
2. **不强制构建第二棵完整 Render Tree**，不叠加 RenderTree → Scene → RenderCommit → RenderPipeline 等重复中间表示。以现有 GPUI Element/布局身份为基础，使用最少的持久绘制记录与 GPU 状态；若确有独立的 Property/Filter DAG，要有可验证的依赖必要性。
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

## 阶段与状态（勾选代表代码/文档已写，不代表四后端实机验证）

### P0-A 共享 GPU Filter / Color Target 生命周期

- [x] 审计 Zero-radius Capture、Root Retained Color、目标布局复用路径和最新日志。
- [x] 增加**稳定前缀布局**复用：在现有 GPU 纹理规格/目标身份均不变且只改变活动 Capture 数时，保留已有分配，不再走完整目标链重建；覆盖 Gaussian Source 情况。变更后保守刷新 Filter Cache 和 DrawStep。
- [x] 为上述稳定前缀路径增加无实际 GPU 依赖的单元测试，确认 TextureView 身份保留及不兼容时拒绝复用。
- [x] 为无法复用而需要 Recreate 的情况补充拓扑、Root Source、Variant 计数和 in-flight 数量诊断。
- [ ] Windows 真机确认新增路径命中率、纹理重建次数与 GPU/CPU 耗时（DX11/DX12/Vulkan/OpenGL）。
- [ ] 分离 Root Color Residency 与 Gaussian/Backdrop Filter Target；取消零半径 Blur 伪合成的结构性耦合。
- [ ] 过滤布局以稳定 Capture ID/Generation 表示，避免扁平索引扰动带来的全链路失效；处理跨帧 Fence 与资源身份。
- [ ] GPU 纹理预算、空闲 Target 复用与高频弹窗的压力/泄漏测试。

### P0-B 帧请求、UI 计算与 Windows Resize

- [ ] 详细计数与标注每次 Frame Request 的 UI/Paint/Composite/Surface 原因，消除同一时钟重复唤醒。
- [ ] 剖析 Prepaint 长尾：Taffy Bounds 缓存失效、Text Layout、CachedView Selective Splice、Overlay/Dropdown 的具体触发者。
- [ ] 将 Window Resize Native Message 快路径与可能耗时的 Layout/Prepaint 解耦；确认 Win32 modal event loop 仍可及时调度 UI。
- [ ] 建立 Native/UI/Surface/Presented 代次与最终像素覆盖诊断；区分窗口 Move 与 Resize。
- [ ] 复现并定位 DX12 子窗口关闭异常、Vulkan Resize 拉伸、DX11 不出帧和 OpenGL 交互卡顿。

### P1 统一帧内容表示与合成提交

- [ ] 明确目前 Frame → SceneSlot → PresentationPacket → pack_scene → FrameUpload → DrawSteps 的职责重复点及每帧字节数。
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

- [ ] 清理旧 SceneSlot / Frame.scene / PresentationPacket.scene / pack_scene / 旧统计（在新主链路可用后一次性删除，**不保留双路径**）。
- [ ] 独立 GPUI 性能实验：Dropdown 高频展开、Blur 切换、静态页面、Text/Image、动画、窗口 Resize/多窗口。
- [ ] BMCBL 实机回归，尤其页面切换、弹窗、地图、透明/模糊与输入法。
- [ ] 对比 CPU 帧长尾、GPU Present 频率、显存、上传量、Filter Target Recreate、资源回收；不可编造提升百分比。

## 本次改动的边界与验证状态

- **代码层面**：稳定滤镜目标前缀复用与可诊断的 Recreate 路径。这只减少满足条件时的资源链重建，不构成整条合成管线重构。
- **本地编译/测试**：当前对话运行环境未提供 Rust/Cargo，不能执行 `cargo test`、Windows/D3D 实机测试；已对更改进行静态代码/调用审查。单元测试加入源码，**未运行**。
- **不得宣称**：Windows 黑边已修复、四后端 GPU 占用已改善、Scene 已删除、Prepaint 长尾已解决。

## 跨对话恢复协议（必须执行）

1. 读取本文件和 `docs/AI.md`，再读取**最新远程 master SHA、Tree SHA**。检查最近 Git 历史是否已改动相关模块。
2. 阅读本文件中最后完成项、未完成项和验收证据。逐条检查实际代码，**不能仅依据 TODO 勾选状态**。
3. 选取可以独立验证的一条纵向切片，先定位真实依赖和正确性约束，避免新增更多冗余生命周期状态。
4. 修改后至少静态检查；能运行 Rust 时优先 `cargo fmt --all -- --check`、针对性测试、必要的 Windows 4 后端截图/GPU 测试。**CI 不得作为本地测试替代品**。
5. **每次提交同步更新本文件**：变更路径、commit SHA 或历史指针、已验证事实、未验证风险、下一步起点。避免一边提交代码，一边让状态过期。
6. 提交前重新读取 master HEAD，比较基线，使用 blob → tree → commit → compare → update_ref(expected_sha) **非强制快进**；HEAD 发生变化则停止并重新对照，禁止覆盖他人提交。
7. 提交消息采用**中文 Conventional Commits + [skip ci]**，例：`perf(gpui): 复用滤镜目标并减少纹理布局重建 [skip ci]`；不要把未通过的测试写成“通过”。
8. 更新 README 或引用入口应始终指向**本文件的固定路径**，禁止创建第二套互相冲突的重构进度文件。

## 下一次对话的直接起点

- 查看 `docs/GPUI_UNIFIED_COMPOSITOR_TODO.md`，从 **P0-A 的 Windows 实机命中率/重建日志** 和 **P0-B 的同步 WM_SIZE Prepaint 时长** 开始。
- 确认下一步先处理 Root Color Cache 与 Blur Capture 的身份耦合，还是 Frame Scheduler 的统一调度；结论必须用同一份 GPUI 压力测试证明。
- 严禁直接执行“删除 Scene”而不建立 GPU Owner 保留内容、资源退休和有序绘制的正确性方案。
