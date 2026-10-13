# Windows 交互式窗口缩放：三阶段尺寸一致性验证

目标：验证最新一次真实窗口客户区尺寸最终被 GPUI 布局与 Nova-GFX Swapchain 消费，而不是用旧帧拉伸或连续全窗口重绘掩盖问题。

## 三阶段日志

默认关闭诊断，启动进程前设置 `BMCBL_GPUI_RESIZE_TRACE=1` 才会产生日志：

| stage | 来源 | 字段 | 判定 |
| --- | --- | --- | --- |
| `native` | HWND `GetClientRect` + winit WM_SIZE | `window_id`, `physical_px`, `event_px`, `logical_px`, `scale` | 真实 Client 区，区别于可能过时的事件载荷 |
| `viewport` | `Window::content_bounds_changed` | `window_id`, `logical_px`, `physical_px`, `scale` | GPUI 收到新的 Layout 约束 |
| `gpu_queued` | GPU RenderOwner 收到 Resize 命令 | `window_id`, `owner_id`, `physical_px` | 排队完成，不代表底层 Swapchain 已经 Resize |
| `gpu` | GPU RenderOwner 执行命令后读取 Nova committed extent | `window_id`, `owner_id`, `physical_px`, `target_px` | **实际已经提交的 Swapchain 尺寸**，与 pending request 分离 |
| `scene` | GPUI 完成一次全视口重绘后的 CPU Scene | `window_id`, `physical_px`, `scene_primitives`, `quads`, `quad_right_px`, `quad_bottom_px` | 识别旧尺寸 Retained 子树；quad 覆盖范围仅是启发式诊断，不等于最终合成像素覆盖 |

所有阶段按 GPUI `window_id` 关联，便于多窗口分别对比。未绑定首帧 WindowId 的 GPU 预热日志可能带 `window_id=0`，检查器会忽略。

这不是要求缩放中每一帧瞬间完全相等：异步提交和 GPU Fence 背压允许短期不同步。**验收条件是松开鼠标、GPU 恢复 Ready、等待约 2 秒后，最新三个实际尺寸完全收敛。**

## Windows 实机复现

1. 关闭旧 BMCBL 进程；在 PowerShell 中运行 `$env:BMCBL_GPUI_RESIZE_TRACE = "1"`，从**同一个环境**启动你刚编译的 BMCBL Windows 程序。保持默认日志级别包含 `info`。
2. 先单窗口测试：缓慢拉大/缩小，再连续快速拉大/缩小；操作结束等待 2 秒。观察内容能否覆盖整个客户区。
3. 同时打开主窗口、地图查看器与一个独立子窗口：只缩放地图窗口，再分别缩放两个窗口。验证静止窗口既不丢画面，也没有频繁不必要的 GPU Present。
4. 把窗口移动到不同 DPI 比例显示器（如 100% → 125% → 150%），交替缩放；重做最大化/恢复、最小化/恢复、DWM 拖拽过程中短暂 GPU 负载升高等情况。若环境只有一块显示器，需标记跨 DPI 为**未测试**。
5. 等待最后一个 Resize 完成，关闭程序，找到本次 BMCBL 的 `latest.log`。从仓库根目录执行：

```powershell
py -3 scripts/verify_gpui_resize.py --self-test
py -3 scripts/verify_gpui_resize.py "C:\path\to\BMCBL\latest.log" --min-windows 2
```

日志检查器只读取文件，不修改项目或 GPU。正常输出每个窗口的 `OK`；不一致会返回非零退出码，并指出事件滞后、Viewport 未更新、或 GPU committed extent 仍是旧尺寸。只有一个窗口被缩放时，改用 `--min-windows 1`。

## 性能与正确性门槛

- 静止窗口：不应因另一个窗口的移动、无效点击而重复进行 GPUI Layout 或无意义的 Present。
- 只移动、不改变 DPI/客户区尺寸：不应产生新 Viewport Layout。
- Resize 中：`native` 可领先于 `viewport`、`gpu`；旧帧拉伸仅作过渡，不能替代新的 GPUI Scene。
- 松开鼠标后的稳定状态：`native.physical_px ≈ viewport.physical_px`（单轴容许 1px 的 DPI 舍入误差），且 `native.physical_px = gpu.physical_px`。
- GPU 暂时繁忙：不得导致事件永久丢失；新尺寸必须最终收敛。记录 GPU Owner 的 `queue_wait_us`、`owner_blocking_wait_us` 和 p95/p99，结合截图判断卡顿发生位置。
- Vulkan / DX11 / DX12 / OpenGL 必须分别验证；不应把单一后端的通过推断为其他后端也安全。

**限制**：纯 Rust GPUI TestPlatform 可以验证回调、Viewport 更新和窗口间隔离，但无法生成真实 Win32 `WM_ENTERSIZEMOVE` 模态循环、DWM 合成行为或硬件 Swapchain Fence。本仓库提交的脚本只验证日志最后已稳定尺寸，不能替代实际 Windows 视频、GPU 占用和 DX12/Vulkan 同步压力测试。日志若在尚未收敛时截断也会合理地报告失败。

## 2026-10-12 DX11 实际日志定位

用户提供的 DX11 日志中主窗口 `4294967297` 的最终 `native`、`viewport`、`gpu` 都达到 **1873×900**，缩放期间共出现 75 次 Viewport 更新且相应 GPU Resize 最终均被执行。窗口仍持续存在黑色未渲染区域。因此已排除“仅因 WM_SIZE 丢失，或最终 Swapchain 一直是旧尺寸”这两种解释。

新的 P0 在 `Window::content_bounds_changed`：**窗口可用布局范围变化，必须使该窗口的根 CachedView 布局边界失效**，即使根 View 没有显式调用 `window.viewport_size()`（例如 `RootView` 只返回 `div().size_full()`）。仍保留子 View 的独立 retained 重用，且仅在真实尺寸变化时执行，不对无效点击和纯移动执行。新增 `stage=scene`，便于进一步判断 Scene 原语是否随新尺寸重建。

此记录只证明三阶段 Resize 的调用链和已提交尺寸最终收敛，不证明新提交一定修好实际 Windows 黑色空白；仍需相同场景实机回归。此日志另有 **130.9ms 的 GPUI 帧构建**，其中约 **129.5ms 是 prepaint**，属于独立的 UI 性能问题，不应混同为 GPU Fence 阻塞。


## 2026-10-13 后端差异与修复分层

- **DX11 黑色新增客户区**：不透明 `CreateSwapChainForHwnd` 原来采用 `DXGI_SCALING_NONE`。按 Microsoft DXGI 文档，后台缓冲区小于 HWND 目标时，多出的客户区会填充背景色。改成 `DXGI_SCALING_STRETCH`，在 WM_ENTERSIZEMOVE 拖拽中以旧帧临时铺满，真正的 ResizeBuffers/Present 完成后恢复 1:1。
- **交互式 Resize 的最终提交**：WM_EXITSIZEMOVE 结束后主动读取最新客户区，并安排一次 GPUI UI commit，防止只有过渡帧拉伸没有最终重绘。
- **DX12/OpenGL 卡顿、Vulkan 裁剪异常**：不要以 DX11 的 DXGI 缩放策略替代其他后端的同步与裁剪修复。新增后端 `slow_surface_resize` 日志，拆分资源同步等待、Swapchain 重建、其余大小依赖资源创建耗时；仅 Resize 超过 16ms 才发警告。
- **成功呈现检查**：启用 `BMCBL_GPUI_RESIZE_TRACE=1` 后新增 `stage=present`，记录真正提交成功后的 WindowId、实际 Drawable extent、Scene revision 和 dirty_full。旧的 `stage=gpu` 只证明 ResizeBuffers 生效，不等于内容 Present 成功。

新的观察重点：连续缩放后检查最后一个 `native`、`viewport`、`gpu`、`present` 尺寸。Vulkan 若仍有裁剪，收集 `dirty_full`、滤镜/Retained GPU Layer 日志与 Render Graph Scissor 范围，不应先禁用局部绘制或全局缓存。


## 2026-10-13 第二阶段：高频 Resize 与 Vulkan 裁剪正确性

**首次新尺寸帧的 GPU DirtyRegion**：Swapchain 刚刚完成 `resize` 后，`swapchain_warmup_frames` 保持非零直到真正成功 Present。此前 `FullRedraw` 只控制原生 Present 策略，不一定清除仍沿用旧尺寸的局部 `DirtyRegion`/Retained Layer Scissor。现改为在两条绘制入口（`draw_frame`、`present_framebuffer_only`）将 warmup 帧的 GPU DirtyRegion 升级为新 Drawable 完整覆盖范围，并强制刷新对应滤镜捕获。**只有首个成功新尺寸帧执行一次全覆盖**；后续静态/动画帧保留独立层缓存与局部裁剪，GPU 被延迟时不会提前清除 warmup。

**资源复用**：Path Mask 未激活时通常是 1×1 占位纹理。DX11/DX12/OpenGL/Vulkan 等后端在窗口放大缩小时不再反复创建/销毁这个大小不变的占位纹理、TextureView 和 ResourceSet；实际已提升为全尺寸 Path Mask 的情形继续重建。此更改不会缓存一套新的备用大纹理，也不改变 Shader/Pipeline 启动预热逻辑。

**跨窗口长尾阻塞**：新增 `gpui_gpu_stall: gpu_owner_submission_wait`（等待超过 8ms）记录后端、等待时间及 GPU 提交数；结合此前 `slow_surface_resize` 的 `submission_wait_us`、`swapchain_us`、`resources_us`，可区分驱动 Fence 阻塞与纹理/资源重建。GPU Worker 仍共享且 Vulkan 的 `try_resize` 保持原有保守同步边界：该轮**没有宣称消除所有跨窗口 GPU 头部阻塞**。

建议同一台 Windows 实机每个后端运行 5 轮：主窗口缩放（50 次边缘拖动）、地图窗口动画并行、快速最大化/还原、多窗口交替缩放、最终固定尺寸 2 秒。重点核对第一次 `stage=present` 的 Drawable 尺寸和是否 `dirty_full=true`，以及 `gpu_owner_submission_wait` 和 `slow_surface_resize` 是否集中在单个后端。若 Vulkan 继续裁剪，接下来定位 Render Graph Scissor 与 Path Mask/Retained root target，而不是立即取消全局的 Retained Layer 机制。

## 2026-10-13 DX11 边缘拖拽真实帧推进修复

- **Win32 Modal UI Wake：** 原先 `RunMainThreadTasks` 外层 winit 事件在模态拖拽期间无法被处理，`main_thread_wakeup_pending=true` 会阻止后续前台任务投递 `WM_MODAL_UI_TASKS`。现把 HWND 唤醒与外层 winit 事件去重分离。模态消息继续去重，不向无法消费事件的 winit 外层队列堆积无效唤醒。
- **DWM 模态消息保底：** `WM_ENTERSIZEMOVE` 内 DWM 正常调度仍优先；另外仅在该拖拽期间使用 33ms 低频保底检查，DWM 不可用时沿用 Win32 最小周期。只有存在待处理 GPUI/尺寸工作时才进一步分发，不对静止窗口持续重绘。`WM_MODAL_UI_TASKS` 消费前台 Runnable 后立即检查最新窗口尺寸/帧请求。
- **DX11 自动复位事件：** `FrameLatencyWait::cancel()` 已安全停止旧原生回调但保留已领取的 Ready 信号。此前 `ResizeBuffers` 后再次 `reset()` 可能在首个新尺寸 Present 之前丢弃自动复位的 Ready credit，形成等待首帧与需要首帧触发下一个信号的循环。现只在成功 Present 后 `consume_ready()`，更换 Swapchain 时仍使用新的 pacing 实例。
- **测试：** 补充外层 winit pending 时原生 Modal HWND 唤醒、多次 Runnable 不积压外层事件、模态保底周期，以及模拟自动复位 Ready 信号在 Resize 前后保留的回归测试。

验收时连续拖拽 DX11 窗口边缘，不仅检查窗口显示旧画面 Stretch，更应检查 Resize 过程中 `stage=scene` 和 `stage=present` 连续出现，并在停止后最终与 Native/Viewport/GPU 目标尺寸收敛。仍必须在实际 Windows GPU 上编译和复测后才能确认修复成功。


## 2026-10-13：交互式 Resize 性能链路与 GPU 队列合并

本阶段优化的场景是**鼠标持续拖动且客户区实际变化时**，不是仅测 `WM_EXITSIZEMOVE` 后的静态结果。

- Win32 Native：`GetClientRect` 更新最新物理大小；只在真实尺寸变化时进入 GPUI Viewport 更新，纯移动不触发布局。
- GPUI CPU：`content_bounds_changed` 的根布局约束改变会触发 View / Taffy / Prepaint / Paint。**这部分仍同步执行，不能因为 GPU 线程独立就假设 240Hz 已实现。**
- GPU Owner：一段尚未执行的 `StretchResize/Resize/Draw/Tick` 如果已经累积了两次以上 Resize，新的完整 Draw 会收敛到最新 `Resize + Draw`（支持的后端保留与最终尺寸匹配的 stretch），并合并前序未提交 damage。不同步、已开始执行的操作不会被取消。
- 安全边界：不跨越资源控制 `Call`、透明度修改、Shutdown、同步首帧以及其他非 resize 操作；保留旧 scene 的累积 dirty/backdrop damage。
- 后端策略：Vulkan/OpenGL 不再向 GPU Owner 提交无意义的 D3D 旧帧 stretch 命令；DX11/DX12 按后端能力保留临时 stretch。
- GPU Render：实际 drawable 改变后仍要求完整正确的 damage，并重建必需的 swapchain/depth/滤镜目标。**本轮没有实现异步资源分配或解除 Vulkan 的保守 Fence 等待。**

建议分别采集 `WM_SIZE/native`、`viewport`、`scene`、`gpu_queued`、`gpu`、`present` 的时间线；结合 Debug 窗口中的 `surface_resize_wait/swapchain/resources/total` 与 `gpui_gpu_stall` 排查。不要把 GPU Owner 接受了队列命令误计为真正 Present 帧率。需要从成功 Present 的时间戳计算帧间隔、p95/p99 和实际 FPS，同时记录输入事件到新尺寸 Present 的延迟。

在 240Hz 显示器上，每刷新周期约 4.17ms；如果 UI Prepaint 本身超过这个预算，就不能期待每次 Resize 都产出完全重排的新 Scene 并达到 240 FPS。之前记录的 130.9ms 帧构建（其中 129.5ms Prepaint）是需要单独用火焰图定位的 CPU 长尾，不能靠优化 Swapchain 处理。

注意：`BMCBL_GPUI_RESIZE_TRACE=1` 会逐阶段写大量日志，**适合定位正确性和因果顺序，不适合作为无扰动 FPS 基准**。测量真实 FPS/延迟时，应使用低开销统计或外部图形帧分析，并对比开启与关闭该变量的结果。

## 2026-10-13：展开动画的 CachedView 失效和 DX11 ResizeBuffers 细分

本轮排查来源：DX11 / AMD Radeon 780M 的 Debug 日志（其余后端不在这份日志中），一次 `slow_surface_resize`：`total_us=115442`、`swapchain_us=114845`、`submission_wait_us=0`、`resources_us=596`。这只能定位到 DX11 后端 Swapchain API 链路，尚不能归因到 `ResizeBuffers` 本体。另有 GPUI 帧构建约 54.16ms、Prepaint 约 50.56ms，属于独立 CPU 帧长尾。

DX11 现在仅在一次具体 resize 超过 16ms 时附加 `slow_dx11_resize_phases`，字段为 `pacing_cancel_us`、`context_flush_us`、`dxgi_resize_buffers_us`、`backbuffer_view_us`、`dcomp_reset_us` 和 `total_us`。先确认 115ms 是 DXGI 阻塞还是 Flush/准备资源，再决定是否改变帧延迟队列或内存退休策略。其余 DX12、Vulkan、OpenGL 继续使用已有 `slow_surface_resize` 的 wait/swapchain/resources 切分；**没有对应实机日志就不宣称它们的驱动瓶颈与 DX11 相同**。

针对嵌套 CachedView：布局动画默认使用 `RelayoutSubtree`，禁用旧几何约束上的 selective prepaint splice 并使受影响的目标及子元素重新计算。确定为绝对定位、不改变父级流布局的导航选中胶囊和模态透明度/缩放，则使用 `with_local_layout_animation_target` 保留局部 reconcile。一个目标在同一帧收到不同 scope 时取最强值，防止动画展开或快速反向时丢失祖先布局失效。

P1 响应式迟滞采用 Schmitt trigger 而非全局防抖：主导航标签和导入窗口的双栏、紧凑标题栏、文件路径显示，只在跨越带宽后切换状态，持续 Resize 的实际客户区尺寸与 Scene 计算不人为延迟。

尚未把所有纯视觉展开组件迁到 GPU Presentation Lane。首页加载脉冲等已有 renderer-owned opacity animation；需要先为带中断/关闭回调的模态、带动态文字的布局展开建立等价的 timeline 生命周期和 Retained Layer，再去掉 CPU 每帧驱动，不能简单替换成 opacity 动画导致文本、裁剪和关闭回调错序。

验收：在 Release 下验证嵌套 CachedView 展开/收起/中途反向/Resize 同时进行，检查布局、文字、背景、Hit-test 同步；每个后端分别进行 5 轮拖拽，收集 Present 成功率、帧间隔 P95/P99、GPU owner queue、resize 子阶段长尾与 GPUI Prepaint；此环境没有 Windows/Rust 工具链，修改本身不构成编译或实机通过。

## 2026-10-13：Windows 帧请求双时钟诊断与修复

最新 DX11 Debug 日志的新证据不是 GPU stall：5 秒窗口（约 5.1–10.2s）中 `frame_requests=1979 draws=1837 presents=1461`，但活动窗口累计 `native_vsync.wake_count` 只从 66 增至 73。该时段 `present_interval_p50_micros=1624`，明显短于显示器报告的约 4166µs 刷新周期。这证明存在大量 **DWM VSync 以外的帧消费**，但不能由此证明显示器扫描出上千帧。日志其他 5 秒窗口仍存在长尾 Prepaint 19/28.9ms，属于独立 CPU 问题。

根因入口：`WindowsApplication::window_event(RedrawRequested)`、`WM_MODAL_UI_TASKS` 和 `WM_TIMER` 都可以进入 `WindowsWindow::dispatch_pending_update()`。旧代码把所有普通 redraw / modal-task 唤醒都视为可提交帧，因此 Win32 的 WM_PAINT 和嵌套尺寸变化消息循环成为第二个不受刷新率约束的帧时钟，与 `VSyncScheduler` 竞争取走同一 `pending_frame_request`。

新约束参考 Flutter 的 scheduleFrame/handleBeginFrame、Chromium 的 BeginImplFrame、Qt Quick 的 threaded scenegraph：
- `RedrawRequested`、modal 前台任务和 WM_TIMER **可处理事件和查询窗口尺寸，但不直接跨过健康的 DWM 节拍消费绘制请求**；
- 所有待绘制需求继续进入每窗口 latest-wins mailbox，并用 `VSyncScheduler::request_frame()` 安排下一次显示节拍；
- 如果该窗口超过 33ms 没有收到可用于更新的原生 VSync，才允许 unpaced watchdog，且每窗口兜底最多每 33ms 一次；DWM 不可用时沿用原先的 winit fallback；
- 初始隐藏 HWND 的首帧仍走 `request_first_presentable_frame()` 独立握手，不受 WM_PAINT 门限限制；
- 自带固定边界的 Chrome Nav 标签宽度过渡现在使用局部布局 invalidation，而非继续强制祖先 cached 几何重排；流布局真正变化的其他组件继续使用 `RelayoutSubtree`。

与对照框架的共性是 **一个统一帧节拍、去重的 request、在呈现时采样动画、保留 compositor 自主呈现**。尚未测量此补丁的 Release 帧率；Debug 日志也不能证明实际扫描率。本次差异需要至少在 DX11/DX12/Vulkan/OpenGL + 240Hz 拖拽/不拖拽及导航展开/收起时回归。

验收重点：`request_redraw_count` 对实际 native wake 的异常倍率应显著下降；native VSync 唤醒间隔与 4.17ms 周期更接近；`present_interval_p50` 不应在普通活动场景持续小于实际刷新周期；注意累计 metrics 和 5 秒 delta 混用会导致统计误判。若仍出现 19–29ms Prepaint 长尾，应继续拆分 CachedView selective splice 和元素绘制，但不能重新开放 WM_PAINT 高频绕过。
