# GPUI 全链路性能审计与优化 TODO

> 目标：记录 BMCBL 当前 GPUI/Nova 路径中已经能从代码与运行日志确认的卡顿、阻塞、异常 CPU/GPU 占用风险，以及建议的本地修复顺序。本文是工程 TODO，不代表相关问题已经修复。
>
> 原则：不靠删除动画、降低动画帧率、缩短动画时长或强制所有东西“GPU 化”掩盖问题。真正的目标是让每类变化只经过它必须经过的 pipeline 阶段。

## 0. 当前运行日志判读

本次典型日志：

```text
gpui frame generation budget hit:
window=4294967297
elapsed=8.4596ms
budget=7.6ms
progressive_budget=4ms
progressive_degraded=false
layout_nodes=307
measured_layout_nodes=51
layout_roots=3
layout_cache_hits=1
layout_cache_misses=2
layout_cache_reused_roots=1
layout_cache_saved_nodes=6
layout_bounds_cache_hits=327
layout_bounds_cache_misses=300
text_layout_hits=205
text_layout_reuses=86
text_layout_misses=0
scene_primitives=46
scene_batches=24
scene_replayed_primitives=27
scene_retained_capacity=1148
frame_retained_capacity=5654
dirty_refreshes=0
dirty_view_marks=2
dirty_notify_invalidations=7
first_view_dirty_entity=MainWindowView
first_notify_entity=AppChromeView
```

### 当前最重要的结论

- [ ] **优先处理 CPU build/layout/invalidation，而不是先优化文字 shaping。** `text_layout_misses=0`，说明这一帧文字布局缓存没有发生真正 miss；文字仍有调用/查表成本，但不是这里最明显的根因。
- [ ] **局部顶栏变化正在扩散成过大的 CPU 更新范围。** 第一个 `notify` 是 `AppChromeView`，第一个 dirty view 已经是 `MainWindowView`。`mark_view_dirty()` 会把 owner 的 ancestor path 放进 `dirty_views`，当前 retained traversal 与真正“需要重建”的 dirty 语义没有充分分离。
- [ ] **Taffy retained root 复用率不足。** 本帧 3 个 layout root 只有 1 个复用，307 个 layout node、300 次 bounds miss。对于一个顶部胶囊/下拉等小动画，这个更新规模明显过大。
- [ ] **8.46 ms CPU generation 对高刷窗口已经不可接受。** 120 Hz 一帧约 8.33 ms，144 Hz 约 6.94 ms，165 Hz 约 6.06 ms，240 Hz 约 4.17 ms；即使 GPU 很快，CPU 生成阶段也已经足够造成 missed presentation。
- [ ] **`progressive_degraded=false` 说明这条日志不是一次已发生的 degraded fallback 本身。** 但当前固定 4 ms progressive deadline 与 degraded recovery 仍存在放大后续抖动的结构性风险，需要单独修。
- [ ] **46 primitives / 24 batches 是值得继续追踪的 batch fragmentation 信号，但不能只凭这一帧认定 GPU-bound。** 应增加 pipeline/material/clip/texture/order 维度的 batch-break reason。
- [ ] **`scene_retained_capacity=1148`、`frame_retained_capacity=5654` 远大于当前 46 primitives 不是立即 shrink 的理由。** 先确认历史峰值、复用率、allocation count 与 cache residency；错误地每帧收缩会比保留容量更贵。
- [ ] Windows Task Manager 把 D3D12/wgpu 的普通 2D UI 工作显示在 “GPU 3D” engine 是正常现象。真正异常指标是：窗口静止时是否仍持续 submit/present、GPU busy 是否与 dirty area 不成比例、是否存在永不结束的动画/blur refresh。

---

## P0-A — Invalidation：把“路由祖先”与“真正需要重建的 View”分开

### 当前问题

`crates/gpui/src/window/frame_lifecycle.rs::mark_view_dirty()` 当前会从直接 dirty view 沿 dispatch tree 向上把 ancestor 全部加入 `dirty_views`。这对查找 retained path / traversal 有用，但如果后续 cache reconciliation 把 ancestor dirty 同时解释为“需要重新 render/build”，局部状态变化就会污染整个页面根。

`draw.rs` 已经在 pixel damage 阶段区分 `directly_dirty_views`，说明渲染区域层面已经认识到“祖先 dirty 不等于祖先所有像素改变”；下一步应把同样的区分提前到 CPU build/layout 阶段。

### TODO

- [ ] 将 dirty 状态拆成至少两类：`DirectDirty` 与 `TraversalAncestor`。Ancestor 只用于定位/遍历 retained path，不自动失效其 build/layout/paint cache。
- [ ] 为 dirty 原因增加 bitset/enum，例如 `STATE / LAYOUT / PAINT / TRANSFORM / HIT_TEST / CHILD_ROUTE`，不要用一个 bool 表示所有无效化。
- [ ] `cx.notify()` 只记录直接 owner；祖先由 scheduler 作为 traversal metadata 处理，而不是直接升级为普通 render invalidation。
- [ ] retained reconciliation 从 root 向下时，允许 ancestor 仅“穿过”，到真正 dirty child 才执行 render。
- [ ] 增加 `view_render_count_by_type` 与 `view_dirty_reason_by_type`，动画一帧明确看到 `MainWindowView` 是否真的执行了 render。
- [ ] 增加 `ancestor_only_dirty_count`，验证优化后 root 只作为 traversal node。

### 推荐设计

借鉴 Jetpack Compose 的 restartable/skippable scope：高频 state read 应尽可能下沉到最小 scope；父 scope 如果输入稳定就跳过 composition。Chromium 也把 compositor update 与需要 BeginMainFrame 的 main-thread damage 分开，没有 main-thread damage 时可以直接跳过 main thread 的 style/layout/commit。

### 验收

- [ ] 顶栏 pill/chevron 动画时，`MainWindowView` 可以出现在 traversal path，但不得因此执行完整 render/layout。
- [ ] 一个叶子 transform/opacity 动画不得让 route sibling、background、page body 重新 build。
- [ ] Debug overlay 能明确显示 “direct dirty” 与 “ancestor traversal” 数量。

---

## P0-B — `AppChromeView` 过于单体化

### 当前问题

`src/ui/main_window/chrome_view.rs` 订阅并对整个 View `cx.notify()`：

- `BedrockAuthState`
- router state
- `NavState`
- `ThemeState`
- `I18n`
- `UpdateState`
- Settings state
- Plugin registry
- window bounds
- window activation

因此一个 Nav spring tick、主题颜色 tick、登录弹层变化都会重新进入整个顶栏 render。

`render_app_chrome()` 每次又会重新构造/计算：

- built-in nav item `Vec`
- plugin navigation pages 的 clone/映射
- 多个 `SharedString` / label
- SVG/icon element
- version `format!`
- logo/control/auth/nav 整个 element tree
- theme colors
- nav 几何

另外 `prepare_render_state()` 目前每次 render 都执行 `Arc::new(navigation_pages(cx))`，高频动画期间不应重复发现/克隆 plugin navigation model。

### TODO

- [ ] 拆成稳定子 View：`BrandChromeView`、`NavChromeView`、`AuthChromeView`、`WindowControlsView`，让每个 state 只 notify 真正依赖它的子树。
- [ ] `NavState` 动画只通知 Nav scope，不触发 auth/brand/window controls build。
- [ ] `ThemeState` 区分 geometry-neutral color update 与真实 layout update；纯颜色变化优先 paint/presentation invalidation。
- [x] plugin navigation model 改为 versioned/memoized snapshot，仅 `PluginRegistry` revision 或 I18n revision 变化时重建。
- [ ] 固定 built-in nav metadata 使用静态 slice/`LazyLock`/稳定 model，不在每个动画帧创建 Vec。
- [ ] app version 字符串预计算并缓存，不在顶栏高频 render 中 `format!`。
- [ ] 对 render 热路径增加 per-frame allocation telemetry：alloc count / allocated bytes / `Vec` growth / `Arc` create / String clone。

### 推荐设计

Compose 的核心做法不是“所有东西都缓存”，而是让频繁变化的 state read 尽量靠近真正消费它的 draw/layout scope，从而跳过不相关 composition/layout。Qt Quick 则把 QML item 与真正用于渲染的 QSGNode tree 分离，稳定 node 不需要因为上层对象重新求值就重新上传 geometry。

### 验收

- [ ] Nav animation 一帧中 `AppChromeView` 根不再因为 Nav tick 完整 rebuild，或根 render 成本接近 O(1) traversal。
- [ ] plugin page 列表在无 plugin/I18n 变化时 allocation=0。
- [ ] 静态 logo/auth/control 的 retained scene 与 layout fingerprint 在 Nav animation 全程稳定。

---

## P0-C — Taffy：当前“每帧重建树 + root fingerprint reuse”粒度仍然太粗

### 当前问题

`TaffyLayoutEngine::clear()` 每帧会保存 retained roots 后清理：

- `taffy`
- `absolute_layout_bounds`
- `unrounded_layout_origins`
- `computed_layouts`
- `node_fingerprints`
- `node_layout_metadata`
- measured subtree state

下一帧 `request_layout()` 又创建新的 Taffy nodes，再依靠 root fingerprint 尝试复用整 root。这个模型对“整个 root 完全稳定”的情况有效，但动画中某个 absolute child 的 `left/width` 改动很容易让 root/subtree fingerprint 改变，随后大片 sibling 被重新建树/重新求 bounds。

### 短期 TODO

- [ ] 记录每个 layout root cache miss 的 **第一处 fingerprint divergence**，不要只统计 hit/miss。
- [ ] 对 absolute-positioned child 建立 dependency 规则：如果它不参与 parent intrinsic size/flex distribution，其 `left/top/transform` 变化不得让无关 sibling layout invalid。
- [ ] layout animation target 除 retained scene boundary 外增加 retained layout boundary。
- [ ] 将“geometry 变化但不影响 parent measure”的 property 单独标为 placement dirty，而不是 measure dirty。
- [ ] measured node 仅在 measure input / font / content / intrinsic constraints 改变时重新调用 measure closure。

### 中期 TODO

- [ ] 引入稳定 `LayoutInstanceId` / persistent layout node identity；不要把 frame-local `LayoutId` 当长期 identity。
- [ ] 每个 persistent node 维护 `needs_measure / needs_layout / needs_placement` dirty bits。
- [ ] 从 dirty node 向上只传播必要依赖；向下只在 parent constraints 改变时传播。
- [ ] layout result cache 从“整 root fingerprint”升级为可复用的 persistent subtree result。
- [ ] 若继续使用 Taffy，设计 GPUI persistent instance → frame Taffy node 的映射/增量同步；不要同时维护两套互相不知道 dirty 语义的半缓存。

### 可借鉴模型

- Flutter `RenderObject`：`markNeedsLayout` / `markNeedsPaint` 明确区分阶段，并通过 relayout/repaint boundary 控制传播。
- Jetpack Compose：composition、layout、draw 三阶段可独立跳过；频繁 state read 可延迟到 layout/draw 阶段。
- Qt Quick：稳定 QSGNode retained tree，geometry 只在真正变化时重新上传。

### 验收

- [ ] 顶栏 pill 动画目标：layout nodes 从当前约 307 降到几十以内；若仅 placement 改变，measured nodes 应接近 0。
- [ ] `layout_bounds_cache_misses` 不再接近整棵树规模。
- [ ] 无关 page body 的 layout fingerprint 不因 chrome 动画变化。

---

## P0-D — 固定 4 ms progressive deadline 与 degraded recovery 可能形成抖动放大器

### 当前问题

`TARGET_FRAME_GENERATION_BUDGET = 4ms` 同时被用于 dirty-frame backpressure / draw deadline，但实际 display interval 可能是：

- 60 Hz：16.67 ms
- 120 Hz：8.33 ms
- 144 Hz：6.94 ms
- 165 Hz：6.06 ms
- 240 Hz：4.17 ms

固定 4 ms 并不能表达真实的 present deadline、GPU tail latency、平台 pacing wait 或当前帧已经消耗的时间。

更重要的是，`finish_degraded_draw()` 当前会：

- 丢弃 `next_frame`
- 再次把 window 标脏
- `force_full_redraw = true`
- `force_view_cache_refresh = true`
- 进入 recovery 状态

如果一帧只是局部 subtree 超时，下一帧强制更大范围 rebuild 可能比原工作更重，形成 `deadline miss -> recovery full work -> 再 miss` 的振荡。

### TODO

- [ ] 把 frame deadline 改为由实际 `predicted_present_time - safety_margin` 导出，而不是全刷新率固定 4 ms。
- [ ] 区分 `InteractiveMain`, `CompositorOnly`, `BackgroundProgressive`, `Recovery` 工作类别，各自有 deadline 策略。
- [ ] 记录 `deadline_remaining_at_phase_start`，确认究竟在 build/layout/prepaint/paint 哪个阶段越界。
- [ ] degraded 时不要默认丢弃所有已经完成且 immutable 的 retained subtree；研究 resumable/restartable work。
- [ ] recovery 不应无条件 `force_full_redraw + force_view_cache_refresh`。只有 retained state 不可信时才升级 full rebuild。
- [ ] 增加 hysteresis，避免刚恢复一帧又立刻进入 degraded/recovery。
- [ ] 若 main-thread frame 来不及，允许 compositor/presentation-only 动画继续显示上一份稳定 main scene，而不是阻塞视觉进度。

### 借鉴 Chromium

Chromium scheduler 有明确 frame deadline；main thread 慢时可先用 active compositor tree 绘制，并进入 high-latency mode，而不是把一次 deadline miss 简单升级为下一帧完整重建。恢复时还可以跳过一次 BeginMainFrame 来追上显示节拍。

### 验收

- [ ] 单次 8–10 ms spike 不得自动引发连续多帧 full rebuild。
- [ ] `degraded_count`、`recovery_full_redraw_count`、连续 degraded 长度可观察。
- [ ] 120/144/165/240 Hz 分别使用对应 display deadline，不用一个常量假装全部刷新率相同。

---

## P0-E — Windows frame pacing：必须只有一个真正的节拍权威

### 已做

`a2fe9e6a` 已避免正常成功的 `DwmFlush()` 后再按启动时缓存 refresh interval 做一次额外 sleep/interval 限速。

### 仍需审计

- [ ] `DwmFlush`、DXGI frame-latency waitable object、wgpu/Nova `PresentMode` 是否还有两个以上 blocking pacing point。
- [ ] 明确 `request_frame -> platform callback -> encode/submit -> Present` 每一步实际时间。
- [ ] `Present` 是否因为 FIFO/AutoVsync 再次阻塞，导致 DWM pre-pacing 后错过当前 vblank。
- [ ] VRR 下不要假设固定 refresh interval。
- [ ] 跨屏移动窗口后刷新率/refresh source 是否及时更新。
- [ ] 多窗口不能因为一个全局/主屏 refresh estimate 互相拖慢。
- [ ] occluded/minimized/inactive window 不参与高频 pacing，除非有明确 presentation 需求。
- [ ] platform frame watchdog 的 100 ms recovery 统计真实触发率；排除正常 compositor 延迟被误判成 stall 后直接 `run_platform_frame()`。

### 推荐 A/B

- [ ] A：DWM/display callback 作为唯一 BeginFrame source，Present 尽量只提交，不再人为 sleep。
- [ ] B：DXGI frame latency/present feedback 作为权威，取消 pre-DwmFlush pacing。
- [ ] 用同一 benchmark 比较 p50/p95/p99 frame interval、input-to-present latency、CPU wakeup 次数，而不是只看平均 FPS。

### 借鉴 Qt Quick

Qt Quick threaded render loop明确把 animation driver 与实际 render/vsync 节拍绑定，同时也提供 elapsed-time driver 来规避“系统实际 vsync 行为与框架假设不一致”造成的动画速度/卡顿问题。关键不是照搬 Qt 线程模型，而是保证 animation clock 与真正的 presentation cadence 一致。

---

## P0-F — Animation Engine：建立真正的 execution class

### 建议分类

```text
LayoutDependent
  width / height / flex / intrinsic size / topology / parent-dependent position

PaintDependent
  color / border / shadow params / local custom paint without geometry dependency

CompositorIndependent
  translation / opacity / supported scale / clip reveal / retained subtree transform
```

### TODO

- [ ] 每个 animation property 在注册时确定 execution class，运行时禁止隐式升级到更重阶段而无 telemetry。
- [ ] `CompositorIndependent` tick 不允许调用 `AnyView::render`、Taffy、text shaping、image decode。
- [ ] `PaintDependent` 不允许无原因触发布局。
- [ ] 同一个动画只有一个 cadence owner；禁止 layout target + presentation sampler 双方都 self-reschedule。
- [ ] 统一用 `window.animation_time()` 作为 frame-stable clock；每帧所有 animation sample 共享同一 timestamp。
- [ ] 动画视觉已经落在亚像素 settle 区间时停止继续请求无意义 frame，或直接把 spring snap 到目标。
- [ ] 记录 `animation_frames_after_visual_settle`，查找“看起来静止但 scheduler 仍跑”的尾巴。

### 快速反向点击 / SpringValue

当前 `responsive_retarget_velocity()` 在 `current_velocity * delta <= 0` 时直接把 velocity 设 0。快速展开→收起→展开会产生明显“刹停再起步”，这不是连续 retarget。

- [ ] `SpringValue` retarget 至少保证 C0：position 连续。
- [x] 对需要自然连续的交互 spring，保留/投影当前 physical velocity，使方向变化由新弹簧力完成，而不是人为瞬间归零。
- [ ] 对非常短的剩余距离限制 normalized velocity，避免 velocity / delta 爆炸。
- [ ] 区分 geometry spring 与 icon rotation progress。列表允许 overshoot 不代表 chevron 必须转过 180°。
- [ ] 增加快速 `open -> close -> open`、`next -> previous -> next` 连续 retarget 单元测试。

### 验收

- [ ] 快速连续点击箭头角速度不突变、不停顿、不超出设计角度。
- [ ] 动画结束后 active ticket/slot/frame request 都归零。

---

## P0-G — Main-thread 与 renderer/compositor 解耦

当前目标不是简单“加一条渲染线程”，而是让可独立更新的视觉属性在 main UI thread 卡住时仍然可继续。

### TODO

- [ ] 设计类似 `BeginMainFrame` / `CompositorFrame` 的两级 pipeline：main scene commit 与 compositor/presentation update 分离。
- [ ] main thread 只有内容/layout/paint 发生变化时才 commit static scene。
- [ ] compositor-owned transform/opacity 可在 active scene 上直接刷新 animation value。
- [ ] main-thread deadline miss 时，renderer 可继续使用上一份 active scene 推进安全的 compositor animation。
- [ ] static scene topology revision 与 animation value revision 分开。
- [ ] commit/activate 必须原子，避免 renderer 看到半更新 scene。

### 借鉴

- Chromium：main/pending/active tree；无 main-thread damage 时可跳过 BeginMainFrame，impl/compositor 仍可做异步动画/滚动。
- Qt Quick：稳定 QSGNode tree 可由 render thread 处理；Animator 类型可在 GUI thread 被阻塞时继续。
- Windows Composition：Compositor 自己管理 visual/effect/animation system，视觉层动画不要求应用每帧重建 UI tree。

---

## P0-H — Nova upload / batch / persistent GPU state

### 当前风险

- [ ] 46 primitives / 24 batches 需要知道为什么断 batch；仅看数量不够。
- [ ] presentation animation 如果仍触发 static scene encode/full stream upload，就失去 renderer-owned animation 的意义。
- [ ] retained static upload signature、animation topology、animation values 必须是三个不同 revision 维度。

### TODO

- [ ] batch debug 增加 break reason：`pipeline / texture / clip / order / blend / material / target / animation ownership`。
- [ ] 对每种 primitive stream 统计 encoded bytes、uploaded bytes、changed range bytes。
- [ ] 静态 scene 未改时完全复用 GPU buffer；动画值只 patch 最小 range。
- [ ] persistent buffer 使用 capacity + dirty-range，不在每帧重建 staging Vec/whole buffer。
- [ ] descriptor/bind group 只有 resource topology 改变时重建。
- [ ] 对 animation slot topology 建稳定 cache；slot value 改变不得重新建立 map。
- [ ] 增加 GPU timestamp query：render pass、blur、composite、upload/copy、submit-to-fence。
- [ ] 增加 queue depth / in-flight frames 统计，检查 CPU 是否因为 GPU fence/queue saturation 被反压阻塞。

### Path / Surface 限制

- [ ] Path GPU animation 在 ownership、base raster sampling bounds、animated bounds、opacity、ABI stride、benchmark 全闭合前不要“为了 GPU 化”拆坏 batching。
- [ ] Path 先 raster 到 screen-space intermediate 时，未来 translation/scale 必须让 vertex 用 animated bounds，而 texture coordinate 仍采样 base bounds。
- [ ] Surface 在 Nova 没有完整 animation ABI 前保持 unsupported，不伪造 animation id。

### 借鉴 Qt Quick

Qt Quick 默认 renderer 重点就是 draw-call batching 与 GPU geometry retention；真正优化方向是减少重复 geometry upload 与无意义 batch fragmentation，而不是把所有 UI 元素都独立变成 layer。

---

## P0-I — Backdrop blur / filter / composite layer

### 当前问题

顶栏开启 glass 时，full-width backdrop blur 是高成本依赖节点。即使最终 dirty rect 很小，只要 blur history/source dependency 判定过于保守，就可能重新采样大范围背景或让 intermediate target 失效。

### TODO

- [ ] 建明确 `source damage -> blur dependency -> output damage` 图。
- [ ] stable blur topology 下，只 refresh 与 source dirty rect 相交并经过 kernel expansion 的区域。
- [ ] blur primitive add/remove 允许一次 full redraw，但不能在后续稳定帧继续 full redraw。
- [ ] 记录 `blur_refresh_reason`：source damage / topology / animation conservative fallback / resize / target recreate。
- [ ] 记录 blur source area、expanded area、实际 processed pixels。
- [ ] filter intermediate texture 使用尺寸/format key cache，避免参数不变时重分配。
- [ ] 可 A/B downsample + separable blur，但必须以视觉误差与 GPU timestamp 验证，不默认降低质量。
- [ ] composite layer 使用 cost model：primitive count、pixel area、update frequency、texture bandwidth、blur/filter dependency，共同决定是否 raster-cache。
- [ ] 16px chevron/spinner 不要为了一个 transform 单独分配 offscreen texture。

---

## P0-J — 空闲 GPU/CPU 异常占用

### 目标不变量

一个完全静止、未遮挡变化、没有 active animation 的窗口，除平台 exposure/compositor 明确要求外，应接近：

```text
0 layout frame
0 view rebuild
0 scene encode
0 upload
0 GPU submit
0 present
```

### TODO

- [ ] 增加 `idle_frame_reason` / `frame_requested_by`，列出所有 frame requester。
- [ ] 查 stale animation ticket、orphan `SceneAnimationId`、永不完成的 spring、循环 theme tick、loading pulse、blur refresh。
- [ ] animation engine active count=0 时不得继续按 VSync self-schedule。
- [ ] `needs_present=false` 且无 platform exposure 时不得 submit identical framebuffer。
- [ ] 背景图片、时钟、hover、鼠标 tracking 不得在值未变化时 notify。
- [ ] inactive/minimized 窗口 continuous animation 自动暂停或降低 cadence；重新激活从 wall-clock 正确采样，不补跑积压帧。
- [ ] Windows Task Manager “3D” 不作为 bug 判定标准；以 GPU timestamp、queue submit count、present count 为准。

---

## P0-K — Text / SVG / Image：避免在错误方向花时间

### 从当前日志看

`text_layout_misses=0`，所以当前这条顶栏卡顿不是“字体 shaping 太慢”的直接证据。优先修 invalidation/layout 后再看文本。

### TODO

- [ ] text layout cache 除 hit/miss 外记录 lookup cost、glyph upload、atlas miss。
- [ ] 静态 text 在 transform/opacity animation 中不重新构建 shaped runs。
- [ ] SVG/path tessellation 以 resource/style/scale key 缓存；父 transform 变化不重新 tessellate。
- [ ] image decode 永远不在 frame critical path；async 完成后只 invalidate 消费它的最小 view/item。
- [ ] atlas upload 做 dirty-region/subresource 更新，不因动画 frame flush 全 atlas。
- [ ] 同一 URL/resource 的 cache lookup/prefetch signature 在 render 热路径避免 Vec/String clone。

---

## P0-L — 顶栏 Nav / 首页 Dropdown 专项

### 顶栏 Nav

- [ ] 修正 titlebar 真正数学居中：不要依赖 `justify_between` 在不等宽左右区域之间得到“视觉中点”。使用独立 absolute center layer 或三列布局（左右等宽 reservation + center）。
- [ ] pill 的 `left + width` 是真实 geometry，当前先保留 CPU placement/layout；不要用错误的 uniform scale 模拟。
- [ ] 但 pill 作为 absolute child 时，只应 invalidate pill placement，自身变化不应让全部 nav item 重新 measure。
- [x] `pill_edges()` 已经在接近目标时返回 exact target，但 `is_animating()` 仍可能为 true；统一 visual settle 与 cadence stop 条件。
- [ ] Nav spring tick 不再重建 plugin pages / auth / logo / controls。

### 首页启动按钮 Dropdown

- [ ] dropdown panel height/top/item geometry 与 chevron rotation 分离 execution class。
- [ ] chevron 使用 presentation transform，列表 geometry 才使用 layout cadence。
- [ ] chevron progress clamp/独立 spring，不继承列表 bouncy overshoot 导致旋转超过目标角。
- [ ] rapid reverse retarget 保持连续 position/velocity，不瞬间 velocity=0。
- [ ] 展开/收起只 invalidate dropdown retained subtree，不 notify Home root。
- [ ] 预加载/图片/版本列表数据更新与动画 cadence 解耦，后台结果每 frame coalesce 一次 UI commit。

---

## P1-A — 大列表、异步任务、图片与网络结果不要制造 UI storm

- [ ] virtual list item 使用稳定 key；滚动时不因 index 变化重建所有 item state。
- [ ] viewport 外 item 不执行 image decode/SVG build/expensive text parse。
- [ ] async worker 每个结果单独 `cx.notify()` 改为 batch/coalesce：一个 frame 最多一次 UI commit。
- [ ] image load 完成只 dirty 对应 row/card，不能 dirty 整个 Results page。
- [ ] prefetch 使用 bounded concurrency + cancellation + generation token；快速搜索/切 tab 时停止过期工作。
- [ ] plugin registry、filesystem watch、download progress 高频事件做 state aggregation，不直接一事件一帧。
- [ ] 进度类 UI 将 worker 1000Hz 更新降为“保存 latest value，按 display frame 读取”，不是丢数据，而是合并显示提交。

---

## P1-B — 主线程阻塞审计

### 搜索范围

- [ ] UI/render callback 内的同步文件 I/O。
- [ ] UI thread 内的 ZIP/Appx/JSON/NBT/LevelDB 大解析。
- [ ] `Mutex/RwLock` 竞争，尤其 worker 持锁后再通知 UI。
- [ ] channel recv / thread join / blocking wait / GPU fence wait。
- [ ] 大 Vec sort/hash/clone 在 render 中。
- [ ] `Arc::make_mut` / COW 大对象在 UI tick。
- [ ] 日志格式化和同步日志 backend 在高频 frame path。

### 规则

- [ ] frame critical path 禁止不可控阻塞操作。
- [ ] 重 CPU 工作放 background executor/线程池，结果使用 immutable snapshot/generation id 回到 UI。
- [ ] UI commit 有明确时间预算，例如 0.5–1 ms 级别，而不是把 worker 工作搬回 UI closure。
- [ ] debug instrumentation 自身不能改变 release 性能结论；高频 metrics 用原子计数/ring buffer，避免每帧 format String。

---

## P1-C — 内存、Arena 与每帧 allocation

- [ ] 增加 `alloc_count/frame`、`alloc_bytes/frame`、peak transient bytes。
- [ ] 区分 persistent retained capacity 与 transient frame arena，不能只看 capacity 大就 shrink。
- [ ] retained capacity 按长时间低水位 hysteresis trim，禁止一帧低用量就 `shrink_to_fit`。
- [ ] element/model 中稳定 `SharedString`/Path/Arc/metadata 尽量长期持有。
- [ ] render 内避免 `format!`、临时 Vec、HashMap、路径 clone、重复 `Arc::new`。
- [ ] arena reset 应 O(1)/批量回收，避免大量 Drop 在 present critical section。

---

## P1-D — Scene / dirty region / retained replay 正确性

- [ ] dirty region 记录 old bounds + new bounds；移动物体必须清掉旧位置。
- [ ] transform animation 使用 swept bounds，不因一个小元素升级 full viewport。
- [ ] retained scene replay 的 identity 必须稳定；frame-local order/id 不应让静态 primitive 看起来“全部变化”。
- [ ] primitive ordering 尽量局部 layer scope，避免局部插入导致后续所有 order 改变。
- [ ] clip stack identity 稳定；clip index 重排会破坏 batch/cache reuse。
- [ ] debug 显示 full-redraw fallback reason，并统计面积比，不只显示 bool。

---

## P1-E — 输入延迟与交互优先级

- [ ] 输入事件到下一 present 增加 `input_to_present_us`。
- [ ] 新输入到达时允许打断 background progressive throttle，但不能每个 mouse-move 触发 full build。
- [ ] pointer move/hover 使用 latest-wins 合并；同一 display frame 不处理无意义的所有历史位置。
- [ ] 点击后的视觉反馈（pressed/hover/ripple）优先于后台列表/图片更新。
- [ ] frame scheduler 明确优先级：Input > interactive animation > visible async result > background maintenance。

---

## P2 — Profiler / Telemetry：没有这些数据不要继续凭 GPU 百分比猜

### 每帧 CPU phase

- [ ] `event_to_begin_frame_us`
- [ ] `view_build_us`
- [ ] `layout_us`
- [ ] `measured_layout_us`
- [ ] `prepaint_us`
- [ ] `paint_us`
- [ ] `scene_finish_us`
- [ ] `nova_encode_us`
- [ ] `upload_prepare_us`
- [ ] `queue_submit_cpu_us`
- [ ] `present_call_us`

### Pacing

- [ ] `frame_request_time`
- [ ] `platform_callback_time`
- [ ] `DwmFlush_wait_us`
- [ ] `frame_latency_wait_us`
- [ ] `predicted_present_time`
- [ ] `actual_present_time/presentation feedback`
- [ ] `missed_vblank_count`
- [ ] `consecutive_missed_vblank`
- [ ] frame interval histogram p50/p95/p99/max

### Invalidation / retained

- [ ] direct dirty view ids/types + reason
- [ ] traversal-only ancestor ids/types
- [ ] render count by view type
- [ ] layout root fingerprint miss reason
- [ ] retained replay primitive count / rebuild primitive count
- [ ] full redraw fallback reason
- [ ] dirty area ratio / rect count

### GPU

- [ ] encoded bytes / uploaded bytes by stream
- [ ] bind group/descriptor rebuild count
- [ ] render pass count
- [ ] intermediate texture allocate/reuse count
- [ ] blur processed pixel area
- [ ] GPU timestamps by pass
- [ ] queue depth / fence wait
- [ ] present count vs visual-change count

### 动画

- [ ] active animations by execution class
- [ ] frame requester count by animation id
- [ ] orphan animation slot/ticket
- [ ] animation frame after visual settle
- [ ] layout animation fallback-to-owner/root count

---

## P2 — 建议的实施顺序

### Phase 1：先消除明显 CPU 扩散

- [ ] 拆 `AppChromeView` subscriptions/scope。
- [ ] direct dirty 与 traversal ancestor 分离。
- [ ] 修 Nav / Dropdown spring cadence 与快速 retarget。
- [ ] 消除顶栏 animation frame 中 plugin nav/静态 metadata allocation。
- [ ] 加 layout fingerprint miss reason。

### Phase 2：布局增量化

- [ ] absolute placement dirty 与 measure dirty 分离。
- [ ] persistent layout instance identity。
- [ ] subtree layout cache / dirty propagation。
- [ ] 验证 307 layout nodes 是否下降到局部规模。

### Phase 3：main/compositor 分层

- [ ] execution class 正式进入 GPUI animation API。
- [ ] static scene revision 与 animation value revision 分离。
- [ ] presentation-only frame 不进入 View/Taffy。
- [ ] renderer active scene 可在 main thread missed deadline 时继续安全动画。

### Phase 4：Nova / GPU

- [ ] GPU dirty-range upload。
- [ ] batch-break profiler。
- [ ] blur dependency 精确化。
- [ ] GPU timestamps。
- [ ] 根据实测决定 layer raster cache，而不是提前大规模 composite。

### Phase 5：scheduler/pacing

- [ ] predicted present deadline 动态预算。
- [ ] 重做 degraded/recovery 策略。
- [ ] Windows pacing A/B。
- [ ] Linux Wayland presentation feedback / macOS display link 对齐同一调度模型。

---

## 验收基准

建议建立至少以下场景，每个场景分别在 60/120/144/165/240 Hz 测 p50/p95/p99，而不是只看 FPS 平均值：

- [ ] Home 静止 10 s。
- [ ] Home 启动 dropdown 单次展开/收起。
- [ ] Home dropdown 20 次快速反向点击。
- [ ] 顶栏 tab 连续跨 1/3/5 项跳转。
- [ ] 页面切换 + 背景图 + glass blur。
- [ ] Modal 打开/关闭。
- [ ] Toast 多条进入/退出。
- [ ] CurseForge 列表滚动 + 图片异步加载。
- [ ] 大 Manage list 滚动。
- [ ] 窗口 resize。
- [ ] 最小化/恢复、失焦/重新激活。
- [ ] 60Hz 与高刷显示器之间移动窗口。

关键门槛：

- [ ] idle 时没有无理由 continuous render/submit/present。
- [ ] compositor-independent animation 的 View build/layout count = 0。
- [ ] Nav/chevron 等局部动画不重建 MainWindow/page body。
- [ ] 静态文本 shaping miss 接近 0，静态图片 decode/upload = 0。
- [ ] GPU upload bytes 与实际变化量相关，而不是 scene 总大小相关。
- [ ] 一次 CPU spike 不引起连续 degraded/full-redraw recovery。
- [ ] 快速反向 spring 无 plateau、无速度断层、无尾部空转。

---

## 主流框架参考与可借鉴原则

> 只借鉴机制，不机械照搬线程数、batch 数阈值、layer 阈值或平台 API。

### Qt Quick / Scene Graph

- Scene graph renderer：<https://doc.qt.io/qt-6/qtquick-visualcanvas-scenegraph-renderer.html>
- Scene graph / threaded render loop：<https://doc.qt.io/qt-6/qtquick-visualcanvas-scenegraph.html>
- Threaded animation example：<https://doc.qt.io/qt-6/qtquick-scenegraph-threadedanimation-example.html>
- Performance：<https://doc.qt.io/qt-6/qtquick-performance.html>

可借鉴：稳定 retained render tree、GPU geometry retention、有效 batching、render-thread animation、animation driver 与 presentation cadence 对齐。

### Chromium cc / Viz

- How cc Works：<https://chromium.googlesource.com/chromium/src/+/refs/heads/main/docs/how_cc_works.md>
- Life of a frame：<https://chromium.googlesource.com/chromium/src/+/refs/heads/main/docs/life_of_a_frame.md>

可借鉴：BeginImplFrame/BeginMainFrame 分层、main/pending/active tree、没有 main-thread damage 就跳过 main lifecycle、deadline/high-latency recovery、compositor thread 独立处理安全动画。

### Jetpack Compose

- Performance：<https://developer.android.com/develop/ui/compose/performance>
- Phases and performance：<https://developer.android.com/develop/ui/compose/performance/phases>

可借鉴：composition/layout/draw 阶段独立跳过、restartable/skippable scope、延迟读取高频 state、稳定 key，避免一个 state read 让大父树每帧 recomposition。

### Flutter

- Rendering performance：<https://docs.flutter.dev/perf/rendering-performance>

可借鉴：明确 layout/paint 边界、局部 repaint/raster cache、谨慎使用昂贵 opacity/saveLayer/filter，不把小动画扩散成大区域重绘。

### Windows Composition

- Microsoft.UI.Composition：<https://learn.microsoft.com/windows/windows-app-sdk/api/winrt/microsoft.ui.composition>

可借鉴：Visual/effect/animation 由 compositor 管理，应用 UI tree 不需要为了 compositor-safe transform/opacity 每帧完整重建。

---

## 最终架构目标

理想状态下，一次普通 transform/opacity 动画帧应接近：

```text
Display/BeginFrame
  -> sample frame-stable animation clock
  -> patch a few animation values
  -> calculate small swept damage
  -> reuse static scene / GPU buffers
  -> encode minimal pass
  -> submit
  -> present
```

而不应该是：

```text
animation tick
  -> cx.notify(AppChromeView/Home/MainWindow)
  -> render large View tree
  -> recreate hundreds of Taffy nodes
  -> measure/text/cache lookups
  -> rebuild scene
  -> broad upload
  -> blur/full redraw
  -> present
```

真实 geometry 动画仍然允许进入 layout，但必须保证其 invalidation 范围与 dependency graph 精确；这才是 BMCBL 当前 GPUI 分支从“能 retained replay”继续走到“高刷下稳定、低延迟、低占用”的主要方向。
