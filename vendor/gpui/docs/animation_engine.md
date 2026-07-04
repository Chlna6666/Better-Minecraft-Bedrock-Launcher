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

- Keep `Animation::new`, `Animation::repeat`, `Animation::with_easing`,
  `AnimationExt::with_animation`, `AnimationExt::with_animations`, and the
  existing easing helper functions source-compatible.
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
  `Custom(Rc<dyn Fn(f32) -> f32>)` for compatibility.
- `AnimationSpec`: duration, delay, repeat mode, direction, fill mode, easing,
  and driver policy.
- `AnimationSequence`, `AnimationParallel`, and `AnimationStagger`: grouped
  timeline descriptions sampled by the same engine clock.
- `AnimationGroupId` and `AnimationGroupSample`: handles and samples for
  window-owned grouped timelines.
- `AnimationDriver`: `Auto`, `Gpu`, `Paint`, and `Layout`.
- `Animatable`: interpolation for core value types such as `f32`, `Pixels`,
  `Hsla`, `Point<Pixels>`, `Size<Pixels>`, `TransformationMatrix`, shadows, and
  layout lengths.
- `Transition`: builder for state-change animation metadata.
- `TransitionProperty`: property classification for opacity, transform, color,
  blur, shadow, width, height, inset, margin, padding, gap, and border width.

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

## Scene And nova-gfx Data Path

Scene primitives that can participate in visual animation may carry a
`SceneAnimationId`. The nova-gfx frame upload path records packed animation
bindings containing:

- scene animation ID;
- animated primitive kind;
- primitive buffer index;
- reserved data for future expansion.

This is the renderer data channel needed for shader-side interpolation. Current
CPU fallback remains correct for unsupported primitives, custom easing, and
layout properties. Shader-side interpolation should be added per primitive type
before declaring a property fully GPU accelerated.

## Current Limitations And Improvement Areas

The current engine establishes the framework contract and scheduling foundation,
but it is not yet a complete end-to-end animation system. The main limitations
are:

- GPU acceleration is a data path, not a completed shader path. Scene primitives
  can carry animation IDs and nova-gfx can upload animation bindings, but
  primitive shaders still need property-specific interpolation for opacity,
  transform, colors, blur, and shadows before those properties are fully GPU
  accelerated.
- Transition metadata exists, but style-diff application is still limited.
  The engine can describe which properties should transition, but a complete
  computed-style previous/current comparison layer is still needed to
  automatically start transitions from old style values to new style values.
- Legacy closure animations are safe but expensive. They must use the layout
  driver because the closure can mutate any element builder state. This
  preserves compatibility, but it can notify views and recompute layout even
  when the closure only changes opacity or transform.
- `Easing::Custom` is runtime-only. It works for legacy animation closures, but
  it cannot be serialized into `StyleRefinement` or evaluated by GPU shader code
  without an explicit fallback.
- Grouped timelines are engine-owned, but they are still a low-level API. GPUI
  does not yet provide style-diff driven sequence orchestration, reusable motion
  tokens, parent/child propagation, or a timeline reuse pool.
- Layout animation remains CPU-bound. This is correct for layout-affecting
  properties, but heavy width/height/margin/padding animation can still be
  expensive in deep element trees.
- Paint invalidation is precise when callers provide bounds for engine-owned
  timelines. Automatic bounds discovery for all animated primitives and CPU
  fallback paths is still incomplete.
- Authoring ergonomics are early. The transition builder and grouped timeline
  APIs are usable, but GPUI does not yet provide higher-level helpers for common
  patterns such as grouped transitions, reusable motion tokens, or
  reduced-motion policies.
- Observability is incomplete. Tests cover timing, scheduling, and nova binding
  packing, but runtime diagnostics should expose active animation counts, driver
  fallback reasons, layout-vs-paint frame counts, and long-running animations.

Performance work should prioritize the largest avoidable costs first:

1. Implement style-diff driven transitions so visual-only changes do not need
   closure wrappers.
2. Complete shader interpolation for the GPU-eligible primitives already carrying
   `SceneAnimationId`.
3. Add fallback diagnostics so unsupported properties and custom easing are
   visible during development.
4. Complete automatic dirty-bound discovery for CPU paint fallback.
5. Add ergonomic motion helpers only after the low-level property path is stable.

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

Run formatting for touched files or the whole workspace when unrelated
formatting drift is not present. Use the project clippy script if it exists in
the checkout.
