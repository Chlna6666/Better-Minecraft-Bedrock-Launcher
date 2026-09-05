# BMCBL Application Agent Rules

These rules apply to every file under `src/`, including `src/ui`, UI code in
`src/plugins`, onboarding flows, standalone windows, and application-owned
render helpers. Framework-only rules live under `crates/gpui/AGENTS.md`.

Before changing animation, theme interpolation, layout motion, retained
rendering, text positioning, frame scheduling, or visual effects, read:

- `docs/GPUI_ANIMATION_CONVENTIONS.md`
- `docs/GPUI_VENDOR_RENDERING.md`
- `src/ui/README.md` when the change is under `src/ui`

## Frame clock is a correctness invariant

Current-frame visual output MUST be sampled from one frame timestamp:

```rust
let now = window.animation_time();
```

Use this timestamp for every visual value derived during the current frame,
including theme interpolation, animation progress, spring sampling, opacity,
transform, clip/reveal geometry, layout animation, navigation motion, modal,
dropdown, tab, toast, loading pulse, and renderer-facing animation state.

Do not call `Instant::now()` or `std::time::Instant::now()` from `Render`,
`RenderOnce`, `Element::request_layout`, prepaint, paint, or helpers invoked by
those paths to derive visible output.

A helper that needs current-frame visual time must receive `now: Instant`,
`&Window`, or another explicit frame-sample input from its caller. Do not hide
a fresh `Instant::now()` inside `theme_colors()`, `current_theme_colors()`,
`render_*()`, or similar convenience helpers.

## Real monotonic time remains valid outside visual sampling

Keep `Instant::now()` for actual event and scheduler semantics:

- input/click/key events that start or retarget an animation;
- timer/deadline/retry/timeout calculations;
- task timestamps and I/O bookkeeping;
- performance/profiling measurements;
- frame throttle, watchdog, recent-input age, and other scheduler state.

Do not replace those clocks with `window.animation_time()`.

## Do not mix the clocks in one visual frame

A render path must not do this:

```rust
let frame_now = window.animation_time();
let theme = theme.factor(frame_now);
let spring = motion.sample(Instant::now());
```

Nor may a parent use `window.animation_time()` while a child helper samples
`Instant::now()`. All values contributing to one presented frame must share the
same frame sample.

If a transition is discovered while rendering, initialize and sample it with
the same `window.animation_time()` value. Event-driven transitions may still
record the real event timestamp.

## Visual animation ownership

- Prefer renderer-owned opacity/translation/scale/rotation/transform when
  layout does not need to change.
- Keep text at stable final geometry for visual-only motion; animate a retained
  clip/composite transform rather than text layout origin, wrapping width, or
  baseline.
- Prefer a stable retained/composite owner for a subtree containing text, SVG,
  images, quads, and shadows that moves or fades as one object.
- Use layout animation only when the property truly participates in layout.
- Do not use `cx.refresh_windows()` as a generic animation clock.
- Do not add a global animation/backpressure bypass.

## Retained invalidation

Use the narrowest valid invalidation. A child hover/active/focus/animation must
not dirty static siblings or ancestors beyond the structural path needed to
reach the target. Static text should retain shaping/layout when its output did
not change.

Animation requests should coalesce/latest-win per target. Do not queue stale
animation samples.

## Required review before finishing a UI/render change

Search every touched render/lifecycle path, including helper functions, for:

```text
Instant::now()
std::time::Instant::now()
raw_progress(
eased_progress(
.sample(
.value(
.factor(
.is_animating(
theme_colors(
current_theme_colors(
request_animation_frame
request_layout_animation_frame_if
with_layout_animation_target
```

Classify every time read. A search result is not automatically wrong, but every
fresh clock used to derive current-frame visual output is a bug.

When a helper does not receive `Window`, pass the frame timestamp into it rather
than falling back to `Instant::now()`.

Keep animation/rendering fixes independently testable. Do not use GitHub CI as
an automatic repair loop; the maintainer performs local build/profiling and A/B
validation.
