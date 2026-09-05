# GPUI Frame Animation Clock Contract

This document is the framework-local source of truth for animation time inside
`crates/gpui`. BMCBL application rules are documented in
`docs/GPUI_ANIMATION_CONVENTIONS.md` and `src/AGENTS.md`.

## Ownership

`Window::run_platform_frame` owns the timestamp for one rendered/presented
frame. At the beginning of the platform frame it captures one monotonic instant
and stores it in `Window::animation_time()` before advancing the animation
engine and before layout, prepaint, paint, scene construction, and renderer
preparation.

Every visual animation sample participating in that frame must observe the same
value.

```text
platform frame begins
    -> frame_started_at = Instant::now()
    -> animation_time = frame_started_at
    -> animation engine tick
    -> request_layout / layout
    -> prepaint
    -> paint
    -> retained scene / scene-animation values
    -> Nova frame preparation
    -> present
```

The initial `Instant::now()` above is the capture point for the frame. It must
not be repeated inside lifecycle stages to obtain a newer visual sample.

## Visual-frame clock

Use `Window::animation_time()` for time-dependent state that changes visible
output in the current frame:

- layout animation progress;
- element animation progress;
- spring/tween values used by layout, prepaint, or paint;
- scene-animation values supplied to the renderer;
- visual transform, opacity, scale, rotation, clip, blur-display geometry, or
  animation damage bounds when derived for this frame;
- current-frame state compared across retained CPU and GPU animation paths.

If a helper is called from lifecycle code and needs animation time, pass the
frame timestamp explicitly or read it from `Window`. Do not let a helper hide a
fresh `Instant::now()` sample.

## Real monotonic clock

Use real monotonic time for scheduler/event semantics that are not a visual
sample of the current frame:

- platform input timestamps and recent-input age;
- frame throttle and retry deadlines;
- watchdog deadlines;
- timer wake-up and timeout calculations;
- profiling and elapsed-duration measurement;
- asynchronous I/O bookkeeping;
- event occurrence timestamps that later become animation start/retarget
  anchors.

These must not be converted to `animation_time()` merely to satisfy the visual
clock rule.

## Forbidden mixed-frame sampling

The following is a correctness bug:

```rust
let frame_now = window.animation_time();
let layout_progress = timeline.sample(frame_now);
let paint_progress = timeline.sample(Instant::now());
```

It allows layout, text, paint, retained bounds, damage tracking, and GPU state
to describe different animation samples while being presented as one frame.
The visible result can be jitter, one-frame geometry disagreement, invalid
retained reuse, or unstable text positioning.

The same bug can be hidden behind helpers:

```rust
fn current_theme_colors(cx: &App) -> ThemeColors {
    theme.factor(Instant::now())
}
```

Framework code should make clock ownership explicit in API shape when a helper
participates in visual sampling.

## Retained rendering interaction

A retained element may reuse layout/paint/scene state only if the reused state
and newly sampled animation state belong to a coherent frame. Do not use a
fresh clock to decide one lifecycle stage while another stage replays data
sampled at `animation_time()`.

Structural ancestors may be traversed for reconciliation without becoming
layout or paint dirty. Time sampling itself is not a reason to widen a precise
retained target.

## Nova interaction

Renderer-owned visual animation should receive stable primitive geometry plus a
stable primitive-to-animation binding and compact per-frame values derived from
the frame clock. The target steady state must not clone, mutate, serialize, and
rewrite every animated primitive merely to advance visual time.

CPU-only filter/damage planning may sample lightweight geometry, but it must use
the same current-frame animation sample as renderer-visible values.

## Review checklist

Before completing GPUI animation/render work:

- inspect lifecycle code and every helper it calls for `Instant::now()`;
- classify each time read as visual-frame or scheduler/event/profiling time;
- verify one frame does not mix the two clocks for visual output;
- verify retained targets are not widened merely because animation advances;
- verify text layout origins remain stable for visual-only animation;
- verify renderer-owned animation does not force unrelated layout/paint work;
- keep backpressure policy separate from animation sampling semantics.
