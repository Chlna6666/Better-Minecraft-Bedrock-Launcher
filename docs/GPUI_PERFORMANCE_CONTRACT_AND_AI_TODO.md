# GPUI 性能契约、架构目标与本地 AI 执行 TODO

> 本文与 [`GPUI_PERFORMANCE_AUDIT_TODO.md`](./GPUI_PERFORMANCE_AUDIT_TODO.md) 配套使用。
>
> - `GPUI_PERFORMANCE_AUDIT_TODO.md`：记录现状、问题、证据、热点、profiling 结果与具体待办。
> - 本文：规定 **什么改动才算正确的性能优化**、GPUI/Nova 应满足什么长期架构契约、本地 AI 应按什么顺序和标准执行任务。
>
> 本文不是“把所有动画 GPU 化”的计划，也不是“为了 benchmark 删除效果”的计划。性能优化的核心目标是：**让每一类变化只经过它真正需要经过的 pipeline 阶段，同时保持视觉、交互、可访问性、命中测试与生命周期语义正确。**

---

## 0. 给本地 AI 的最高优先级规则

以下规则优先级高于本文后续任何具体 TODO。任何 AI 在修改本仓库 GPUI / Nova / UI 动画前都必须先阅读并遵守。

### 0.1 禁止用“少做功能”冒充性能优化

不得通过以下方式让 profiler 数字变好：

- [ ] 删除动画、关闭动画、减少功能。
- [ ] 无依据地把动画从 120/144/240 Hz 降到 60 Hz。
- [ ] 缩短动画时长使问题“看起来不明显”。
- [ ] 把动态内容改成静态图。
- [ ] 禁止 blur / shadow / opacity / transition，而没有解决其真正成本来源。
- [ ] 用 `sleep`、节流、debounce 掩盖错误的 invalidation storm。
- [ ] 因为某个路径慢就无条件跳过它，破坏状态一致性。

如果产品语义确实允许降低更新频率，必须明确证明“视觉状态本身是离散低频状态”，例如 Minecraft `§k` 每 16 ms 才生成新字符，此时不需要在 240 Hz 屏幕上每 4.17 ms 重算同一个离散状态。这种情况属于 **cadence 与内容真实更新频率对齐**，不是偷降帧率。

### 0.2 禁止“GPU = 一定更快”

GPU/compositor 不是默认答案。

只有当以下条件基本成立时，才考虑把动画转为 presentation/compositor-only：

1. 几何布局结果本身不需要变化；
2. parent/sibling measure 不依赖该动画值；
3. hit-test/semantic geometry 可以明确保持正确；
4. transform/opacity 可以由 retained primitive/subtree 在 renderer 中稳定重放；
5. 不会因为新增 offscreen texture、raster pass、composite layer、buffer upload、pipeline switch 造成更高总成本；
6. 不会导致 Text/Image/Path/Surface 等不同 primitive 类型不同步。

以下变化原则上继续走 CPU layout/placement，除非已经设计出正确的替代语义：

- `width / height` 真正影响 sibling/parent layout；
- flex/grid 分配变化；
- 文本 reflow；
- intrinsic size 变化；
- scroll extent 变化；
- 命中区域必须跟真实几何同步变化；
- 3D/mesh 骨骼本身在 CPU 构建。

### 0.3 禁止无脑 `composite_layer()` / offscreen promotion

对 14px chevron、单个小图标、小文字做旋转时，不得仅因为“GPU 动画”就创建独立 offscreen layer。

任何 layer promotion 必须说明：

- layer 面积；
- raster 分辨率；
- 每帧是否重新 raster；
- VRAM 占用；
- upload 带宽；
- 新增 render pass 数；
- batch break；
- 是否能复用 texture；
- 相比 CPU 重画到底省了什么。

如果不能证明收益，保持 CPU primitive 更新通常更正确。

### 0.4 不允许扩大 invalidation 范围换代码简单

禁止通过以下方式修复局部动画：

- `window.request_animation_frame()` 每帧驱动整个 Window；
- 在 root `cx.notify()` 只为了刷新一个 child；
- `force_full_redraw = true` 作为常态；
- 每帧 `force_view_cache_refresh = true`；
- 给整个页面套 `with_layout_animation_target(true)`；
- 一个 spinner 导致整个 card/page layout target；
- 一个 dropdown chevron 导致整个 overlay/root render。

如果真正变化只有一个 subtree，那么 dirty、layout、paint、scene replay 与 frame scheduling 都应尽量收敛到该 subtree。

### 0.5 任何性能改动必须保持视觉语义

优化前后必须保持：

- easing 曲线；
- 动画持续时间；
- overshoot / spring 行为；
- opacity；
- translation / scale；
- clipping；
- z-order；
- blur 视觉；
- shadow；
- pointer hit-test；
- focus/keyboard；
- modal dismiss 生命周期；
- retained subtree identity；
- 动画完成时的最终精确值。

如果优化方案需要改变视觉，必须把它作为单独产品/UI 变更，不得混入 `perf` 提交。

---

## 1. 性能架构的核心 Contract

### 1.1 Invalidation Contract

GPUI 的长期目标必须明确区分“谁发生了变化”与“为了找到它必须经过谁”。

建议基础语义：

```rust
pub enum DirtyScope {
    Direct,
    TraversalAncestor,
}

bitflags! {
    pub struct InvalidationKind: u16 {
        const STATE      = 1 << 0;
        const MEASURE    = 1 << 1;
        const LAYOUT     = 1 << 2;
        const PLACEMENT  = 1 << 3;
        const PAINT      = 1 << 4;
        const TRANSFORM  = 1 << 5;
        const OPACITY    = 1 << 6;
        const HIT_TEST   = 1 << 7;
        const SEMANTICS  = 1 << 8;
        const CHILD_ROUTE = 1 << 9;
    }
}
```

这只是语义示例，不要求照搬类型名。真正 Contract 是：

- [ ] `DirectDirty`：状态/输入真的变化的 owner。
- [ ] `TraversalAncestor`：只为了 retained reconciliation/dispatch/tree traversal 经过的父节点。
- [ ] `TraversalAncestor` 不得默认等价于重新 `Render`。
- [ ] `TraversalAncestor` 不得默认等价于 layout dirty。
- [ ] `TraversalAncestor` 不得默认等价于 paint dirty。
- [ ] dirty reason 必须可观察，不能只有一个 bool。

### 1.2 Layout Contract

layout invalidation 至少应区分三类：

```text
Measure Dirty
  ↓ 可能影响自身 intrinsic size / parent constraints
Layout Dirty
  ↓ 需要重新求子布局
Placement Dirty
  ↓ 仅位置改变，不影响 sibling/parent measure
```

长期原则：

- [ ] `left/top` 对 absolute child 的变化，如果 parent size 与 sibling 分布不依赖它，应是 placement dirty。
- [ ] transform-only 变化不进入 Taffy。
- [ ] opacity 不进入 Taffy。
- [ ] measure closure 仅在输入真正变化时运行。
- [ ] parent constraints 未变化时，不应因为无关 sibling 动画而重新 measure 静态 child。
- [ ] persistent layout identity 不应依赖 frame-local node ID。

### 1.3 Scene/Renderer Contract

静态 scene 应最大化 retained replay，而不是“每帧重新 encode 但 GPU 很快”。

必须能够回答：

```text
这一帧哪些 primitives 真正重新生成？
哪些直接 replay？
哪些 buffer 被重新 upload？
哪些 batches 因为什么原因断开？
哪些 textures/pass 是新增的？
```

目标：

- [ ] presentation-only transform/opacity 更新时，静态文字、图片、背景、shadow 不重新 build。
- [ ] scene replay 不能因为一个 animation slot 导致整个 batch 重新编码。
- [ ] batch split 必须有真实 backend 收益，否则不要提前拆批。
- [ ] 不支持某 primitive 的 GPU 动画时，必须安全 fallback，而不是出现 primitive desync。

### 1.4 Scheduler Contract

一帧为何被请求必须可解释。

建议 reason：

```rust
pub enum FrameRequestReason {
    Input,
    StateNotify,
    LayoutAnimation,
    PresentationAnimation,
    ProgressiveWork,
    ImageReady,
    AsyncCompletion,
    Timer,
    Recovery,
    ExplicitRedraw,
}
```

Contract：

- [ ] 静态窗口没有 reason 时必须进入真正 idle。
- [ ] 同一个 retained target 同一 deadline 不重复堆 timer。
- [ ] settled animation 不继续请求 frame。
- [ ] presentation-only animation 不应该迫使 main-thread build/layout。
- [ ] progressive work 不得无限抢占 interactive frame。
- [ ] recovery 不得无条件升级 full rebuild。

### 1.5 Windows Presentation Contract

Windows 下只允许存在一个逻辑清晰的 pacing authority。

不得同时出现多个互相独立的：

- timer cadence；
- `request_animation_frame` cadence；
- DXGI waitable object cadence；
- present feedback cadence；
- winit redraw cadence；
- custom backpressure cadence；

然后彼此“谁先醒谁画”。

目标：

- [ ] frame schedule 基于真实 refresh/present feedback。
- [ ] 不产生稳定 2× refresh interval 的 cadence aliasing。
- [ ] main thread 超时不阻塞已提交 compositor-only animation。
- [ ] 120/144/165/240 Hz 均保持稳定 pacing。

---

## 2. 必须补齐的可观测性

在大改 pipeline 前，优先完成 instrumentation。没有证据时禁止大范围“优化”。

### 2.1 Dirty provenance

至少记录：

```text
entity/type
DirectDirty or TraversalAncestor
reason
source_location / caller
first timestamp
frame id
```

TODO：

- [ ] `direct_dirty_count`
- [ ] `ancestor_only_dirty_count`
- [ ] `dirty_reason_count_by_kind`
- [ ] `view_render_count_by_type`
- [ ] `view_render_cpu_time_by_type`
- [ ] `first_direct_dirty_entity`
- [ ] `first_traversal_ancestor`
- [ ] `notify_source_location`

### 2.2 Layout provenance

TODO：

- [ ] `measure_dirty_nodes`
- [ ] `layout_dirty_nodes`
- [ ] `placement_dirty_nodes`
- [ ] `measure_closure_calls`
- [ ] `constraint_changed_nodes`
- [ ] `layout_root_hit/miss`
- [ ] root miss 第一处 fingerprint divergence
- [ ] bounds miss reason
- [ ] stable subtree reused node count

### 2.3 Frame provenance

TODO：

- [ ] 每帧 request reason bitset。
- [ ] 每类 reason 第一个 caller。
- [ ] requested frame → actually presented frame 的链路 ID。
- [ ] animation active count。
- [ ] animation settled-but-still-requesting count，必须长期为 0。
- [ ] pending timed retained target 数。

### 2.4 Scene/Nova provenance

TODO：

- [ ] `scene_new_primitives`
- [ ] `scene_replayed_primitives`
- [ ] `scene_reencoded_primitives`
- [ ] `uploaded_bytes_by_primitive_type`
- [ ] `staging_buffer_allocations`
- [ ] `buffer_growth_events`
- [ ] `batch_break_reason`
- [ ] `draw_call_count`
- [ ] `render_pass_count`
- [ ] `offscreen_surface_count`
- [ ] `offscreen_pixels_rasterized`
- [ ] `blur_source_damage_pixels`

建议 batch break reason 至少能表达：

```text
pipeline
material
texture
clip
z/order barrier
animation ownership
blend mode
render target
primitive type
buffer capacity
```

### 2.5 Allocation provenance

对高频 UI render 开 debug-only allocation instrumentation：

- [ ] alloc count/frame
- [ ] allocated bytes/frame
- [ ] Vec growth count
- [ ] String allocation
- [ ] SharedString conversion
- [ ] Arc creation
- [ ] HashMap growth

不要直接把“allocation=0”作为所有路径目标；重点是**静态/重复帧不应重复分配同样的 immutable model**。

---

## 3. AnimationExecutionClass：所有动画必须分类

新增动画前，AI 必须先给动画分类，再决定 API。

建议语义：

```rust
pub enum AnimationExecutionClass {
    LayoutDependent,
    PlacementOnly,
    PaintOnly,
    PresentationOnly,
    ContentCadence,
    ExternalScene,
}
```

不要求立即引入同名 enum，但任何 animation API 必须有明确对应语义。

### 3.1 LayoutDependent

典型：

- accordion 高度推动后续内容；
- tab 内容尺寸变化；
- dropdown 真正展开后改变周围布局；
- width 改变 sibling flex 分配；
- 文本容器宽度变化引起 reflow。

要求：

- [ ] CPU layout。
- [ ] dirty 范围限制在 relayout boundary。
- [ ] 不因此重建无关 root。

### 3.2 PlacementOnly

典型：

- absolute nav pill 的 `left` 改变；
- overlay panel 位置改变但不参与 parent size。

要求：

- [ ] 不重新 measure。
- [ ] 尽量不重新布局无关 sibling。
- [ ] 可选择 CPU placement 或 renderer transform，必须依据 cost/correctness。

### 3.3 PaintOnly

典型：

- color；
- border alpha；
- 简单 path rotation（若 CPU primitive 更新更便宜）；
- 无 geometry 变化的 visual state。

要求：

- [ ] 不进入 layout。
- [ ] 不强制 View root rebuild。

### 3.4 PresentationOnly

典型：

- retained card 的 translation；
- retained modal content 的 scale/opacity；
- toast translate + opacity。

要求：

- [ ] scene/subtree identity 稳定。
- [ ] 主 scene 不因每个 tick 重新构建。
- [ ] renderer 能对 subtree 内所有需要同步的 primitive 正确变换。
- [ ] clip/hit test 语义闭合。

### 3.5 ContentCadence

典型：

- Minecraft `§k` 每 16 ms 生成新 glyph 内容；
- 秒表每 1s 改数字；
- 低频状态灯每 N ms 改状态。

要求：

- [ ] 按内容真实变化 interval 请求 retained frame。
- [ ] 不跟显示器 refresh 绑定重复计算相同状态。

### 3.6 ExternalScene

典型：

- map 3D preview；
- skin model；
- custom mesh canvas；
- audio visualization 等自定义 renderer。

要求：

- [ ] 其 frame cadence 与 UI layout cadence分离。
- [ ] UI widget wrapper 不得因内部 mesh 变化全量 rebuild。
- [ ] GPU/CPU mesh 更新依据真实 scene dirty。

---

## 4. 动画 API 设计 TODO

现有动画 API 后续应朝“语义化 execution class”发展，而不是调用者自己拼各种 frame driver。

### 4.1 保留/完善 retained frame target

当前已有：

- `with_layout_animation_target(...)`
- `with_layout_animation_target_interval(...)`
- sampled/stable sampled animation
- renderer scene animation infrastructure

TODO：

- [ ] API 文档明确每个 wrapper 会触发哪些阶段。
- [ ] debug build 下检测重复 frame driver：同一 subtree 同时被 root notify、layout target、sampled animation 三重驱动时输出 warning。
- [ ] 动画 target debug overlay 显示 owner、execution class、cadence、剩余 duration。
- [ ] stable sampled animation key collision/identity 加 debug assertion。

### 4.2 不要叠加多个 cadence owner

一个动画原则上只应有一个 cadence owner。

错误例：

```text
View cx.notify per tick
+ with_layout_animation_target
+ sampled animation requests frame
+ child spinner requests frame
```

正确目标：

```text
一个 owner 请求下一帧
→ subtree 被精准 invalidated/replayed
→ 子属性从同一 progress 采样
```

TODO：

- [ ] 建立 duplicate animation driver telemetry。
- [ ] 对 dropdown、modal、auth panel、nav pill 做逐个审计。

### 4.3 动画完成必须自动 quiesce

每个动画 API 都必须保证：

- [ ] progress=1 后不再 request frame。
- [ ] close/disappear 完成后最多只需要状态提交/移除所需的一次最终 frame。
- [ ] steady state 不因为 `disappear_t == 0`、`elapsed == duration` 等边界继续请求帧。
- [ ] 添加 settled state regression test。

---

## 5. View Scope / Composition Scope TODO

高频状态读取必须下沉到真正消费它的 View/subtree。

### 5.1 `AppChromeView`

目标拆分：

```text
AppChromeView
├── BrandChromeView
├── NavChromeView
├── AuthChromeView
└── WindowControlsView
```

TODO：

- [ ] Nav spring tick 只进入 `NavChromeView`。
- [ ] Auth 状态只进入 Auth scope。
- [ ] Update badge/version 变化只影响对应 scope。
- [ ] Theme geometry-neutral color change 不让整个 chrome layout。
- [ ] window active 状态只更新需要改变颜色/按钮状态的 scope。

### 5.2 View 拆分原则

不要按“视觉上是一张卡片”机械拆 View。优先按以下维度拆：

1. 更新频率；
2. state dependency；
3. layout dependency；
4. retained scene boundary；
5. async 生命周期；
6. hit-test/focus ownership。

如果父 child 每次都同时变化，拆 View 可能反而增加 bookkeeping。

### 5.3 静态 model 与高频 render 解耦

TODO：

- [ ] plugin navigation pages versioned snapshot。
- [ ] built-in nav metadata 静态化。
- [ ] app version string 预计算。
- [ ] 不在 animation render tick 重复 `format!` immutable string。
- [ ] theme derived constants 可按 theme revision memoize。
- [ ] menu option metadata 与动画 progress 分离。

---

## 6. Taffy / Persistent Layout TODO

这是 CPU generation 的核心长期项目，不应只靠 root fingerprint 缓存。

### Phase L0：先补证据

- [ ] root miss divergence telemetry。
- [ ] child constraints changed telemetry。
- [ ] measure closure callsite telemetry。
- [ ] placement-only candidate telemetry。

### Phase L1：dirty class

- [ ] `needs_measure`
- [ ] `needs_layout`
- [ ] `needs_placement`

传播规则必须明确：

```text
content/intrinsic size change
  -> measure dirty
  -> parent may become measure/layout dirty

parent constraints change
  -> child layout/measure dirty depending on child type

absolute left/top change
  -> placement dirty only if no intrinsic dependency

transform/opacity
  -> no Taffy dirty
```

### Phase L2：persistent identity

设计目标：

```rust
struct LayoutInstanceId(...);
```

- [ ] identity 跨帧稳定。
- [ ] frame-local `LayoutId` 只作为 ephemeral handle。
- [ ] persistent node 保存上次 constraints/fingerprint/result。
- [ ] stable child order 不重新创建全部 metadata。

### Phase L3：subtree reuse

- [ ] 可复用 persistent subtree layout result。
- [ ] parent 只在必要依赖改变时 propagation。
- [ ] absolute overlay 不污染 page body root cache。
- [ ] animation target 形成 layout boundary。

### Phase L4：评估与 Taffy 的职责边界

不要维护两套重复 layout engine。

必须回答：

- GPUI persistent node 保存什么？
- Taffy node 是否长期存在？
- 若每帧重建 Taffy node，persistent metadata 如何映射？
- Taffy 自身 cache 是否与 GPUI cache重复？
- measured node ownership 在哪一层？

没有架构答案前，不要不断叠新 HashMap cache。

---

## 7. Retained Scene / Nova TODO

### 7.1 第一原则：先减少 CPU scene generation，再谈 shader 微优化

当一帧只有几十个 primitive，却 CPU build/layout 8 ms 时，先修 invalidation/layout。

只有下列指标显示 renderer 是热点时再做 Nova 优化：

- scene encode CPU 明显高；
- upload bytes 高；
- render pass 过多；
- draw call/batch break 高；
- GPU busy 高；
- overdraw/offscreen 明显。

### 7.2 Batch

- [ ] batch break reason telemetry。
- [ ] animation 不得无 backend 收益地强制 split。
- [ ] texture/material/clip state 尽量连续。
- [ ] 不要为了“所有 animated primitive 独立”破坏普通静态 batch。

### 7.3 Upload

- [ ] staging buffer reuse。
- [ ] 避免每帧 Vec/Buffer grow。
- [ ] primitive data 仅变化的 segment 更新。
- [ ] 大 scene 评估 dirty-range upload。
- [ ] 小 scene 不要引入复杂 partial upload 反而提高 CPU bookkeeping。

### 7.4 Path GPU 动画：暂停在“有证据再做”

当前 Path 路径的重要事实：

- Path 已具有 `animation_id` scene metadata；
- Nova Path 会先 rasterize 到 screen-space intermediate texture，再画 PathSprite；
- 如果仅移动 sprite bounds，但 UV 也跟 animated bounds 一起变，会采样错误/空区域；
- 正确设计需要区分 **base raster bounds** 与 **animated presentation bounds**；
- opacity 可乘 premultiplied RGBA；
- rotation 不能简单套普通 bounds transform；
- raw Path rotation 当前不应为了统一 API 强行实现。

TODO 只有在 profiling 证明 Path presentation 是真实热点后再继续：

- [ ] 定义 base bounds / presentation bounds。
- [ ] 明确 UV 保持 base raster coordinates。
- [ ] 验证 clip/damage/swept bounds。
- [ ] 测量增加 PathSprite bytes 后的 bandwidth。
- [ ] 测量 batch fragmentation。
- [ ] 对比 CPU rotate path vertices 的实际成本。

如果收益不明显：保持 CPU Path 更新。

---

## 8. Composite / Offscreen Layer Cost Model

任何新增 retained composite layer 前，AI 必须填写：

```text
Layer name:
Logical size:
Physical pixel size:
Estimated bytes:
Raster frequency:
Can texture be reused?:
Extra render passes:
Extra sampling:
Expected CPU work removed:
Expected upload removed:
Batch impact:
Clip correctness:
Hit-test correctness:
Break-even estimate:
```

### 推荐 promotion 场景

- 大 subtree 内容稳定，但整体 translation/scale/opacity 高频变化；
- raster 成本高且可多帧复用；
- modal/card/toast 等明确整体 motion；
- 不需要 subtree 内部动态更新。

### 不推荐 promotion 场景

- 单个小 SVG chevron；
- 小 spinner；
- 每帧内容本身都变化的 subtree；
- layer 面积接近全窗口，只为了轻微 opacity；
- blur source 每帧变化导致必须重 raster；
- 一次性很短的小动画但 promotion setup 成本更高。

---

## 9. Backdrop Blur Contract

blur 是 dependency-based effect，必须按 source damage 更新。

目标：

```text
blur output dirty
= blur parameters changed
OR source pixels under blur sampling footprint changed
OR blur geometry changed
```

不得：

```text
因为 modal animation active
=> 每帧重新 blur 整个窗口
```

TODO：

- [ ] blur node 记录 sampling footprint。
- [ ] source damage 与 blur footprint 相交才 refresh。
- [ ] blur radius 固定时缓存 kernel/pipeline state。
- [ ] overlay alpha 动画不让 blur source重算。
- [ ] backdrop geometry 不变时 scale/opacity card 不刷新 backdrop。
- [ ] telemetry：`blur_refresh_reason` + `blur_source_damage_pixels`。

验收：

- [ ] modal card scale animation，静态背景时 backdrop blur 不重复昂贵重建。
- [ ] 背景真的更新时 blur 正确刷新。

---

## 10. Progressive Rendering / Recovery Contract

### 10.1 工作类别

建议至少区分：

```rust
pub enum FrameWorkClass {
    InteractiveMain,
    PresentationOnly,
    BackgroundProgressive,
    Recovery,
}
```

### 10.2 deadline

禁止永久固定 `4ms` 作为所有显示器/所有工作统一 deadline。

目标依据：

```text
predicted_present_time
- estimated GPU tail
- platform safety margin
- already elapsed generation time
= remaining CPU deadline
```

### 10.3 degraded recovery

局部超时不代表 retained scene 全部不可信。

TODO：

- [ ] 记录 phase timeout 发生位置。
- [ ] 保留已完成 immutable subtree。
- [ ] resumable progressive work。
- [ ] recovery 升级 full redraw 必须有明确 `FullRedrawReason`。
- [ ] `force_view_cache_refresh` 必须有明确 reason。
- [ ] hysteresis 防止 `degraded -> full -> degraded` 振荡。
- [ ] presentation-only animation 可在旧 main scene 上继续。

建议：

```rust
pub enum FullRedrawReason {
    SurfaceLost,
    DeviceRecovery,
    RetainedStateCorrupt,
    GlobalScaleChanged,
    ExplicitDebug,
    // 不应有“只是上一帧超时”这种泛化原因
}
```

---

## 11. Idle Contract

真正静止窗口必须可以完全 idle。

### Idle 目标

没有 input、timer、动画、async completion、progressive work 时：

```text
frame requests = 0
view renders = 0
layout nodes = 0
scene encode = 0
uploads = 0
GPU submit = 0
present = 0（除平台明确要求）
```

TODO：

- [ ] idle watchdog debug telemetry。
- [ ] 如果静止 2s 仍在 frame loop，输出所有 active frame reasons。
- [ ] 输出所有 active animation target。
- [ ] 输出未完成 timers/progressive jobs。
- [ ] 输出导致 submit 的 first dirty primitive。

这比看 Task Manager 的 “GPU 3D” 百分比更有诊断价值。

---

## 12. Async / Resource / Image Contract

### 12.1 Cache owner 必须唯一

例如图片加载：

```text
UI list render
  -> cache.load(uri)
  -> cache owns async task
  -> completion schedules exact dirty target
```

不得：

```text
UI prefetch task
  -> cache.load()
  -> cache内部又 spawn
  -> 外部再 spawn/notify
  -> signature 每次 render 变化
  -> re-entry storm
```

TODO：

- [ ] async owner 唯一。
- [ ] cache miss/hit/pending reason telemetry。
- [ ] completion 只 dirty 真正消费资源的 scope。
- [ ] virtualized list 只 prefetch 可见区 + bounded lookahead。
- [ ] URL/signature 只在 inputs revision 改变时重建。
- [ ] cancellation 不保留无效 owner wakeup。

### 12.2 Cache Miss Reason

建议：

```rust
pub enum CacheMissReason {
    FirstUse,
    KeyChanged,
    SourceRevisionChanged,
    Evicted,
    CapacityPressure,
    ExplicitInvalidation,
    DeviceReset,
}
```

cache miss 必须可解释，不能只有一个 miss counter。

---

## 13. Text Contract

文字优化优先级取决于真实 miss。

若：

```text
text_layout_misses = 0
```

则不要先重写 shaping engine。

TODO：

- [ ] 区分 shape cache / line layout / glyph atlas / raster miss。
- [ ] stable text 不重复创建完整 String。
- [ ] immutable parsed representation 使用 cheap clone（SharedString/Arc 等）。
- [ ] Minecraft 格式文本 cache 命中时不深拷贝 run Vec。
- [ ] `§k` 属 ContentCadence，不随 display refresh 重算。
- [ ] glyph atlas eviction 需 telemetry。

验收：

- [ ] 静态文本动画邻居变化时不 reshape。
- [ ] cache hit 不产生大对象 clone。

---

## 14. 当前 UI 动画逐项执行策略

本节是给本地 AI 的直接决策参考。

### Dropdown popup

当前策略：保留稳定 sampled reveal/opacity，不再额外叠 scene animation owner。

TODO：

- [ ] 检查 popup 是否仍有 duplicate cadence owner。
- [ ] height/reveal 如果是真 layout geometry，保留 CPU。
- [ ] popup 的静态内容在 reveal 时尽量 retained。
- [ ] chevron 单独处理，不让 trigger/root 跟 popup共享错误 invalidation。

### Dropdown chevron

默认继续 CPU Path rotation。

原因：

- 图标很小；
- Path 顶点少；
- composite layer 可能比 CPU rotate 更贵；
- Nova raw Path rotation 语义未闭合。

TODO：

- [ ] target 只包 chevron/icon scope。
- [ ] 不包 trigger 全 subtree。
- [ ] profiling 证明 CPU Path rotation 热点前不做 GPU promotion。

### Tabs indicator

当前实际 `left + width` 几何动画继续 CPU layout/placement。

TODO：

- [ ] 识别是否可转 placement-only + width local layout。
- [ ] 不让 tab labels 重新 measure。
- [ ] indicator bounds change 不 dirty content page。

### Main nav pill

同 Tabs indicator。

- [ ] parent/sibling measure稳定时减少到 placement/local geometry。
- [ ] 不改变视觉曲线。
- [ ] 不因 nav spring tick 重建 Brand/Auth/WindowControls。

### Xbox auth panel

当前 stable sampled scale/opacity 已去掉重复 layout driver。

TODO：

- [ ] 验证 mixed primitive 是否同步。
- [ ] 验证 scene replay 与 layout count。
- [ ] 不额外 composite promotion，除非 profile 有收益。

### Xbox account rows

当前 layout target 已收窄到 rows container。

TODO：

- [ ] 进一步确认 row removal/selection 是否需要 measure。
- [ ] 静态 header/count/hint 保持 retained。

### Dismissible modal

当前连续 frame target 已收窄到 modal subtree。

TODO：

- [ ] close click 的 one-shot root frame 若未来有 targeted event kick API，可进一步去掉。
- [ ] 不改变 dismiss lifecycle。
- [ ] backdrop blur 只按 source damage。

### Toast

当前 translate/opacity 走 retained composite/stable sampled 路径。

TODO：

- [ ] steady toast 不请求动画帧回归测试保留。
- [ ] 多 toast stacking 真实 layout 变化继续 CPU。
- [ ] 单 toast presentation motion 不让 overlay 全 layout。

### Minecraft `§k`

当前 16 ms retained layout cadence 合理。

TODO：

- [ ] 验证多个 `§k` target deadline 合并/去重。
- [ ] 避免每个 run 独立 timer storm。
- [ ] 静态格式文本不受影响。

### Import spinner

已知高优先级局部优化：

- [ ] `src/ui/window/import/view.rs` 中 `self.render_preview_card(...).with_layout_animation_target(self.is_inspecting)` 范围过大。
- [ ] 将 target 下沉到 spinner/icon 本身或最小 icon container。
- [ ] 不需要 GPU layer。
- [ ] preview card 其它静态文本/按钮不得每帧 layout。

### Skin preview / Map viewer

属于 ExternalScene。

- [ ] 外部 mesh/canvas cadence 不扩大到普通 UI wrapper。
- [ ] progressive image/mesh budget 允许自己请求后续帧，但 reason 必须明确。
- [ ] 不因相机 motion 重新 build整个 surrounding UI。

---

## 15. 禁止模式清单

本地 AI 发现以下模式时必须先说明是否构成问题，再改：

```rust
window.request_animation_frame(); // 出现在连续动画 loop
cx.notify();                      // root View 高频 tick
with_layout_animation_target(true) // 包住超大 subtree
force_full_redraw = true;         // 普通动画/超时恢复
force_view_cache_refresh = true;  // 普通动画/超时恢复
format!(...)                      // 高频 immutable UI render
collect::<Vec<_>>()               // 每 tick 构造稳定 metadata
Arc::new(...)                     // 每 tick 重建稳定 snapshot
composite_layer()                 // 小图标/小 path 无 cost model
```

注意：这些 API 本身不是错误。禁止的是**没有对应语义和 cost justification 的使用**。

---

## 16. 本地 AI 执行协议

每次开始一个性能任务前，AI 必须先输出下面模板并自行填写，然后才允许改代码。

```markdown
## Task Contract

任务：
涉及路径：
当前 master HEAD：

### 证据
- profiler/log：
- 代码链路：
- 为什么这是热点：

### 当前 pipeline
state/input
-> ...
-> present

### 目标 pipeline
state/input
-> ...
-> present

### 根因

### 准备修改

### 必须保持
- visual：
- layout：
- hit-test：
- lifecycle：
- async：

### 预期减少的工作
- CPU build：
- CPU layout：
- CPU scene encode：
- allocations：
- GPU upload：
- draw/pass：

### 新增成本
- bookkeeping：
- memory：
- GPU buffer：
- offscreen：
- code complexity：

### 指标
优化前：
优化后目标：

### 验证
- 静态检查：
- cargo：
- 单测：
- 手动 UI：
- profiler：

### 回退条件

### TODO 状态
[ ] 未实现
[?] 已实现但需要本地 benchmark/运行验证
[x] 已验证完成
```

### 16.1 `[x]` 的规则

只有满足以下条件才能标 `[x]`：

- 代码已落地；
- 相关静态语义已确认；
- 如果任务依赖运行性能结论，已经本地运行/benchmark；
- 没有已知 regression。

如果当前环境没有 cargo/toolchain/GUI runtime，只能：

```text
[?] 代码已实现，等待本地编译/运行验证
```

不得声称 `cargo check/test/build/clippy` 通过。

### 16.2 提交规则

- 直接 `master`，不创建无意义分支。
- 每次写前重新读取最新 `master HEAD`。
- 如果 HEAD 变化，重新读取目标文件并 merge，不覆盖并发提交。
- 中文 Conventional Commit。
- 提交信息带 `[skip ci]`。
- 一个 commit 尽量只解决一个 coherent performance problem。
- 不把无关格式化、大规模 rename 混入 perf commit。

示例：

```text
perf(gpui): 区分直接失效与祖先遍历 [skip ci]
perf(ui): 收窄导入预览转圈动画范围 [skip ci]
feat(gpui): 添加帧请求来源遥测 [skip ci]
```

---

## 17. Benchmark / Profiling 场景矩阵

本地 AI 完成核心架构任务后，应按固定场景比较，而不是随意点击。

### 场景 A：完全 idle

持续 5s 不操作。

观察：

- frame requests/s
- render/s
- layout/s
- scene encode/s
- upload/s
- present/s
- CPU usage
- GPU busy

目标：接近真正 0 工作。

### 场景 B：Nav pill 连续切换

观察：

- `MainWindowView` render count
- `AppChromeView` render count
- `NavChromeView` render count
- layout nodes
- measure calls
- scene replay ratio
- alloc/frame

### 场景 C：Dropdown open/close

观察：

- trigger/popup 两个 scope 是否独立。
- duplicate frame driver。
- popup layout nodes。
- chevron Path CPU cost。

### 场景 D：Modal open/close

观察：

- backdrop blur refresh count。
- underlying page render count。
- modal subtree scene replay。
- close 生命周期最终帧。

### 场景 E：Minecraft `§k`

在 60/120/144/240 Hz 下观察：

- content update ~16 ms cadence。
- 不随 240 Hz 变成 240 次/s CPU重算。
- deadline timer 不堆积。

### 场景 F：长列表快速滚动

观察：

- virtualization item count。
- allocation。
- async image cache pending/hit。
- upload bytes。
- text shaping miss。

### 场景 G：Map viewer / skin preview

观察：

- UI wrapper build/layout。
- custom scene CPU/GPU。
- camera motion 时 surrounding UI 是否 retained。

---

## 18. 性能预算参考

不要把以下数字当死规则，而是作为异常检测基线。

### 60 Hz

```text
frame interval ~16.67ms
```

### 120 Hz

```text
~8.33ms
```

### 144 Hz

```text
~6.94ms
```

### 165 Hz

```text
~6.06ms
```

### 240 Hz

```text
~4.17ms
```

CPU generation 不能占满整个 interval，因为还需要：

- event handling；
- render encode；
- GPU submit；
- GPU execution；
- present/pacing margin。

长期目标不是“CPU build 恰好小于 4.17ms”，而是让普通局部动画的 main-thread generation 足够小，使高刷下有稳定余量；presentation-only animation 更应避免 main-thread 重工作。

---

## 19. 负优化判定

出现任意以下情况时，优化必须重新评估甚至 revert：

- CPU 降 0.1ms，但新增每帧全窗口 offscreen raster；
- draw calls 大幅增加；
- upload bytes 翻倍；
- VRAM 常驻明显增加但 animation 极少发生；
- batch 被 animation ID 无意义拆散；
- idle GPU submit 增加；
- Path/Text/Image 同一 subtree 动画不同步；
- hit-test 与视觉位置错位；
- 视觉卡顿从 CPU 变成 GPU；
- 简单代码路径被复杂缓存 bookkeeping 取代，且没有 measurable gain；
- 每帧 cache hash/fingerprint 成本比原重算还高；
- 通过降低动画 fidelity 才得到收益。

必须允许 revert。错误优化不是“继续补更多 patch”才能接受。

---

## 20. 完整实施顺序

严格建议按以下阶段推进，除非 profiling 给出更强证据。

### Phase 0 — 建立 baseline 与 telemetry

- [ ] frame request reason
- [ ] dirty provenance
- [ ] render count by View
- [ ] layout miss divergence
- [ ] allocation telemetry
- [ ] batch break reason
- [ ] upload/pass metrics

### Phase 1 — Invalidation scope

- [ ] DirectDirty / TraversalAncestor
- [ ] reason bitset
- [ ] ancestor traversal 可跳过 render
- [ ] root notify storm 清理

### Phase 2 — View scope

- [ ] AppChromeView 按 dependency/frequency 拆分
- [ ] Import spinner target 收窄
- [ ] 继续审计 modal/dropdown/list spinner
- [ ] 静态 metadata snapshot

### Phase 3 — Layout dirty class

- [ ] measure/layout/placement 分离
- [ ] absolute child dependency rule
- [ ] measured closure skip
- [ ] retained layout boundary

### Phase 4 — Persistent layout identity

- [ ] `LayoutInstanceId`
- [ ] persistent constraints/result
- [ ] subtree reuse
- [ ] Taffy/GPUI cache职责收敛

### Phase 5 — Animation engine semantics

- [ ] AnimationExecutionClass
- [ ] one cadence owner
- [ ] compositor-only enforcement
- [ ] settled quiesce diagnostics

### Phase 6 — Scheduler / Windows pacing

- [ ] real deadline
- [ ] pacing authority
- [ ] progressive work class
- [ ] non-destructive recovery
- [ ] cadence aliasing metrics

### Phase 7 — Nova

只有前面 CPU pipeline 已明显改善后：

- [ ] upload reuse/dirty range
- [ ] batch fragmentation
- [ ] pipeline state churn
- [ ] offscreen cost
- [ ] Path animation feasibility benchmark

### Phase 8 — Blur / composite specialization

- [ ] dependency damage
- [ ] source footprint
- [ ] layer cost model
- [ ] promotion heuristics

### Phase 9 — 长期防回归

- [ ] perf lab scenarios
- [ ] debug assertions
- [ ] regression tests
- [ ] stable metrics snapshots
- [ ] 新 animation API review checklist

---

## 21. 新增动画 Review Checklist

以后新增任何动画，代码审查必须回答：

- [ ] 它属于哪种 `AnimationExecutionClass`？
- [ ] progress 由谁拥有？
- [ ] 下一帧由谁请求？
- [ ] 是否存在第二个 cadence owner？
- [ ] 动画值是否参与 measure？
- [ ] 是否真的需要 layout？
- [ ] dirty 最小 scope 是谁？
- [ ] parent 是否会被错误 notify？
- [ ] scene 是否可以 retained replay？
- [ ] 是否新增 composite layer？为什么值得？
- [ ] blur/source dependency 是否变化？
- [ ] settled 后谁停止 frame request？
- [ ] 240 Hz 下 CPU 做多少工作？
- [ ] hit-test 是否跟视觉一致？
- [ ] Path/Text/Image 是否同步？

任一问题回答不了，不应直接合并“性能优化版动画”。

---

## 22. 建议新增的框架级 Debug API

这些 API 名称仅是建议，可根据项目风格调整。

```rust
window.debug_frame_reasons()
window.debug_dirty_provenance()
window.debug_layout_dirty_nodes()
window.debug_active_animations()
window.debug_scene_replay_stats()
window.debug_batch_breaks()
window.debug_gpu_upload_stats()
```

也可以统一为：

```rust
struct FrameDiagnostics {
    frame_id: u64,
    requested_by: FrameRequestReasonSet,
    direct_dirty: usize,
    traversal_ancestors: usize,
    rendered_views: usize,
    measured_nodes: usize,
    layout_nodes: usize,
    placement_nodes: usize,
    scene_new: usize,
    scene_replayed: usize,
    upload_bytes: usize,
    draw_calls: usize,
    render_passes: usize,
    active_animations: usize,
    cpu_phase_times: ...,
}
```

要求 debug instrumentation 默认 release 零/低成本，不能因为 profiler 本身让 hot path 明显变慢。

---

## 23. 参考框架应该借什么，不应该借什么

### Windows Composition / WinUI

借：

- independent animation；
- compositor thread/presentation 与 UI thread 解耦；
- property animation 不必重走 layout。

不借：

- 不要因此把所有元素都 promotion 成独立 layer。

### Qt Quick

借：

- retained scene graph；
- QSGNode 稳定复用；
- render-thread Animator 类思路；
- geometry/material update 分离。

不借：

- 不需要复制 QML binding 系统。

### Flutter

借：

- build/layout/paint phase boundary；
- RenderObject dirty propagation；
- relayout/repaint boundary。

不借：

- 不要为了模仿 Flutter 重写整个 GPUI widget 模型。

### Jetpack Compose

借：

- restartable/skippable scope；
- state read 下沉；
- composition/layout/draw 独立 invalidation。

不借：

- 不需要实现完整 snapshot runtime 才能获得 scope 思路。

### Chromium

借：

- BeginMainFrame 与 compositor-only 更新分离；
- pending/active tree 思路；
- damage/recovery 有明确原因；
- presentation deadline/pacing。

不借：

- BMCBL UI 不需要浏览器级多进程复杂度。

### egui

借：

- 简单 immediate-mode 路径作为 CPU baseline；
- 小 UI 重新生成有时比复杂 cache便宜。

不借：

- 不要因此放弃 GPUI retained 优势。

### upstream Zed GPUI

借：

- API 简洁性；
- 已验证的生命周期设计。

BMCBL fork 的改动必须证明：

```text
新增复杂度 < 实际节省的 frame work
```

### WGPUI / retained layer 实验

借：

- renderer ownership / retained layer cost 思路。

不借：

- 不要看到 GPU/WGPU 就假定所有 UI 动画都应 raster layer。

---

## 24. 本地 AI 每轮任务结束时必须回写 TODO

完成代码后必须同步更新：

1. `GPUI_PERFORMANCE_AUDIT_TODO.md` 对应问题状态；
2. 如框架 Contract 改变，更新本文；
3. 写明 commit SHA；
4. 写明是否实际运行 cargo/test/UI；
5. 未运行则明确 `[?] awaiting local validation`。

建议每个已落地任务追加：

```markdown
#### 实施记录

- Commit: `<sha>`
- 状态: `[?] 已实现，等待本地验证`
- 变化:
  - ...
- 理论收益:
  - ...
- 新增成本:
  - ...
- 未验证项:
  - cargo check
  - runtime frame metrics
```

这样下一轮本地 AI 不会重新猜历史背景，也不会重复做已经证明是负优化的方案。

---

## 25. 当前明确暂停的方向

除非有新的 profiling 数据，否则暂时不要主动推进：

- [ ] “所有 Path 动画 GPU 化”。
- [ ] 为小图标建立独立 compositor texture。
- [ ] 每个 animation primitive 强制拆 batch。
- [ ] 为了统一 API 给 `Surface` 伪造 scene animation ABI。
- [ ] 大面积修改 shader 但 CPU frame generation 仍是主要瓶颈。
- [ ] 全局 layer cache / texture cache 大重构但没有 cache miss 证据。
- [ ] 每帧 shrink retained Vec capacity。

这些方向未来不是永久禁止，而是必须先有证据和完整 correctness design。

---

## 26. 当前下一批推荐任务

按风险/收益排序，本地 AI 可以直接从这里领取：

### Task 1 — Import spinner invalidation 收窄

- [ ] 审计 `src/ui/window/import/view.rs` 两处 broad `with_layout_animation_target(self.is_inspecting)`。
- [ ] 下沉到 spinner/icon minimum subtree。
- [ ] 保持 card geometry 与其它控件静态。
- [ ] 不增加 GPU/composite layer。
- [ ] 本地验证 inspecting 状态进出无闪烁。

### Task 2 — FrameRequestReason telemetry

- [ ] 为 frame invalidator 建 reason bitset。
- [ ] 对 `request_animation_frame` / retained deadline / dirty notify / async wakeup 标记来源。
- [ ] debug 日志输出 first reason/caller。
- [ ] release 路径低开销。

### Task 3 — DirectDirty / TraversalAncestor 原型

- [ ] 不立即大重构。
- [ ] 先并行记录两套集合。
- [ ] 验证现有 pipeline 哪些地方真的依赖 ancestor in `dirty_views`。
- [ ] 加 render-count telemetry。
- [ ] 证明 MainWindow ancestor 是否被重新 Render。

### Task 4 — layout root miss divergence

- [ ] 在 fingerprint mismatch 时记录第一个不同 child/property。
- [ ] 用 Nav pill/dropdown 复现。
- [ ] 根据证据决定 placement dirty 实现入口。

### Task 5 — AppChrome scope 拆分设计

- [ ] 先列 state dependency graph。
- [ ] 再逐个抽 child View。
- [ ] 一次 commit 不要同时拆全部。
- [ ] 首先拆更新最频繁的 Nav scope。

### Task 6 — Windows frame pacing telemetry

- [ ] 记录 requested timestamp / wake timestamp / CPU begin/end / submit / predicted present / actual feedback。
- [ ] 在 60/120/144/165/240 Hz 收集 interval histogram。
- [ ] 先证实是否存在 double pacing / 2× interval，再改 scheduler。

---

## 27. 最终长期目标

当这套优化完成后，BMCBL GPUI 应具备如下特征：

```text
状态变化
  ↓
精确识别直接 owner + invalidation kind
  ↓
只穿过必要 retained ancestor
  ↓
只有真正 dirty scope 重新 build
  ↓
layout 根据 measure/layout/placement dependency 增量运行
  ↓
静态 scene 直接 replay
  ↓
presentation-only 属性由 renderer 更新
  ↓
只上传真正变化的数据
  ↓
统一 scheduler 在真实 present deadline 下提交
  ↓
没有工作时完全 idle
```

高性能不是“GPU 占用越高越好”或“CPU 占用越低越好”，而是：

> **每一次输入、状态变化和动画 tick，都只付出实现该视觉结果所必需的最小工作量。**

这也是后续所有本地 AI 性能修改的统一判断标准。
