# 动画引擎

[English](animation_engine.md)

GPUI animation v2 是框架级动画引擎。它提供 timing、easing、属性分类、
transition metadata、窗口调度、renderer animation id 和 grouped timeline，
同时保留旧的 element-wrapper 动画 API。

这个引擎以属性为中心。GPUI 应该提供能动画 style、paint 和 layout 值的系统，
而不是在框架里硬编码 “按钮 hover” 或 “页面进入” 这类应用效果。

## 目标

- 保留现有动画构造器、element-wrapper 方法和 easing helper 名称；自定义视觉
  easing 闭包必须满足线程安全约束。
- 为 styled element 的状态变化提供 transition API。
- 对支持的纯视觉属性使用 retained paint 或 GPU 路径。
- 对影响 layout 的属性使用 layout invalidation，因为它们必须重新计算布局。
- 暴露窗口拥有的 sequence、parallel 和 stagger timeline，让应用编排动画时不再绕过
  engine。
- 保持 GPUI 框架代码不依赖 BMCBL 页面、资源、路由或窗口策略。

## 核心类型

公开 animation 模块导出：

- `Easing`：内置曲线包括 `Linear`、`InCubic`、`OutCubic`、`InOutCubic`、
  `OutBack`、`OutElastic`、`OutQuint` 和 `Spring`，并支持运行时自定义曲线
  `Custom(Arc<dyn Fn(f32) -> f32 + Send + Sync>)`。
- `AnimationSpec`：duration、delay、repeat mode、direction、fill mode、
  easing 和 driver policy。
- `AnimationSequence`、`AnimationParallel` 和 `AnimationStagger`：由同一个 engine
  时钟采样的 grouped timeline 描述。
- `AnimationGroupId` 和 `AnimationGroupSample`：窗口拥有的 grouped timeline handle
  与采样结果。
- `AnimationDriver`：`Auto`、`Gpu`、`Paint` 和 `Layout`。
- `VisualAnimationError`：callback-free retained visual 动画的校验错误。
- `Animatable`：为 `f32`、`Pixels`、`Hsla`、`Point<Pixels>`、
  `Size<Pixels>`、`TransformationMatrix`、shadow 和 layout length 等核心
  值提供插值。
- `Transition`：状态变化动画 metadata 的 builder。
- `TransitionProperty`：对 opacity、transform、color、blur、shadow、width、
  height、inset、margin、padding、gap 和 border width 进行属性分类。

绑定到 Scene 的视觉 timeline 会复制到不可变 `PresentationPacket`，并在采样时不读取
可变的 `Window` 或 `App` 状态。动画完成通过 ID 和 retained target 作为事件回传，最终
invalidation 仍由 UI owner 处理。Windows 与 Linux/FreeBSD 的 Nova 路径共用独立 GPU
owner；原生窗口协议和 pacing 留在平台线程。阻塞 Render 验证用于检查各 backend 的采样
和呈现是否持续；Linux Wayland/X11 仍需单独完成原生生命周期验证。

## Transition API

属性级状态变化使用 transition：

```rust
use std::time::Duration;

use gpui::{AnimationDriver, Easing, Styled as _, Transition, TransitionProperty, div};

let element = div().transition(
    Transition::new(Duration::from_millis(180))
        .ease(Easing::OutCubic)
        .properties([TransitionProperty::Opacity, TransitionProperty::Transform])
        .driver(AnimationDriver::Auto),
);
```

`Transition` 会在 `StyleRefinement` 中保存可序列化的 style metadata。内置
easing 曲线可以完整进入 style metadata。运行时自定义 easing closure 由旧 wrapper
路径支持；不能访问 closure 的 transition driver 必须回退到安全的 CPU/layout 路径。

## Grouped Timeline

应用可以从 `Window` 启动 engine 拥有的 timeline group：

```rust
use std::time::Duration;

use gpui::{AnimationSequence, AnimationSpec, Easing};

let group_id = window.start_animation_sequence(AnimationSequence::new(vec![
    AnimationSpec::new(Duration::from_millis(120)).ease(Easing::OutCubic),
    AnimationSpec::new(Duration::from_millis(180)).ease(Easing::Spring(Default::default())),
]));

if let Some(sample) = window.sample_animation_group(group_id) {
    // 将采样进度应用到应用自己的 view state。
}
```

公开 `Window` API 还包括 `start_animation_parallel`、`start_animation_stagger`、
`cancel_animation_group` 和 `set_animation_group_bounds`。engine 会根据 child spec
把 group 解析到 `Paint`、`Gpu` 或 `Layout`，并调度对应的 frame 路径。

### Retained 并行视觉轨道

### 单轨 retained visual 动画

一个已声明的视觉属性可以直接使用 `with_visual_animation`。它会校验视觉属性并拒绝
`Layout` driver，不分配或调用 animator closure：

~~~rust
use std::time::Duration;

use gpui::{Animation, AnimationExt as _, div};

let fade = div()
    .with_visual_animation(
        "fade",
        Animation::new(Duration::from_millis(180)).with_opacity(0.0, 1.0),
    )
    .expect("opacity 是 presentation 属性");
~~~

单轨入口也支持通过 `with_property` 声明的子树捕获、rotation、元素 filter blur 和 clip。
当每个 sample 确实需要改变 layout 或内容时，使用 `with_animation`。

### Retained 并行视觉轨道

同一个 retained element 需要同时改变多个视觉属性、且每条轨道有独立时序时，使用
`AnimationGroup`。每条轨道保留自己的 duration、delay、repeat、fill mode、easing
或 spring；presentation 会把 opacity、translation 和 scale 合并为一次 renderer 值：

```rust
use std::time::Duration;

use gpui::{Animation, AnimationExt as _, AnimationGroup, Point, div, point, px};

let enter = AnimationGroup::parallel([
    Animation::new(Duration::from_millis(220)).with_opacity(0.0, 1.0),
    Animation::new(Duration::from_millis(320)).with_translation(
        Point::default(),
        point(px(0.0), px(18.0)),
    ),
])
.expect("每条轨道必须使用不同的视觉属性");

let row = div().with_animation_group("row-enter", enter);
```

一个 group 最多包含一条 opacity、translation 和 scale 轨道。完成后仍保留同一元素绑定，
包括 `fill-forwards` 终值；采样不会调用所属 view 的 `Render`。Scale 沿用每个 primitive
自身中心的 pivot。影响 layout、捕获子树的属性（例如 `clipped_translation`）和重复属性
都会被拒绝，避免悄悄改变渲染语义。单轨 rotation、clip、blur、显式共享 pivot 的
transform 或捕获子树动画使用 `with_visual_animation`。

## Driver 选择

`AnimationDriver::Auto` 根据动画属性解析：

- opacity、transform、color、blur 和 shadow 等纯视觉属性可以使用 `Gpu` 或
  `Paint`；
- width、height、inset、margin、padding、gap 和 border width 等影响 layout
  的属性强制使用 `Layout`；
- 基于 closure 的旧动画默认使用 `Layout`，因为框架无法知道 closure 修改了哪些
  属性。

layout 动画按设计仍然由 CPU 驱动。width、height、margin、padding 等属性会影响
子节点和兄弟节点布局，所以必须 invalidate view 并重新计算 layout。

## 窗口调度

每个窗口拥有一个 `AnimationEngine`。engine 按 element/property target 跟踪活跃
timeline，每帧只采样一次窗口动画时钟，合并重复 frame request，并在有限动画完成后
停止继续请求帧。

Paint 和 GPU 动画帧使用 engine 专用调度路径。这个路径推进 retained visual state，
不会对当前 view 调用 `cx.notify()`。Layout 动画会刻意回退到
`Window::request_animation_frame`，以保留现有 invalidation 行为。

`Window::request_animation_engine_frame(driver)` 已公开给明确知道所需 driver 的代码。
对 grouped paint/GPU timeline，`set_animation_group_bounds` 允许调用方提供 dirty
visual bounds，使窗口标记受影响的 retained region，而不是强制全量重绘。

非 active 或 minimized 窗口继续复用现有 frame throttling 与 inactive animation
frame 策略。

## 旧 API 兼容

已有代码继续有效：

```rust
use std::time::Duration;

use gpui::{Animation, AnimationExt as _, div, easing};

let element = div().with_animation(
    "fade",
    Animation::new(Duration::from_millis(200)).with_easing(easing::ease_out_quint()),
    |element, progress| element.opacity(progress),
);
```

旧的 chained animation 保留 oneshot 和 repeat 语义。它们通过 v2 timing 代码采样，
但仍通过 animation engine 请求 layout animation frame，因为 closure 可以修改任意
element builder 状态。

## Scene 与 Nova 动画路径

视觉 scene value 随 retained scene 传递，并作为打包后的 animation binding 上传。
presentation owner 每帧采样 timeline 和 easing；Nova shader 将结果应用到支持的
primitive。当前 shader 路径包括 opacity、translation、scale、scale 加 opacity 的
transform、clip reveal，以及打包后的 opacity/translation/scale group。支持范围因
primitive 类型而异，不代表每种 draw type 都支持所有属性。影响 layout 的属性仍需 UI
重新布局；不支持的视觉绑定沿用现有 CPU 或 retained-composite 路径。

并行视觉 group 将打包契约限制为一条 opacity、translation 和 scale 轨道。每个目标的
一次呈现采样只提交一个 renderer value，各轨道仍保留独立 timing 和 retarget 状态。这与
[Qt Quick Animator](https://doc.qt.io/qt-6/qml-qtquick-animator.html) 和 [Avalonia
Composition](https://docs.avaloniaui.net/docs/graphics-animation/composition-animations)
采用的 retained scene 与 render-thread 分工相近，但 GPUI 目前没有它们完整的 style
transition 或属性覆盖范围。

## 当前不足与改进方向

支持的 retained presentation 视觉轨道已可在不调用所属 view `Render` 的情况下推进，
但引擎仍有以下明确缺口：

- `Transition` 会保存 metadata；自动比较 computed style 前后值并启动 transition 尚未实现。
- 并行视觉 group 目前只支持 opacity、translation 和 scale。Rotation、blur、clip reveal
  和显式共享 pivot 的 transform 继续走各自的单轨或 composite 路径。
- 旧 closure wrapper 无法判断它修改了哪些属性，因此仍使用 UI/layout invalidation。
  纯视觉变化应使用按属性描述的 retained animation。
- 影响 layout 的动画仍由 UI 所有；在深层树中仍可能产生较高成本。
- 自动 dirty-bounds 发现，以及 driver fallback 和活跃动画数量的诊断尚未完整。
- Windows DX12/Vulkan 与 Linux/FreeBSD Nova 已接入独立 GPU owner。Linux Wayland/X11
  的原生生命周期验证仍是单独门槛，Windows 结果不能证明 Linux 运行时行为。

性能结论必须来自实际工作负载。packet 自有 scratch storage 已避免呈现采样时重复创建
临时集合，但在称为实测收益前，仍需比较 CPU p50/p95/p99、上传成本、帧间隔和输入延迟。

后续优先级：

1. 增加 computed-style diff，让常见视觉 transition 不需要 closure 或逐属性样板代码。
2. 仅在保持明确属性语义且一次 renderer 更新可以表达时，扩展并行 retained 轨道。
3. 补充 fallback diagnostics 和 CPU paint 路径的自动 dirty-bounds 发现。
4. 先比较 Windows DX12/Vulkan 的 CPU、上传、帧间隔和输入延迟，再接纳后续热路径改动。

## 实现边界

- GPUI 负责 animation timing、driver policy、scene metadata 和 renderer 数据通道。
- 应用负责视觉设计选择：哪些元素 transition、duration、easing 和具体交互效果。
- 不要把 BMCBL routes、assets、launcher state 或 theme defaults 放进 GPUI
  animation internals。
- 不要把影响 layout 的动画路由到 GPU-only 路径。

## 验证

开发 animation internals 时使用聚焦验证：

```bash
rtk cargo test -p gpui animation
rtk cargo test -p gpui window::tests
rtk cargo test -p gpui nova
```

### Animation 性能测试台

`animation_perf_lab` 会打开一个真实窗口，展示 retained 视觉动画属性、spring retarget，
以及单独对照的 layout/color callback 动画。compositor 轨道以往返方向无限重复，避免只因
动画最终到达终点就误判通过。测基线前会先填满 256 个样本的间隔历史，避免 surface 启动时的
间隔污染稳态门槛。200 ms UI `Render` 阻塞门槛记录精确起止时间，按成功 present 时间戳检查
阻塞区间内每个样本及首尾覆盖；相邻样本间隔须小于基准中位数的 1.8 倍，每个样本中的活动
视觉轨道合成值都必须变化，UI `Render` 计数在阻塞边界间保持不变。样本历史溢出会判失败。
持续报告对每个测量区间执行相同的逐样本和帧间隔检查；timeline 是否到达终点不作为通过条件。
最近 256 项间隔分位数、窗口回调的帧间隔和 present 间隔 p50、p95、p99 仅作为辅助数据。
Windows 报告还会给出成功样本对应的 DWM 节拍等待、winit 事件队列、逐窗口派发和 active present
阶段最大耗时，用于定位间隔；这些阶段数据不能替代或放宽逐帧连续性检查。

Windows 上同时构建 Nova 两种后端，并分别运行：

```powershell
cargo run --manifest-path crates/gpui/Cargo.toml --example animation_perf_lab --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx12,nova-gfx-vulkan -- --backend=nova-dx12 --copies=4 --seconds=30
cargo run --manifest-path crates/gpui/Cargo.toml --example animation_perf_lab --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx12,nova-gfx-vulkan -- --backend=nova-vulkan --copies=4 --seconds=30
```

`--copies` 增加 retained 轨道数量；`--seconds` 控制完整测量时长。动画样本间隔分位数和最大值
取最近 256 个活动动画 present 间隔。窗口回调时间分位数是另一组辅助指标，不能替代成功
present 的样本连续性记录。

如果没有无关格式漂移，可以跑全 workspace formatting；否则对触碰文件做定向
format check。checkout 中存在项目 clippy 脚本时优先使用该脚本。
