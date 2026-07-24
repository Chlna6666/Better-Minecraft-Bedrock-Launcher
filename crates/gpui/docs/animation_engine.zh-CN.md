# 动画引擎

[English](animation_engine.md)

GPUI animation v2 是框架级动画引擎。它提供 timing、easing、属性分类、
transition metadata、窗口调度、renderer animation id 和 grouped timeline，
同时保留旧的 element-wrapper 动画 API。

这个引擎以属性为中心。GPUI 应该提供能动画 style、paint 和 layout 值的系统，
而不是在框架里硬编码 “按钮 hover” 或 “页面进入” 这类应用效果。

## 目标

- 保持 `Animation::new`、`Animation::repeat`、`Animation::with_easing`、
  `AnimationExt::with_animation`、`AnimationExt::with_animations` 和现有
  easing helper 的源码兼容。
- 为 styled element 的状态变化提供 transition API。
- 对支持的纯视觉属性使用 retained paint 或 GPU 路径。
- 对影响 layout 的属性使用 layout invalidation，因为它们必须重新计算布局。
- 暴露窗口拥有的 sequence、parallel 和 stagger timeline，让应用编排动画时不再绕过
  engine。
- 保持 GPUI 框架代码不依赖 BMCBL 页面、资源、路由或窗口策略。

## 核心类型

公开 animation 模块导出：

- `Easing`：内置曲线包括 `Linear`、`InCubic`、`OutCubic`、`InOutCubic`、
  `OutBack`、`OutElastic`、`OutQuint` 和 `Spring`，并通过
  `Custom(Rc<dyn Fn(f32) -> f32>)` 保持兼容性。
- `AnimationSpec`：duration、delay、repeat mode、direction、fill mode、
  easing 和 driver policy。
- `AnimationSequence`、`AnimationParallel` 和 `AnimationStagger`：由同一个 engine
  时钟采样的 grouped timeline 描述。
- `AnimationGroupId` 和 `AnimationGroupSample`：窗口拥有的 grouped timeline handle
  与采样结果。
- `AnimationDriver`：`Auto`、`Gpu`、`Paint` 和 `Layout`。
- `Animatable`：为 `f32`、`Pixels`、`Hsla`、`Point<Pixels>`、
  `Size<Pixels>`、`TransformationMatrix`、shadow 和 layout length 等核心
  值提供插值。
- `Transition`：状态变化动画 metadata 的 builder。
- `TransitionProperty`：对 opacity、transform、color、blur、shadow、width、
  height、inset、margin、padding、gap 和 border width 进行属性分类。

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

## Scene 与 nova-gfx 数据通道

可参与视觉动画的 scene primitive 可以携带 `SceneAnimationId`。nova-gfx frame
upload 路径会记录 packed animation binding，包含：

- scene animation id；
- animated primitive kind；
- primitive buffer index；
- 为后续扩展保留的数据位。

这是 shader-side interpolation 所需的 renderer 数据通道。对不支持的 primitive、
custom easing 和 layout property，当前 CPU fallback 仍保持正确。某个属性要宣称完整
GPU 加速前，应按 primitive 类型继续接入 shader-side interpolation。

## 当前不足与改进方向

当前 engine 已经建立框架契约和调度基础，但还不是完整端到端动画系统。主要不足包括：

- GPU 加速目前是数据通道，不是完整 shader 路径。Scene primitive 可以携带
  animation id，nova-gfx 也可以上传 animation binding，但 primitive shader 仍需要
  针对 opacity、transform、color、blur 和 shadow 做属性级插值，才能称为完整 GPU
  加速。
- Transition metadata 已存在，但 style diff 应用还不完整。engine 可以描述哪些属性
  应该 transition，但仍需要 computed style 的 previous/current 比较层，才能自动从旧
  style 值过渡到新 style 值。
- 旧 closure 动画安全但成本较高。closure 可能修改任意 element builder 状态，所以必须
  使用 layout driver。这保留了兼容性，但即使 closure 只改 opacity 或 transform，也可能
  notify view 并重新计算 layout。
- `Easing::Custom` 只适合运行时路径。它适用于旧动画 closure，但不能可靠序列化进
  `StyleRefinement`，也不能直接由 GPU shader 执行，必须显式 fallback。
- Grouped timeline 已由 engine 拥有，但仍是低层 API。GPUI 还没有提供 style-diff
  driven 的 sequence 编排、可复用 motion token、父子传播或 timeline reuse pool。
- Layout 动画仍然受 CPU 限制。对影响 layout 的属性这是正确设计，但在深层 element tree
  中大量动画 width、height、margin 或 padding 仍会昂贵。
- 调用方为 engine-owned timeline 提供 bounds 时，paint invalidation 已能精确标记区域。
  但所有 animated primitive 和 CPU fallback 路径的自动 bounds 发现仍未完整。
- 编写体验仍处于早期。Transition builder 和 grouped timeline API 可用，但 GPUI 还没有
  提供 grouped transition、可复用 motion token 或 reduced-motion policy 这类更高层
  helper。
- 可观测性还不完整。测试覆盖了 timing、scheduling 和 nova binding packing，但运行时
  diagnostics 应继续暴露活跃动画数量、driver fallback 原因、layout-vs-paint frame 数量和
  长时间运行动画。

性能改进应优先处理最大的可避免成本：

1. 实现 style-diff driven transition，让纯视觉变化不再依赖 closure wrapper。
2. 为已携带 `SceneAnimationId` 的 GPU-eligible primitive 完成 shader interpolation。
3. 添加 fallback diagnostics，让不支持的属性和 custom easing 在开发期可见。
4. 补完整 CPU paint fallback 的自动 dirty bounds 发现。
5. 等底层属性路径稳定后，再添加更高层 motion helper。

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

如果没有无关格式漂移，可以跑全 workspace formatting；否则对触碰文件做定向
format check。checkout 中存在项目 clippy 脚本时优先使用该脚本。
