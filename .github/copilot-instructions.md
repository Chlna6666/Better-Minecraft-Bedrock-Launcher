# BMCBL Repository AI Instructions

The repository-wide source of truth for AI/code-agent behavior is the root
`AGENTS.md`. Read it before changing any BMCBL application or workspace code.

For `src/ui`, also read `src/ui/AGENTS.md`. For framework work under
`crates/gpui`, also read `crates/gpui/AGENTS.md` and
`crates/gpui/docs/animation_clock.md`. Scoped rules extend the root rules; they
do not replace or duplicate the repository-level contract.

Animation and retained-rendering changes must follow
`docs/GPUI_ANIMATION_CONVENTIONS.md` and `docs/GPUI_VENDOR_RENDERING.md`.

Hard invariant for BMCBL UI code: all current-frame visual animation sampling
uses the one timestamp exposed by `window.animation_time()`. Do not derive
current-frame theme interpolation, spring progress, opacity, transform, clip,
layout motion, text position, or renderer-facing animation state from a fresh
`Instant::now()` inside render/layout/prepaint/paint or helpers called by those
paths.

`Instant::now()` remains correct for real input/event timestamps, animation
retarget start events, timers, deadlines, retries, timeouts, backpressure,
watchdogs, and profiling.

When reviewing UI code, audit helper calls as well as `Render` bodies. Helpers
such as `theme_colors(cx)`, `current_theme_colors(cx)`, `render_*`, and nested
view/model builders must not hide a fresh clock sample. Pass the frame timestamp
or `Window` explicitly when visual output depends on time.

Prefer stable text geometry, retained/composite visual animation, narrow
invalidation, and latest-wins/coalesced animation scheduling. Never introduce a
global recent-input/backpressure bypass as an animation fix.

Do not use GitHub CI as an automatic repair loop for these performance changes;
keep commits small enough for the maintainer to build, profile, and A/B locally.
