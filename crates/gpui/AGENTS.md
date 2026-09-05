# BMCBL GPUI Fork Agent Rules

These rules apply to every file under `crates/gpui`.

Read `docs/GPUI_ANIMATION_CONVENTIONS.md` and `docs/GPUI_VENDOR_RENDERING.md` before changing frame scheduling, animation, element lifecycle, retained reconciliation, text, scene primitives, or Nova integration.

## Frame timestamp contract

`Window::run_platform_frame` owns the frame timestamp. It captures one monotonic time at frame start and exposes it as `Window::animation_time()`.

- Layout, prepaint, paint, scene-animation sampling, and renderer-visible animation state for the same frame must use that timestamp.
- Do not introduce fresh `Instant::now()` animation samples inside element/render lifecycle code.
- Real monotonic time remains correct for frame throttle, deadlines, watchdogs, recent-input age, timers, timeouts, and profiling.

## Retained rendering contract

- Preserve the semantic distinction between `ElementOnly`, `ReconcileSubtree`, and `InvalidateSubtree`.
- Structural ancestors may be traversed to route reconciliation without becoming paint/layout dirty themselves.
- A precise retained target must not be widened to a full subtree or whole view unless correctness actually requires it.
- Generic view invalidation must not accidentally erase a compatible targeted replay plan.

## Animation scheduling

- Targeted animation scheduling uses latest-state-wins semantics: one outstanding request per target, no stale sample queue.
- Do not reintroduce a global recent-input/backpressure bypass. Coalesce work and reduce lifecycle cost instead.
- Renderer-owned visual animation should request presentation/scene animation work without forcing layout.

## Nova animation contract

- Stable primitives stay stable across visual-only animation frames.
- Shaders should consume stable animation bindings plus compact per-frame animation values.
- Do not restore per-frame clone/mutate/serialize/memcpy of every animated primitive as the steady-state path.
- GPU lookup must not scan all animation bindings per primitive.
- Text/SVG/Image/Quad descendants of one composite animation inherit the same visual transform/opacity owner.
- CPU blur/filter damage planning may sample lightweight bounds, but should not require full primitive rewrites.

Keep framework changes generic: no BMCBL page names, Minecraft-specific state, or application-specific policy in GPUI internals.
