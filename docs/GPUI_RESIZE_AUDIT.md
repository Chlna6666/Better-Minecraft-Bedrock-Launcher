# Windows 快速 Resize 端到端审查（2026-10-18）

## 结论与范围

黑边问题尚未解决。本次检查覆盖 Win32 客户区事件、modal loop、UI 调度与 Scene、
RenderOwner 队列、Nova 资源切换，以及 DX11 / DX12 / Vulkan / OpenGL 原生提交。
以下审查记录描述修改前基线；后续代码修复见末节，不把局部节流调整作为修复结论。

当前证据支持两个独立问题：

1. **延迟**：本次 DX11 日志中，Native resize 到匹配尺寸帧 Present 返回的 P95 为
   17ms。UI 准备与 GPU Owner 服务都超过 1ms，不能只优化事件队列。
2. **覆盖契约**：尺寸与 Scene 原子配对，并不等于客户区、有效颜色和 native surface
   原子交接。扩窗先发生，匹配新尺寸内容随后才生成；当前没有独立管理这段时间有效
   颜色覆盖的 Native Active/Pending 状态。

第二项是代码契约缺口，尚未通过逐帧像素/呈现事件确认它与每一次黑边的对应关系。
本次样本 DPI 始终为 1.0，没有证据支持将它归因于 DPI 变化。

## 样本与计时口径

- 日志：`target/debug/BMCBL/logs/latest.log`，2,985,019 bytes，最后写入
  2026-10-18 16:08:15（UTC+08）。该路径会被下一次运行覆盖。
- SHA-256：`B6EB8E70958348F8E42A8613AB991D7B2A71C8F45B0657A398EAF9D2D2E926AF`。
- 后端：DX11，AMD Radeon 780M；本次运行不能代替另外三个后端的实测。
- 按 window ID、epoch 和尺寸匹配 Native、Scene、Present 与 Owner job。
  调试窗采样区间 1.989–3.965s，主窗 5.546–6.770s。
- Present 时间表示 CPU 调用返回；不是 GPU 完成、DWM 接收或屏幕扫描时间。
  Native 日志在事件处理时写入，也不是操作系统改变客户区的精确起点。
  跨日志差值受毫秒时间戳精度限制。不同阶段的分位数不能直接相加。

| 指标（P50 / P95 / Max） | 调试窗 4294967298 | 主窗 4294967297 |
| --- | --- | --- |
| Native / Scene / Owner / Present 数量 | 144 / 144 / 144 / 144 | 89 / 89 / 88 / 88 |
| Native → Scene | 6 / 8 / 10ms | 4 / 5 / 8ms |
| Native → Present 返回 | 13 / 17 / 20ms | 13 / 17 / 19ms |
| Native → Owner job 完成 | 14 / 18 / 21ms | 14 / 18 / 20ms |
| Owner 队列等待 | 20 / 32 / 71µs | 18 / 4086 / 6299µs |
| Owner job 服务 | 7.827 / 9.917 / 13.194ms | 8.202 / 9.759 / 11.980ms |
| draw_present 内准备 | 268 / 358 / 541µs | 246 / 395 / 633µs |
| backend_submit 区间 | 2.493 / 3.512 / 5.716ms | 3.311 / 4.565 / 6.006ms |
| Prepaint 总计 | 2.363 / 3.388 / 5.556ms | 1.311 / 3.072 / 5.002ms |

主窗最后一个 Native/Scene 没有在样本内匹配到完成记录，不能算作成功 Present。
主窗 Owner 记录含 86 次 Draw、2 次 Continue。Prepaint 显式 root layout 的 P95
约 42/43µs，但 child 的嵌套 layout 仍可能计入 root_non_layout，不能据此认定布局
已经只需几十微秒。backend_submit 包含编码/后端执行/Present 返回，不能称为纯 GPU 时间。

一个可核对的顺序是主窗 epoch 11：日志第 4911 行 Native 为 5.647s；第 4923 行
Scene 为 5.651s；第 4963 行 Present 为 5.665s；第 4968 行 Owner 记录
queue_wait=6299µs、job_elapsed=7975µs。Scene 发布后，Owner 仍在完成 epoch 10。
这说明仅清理尚未执行的队列，不能抢占已经进入后端的工作。

## 全链路检查结果

| 环节 | 已确认行为 | 剩余问题/限制 |
| --- | --- | --- |
| Win32 → resize epoch | `DefSubclassProc` 后同步真实客户区；latest-wins 存储尺寸并唤醒 UI | 客户区可以先于新 Scene 增大，事件合并不能消除物理覆盖空档 |
| modal foreground | 拖动窗优先处理，其他窗轮转；前台 drain 有任务数/时间预算 | 预算只在任务之间检查，单个 UI callback 不能被 2ms 预算抢占 |
| UI cadence | live resize 与显示/配置节奏协同，密集事件合并 | 样本几乎一个 Native 对应一个 Scene，没有 Scene 数量放大的证据；无依据放开每事件重建会增加工作 |
| 布局/Prepaint | viewport 与 Scene 尺寸一致；CachedView 对尺寸、clip、上下文等严格验证 | Prepaint 已占毫秒级；需要进一步区分嵌套 layout、文本、replay 验证及实际 paint 准备 |
| 发布与队列 | Geometry 与 Scene 同包；同窗待执行 Draw 合并 damage、保留最新尺寸 | 只能丢弃未开始工作，不能取消已经进入驱动的同步调用 |
| GPU Owner | 多窗口共用 Owner，按命令边界轮转 | 单次 Draw 达 8–12ms 时会影响别的窗口；不能把公平轮转理解成帧内并行 |
| Nova resize | 先处理资源退休、准备附件，再提交新尺寸，要求 Scene 精确匹配 | 新 swapchain 已切换与首个有效颜色 Present 是两个时刻；没有统一 native color 生命周期 |
| 最终提交 | 精确 extent 检查；resize 首帧 full damage；匹配首帧义务不会被新请求无限取消 | 没有发现允许旧尺寸 Scene 绘入新尺寸 backbuffer 的路径；现有测试不覆盖 DWM 在两次 Present 之间显示什么 |

主要入口：
[Windows window](../crates/gpui/src/platform/windows/window.rs)、
[Windows platform](../crates/gpui/src/platform/windows/platform.rs)、
[UI draw](../crates/gpui/src/window/draw.rs)、
[CachedView](../crates/gpui/src/element/cached_view.rs)、
[Owner queue](../crates/gpui/src/platform/render_owner/queue.rs)、
[Owner worker](../crates/gpui/src/platform/render_owner/worker.rs)、
[Nova renderer](../crates/gpui/src/platform/nova/renderer.rs)、
[surface lifecycle](../crates/gpui/src/platform/nova/renderer/surface_lifecycle.rs)。

## 四后端 Native Surface

| 后端 | 代码确认的切换与同步 | 改造约束 |
| --- | --- | --- |
| DX11 | `surface.rs` 原地 ClearState/Flush、释放 RTV、ResizeBuffers、重建 RTV；resize 退休使用本链 post-Present EVENT | 保留资源依赖正确性；本链 fence 已缩窄，并不意味着 ResizeBuffers/驱动不耗时 |
| DX12 | `device/platform/surface.rs` 等本链 post-Present fence，释放 RTV 后 ResizeBuffers，更新 generation 再建 RTV | generation 保护不能代替有效颜色发布；失败/销毁也必须归属明确代次 |
| Vulkan | `device.rs:545` resize 在 extent 改变时调用 `device_wait_idle()`，然后 build/replace/destroy swapchain；reconfigure 也有设备级等待 | 会等待共享设备其他工作；需按资源/提交追踪退休，且不能把 oldSwapchain 句柄保活误认为它还能继续 acquire |
| OpenGL | `surface.rs:207` 新建 FBO/texture、调整 native window、删除旧附件；Present 才执行默认 framebuffer clear、1:1 拷贝/边缘补齐和 swap_buffers | Present 前没有独立补齐新客户区；保留 OwnerThreadOnly/context 约束，不能直接拆成任意 Worker 提交 |

DX11/12 HWND 路径使用 SCALING_NONE，避免整页文字拉伸。DXGI 对 backbuffer 外区域的
行为是使用 swapchain background color；因此该颜色仍须有明确策略，改色只能处理
覆盖表现，不能交付新的布局内容，也不能据此宣称解决延迟。
参见 [DXGI_SCALING 官方契约](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/ne-dxgi1_2-dxgi_scaling)。

DirectComposition 还有更具体的发布顺序风险：DX11 创建链时先 SetContent/Commit，
随后才创建 backbuffer RTV；DX12 retained visual 替换在 build_composition_swapchain
内 SetContent/Commit，外层之后才完成 render target 重建。新链第一次有效呈现前就已
进入 visual tree。应审查“准备完成 → 首帧有效 → 发布”的顺序与失败回滚，不能只保证
Rust registry 的替换原子性。此问题不单独解释不透明 HWND 上的所有黑边。

Vulkan 传入 oldSwapchain 会改变旧链可用性，不能通过两个字段叫 Active/Pending 就
假定旧 native chain 可以持续使用。参见
[VkSwapchainCreateInfoKHR](https://docs.vulkan.org/refpages/latest/refpages/source/VkSwapchainCreateInfoKHR.html)。

后端入口：[DX11](../crates/nova-gfx/gfx-dx11/src/surface.rs)、
[DX12 surface](../crates/nova-gfx/gfx-dx12/src/device/platform/surface.rs)、
[DX12 composition](../crates/nova-gfx/gfx-dx12/src/device.rs)、
[Vulkan](../crates/nova-gfx/gfx-vulkan/src/device.rs)、
[OpenGL](../crates/nova-gfx/gfx-opengl/src/surface.rs)、
[SurfaceDevice 契约](../crates/nova-gfx/gfx-core/src/backend.rs)。

## 其他已发现的性能风险

- **启动 5 秒后清理全进程 working set**：WindowsPlatform 默认启动线程调用
  K32EmptyWorkingSet（`platform.rs:86–99,454`）。这会移除尽可能多的驻留页，重新访问
  可能带来缺页成本，影响稳定延迟；本次没有缺页跟踪，不能量化。调试窗 2–4 秒已存在
  延迟，因此它不可能解释全部症状。
  语义见 [EmptyWorkingSet](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-emptyworkingset)。
- **热路径同步日志**：应用 console 与 latest.log layer 同步运行；ManagedLogWriter
  在共享 Mutex 下写文件。高频 trace 的锁竞争/IO 可能影响 UI 和 Owner 的 wall time，
  本次未单独计量。应做同操作的低日志量对照，不能把关闭日志当作渲染修复。
- **潜在重复验证**：Owner ready 与 draw_frame 都调用 apply_pending_drawable_size；
  Vulkan 无 pending 时可能重复查询 WSI extent。draw_frame 在 submission gate 前
  pack_scene，gate 失败时可能重做；当前 DX11 日志没有相关 deferred marker，不能
  把这些潜在成本认定为本次主要原因。
- **剩余未归因时间**：Owner job 包括 draw_present 计时之前的 resize、资源处理、
  pack 等阶段。现有准备与 backend_submit 计时不能完全解释 job 服务时间，不能简单
  用各自 P95 相减后归因给某个函数。日志中的 gpu_wait=0 也不排除驱动调用内部阻塞。

## 后续实施顺序与验收

1. **补齐同一 epoch 的耗时归因**：在既有诊断中区分 Owner 开始、native resize、附件
   准备、pack、submission gate、编码、Present 返回；结合 PresentMon/ETW 或逐帧捕获
   测量真正展示延迟与新暴露区域。验收是能解释慢帧的主要时间，而非新增大量日志。
2. **改造 BackendSurface 与 GPU Owner 的有效颜色所有权**：独立记录客户区尺寸、
   Scene geometry、native resource generation、有效颜色 extent 和提交/退休序号。
   准备、首次有效呈现、发布、失败取消、销毁都应有明确代次。DComp 延后绑定新链；
   DXGI 原地 ResizeBuffers 与 Vulkan 旧链退休需要各自方案，不能套用相同句柄交换。
   扩大窗口时必须定义新像素的有效背景/覆盖，单纯保留旧的小图不能覆盖新区域；不得
   将旧 Scene 画入新 backbuffer，也不得缩放旧页面。是否采用独立颜色资源应由实测
   与各 native API 可行性决定，避免未经验证的通用状态机先行。
3. **缩短已测慢路径**：分别处理 UI Prepaint、Owner resize/pack/提交；Vulkan 去除
   无关设备等待需以真实引用与 fence/present 退休关系替代。Worker 只承接纯计算；
   DX11/OpenGL 最终提交继续遵守设备/context 约束。多窗口在资源准备期间也须可服务。

每阶段单独验收：四后端 × 不透明 HWND/适用的 DComp × 单窗/多窗 × 连续放大/缩小/
反向拖动；包括后台动画、隐藏恢复、DPI 切换、销毁 pending surface。记录 P50/P95/P99
的 Native→Scene、Owner queue/service、Present/展示延迟、代次丢弃数和资源退休。
像素验收需证明无未初始化黑边、无旧页面缩放、文字保持原生像素；编译通过和离屏
backbuffer 测试不能代替此项。1ms 是后续分阶段预算目标，当前结果尚未接近，也没有
证据承诺任意硬件/窗口系统上的端到端 1ms。

## 审查阶段验证边界

本次执行只读代码/日志检查，核对关键后端顺序与原生 API 文档，生成本报告。
未改生产代码，未重新构建或运行四后端 GUI，不新增“黑边已修复”的结论。
前轮编译和硬件测试仅证明已有提交约束与资源操作可以运行，用户现场仍失败，
不能作为本次黑边验收通过的证据。

## 审查后的代码修复

- Native pacing 先于 buffer 替换；Owner 明确区分 native callback 与资源退休重试，
  避免因为仍有 pending resize 就把可通知的 DXGI 等待转成退避轮询。
- DX11/DX12 DComp resize 改为未绑定的候选链；保留最后已发布的 native content，
  匹配新尺寸 Scene 成功 Present 后才 SetContent/Commit。失败保留双代引用并尝试回滚，
  连续候选替换保留真正 Active，最终销毁时 detach visual。
- Vulkan maintenance1 保存本链 Present Fence 并复用已完成 fence。资源退休同时覆盖
  graphics frame 与 native presentation，正常 resize 不再等待无关设备工作；未支持
  feature 或 enqueue 状态不确定时保留保守边界。旧 native fence 退休后，公开的完成
  handle 仍能查询完成。
- 删除启动 5 秒后强制清理整个 working set 的线程和环境开关。
- OpenGL 在本窗口 post-SwapBuffers GL sync 已退休后，不再额外等待共享设备上其他
  窗口的全部提交；resize 只替换本窗 FBO/color，仍遵守原有 context owner 约束。

这些改动不缩放旧内容、不允许旧尺寸 Scene 绘入新 backbuffer；也不能据此承诺所有
HWND/驱动场景的扩窗空档已经消失。新暴露区域仍需现场逐帧验证，1ms 目标尚未达到。

修复阶段定向验证：DX11/DX12 原生 surface 测试 3 项、Vulkan retirement 状态测试 3 项、
GPUI 四后端原生 resize 测试 4 项（DX11/DX12 同时覆盖 opaque 与 transparent）、
Owner 测试 43 项和 Windows window 测试 37 项，合计 90 项通过。
`cargo check --manifest-path crates/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --locked`
通过，存在原有警告。测试核对代次、尺寸、首帧收敛和渲染像素，不覆盖两次 Present
之间的 DWM 屏幕像素；DComp 提交失败回滚尚未做驱动故障注入。
`cargo build -p bmcbl --bin BMCBL --locked` 通过（有警告）；应用产物
`target/debug/BMCBL.exe` 的本次构建时间为 2026-10-18 16:56:42（UTC+08）。
DX11 从 DComp 切回 HWND 的 alpha 模式转换仍沿用既有 detach 流程，可能存在首帧前
的覆盖空档；它不属于本次同一模式内 resize 修复，不能据这些测试宣称转换已无缝。
