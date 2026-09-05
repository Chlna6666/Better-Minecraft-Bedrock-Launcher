# GPUI Animation And Retained Rendering Conventions

This document defines hard correctness and performance invariants for BMCBL UI animation and the independently maintained GPUI fork. These rules apply to `src/ui`, `crates/gpui`, and Nova renderer changes.

## 1. One visual timestamp per platform frame

`Window::run_platform_frame` captures one `frame_started_at` and stores it in `Window::animation_time()` before the animation engine, layout, prepaint, paint, scene building, and renderer preparation run.

All visual animation sampling performed while building that frame MUST use that same timestamp.

### Required

Use `window.animation_time()` for values that can change visible output in the current frame, including:

- `raw_progress` / `eased_progress` used by `Render`, `RenderOnce`, `Element::request_layout`, `prepaint`, or `paint`;
- `SpringValue::sample`, `value`, or `is_animating` when they affect current-frame geometry/style;
- `ThemeState::factor` / `is_animating` during rendering;
- navigation pill/label/auth/modal/dropdown/tab/toast progress sampled during rendering;
- any CPU-side animation sample that is compared with renderer-owned scene animation state.

Within one frame, parent, child, layout, paint, text, scene animation, and Nova must observe the same timeline sample.

### Keep real monotonic time

`Instant::now()` remains correct for events and scheduling that occur independently of a frame:

- input/click/keyboard events that record an animation start or retarget instant;
- timer/deadline scheduling and wake-up calculations;
- progressive frame throttle/backpressure and recent-input age;
- performance measurement, profiling, timeout, retry, and I/O bookkeeping;
- background tasks that are not sampling current-frame visual output.

Do not replace these with `window.animation_time()`.

### Forbidden pattern

```rust
fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let now = Instant::now();
    let progress = raw_progress(now, self.started_at, DURATION);
    // ...
}
```

Correct:

```rust
fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let now = window.animation_time();
    let progress = raw_progress(now, self.started_at, DURATION);
    // ...
}
```

If a state transition is discovered while rendering, initialize and sample that transition with the same `window.animation_time()` value. Event-driven transitions continue to record the real event `Instant::now()`.

## 2. Animation ownership and invalidation

Choose the narrowest owner that can produce the visual result.

1. Prefer renderer-owned scene animation for opacity/translation/scale/rotation/transform when layout does not change.
2. Prefer a retained composite layer when a complex subtree containing Text/SVG/Image/Quad must move or fade as one visual object.
3. Use a targeted layout-animation boundary only when geometry genuinely affects layout.
4. Never use `cx.refresh_windows()` or whole-view animation driving as a generic animation clock.
5. A child animation must not invalidate static siblings or ancestors beyond the structural path required to reach the target.

`request_layout_animation_frame_if` can fall back to the owning view when no retained boundary is active. Caller-sampled UI should therefore prefer a stable `with_layout_animation_target` boundary or a renderer-owned scene animation.

## 3. Text geometry must remain stable for visual-only animation

Visual-only animation MUST NOT change text layout origin, wrapping width, baseline, or glyph subpixel variant.

For reveal/open/close effects:

- lay out text and child content at final geometry;
- animate a paint/content mask, retained clip, or compositor transform;
- do not animate ancestor `height`, `top`, `left`, padding, or margin merely to simulate a visual reveal;
- do not hide jitter by rounding every animated coordinate. Fix ownership so glyph base geometry remains stable.

The `VerticalRevealClipElement` pattern exists specifically to keep final child layout while changing only the visible paint mask.

## 4. Inherited subtree scene animation

Text, SVG, Image, Quad, Shadow, and nested content that visually belong to one animated subtree should inherit one scene-animation identity/transform whenever possible.

For complex motion:

```text
static child primitives
        -> retained composite layer
        -> one scene animation binding
        -> GPU presentation
```

Do not multiply the same dynamic opacity/transform independently into every glyph or child primitive if the final composite can own it.

## 5. Nova GPU animation binding

The target architecture is shader-consumed animation metadata:

```text
stable primitive geometry
+ stable primitive -> animation binding
+ small per-frame animation values
        -> shader resolves visual transform/opacity
```

Do not restore the old hot path where every animation tick clones a primitive, mutates geometry, serializes the complete record, and copies it back into Quad/Sprite/Blur buffers.

Animation lookup must be O(1) or effectively O(1) per primitive. Do not implement a shader loop that scans every animation binding for every primitive.

Backdrop/element blur is special: CPU-side filter/damage planning may still need lightweight sampled bounds. That does not justify rewriting the complete GPU primitive record each frame.

## 6. Per-target latest-wins scheduling

Interactive animation uses latest-state-wins semantics:

- at most one outstanding frame request per retained animation target;
- repeated requests for the same target coalesce;
- no stale animation-sample backlog;
- when the frame executes, sample the current `window.animation_time()` rather than replaying intermediate samples.

Do not globally disable progressive frame backpressure. Input responsiveness and animation pacing must be solved with request coalescing and reduced frame work, not an unrestricted backpressure bypass.

## 7. Retained reconciliation

Keep these questions separate:

```text
Does the View need rerender?
Does this element need reconciliation?
Does layout need recompute?
Does paint need rebuild?
Does the retained scene need rebuild?
Is presentation alone sufficient?
```

Expected behavior for a local hover/active/focus/visual child update:

```text
parent view render     no, unless application state changed
parent layout          no, unless geometry changed
parent paint           no, unless its own visual output changed
static siblings        retained replay
static text shaping    retained reuse
target paint/scene     only the affected target
present                dirty region / retained presentation
```

`ElementOnly`, `ReconcileSubtree`, and `InvalidateSubtree` are different contracts. Do not widen a precise target into `InvalidateSubtree` or clear targeted replay merely because an owning view is used as a routing ancestor.

## 8. Review checklist

Before merging an animation/rendering change:

- Search changed render/lifecycle code for `Instant::now()` and classify every occurrence as event/scheduler/perf or visual sampling.
- Visual sampling inside a frame uses `window.animation_time()`.
- Event start/retarget timestamps keep `Instant::now()`.
- Confirm static text does not receive changing layout bounds for a visual-only effect.
- Confirm a complex animated subtree has one stable retained/composite owner where practical.
- Confirm animation frame requests are targeted and coalesced.
- Confirm Nova does not upload unrelated primitive ranges for a visual-only animation.
- Confirm no global frame-throttle bypass was introduced.
- Prefer local build/profiling and focused tests; do not infer smoothness only from average FPS.

## 9. Audit guidance

High-signal searches when reviewing BMCBL UI code:

```text
raw_progress(Instant::now()
eased_progress(Instant::now()
theme.factor(Instant::now())
theme.factor(std::time::Instant::now())
.sample(Instant::now())
.value(Instant::now())
fn render ... Instant::now()
with_layout_animation_target
request_animation_frame
request_layout_animation_frame_if
```

The presence of `Instant::now()` is not automatically a bug. The bug is using a fresh monotonic sample to derive current-frame visual output when a frame timestamp already exists.
