# BMCBL TODO

> 本文件记录当前高优先级工程任务。状态约定：`[ ]` 待处理，`[~]` 进行中，`[x]` 已完成，`[?]` 需要本地 profiling / 编译验证。

## P0 — GPUI 动画、帧调度与异常占用

目标：修复主页等普通 UI 动画的卡顿、plateau → jump、CPU/GPU 占用异常问题，同时保留现有动画效果。原则不是“全部 GPU 化”，而是把 layout-dependent 与 compositor-independent 动画分流，消除重复 frame driver、重复 invalidation、重复 pacing 与无效 GPU 工作。

### 0. 基线与硬性原则

- [x] 保留真实 `width / height / left / top / flex / intrinsic size` 动画在 CPU layout；禁止为了 GPU 化改变布局语义。
- [x] `translation / opacity / scale / reveal` 仅在能证明收益且 renderer 路径闭合时使用 presentation / compositor 动画。
- [x] 不为小型动画无脑创建 offscreen/composite layer；必须考虑 primitive 数、texture 尺寸、upload bytes、blur dependency、更新频率与额外 pass 成本。
- [x] 不移除动画效果、不通过降帧/缩短动画/关闭 easing 掩盖问题。
- [x] Surface 在 Nova 未完整支持 animation ABI 前保持不做伪 GPU animation。
- [x] Path GPU animation 在 batch ownership / raster sampling / ABI / benchmark 未闭合前不继续扩展；已撤销提前拆 batch 的负优化。
- [ ] 建立动画性能基准场景：主页 dropdown、顶部导航 pill、页面切换、loading pulse/spinner、modal、Toast、blur-overlay。
- [ ] 记录 60 / 120 / 144 / 165 / 240 Hz 下 frame time、CPU、GPU、present interval 与 missed-vblank。

### 1. Windows frame pacing — 最高优先级

当前 Windows 路径同时存在 GPUI `DwmFlush()` VSync scheduler、DXGI frame-latency waitable object、Nova FIFO/AutoVsync Present，需要确认是否形成重复节拍或 missed VBlank。

- [~] 审计 `platform/windows/vsync.rs`、`platform/windows/window.rs`、Nova DX12 swapchain / Present 路径。
- [ ] 明确一个权威 pacing source，避免 `DwmFlush + wait_swapchain_frame_latency + FIFO Present` 三层互相阻塞。
- [ ] A/B：DWM pre-vsync pacing + latest-frame/non-blocking present。
- [ ] A/B：移除 pre-DwmFlush pacing，由 DXGI frame-latency object + FIFO Present 主导。
- [ ] 验证不同刷新率、VRR、窗口遮挡/最小化/恢复、多窗口下行为。
- [ ] 防止 queued frame / pending frame request 在 missed VBlank 后连续追帧造成 CPU/GPU 尖峰。
- [ ] 确认 `request_frame` 合并策略不会因为同一动画的多个 requester 重复唤醒平台层。
- [ ] 明确 `Present` 的 `sync_interval / flags` 与 Nova `PresentMode` 映射，避免 AutoVsync 在平台已 pacing 时再二次限流。
- [ ] 检查 resize / occluded / inactive window 是否仍不必要地参与 VSync/present。

### 2. Animation execution class

参考 WinUI / Avalonia / Qt Quick，把“由谁驱动”与“允许污染到哪里”分开。

- [ ] 引入或等价实现 `LayoutDependent / PaintDependent / CompositorIndependent` 执行分类。
- [ ] `CompositorIndependent` 动画禁止触发 Taffy、AnyView render、text shaping、image decode、普通 primitive rebuild。
- [ ] Debug 模式增加断言/统计：independent animation 若造成 layout dirty / full view render 则记录 violation。
- [ ] `LayoutDependent` 明确允许局部 retained reconciliation，但不得默认扩大到 root view。
- [ ] `PaintDependent` 只 invalidate 最小 paint subtree，不触发布局。
- [ ] 同一视觉动画只能有一个权威 cadence driver；禁止 sampled presentation 与 layout target 双驱动。

### 3. Retained replay / progressive rendering

- [x] targeted retained animation frame 中禁止 progressive dirty view 复用旧帧，避免动画时钟继续推进但画面停留后跳变。提交：`8fb616cd`。
- [ ] 为该行为补测试：active targeted animation 不得返回 `DeferredDirtyReuse`。
- [ ] 检查 targeted invalidation 是否把 owner 的 route ancestors 错误标记为普通 `view_dirty`，导致主窗口/背景/顶栏 cache miss。
- [ ] 分离“直接 dirty view”与“仅用于路由的 ancestor view”，避免祖先 cache 被无意义打穿。
- [ ] 验证 `ReconcileSubtree` 只重建目标 retained path，不扩散到兄弟 retained subtree。
- [ ] layout animation target 在缺少 retained identity 时统计 fallback-to-owner/root 次数，优先消除高频 fallback。
- [ ] dirty region 与 retained path 的 swept bounds 必须精确；禁止一个小动画退化成全窗 damage。

### 4. Animation clock / frame lifecycle

- [ ] 全部动画采样统一使用 frame-stable `window.animation_time()`；同一帧禁止混用多次 fresh `Instant::now()` 造成相位不一致。
- [ ] `run_platform_frame()` 中 animation callback、dirty 判定、draw、renderer animation value refresh、next-frame scheduling 只有一条明确时序。
- [ ] active animation timeline 不得因为 presentation-only frame 与 layout frame 双方同时 self-reschedule。
- [ ] completed animation 必须停止请求 frame；建立 stale animation ticket / orphan animation ID 检测。
- [ ] 同一 frame 多个动画请求必须 coalesce，不允许按控件数量放大 platform request 数。
- [ ] deadline/cadence 动画使用 targeted deadline invalidation，禁止为了等待 16ms/33ms 持续按 VSync 空转 render。

### 5. Presentation-only / renderer-owned animation

- [ ] presentation-only frame 理想路径限定为：animation clock → small animation value patch → damage → GPU submit → present。
- [ ] presentation-only frame 不允许进入 `AnyView::render / request_layout / text shaping / image decode`。
- [ ] Scene animation value 更新不得递增 static scene revision；持续保持 retained static upload 可复用。
- [ ] 稳定 `SceneAnimationId`，避免 frame-local ID 让可复用 primitive 被识别为新拓扑。
- [ ] renderer-owned animation 的 slot topology 改变才允许重建 slot map；仅 value 改变时只 patch value buffer。
- [ ] animation values buffer 使用最小 upload 范围，避免整 buffer 每帧重传。
- [ ] GPU queue / command submission 在 presentation-only frame 下避免不必要的 resource barrier / descriptor rebuild。

### 6. Damage / backdrop blur / composite

- [ ] Backdrop blur 仅由其 source dependency damage 触发 refresh，不因无关动画全量重算。
- [ ] swept animation bounds 只污染真实经过区域，避免 blur-history 全局失效。
- [ ] GPU-indexed animation 与 retained blur history 的 conservative fallback 要能统计命中原因。
- [ ] composite layer 基于 cost model 选择 primitive-retained / texture-retained / inline，不按“是否动画”自动分层。
- [ ] 小型 chevron/spinner 等不得因 16px 动画创建高成本 offscreen target。
- [ ] 大型稳定子树的 translation/opacity 可考虑 texture-retained layer，但必须测量 texture bandwidth 与显存占用。
- [ ] filter / blur / shadow 动画检查是否造成每帧扩大 intermediate texture 尺寸或重新分配 render target。

### 7. Layout animation

- [x] 主页 dropdown 去除同一 spring 的重复 layout cadence driver；chevron rotation 动画效果保留。提交：`8c935a01`。
- [ ] 审计全部 `with_layout_animation_target`：仅保留真实 geometry / CPU mesh/layout 变化。
- [ ] 对纯 paint / presentation 动画提供不触发布局的 targeted cadence。
- [ ] parent layout animation 移动整个 descendants subtree 时避免每个 descendant 单独 invalidation。
- [ ] child 局部动画不得把 parent/root layout 标记 dirty。
- [ ] 顶部导航 pill 的 `left + width` 暂保留 CPU layout；在没有独立 X-scale/geometry ABI 前不强迁 GPU。
- [ ] Home dropdown 的 height/top/item stagger 保留 CPU layout，但减少 uniform-list/文本/图片无关重算。
- [ ] Manage/Tabs 等真实 geometry 动画逐项确认 retained boundary 粒度。

### 8. Text / SVG / Image during animation

- [x] Minecraft 格式文本缓存改用 `SharedString + Arc<ParsedMinecraftText>`，减少重复分配与 clone。提交：`792a3e85`。
- [x] `§k` 混淆文字使用 16ms targeted cadence，避免 144/240Hz 下重复 shaping 相同字符。提交：`72c3a1e6`。
- [ ] presentation-only transform/opacity 动画时禁止重新 shape 静态文本。
- [ ] SVG/path 静态内容在父 composite transform 下应复用已生成 geometry，不重 tessellate。
- [ ] 图片动画不得重复 decode / cache lookup / atlas upload。
- [ ] 图像/字体 atlas 更新不得因为动画 frame 全局 refresh。
- [ ] spinner/chevron 若使用 SVG，检查 rotation 是否能只更新 transform，避免重建 path。

### 9. Nova / GPU upload

- [ ] retained static upload signature 与 animation topology/value 分离。
- [ ] animation value 更新不得触发 full scene encode。
- [ ] persistent GPU buffer/slab 只 patch changed ranges；避免全量 upload。
- [ ] 对 Quad / Shadow / Mono / Poly / Underline / BackdropBlur 的 animation slot 增加 ABI/stride/topology 测试。
- [ ] Path animation 暂不实现，直到以下条件同时满足：一 PathSprite 一 owner、base sampling bounds 与 animated bounds 分离、opacity 正确、ABI/stride 测试、batching benchmark 为正收益。
- [ ] Surface animation 在 Nova 原生支持前保持 unsupported。
- [ ] GPU timestamp query：scene encode、upload、blur、composite、present 前后阶段耗时。

### 10. Telemetry / profiler

- [ ] 增加 frame pacing telemetry：`frame_request_time`。
- [ ] 增加 `DwmFlush_wait_us`。
- [ ] 增加 `frame_latency_wait_us`。
- [ ] 增加 `cpu_build_layout_us / prepaint_us / paint_us`。
- [ ] 增加 `nova_encode_us / upload_bytes / upload_us`。
- [ ] 增加 `gpu_submit_us / Present_us / present_interval_us`。
- [ ] 增加 `missed_vblank_count` / consecutive missed-vblank。
- [ ] 增加 `presentation_only_frame_count / layout_animation_frame_count`。
- [ ] 增加 `retained_replay_count / retained_cache_miss_reason`。
- [ ] 增加 `full_redraw_area_ratio / dirty_rect_count / blur_damage_area`。
- [ ] 增加 `active_animation_count / animation_slot_count / orphaned_slot_count`。
- [ ] Debug overlay 显示当前 frame type、CPU/GPU frame time、pacing wait、dirty area。

### 11. 测试与回归标准

- [ ] 单元测试：同一动画多个 request 只产生一个 pending platform frame。
- [ ] 单元测试：presentation-only frame 不进入 layout。
- [ ] 单元测试：targeted layout animation 不被 progressive replay 旧帧。
- [ ] 单元测试：动画结束后不再请求 frame。
- [ ] 单元测试：retained animation values 更新不改变 static scene revision。
- [ ] 单元测试：blur dependency damage 只覆盖 source swept bounds。
- [ ] Windows 集成测试：60/120/144/240Hz frame cadence 无稳定 2× frame interval 模式。
- [ ] Windows 集成测试：窗口最小化/遮挡后 animation scheduler 不空转。
- [ ] Windows 集成测试：恢复窗口后不 burst 多个积压 animation frame。
- [ ] 性能门槛：简单 opacity/translation 动画 CPU 不应显著高于静态 frame；GPU work 与 dirty area 成比例。
- [ ] 性能门槛：主页常规动画在目标刷新率下不出现连续 plateau → jump。
- [ ] 性能门槛：动画过程中静态文本/图片/背景 retained subtree 保持高 cache hit。

## P1 — Retained architecture 后续优化

- [ ] 将 invalidation 明确拆成 `LAYOUT / DISPLAY / HIT / TRANSFORM` 轴，避免一个属性变化默认污染所有阶段。
- [ ] retained element instance 持有稳定 layout/prepaint/paint state，减少依赖 raw frame-range replay。
- [ ] 按更新频率建立 retained layer 边界；禁止一个高频动画把上千静态 primitive 放进同一 dirty layer。
- [ ] persistent Taffy/LayoutId 仅在 instance identity 稳定后推进，不做独立半套缓存。
- [ ] 对大稳定 layer 评估 renderer-side retained texture；小 layer 保持 primitive replay。
- [ ] layer rasterize threshold 由 benchmark/cost model 决定，不固定照搬其它框架数值。
- [ ] retained primitive ordering 改为更局部的 layer/order scope，降低一个局部变化造成全局 sort 的概率。
- [ ] hitbox/dispatch/text retained state 与 visual retained state 生命周期一致，避免视觉复用但交互状态过期。

## P2 — 平台与长期性能

- [ ] Linux Wayland/X11 对齐 frame callback / presentation feedback，不套用 Windows pacing 假设。
- [ ] macOS 使用 display-link / compositor feedback 对齐同一 animation execution model。
- [ ] VRR / fractional refresh / multi-monitor refresh-rate 切换测试。
- [ ] 后台窗口/非激活窗口自动降低或停止非必要 continuous animation，但不改变前台视觉语义。
- [ ] `reduce_motion` 仅作为用户可访问性设置，不作为性能问题兜底。
- [ ] 建立 GPUI/Nova microbench：text、SVG/path、image、blur、shadow、retained replay、animation slot patch、damage/present。

## 已完成的相关修复

- [x] `23cf22cb` — 撤销 Nova Path backend 未闭合前的动画 Path 拆 batch，恢复 batch 合并率。
- [x] `03ae0da5` — Toast 动画迁移到保留合成层并修复 steady toast 持续请求帧。
- [x] `551329b7` / `28ed94a6` — Dropdown 浮层去重动画采样并建立 retained presentation cadence。
- [x] `960f4a2e` / `2da51499` — 收窄 Xbox 动画失效范围并去除重复布局帧驱动。
- [x] `792a3e85` — 减少 Minecraft 格式文本动画热路径复制。
- [x] `72c3a1e6` — `§k` 混淆文字限制到真实 16ms 内容更新 cadence。
- [x] `8fb616cd` — targeted animation 不再被 progressive dirty reuse 跳过。
- [x] `8c935a01` — 主页 dropdown 删除重复 layout driver，视觉动画保留。

## 当前执行顺序

1. Windows frame pacing / Present / frame-latency A/B 与 telemetry。
2. targeted invalidation 的 direct-dirty 与 route-ancestor 分离。
3. animation execution class 与 independent-animation debug contract。
4. presentation-only frame 的真正最短路径。
5. blur/damage/composite cost model。
6. Nova upload / persistent buffer 的增量化。
7. 主页与常用组件逐个基准验证，不以删除动画换性能。
