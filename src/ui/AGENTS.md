# BMCBL UI Agent Rules

These rules apply to every file under `src/ui` and extend the application-wide
rules in `src/AGENTS.md`.

Read `docs/GPUI_ANIMATION_CONVENTIONS.md` before changing animation, layout,
retained rendering, text positioning, frame scheduling, theme interpolation, or
visual effects.

## Animation clock — hard invariant

- Current-frame visual animation sampling MUST use `window.animation_time()`.
- Do not call `Instant::now()` or `std::time::Instant::now()` from `Render`,
  `RenderOnce`, `Element::request_layout`, prepaint, paint, or helpers called by
  those paths to derive animation progress, theme interpolation, spring values,
  opacity, transform, clip, or layout geometry.
- A single render should capture one `let now = window.animation_time();` and
  pass it through every helper that contributes time-dependent visible output.
- Keep `Instant::now()` for actual event occurrence/start/retarget timestamps,
  timers/deadlines, retries/timeouts, and performance measurement.
- Never "fix" the rule by replacing event/scheduler clocks with frame time.

## Helper functions are part of the render path

Do not audit only the `render()` body. Follow helpers transitively.

Common dangerous patterns are helpers such as:

```text
theme_colors(cx)
current_theme_colors(cx)
detached_theme_colors(cx)
render_*(...)
build_render_model(...)
sync_*_animation(...)
```

If such a helper derives current-frame visual output, change its API so it
receives `now: Instant`, `&Window`, or the already-derived visual value. It must
not silently fall back to `Instant::now()` because its current signature lacks
`Window`.

Before finishing a UI animation/rendering change, search the touched render
path and helper path for both `Instant::now()` spellings and classify every
occurrence.

## Theme interpolation

Theme transition sampling is visual animation. During rendering use the same
frame timestamp as all other motion:

```rust
let now = window.animation_time();
let colors = lerp_theme_colors(
    &LightColors::colors(),
    &DarkColors::colors(),
    theme.factor(now),
    theme.accent,
);
```

Do not keep convenience helpers that call `theme.factor(Instant::now())` during
render. Pass `now` into them.

A background/theme tick task may use real monotonic time to determine whether a
transition is still alive and to schedule wake-ups. That scheduler clock is not
the render sample.

## Animation geometry

- Visual-only animation must not move text layout origins or change text
  wrapping/baselines.
- Prefer final child geometry plus paint-only clip/compositor transform.
- Prefer renderer-owned scene animation or a retained `composite_layer()` for
  complex Text/SVG/Image/Quad subtrees.
- Use layout animation only for properties that genuinely affect layout.
- Do not hide visual jitter by integer-rounding every animated layout value;
  fix animation ownership and text geometry instead.

## Invalidation and scheduling

- Prefer the narrowest retained target. Do not use whole-window refresh as an
  animation clock.
- Do not add a global frame-backpressure or recent-input bypass.
- Interactive animation requests should coalesce/latest-win rather than queue
  stale samples.
- Static siblings and static text must remain retained whenever their output did
  not change.
- A generic `cx.notify()`/view invalidation must not erase a compatible targeted
  retained replay plan without a correctness reason.

## Required audit searches

For UI/rendering changes, inspect at least the affected paths for:

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

The presence of a real-time call is not automatically wrong; every occurrence
must be classified by semantics. Visual-frame samples use frame time. Real
event/scheduler/profiling work uses monotonic time.

## Validation

BMCBL development expects focused local build/profiling by the maintainer. Do
not introduce GitHub CI as an automatic repair mechanism. Keep performance and
rendering fixes small enough to A/B and commit independently.
