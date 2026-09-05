# BMCBL UI Agent Rules

These rules apply to every file under `src/ui`.

Read `docs/GPUI_ANIMATION_CONVENTIONS.md` before changing animation, layout, retained rendering, text positioning, frame scheduling, or visual effects.

## Animation clock — hard invariant

- Current-frame visual animation sampling MUST use `window.animation_time()`.
- Do not call `Instant::now()` from `Render`, `RenderOnce`, or element lifecycle code to derive animation progress, theme interpolation, spring values, opacity, transform, clip, or layout geometry.
- A single render may pass one `let now = window.animation_time();` through all parent/child helpers.
- Keep `Instant::now()` for actual event occurrence/start/retarget timestamps, timers/deadlines, retries/timeouts, and performance measurement.
- Never "fix" the rule by replacing event/scheduler clocks with frame time.

Before finishing a UI animation change, search the touched render path for `Instant::now()` and classify each occurrence.

## Animation geometry

- Visual-only animation must not move text layout origins or change text wrapping/baselines.
- Prefer final child geometry plus paint-only clip/compositor transform.
- Prefer renderer-owned scene animation or a retained `composite_layer()` for complex Text/SVG/Image/Quad subtrees.
- Use layout animation only for properties that genuinely affect layout.

## Invalidation and scheduling

- Prefer the narrowest retained target. Do not use whole-window refresh as an animation clock.
- Do not add a global frame-backpressure bypass.
- Interactive animation requests should coalesce/latest-win rather than queue stale samples.
- Static siblings and static text must remain retained whenever their output did not change.

## Validation

BMCBL development expects focused local build/profiling by the maintainer. Do not introduce GitHub CI as an automatic repair mechanism. Keep performance changes small enough to A/B and commit independently.
