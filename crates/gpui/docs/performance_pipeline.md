# Performance Pipeline

[Chinese](performance_pipeline.zh-CN.md)

GPUI records renderer and UI metrics so frame pacing, resource growth, image
caches, and retained GPU resources can be diagnosed without adding
application-specific instrumentation to framework internals.

## Metrics Areas

Performance metrics cover:

- selected renderer backend;
- frame timing and draw time;
- image cache items, bytes, and evictions;
- sprite atlas and texture counts;
- backdrop blur primitive counts;
- GPU mesh resource counts;
- allocator totals where supported;
- retained resource trim activity.

## Retained Resources

The renderer keeps resources such as pipelines, shader modules, atlases,
backdrop blur targets, and mesh buffers across frames. Trimming should release
idle resources without changing application state.

## Guidelines

- Use metrics to confirm performance problems before broad refactors.
- Keep measurement code application-neutral.
- Prefer event-driven rendering for ordinary UI.
- Use continuous rendering only for windows that need it.
- Document new metrics when adding renderer features.
