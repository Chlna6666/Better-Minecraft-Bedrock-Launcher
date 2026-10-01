# Animation Engine

[Chinese](animation_engine.zh-CN.md)

GPUI animation v2 is a framework-level animation engine. It provides timing,
easing, property classification, transition metadata, window scheduling,
renderer animation identifiers, and grouped timelines while preserving the
legacy element-wrapper animation API.

The engine is intentionally property-oriented. GPUI should provide the system
that animates style, paint, and layout values; it should not hard-code
application-specific effects such as "button hover" or "page enter".

## Goals

- Keep the existing animation constructors, element-wrapper methods, and easing
  helper names while requiring custom visual easing closures to be thread-safe.
- Provide a transition API for state-change animations on styled elements.
- Route visual-only properties to retained paint or GPU paths where supported.
- Route layout-affecting properties through layout invalidation because they
  must recompute layout.
- Expose window-owned sequence, parallel, and stagger timelines for application
  animation orchestration without bypassing the engine.
- Keep framework code independent of BMCBL pages, assets, routes, or window
  policy.

## Core Types

The public animation module exports:

- `Easing`: built-in curves such as `Linear`, `InCubic`, `OutCubic`,
  `InOutCubic`, `OutBack`, `OutElastic`, `OutQuint`, and `Spring`, plus
  `Custom(Arc<dyn Fn(f32) -> f32 + Send + Sync>)` for custom runtime curves.
- `AnimationSpec`: duration, delay, repeat mode, direction, fill mode, easing,
  and driver policy.
- `AnimationSequence`, `AnimationParallel`, and `AnimationStagger`: grouped
  timeline descriptions sampled by the same engine clock.
- `AnimationGroupId` and `AnimationGroupSample`: handles and samples for
  window-owned grouped timelines.
- `AnimationDriver`: `Auto`, `Gpu`, `Paint`, and `Layout`.
- `VisualAnimationError`: validation errors for callback-free retained visual
  animations.
- `Animatable`: interpolation for core value types such as `f32`, `Pixels`,
  `Hsla`, `Point<Pixels>`, `Size<Pixels>`, `TransformationMatrix`, shadows, and
  layout lengths.
- `Transition`: builder for state-change animation metadata.
- `TransitionProperty`: property classification for opacity, transform, color,
  blur, shadow, width, height, inset, margin, padding, gap, and border width.

Scene-bound visual timelines are copied into an immutable `PresentationPacket`
and sampled without reading mutable `Window` or `App` state. Completion is sent
back as an animation ID and retained target, so the UI owner performs any final
invalidation. Windows now submits these packets to a native winit/Nova owner
separate from the GPUI UI thread. DX12 and Vulkan continue sampling and
presenting while UI `Render` is blocked for 200 ms. Linux Wayland/X11 still
need this ownership split.

## Transition API

Use transitions for property-level state changes:

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

`Transition` stores serializable style metadata in `StyleRefinement`. Built-in
easing curves can be carried through style metadata. Runtime-only custom easing
closures are supported by the legacy wrapper path; transition drivers that
cannot access a closure must fall back to a safe CPU/layout path.

## Grouped Timelines

Applications can start engine-owned timeline groups from a `Window`:

```rust
use std::time::Duration;

use gpui::{AnimationSequence, AnimationSpec, Easing};

let group_id = window.start_animation_sequence(AnimationSequence::new(vec![
    AnimationSpec::new(Duration::from_millis(120)).ease(Easing::OutCubic),
    AnimationSpec::new(Duration::from_millis(180)).ease(Easing::Spring(Default::default())),
]));

if let Some(sample) = window.sample_animation_group(group_id) {
    // Apply the sampled progress to application-owned view state.
}
```

The public `Window` API also includes `start_animation_parallel`,
`start_animation_stagger`, `cancel_animation_group`, and
`set_animation_group_bounds`. The engine resolves each group to `Paint`, `Gpu`,
or `Layout` from its child specs and schedules the matching frame path.

### Retained parallel visual tracks

### Single-track retained visual animation

Use `with_visual_animation` when one declared visual property owns the whole
motion. It requires an `AnimationProperty` and rejects the layout driver; it
does not allocate or call an animator closure:

~~~rust
use std::time::Duration;

use gpui::{Animation, AnimationExt as _, div};

let fade = div()
    .with_visual_animation(
        "fade",
        Animation::new(Duration::from_millis(180)).with_opacity(0.0, 1.0),
    )
    .expect("opacity is a presentation property");
~~~

The single-track path also supports captured subtree, rotation, element filter blur, and clip
properties declared with `with_property`. Use `with_animation` when each sample
must change layout or content.

### Retained parallel visual tracks

Use `AnimationGroup` when one retained element needs multiple visual properties
with independent timing. Each track keeps its own duration, delay, repeat,
fill mode, easing, or spring, while one presentation sample packs opacity,
translation, and scale into one renderer value:

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
.expect("each track must target a distinct visual property");

let row = div().with_animation_group("row-enter", enter);
```

The group accepts at most one opacity, translation, and scale track. These
tracks keep the same element identity through completion, including
`fill-forwards` endpoints, and samples do not call the owning view's `Render`.
Scale keeps the existing per-primitive center pivot. Layout properties,
subtree-capturing properties such as `clipped_translation`, and duplicate
properties are rejected so they cannot silently change rendering semantics.
Use `with_visual_animation` for single rotation, clip, blur, explicit
shared-pivot transform, or captured subtree motion.

## Driver Selection

`AnimationDriver::Auto` resolves from the animated properties:

- visual-only properties such as opacity, transform, color, blur, and shadow
  are eligible for `Gpu` or `Paint`;
- layout-affecting properties such as width, height, inset, margin, padding,
  gap, and border width force `Layout`;
- closure-based legacy animations default to `Layout` because the framework
  cannot know which properties the closure mutates.

Layout animation is CPU-driven by design. Width, height, margins, padding, and
similar properties affect child and sibling layout, so they must invalidate the
view and recompute layout.

## Window Scheduling

Each window owns an `AnimationEngine`. The engine tracks active timelines by
element/property target, samples one window animation clock per frame, coalesces
duplicate frame requests, and stops requesting frames after finite timelines
complete.

Paint and GPU animation frames use an engine-specific scheduling path. This path
advances retained visual state without calling `cx.notify()` on the current view.
Layout animations intentionally fall back to `Window::request_animation_frame`,
which preserves the existing invalidation behavior.

`Window::request_animation_engine_frame(driver)` is public for code that already
knows the required driver. For grouped paint/GPU timelines,
`set_animation_group_bounds` lets callers provide dirty visual bounds so the
window can mark the affected retained region instead of forcing a full redraw.

Inactive and minimized windows continue to use the existing frame throttling and
inactive animation frame policy.

## Legacy Compatibility

Existing code remains valid:

```rust
use std::time::Duration;

use gpui::{Animation, AnimationExt as _, easing, div};

let element = div().with_animation(
    "fade",
    Animation::new(Duration::from_millis(200)).with_easing(easing::ease_out_quint()),
    |element, progress| element.opacity(progress),
);
```

Legacy chained animations keep their one-shot and repeat semantics. They are
sampled through the v2 timing code but continue to request a layout animation
frame through the animation engine because the closure can mutate any element
builder state.

## Scene And Nova Animation Path

Visual scene values are carried with the retained scene and uploaded as packed
animation bindings. The presentation owner samples timelines and easing once per
frame; Nova shaders apply the resulting values to eligible primitives. Current
shader paths include opacity, translation, scale, scale-plus-opacity transforms,
clip reveal, and the packed opacity/translation/scale group. Availability is
primitive-specific; it does not mean every property is supported by every draw
type. Layout-affecting properties still require UI layout work, and unsupported
visual bindings use their existing CPU or retained-composite path.

The parallel visual group intentionally limits its packed contract to one
opacity, translation, and scale track. It submits one renderer value per target
sample while keeping each track's timing and retarget state independent. This
follows the retained-scene/render-thread split used by [Qt Quick
Animator](https://doc.qt.io/qt-6/qml-qtquick-animator.html) and [Avalonia
Composition](https://docs.avaloniaui.net/docs/graphics-animation/composition-animations),
without claiming that GPUI has their full style-transition or property coverage.

## Current Limitations And Improvement Areas

The retained presentation path now advances supported visual tracks without
calling the owning view's `Render`, but the engine remains incomplete in several
specific areas:

- `Transition` stores metadata; automatic computed-style diffing does not yet
  start transitions from previous to new style values.
- The parallel visual group supports opacity, translation, and scale only.
  Rotation, blur, clip reveal, and explicit shared-pivot transforms keep their
  property-specific single-track or composite paths.
- The legacy closure wrapper cannot know which properties it changes, so it
  still uses UI/layout invalidation. Callers should use property-based retained
  animations for visual-only changes.
- Layout-affecting animations remain UI-owned and can be expensive in deep trees.
- Automatic dirty-bounds discovery and diagnostics for driver fallbacks and
  active animation counts are still incomplete.
- The independent native presentation owner is implemented for Windows DX12
  and Vulkan. Linux Wayland/X11 still needs the same ownership split and its
  native lifecycle validation.

Performance claims require measured workloads. In particular, packet-owned
scratch storage avoids rebuilding temporary collections during presentation
sampling, but CPU p50/p95/p99, upload cost, frame intervals, and input latency
must be compared before calling the change a measured win.

Performance work should prioritize the largest avoidable costs first:

1. Add computed-style diffing so common visual transitions do not need closure
   wrappers or per-property boilerplate.
2. Extend grouped retained tracks only when property semantics and a single
   renderer update can both be preserved.
3. Add fallback diagnostics and automatic dirty-bound discovery for CPU paint
   paths.
4. Compare Windows DX12/Vulkan CPU, upload, frame interval, and input-latency
   measurements before accepting further hot-path changes.

## Implementation Boundaries

- GPUI owns animation timing, driver policy, scene metadata, and renderer data
  channels.
- Applications own visual design choices: which elements transition, durations,
  easing choices, and interaction-specific effects.
- Do not add BMCBL routes, assets, launcher state, or theme defaults to GPUI
  animation internals.
- Do not route layout-affecting animation through GPU-only paths.

## Validation

Use focused validation while developing animation internals:

```bash
rtk cargo test -p gpui animation
rtk cargo test -p gpui window::tests
rtk cargo test -p gpui nova
```

### Animation performance lab

`animation_perf_lab` opens one real window with the retained visual animation
properties, spring retargeting, and separate layout/color callback examples.
Its compositor tracks repeat forever in alternating directions so a cycle
boundary cannot pass merely because an animation eventually reached its end.
Before taking the cadence baseline, the example fills the 256-sample interval
history so initial surface startup does not contaminate the steady-state gap
limit.
The 200 ms UI `Render` block gate records exact start/end times and checks each
successful present sample and both interval boundaries inside that window.
Every gap must stay below 1.8 times the baseline median, each combined active
visual-animation value must change, and UI `Render` must not run again between
the block boundaries. A sample-history overflow fails the gate. Sustained
reports apply the same per-sample and per-interval checks; reaching a timeline's
end is never sufficient to pass. Recent 256-interval percentiles and window
callback frame/present p50, p95, and p99 values remain supplemental diagnostics.
On Windows, each interval also reports the maximum DWM pacing wait, winit event
queue delay, per-window dispatch delay, and active-present duration attached to
successful samples. These stage timings help locate a gap; they do not relax or
replace the per-frame continuity checks.

On Windows, build both Nova backends and run each separately:

```powershell
cargo run --manifest-path crates/gpui/Cargo.toml --example animation_perf_lab --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx12,nova-gfx-vulkan -- --backend=nova-dx12 --copies=4 --seconds=30
cargo run --manifest-path crates/gpui/Cargo.toml --example animation_perf_lab --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx12,nova-gfx-vulkan -- --backend=nova-vulkan --copies=4 --seconds=30
```

`--copies` scales retained tracks; `--seconds` controls the full measurement.
Animation-sample interval percentiles and maximum are computed from the most
recent 256 active-animation presentation intervals. Window callback timing
percentiles are separate diagnostics and do not substitute for the successful
present sample trace.

Run formatting for touched files or the whole workspace when unrelated
formatting drift is not present. Use the project clippy script if it exists in
the checkout.
