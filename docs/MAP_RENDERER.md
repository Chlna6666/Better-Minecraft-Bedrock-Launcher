# Map Renderer

The map viewer uses the destructive `bedrock-render` session API. The important
rule is that the UI opens one render session per selected world/render backend
and streams visible tile events into GPUI instead of waiting for full metadata
indexing.

## Runtime Flow

1. `MapViewerWindowView::refresh_render_session` opens a read-only
   `BedrockWorld`, builds `MapRenderer`, and wraps it in `MapRenderSession`.
2. Metadata indexing still runs in the background for bounds, markers, and
   overview status.
3. `ensure_visible_tiles` plans only the current viewport and schedules missing
   tiles as soon as the session is ready.
4. `render_tile_batch_stream` calls
   `MapRenderSession::render_web_tiles_streaming_blocking` on a background task.
5. `TileStreamEvent::Cached` and `TileStreamEvent::Rendered` are converted into
   GPUI image batches immediately. `Failed` marks only that tile. `Complete`
   updates diagnostics and pipeline stats.

## Cancellation And Generations

World, dimension, mode, backend, layout, and cache-bypass changes cancel the
current `RenderCancelFlag`, increment the render generation, and ignore any
later events from the old task. This keeps fast drag/zoom interactions from
painting stale tiles over the new viewport.

## Cache Policy

Normal viewing uses `RenderCachePolicy::Use`. Cache bypass switches the same
session to `RenderCachePolicy::Bypass` for the next batch, which forces fresh
tile rendering without destroying the session. Cache keys include world id,
world signature, renderer version, palette version, dimension, mode, layout, and
tile coordinate.

## Logging

Debug builds initialize `tracing_log::LogTracer`, so `log` records emitted by
`bedrock-leveldb`, `bedrock-world`, and `bedrock-render` are visible in the app
logger. The debug filter enables:

```text
bedrock_leveldb=debug,bedrock_world=debug,bedrock_render=debug
```

Useful events include LevelDB table scans, key-only render-index probes,
parallel render chunk workers, session cache hits/misses, GPU backend selection,
and CPU fallback reasons.

## Status Bar Metrics

The map viewer should surface:

- render CPU percentage from app sampling
- `RenderPipelineStats::resolved_backend`
- `RenderPipelineStats::cache_hits` and `cache_misses`
- `RenderPipelineStats::peak_worker_threads`
- `RenderPipelineStats::gpu_fallback_reason`

These metrics come from streaming `Complete` events and from the app-level CPU
sampler. They are diagnostic hints, not a stable file format.

## Performance Notes

- Do not block first paint on full `list_chunk_positions` metadata.
- Prefer viewport tile batches over full map export in the GPUI window.
- Keep `MapRenderSessionConfig::cull_missing_chunks` enabled for interactive
  viewing so missing chunks are skipped before bake.
- Treat an indexed tile with zero chunks as `Invalid` in the UI. Do not send it
  to `render_web_tiles_streaming_blocking` and do not build a `RenderImage` for
  it.
- Validate rendered tile buffers before creating GPUI images. `rgba.len()` must
  be exactly `width * height * 4`; invalid buffers are data errors, not
  renderer input.
- Use the interactive render profile unless the user explicitly starts an
  offline export.
- Avoid nested worker pools: `bedrock-render` owns the batch scheduler and calls
  `bedrock-world` render loading with conservative inner threading.
