# Backdrop Blur

[Chinese](backdrop_blur.zh-CN.md)

Backdrop blur is a renderer feature for blurring content behind a painted
primitive. It is part of GPUI scene rendering, not an application-level visual
policy.

## Scene Data

Elements that request backdrop blur insert blur primitives into the scene. The
renderer batches these primitives with the rest of the frame and records
diagnostic counts for performance metrics.

## Nova GPU Pipeline

The Nova renderer uses dedicated WGSL for backdrop blur. It separates the base
shader from blur-specific shader modules so feature pipelines can be created on
demand and released during retained resource trimming.

Backdrop blur rendering may allocate intermediate render targets. Keep blur
radii and primitive counts bounded in UI code that can create many overlapping
blurred regions.

## Guidelines

- Keep blur behavior deterministic and renderer-owned.
- Do not add application theme defaults to the renderer.
- Use metrics when diagnosing blur-heavy windows.
- Prefer local `#[expect(...)]` only for intentionally retained diagnostic or
  platform compatibility code.
