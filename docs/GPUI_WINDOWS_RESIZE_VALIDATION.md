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
