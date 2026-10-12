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
