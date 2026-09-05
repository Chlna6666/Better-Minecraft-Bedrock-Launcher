# BMCBL GPUI Fork Agent Rules

These rules apply to every file under `crates/gpui` and extend the repository-wide
rules in the root `AGENTS.md`.

Before changing frame scheduling, animation, element lifecycle, retained
reconciliation, text, scene primitives, Nova upload, or shader integration,
read all of:

- `docs/GPUI_ANIMATION_CONVENTIONS.md`
- `docs/GPUI_VENDOR_RENDERING.md`
- `crates/gpui/docs/animation_clock.md`

## Frame timestamp contract

`Window::run_platform_frame` owns the frame timestamp. It captures one monotonic
time at frame start and exposes it as `Window::animation_time()`.

- Layout, prepaint, paint, scene-animation sampling, retained animation damage,
  and renderer-visible animation state for the same frame must use that timestamp.
- Do not introduce fresh `Instant::now()` animation samples inside element/render
  lifecycle code or helpers invoked by those lifecycle stages.
- A helper participating in current-frame visual output must receive the frame
  timestamp or `Window`; lack of `Window` in the current signature is not a reason
  to sample a fresh clock.
- Real monotonic time remains correct for frame throttle, deadlines, watchdogs,
  recent-input age, timers, timeouts, and profiling.
- Do not mix `animation_time()` and a fresh `Instant::now()` for two pieces of
  visual state that will be presented as one frame.

## Retained rendering contract

- Preserve the semantic distinction between `ElementOnly`, `ReconcileSubtree`,
  and `InvalidateSubtree`.
- Structural ancestors may be traversed to route reconciliation without becoming
  paint/layout dirty themselves.
- A precise retained target must not be widened to a full subtree or whole view
  unless correctness actually requires it.
- Generic view invalidation must not accidentally erase a compatible targeted
  replay plan.
- Visual-only animation is not, by itself, justification to invalidate layout or
  text shaping.

## Animation scheduling

- Targeted animation scheduling uses latest-state-wins semantics: one outstanding
  request per target, no stale sample queue.
- Do not reintroduce a global recent-input/backpressure bypass. Coalesce work and
  reduce lifecycle cost instead.
- Renderer-owned visual animation should request presentation/scene animation
  work without forcing layout.
- Scheduling/backpressure uses real monotonic time; animation sampling uses the
  frame timestamp. Keep those concerns separate.

## Text and subtree animation

- Visual-only motion must preserve stable text layout origins, wrapping widths,
  baselines, and glyph raster/subpixel identity.
- Text/SVG/Image/Quad/Shadow descendants that visually move or fade together
  should inherit one retained/composite animation owner where possible.
- Do not solve jitter by independently re-sampling or rounding each descendant.

## Nova animation contract

- Stable primitives stay stable across visual-only animation frames.
- Shaders should consume stable animation bindings plus compact per-frame
  animation values.
- Do not restore per-frame clone/mutate/serialize/memcpy of every animated
  primitive as the steady-state path.
- GPU lookup must not scan all animation bindings per primitive.
- Text/SVG/Image/Quad descendants of one composite animation inherit the same
  visual transform/opacity owner.
- CPU blur/filter damage planning may sample lightweight bounds, but should not
  require full primitive rewrites.

## Required review

Before finishing GPUI animation/rendering work, inspect the touched lifecycle and
helper paths for `Instant::now()`, animation sampling, targeted invalidation, and
frame scheduling. Classify every time read as either current-frame visual time or
real scheduler/event/profiling time; do not leave ambiguous helper-owned clocks.

Keep framework changes generic: no BMCBL page names, Minecraft-specific state,
or application-specific policy in GPUI internals.
