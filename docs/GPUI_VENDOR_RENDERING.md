# GPUI Structure And Rendering Pipeline

This document describes the independently maintained GPUI framework structure used by BMCBL and
the current rendering pipeline from application invalidation to nova-gfx
presentation. It is a BMCBL-facing guide to `crates/gpui`; the framework's own
documents remain under `crates/gpui/docs`.

## Scope

This document covers:

- the main GPUI source directories and ownership;
- the public API surface exposed by `crates/gpui/src/gpui.rs`;
- BMCBL renderer startup integration in `src/app.rs`;
- frame invalidation and scheduling;
- element layout, prepaint, paint, and scene construction;
- dirty regions, retained scene reuse, presentation-only frames, and memory
  trimming;
- nova-gfx frame upload, GPU pass construction, partial present, and swapchain
  submission;
- diagnostics and rules for future changes.

## Source Map

| Path | Responsibility |
| --- | --- |
| `crates/gpui/src/gpui.rs` | Public GPUI crate surface and re-exports. |
| `crates/gpui/src/app` | `Application`, `App`, contexts, entity map, globals, effects, async contexts, and actions. |
| `crates/gpui/src/window` | Window lifecycle, platform frame scheduling, drawing, presentation, input, focus, action dispatch, and window-local state. |
| `crates/gpui/src/element` | Element trait implementations and built-in elements such as `div`, text, image, SVG, list, canvas, and surface. |
| `crates/gpui/src/layout` | Layout engine wrapper, layout builders, layout cache, layout metrics, and conversion helpers. |
| `crates/gpui/src/text_system` | Fonts, fallback, line layout, wrapping, truncation, shaping, glyph rasterization, and text paint helpers. |
| `crates/gpui/src/scene` | Scene primitives, path data, batches, prepared scene data, bounds trees, transforms, and 3D mesh descriptors. |
| `crates/gpui/src/render_pipeline` | Renderer backend options, shader helpers, and SVG renderer bridge. |
| `crates/gpui/src/platform` | Platform windows, GPU backend adapters, clipboard, displays, keyboard, and test platforms. |
| `crates/gpui/src/platform/nova` | nova-gfx renderer integration, resources, pipelines, frame upload, swapchain, and backend-specific submission. |
| `crates/gpui/src/diagnostics` | Performance metrics, frame counters, inspector data, and diagnostic recording. |

## Public Surface

`crates/gpui/src/gpui.rs` re-exports the framework API used by application
code:

- app and entity APIs: `Application`, `App`, `Context<T>`, `AsyncApp`,
  `AsyncWindowContext`, `Entity<T>`, `WeakEntity<T>`, `Global`, `Subscription`;
- render APIs: `Render`, `RenderOnce`, `IntoElement`, `Element`, `AnyElement`,
  `div`, `img`, `svg`, `uniform_list`, layout builders, and style traits;
- window APIs: `Window`, `WindowOptions`, `WindowBounds`, titlebar options,
  actions, key bindings, focus, input, and drawing helpers;
- geometry and style: pixels, points, sizes, bounds, colors, fonts, backgrounds,
  borders, shadows, layout styles, and text styles;
- renderer APIs: `RendererOptions`, `RendererBackend`, `GpuPowerPreference`,
  `PresentModePreference`, `GpuSubmissionMode`, `RenderPolicy`, metrics, and
  backend enumeration;
- assets and images: `AssetSource`, `ImagePipelineConfig`,
  `BoundedImageCache`, animated image configuration, and render image support.

Application code should consume this public surface. It should not reach into
private GPUI modules unless it is deliberately changing framework internals.

## BMCBL Renderer Startup

BMCBL configures GPUI in `src/app.rs`:

```text
AppBootstrap::from_config(...)
  -> renderer_backend_from_config(...)
  -> gpu_adapter_name_from_config(...)
  -> Application::new_with_renderer_options(RendererOptions { ... })
  -> with_image_pipeline_config(...)
  -> with_default_font_or_platform_default(...)
  -> with_assets(AppAssets)
```

BMCBL-owned startup choices:

- renderer backend preference from launcher config;
- optional exact GPU adapter name;
- high-performance GPU preference;
- image pipeline budget and animated image policy;
- default font selection;
- BMCBL `AssetSource`;
- transparent or opaque window background choices.

GPUI-owned generic behavior:

- backend default selection per platform;
- renderer options and frame policy model;
- window frame scheduling;
- image pipeline implementation;
- rendering metrics and backend diagnostics.

## End-To-End Frame Path

```mermaid
flowchart TD
    "Entity or window state changes" --> "cx.notify / window.refresh / animation request"
    "cx.notify / window.refresh / animation request" --> "Window schedules RequestFrameOptions"
    "Window schedules RequestFrameOptions" --> "Platform delivers frame callback"
    "Platform delivers frame callback" --> "Window::run_platform_frame"
    "Window::run_platform_frame" --> "Frame work decision"
    "Frame work decision" -->|"draw frame"| "Window::draw"
    "Frame work decision" -->|"present only"| "present_framebuffer_only"
    "Window::draw" --> "prepaint/layout"
    "prepaint/layout" --> "paint"
    "paint" --> "Scene + FrameRenderPlan"
    "Scene + FrameRenderPlan" --> "platform_window.draw"
    "platform_window.draw" --> "NovaRenderer::draw"
    "NovaRenderer::draw" --> "NovaFrameUpload::encode"
    "NovaFrameUpload::encode" --> "GPU buffers and atlas upload"
    "GPU buffers and atlas upload" --> "GPU render steps"
    "GPU render steps" --> "swapchain present"
    "swapchain present" --> "Window::complete_frame"
```

The normal path is event-driven. A static idle window should not continuously
rebuild the scene or present frames solely because time passes.

## Invalidation Sources

GPUI schedules a frame when state changes or presentation is required.

Common sources:

- `cx.notify()` marks an entity dirty and schedules a dirty frame for the
  owning window.
- `window.refresh()` marks the whole window dirty and forces view cache refresh.
- `window.request_animation_frame()` schedules a layout-affecting animation
  update for the current view or root view.
- `window.request_animation_engine_frame(driver)` advances paint or GPU
  animation state without necessarily forcing a full layout pass.
- `window.on_next_frame(...)` schedules a callback after the next rendered
  frame and requests presentation.
- image animation and deadline invalidation schedule targeted future
  invalidations.
- platform input, focus, layout, and window events can mark state dirty through
  the normal invalidation path.

Important rule: use the narrowest invalidation. Prefer notifying the affected
entity over refreshing the whole window. Use presentation-only requests when
the scene is already prepared and only GPU output needs to be shown.

## RequestFrameOptions

Frame requests are expressed through `RequestFrameOptions`:

| Field | Meaning |
| --- | --- |
| `force_render` | A fresh scene is required. GPUI should rebuild layout, prepaint, paint, and submit a new frame. |
| `require_presentation` | Prepared content or GPU output needs presentation, but the scene may not need to be rebuilt. |

Typical request shapes:

| Request | Use |
| --- | --- |
| `force_render = true`, `require_presentation = true` | Dirty UI, active animation, or visible state change that needs a new frame. |
| `force_render = true`, `require_presentation = false` | Initial or deferred dirty work where presentation can wait until content exists. |
| `force_render = false`, `require_presentation = true` | Presentation-only frame or next-frame callback. |
| both false | No meaningful frame work. Avoid issuing this. |

Frame requests are coalesced before platform wakeup. GPUI also arms a watchdog
so a stalled platform frame callback can be recovered by running frame work
directly.

## Frame Scheduling And Decisions

The scheduling logic lives primarily in:

- `crates/gpui/src/window/frame_scheduling.rs`
- `crates/gpui/src/window/frame_lifecycle.rs`
- `crates/gpui/src/window/frame_lifecycle/throttle.rs`

Key behavior:

- dirty frames are coalesced when another frame is already scheduled;
- inactive windows can defer dirty work when a retained scene is already
  available and no presentation is pending;
- frame throttle can delay progressive work to protect frame pacing;
- animation engine ticks can request paint/GPU or layout follow-up work;
- `run_platform_frame` evaluates whether to draw, present retained content,
  defer inactive dirty work, or skip;
- retained resource trim policy is updated as windows remain idle.

The frame decision uses these inputs:

- dirty state;
- pending presentation;
- active or inactive window state;
- minimized state;
- `RequestFrameOptions`;
- next-frame callbacks;
- recent input;
- throttle state;
- retained scene availability.

## Draw Cycle

`Window::draw` is the CPU-side frame generation path.

Important files:

- `crates/gpui/src/window/draw.rs`
- `crates/gpui/src/window/layout.rs`
- `crates/gpui/src/window/paint.rs`
- `crates/gpui/src/window/draw_reuse.rs`
- `crates/gpui/src/window/paint_resources.rs`

The current lifecycle is:

1. Begin draw cycle.
2. Consume dirty entity invalidations.
3. Clear accessed entity tracking for the frame.
4. Prepaint root element.
5. Request and compute layout.
6. Prepaint inspector, deferred draws, prompts, drag layer, and tooltip where
   needed.
7. Build hit testing and dispatch data.
8. Paint root element and overlays.
9. Insert scene primitives into `next_frame.scene`.
10. Finish layout and text frame metrics.
11. Build dirty region and partial present mode.
12. Swap `next_frame` into `rendered_frame`.
13. Record accessed entities for future invalidation.
14. Mark `needs_present`.

Current GPUI still uses request-layout, prepaint, and paint. The vNext typed
frame context work is documented in `crates/gpui/docs/element_lifecycle*.md`,
but it is not yet the active element API.

## Layout

Elements call `Window::request_layout`, `request_measured_layout`, and
`compute_layout` during prepaint. The layout engine owns:

- style-to-layout conversion;
- measured layouts for text and custom elements;
- layout cache metrics;
- bounds calculation in window coordinates;
- rem size and scale factor conversion.

Layout APIs assert that they run in the correct draw phase. Paint code should
not mutate layout state.

## Paint And Scene Primitives

Paint methods add primitives to the frame scene:

- quads and borders;
- shadows;
- paths;
- underlines and strikethroughs;
- monochrome and polychrome sprites;
- text glyphs and emoji glyphs through the text system and sprite atlas;
- images and SVG output through element-specific rendering;
- backdrop blur primitives;
- custom 3D mesh primitives.

Scene ownership lives under `crates/gpui/src/scene`:

| Area | Role |
| --- | --- |
| `primitive.rs` | Primitive data types for renderer upload. |
| `batch.rs` | Primitive batching and batch metadata. |
| `prepared.rs` | Prepared frame data. |
| `path.rs`, `path_builder.rs` | Path storage and path geometry. |
| `mesh.rs` | GPU 3D mesh descriptors and draw ranges. |
| `bounds_tree.rs` | Spatial data for bounds and dirty region support. |
| `transform.rs` | Transformation matrices. |

Scene data is generic. BMCBL-specific panels, pages, Minecraft concepts, or
asset names must not appear in this layer.

## FrameRenderPlan

After a successful draw, GPUI builds a render plan from:

- the retained scene;
- dirty region;
- partial or full present mode;
- retained resource trim policy;
- visual effect quality.

Dirty region behavior:

- full redraw is used for the first frame, forced redraws, unsupported partial
  cases, and large coalesced regions;
- partial present is used when dirty retained scene segments can be bounded
  safely;
- backdrop blur can expand the dirty region because it samples previous
  content;
- animation dirty bounds can request partial redraw;
- unsupported batches force a safe fallback to full redraw.

`Window::present` calls `platform_window.draw(render_plan)`. Presentation-only
frames call `platform_window.present_framebuffer_only(render_plan)`.

## Renderer Backend Options

Renderer startup is configured with `RendererOptions`:

| Option | Meaning |
| --- | --- |
| `backend` | `Auto`, `NovaVulkan`, `NovaDx12`, `NovaMetal`, or `HeadlessTest`. |
| `adapter_name` | Optional exact GPU adapter name. |
| `power_preference` | Low-power or high-performance preference. |
| `present_mode` | Vsync, mailbox, or immediate preference. |
| `submission_mode` | Deferred or synchronous GPU submission policy. |
| `render_policy` | Event-driven, continuous, or on-demand. |
| `frame_metrics` | Extra frame metrics for profiling and diagnostics. |

`RendererBackend::platform_default()` currently resolves to:

- Windows: Nova DX12 when the DX12 feature is enabled, otherwise Nova Vulkan
  if available;
- Linux and FreeBSD: Nova Vulkan;
- macOS: Nova Metal;
- otherwise: `Auto`.

The configured backend is the only renderer selection input. Applications must
pass it through `RendererOptions` or `new_with_renderer_backend`; GPUI does not
read an environment-variable override.

## nova-gfx Renderer

The nova-gfx renderer integration lives under `crates/gpui/src/platform/nova`.

Major modules:

| Path | Responsibility |
| --- | --- |
| `nova_renderer.rs` | Renderer state, draw entry point, retained resources, memory trim, and backend-independent orchestration. |
| `nova_renderer/init.rs` | Device, surface, swapchain, resource, and pipeline initialization. |
| `nova_renderer/draw_steps.rs` | Conversion from frame upload data to render step descriptors. |
| `nova_renderer/present.rs` | Buffer upload, atlas upload, offscreen passes, direct swapchain rendering, and submission. |
| `nova_renderer/submission.rs` | GPU submission and pending submission handling. |
| `nova_renderer/surface_lifecycle.rs` | Resize and surface lifecycle behavior. |
| `nova_renderer/custom_mesh_pipeline.rs` | Custom 3D mesh pipeline management. |
| `nova_renderer/mesh_cache.rs` | Retained custom mesh buffer cache. |
| `frame_upload` | CPU packing of scene primitives into GPU upload buffers. |
| `resources` | Buffer, texture, depth, shader, pipeline, and resource set creation. |
| `shader.rs`, `shaders/*.wgsl` | Shader module loading and WGSL shader sources. |
| `atlas.rs`, `atlas_resources.rs` | Sprite atlas management and GPU atlas synchronization. |
| `swapchain.rs`, `surface.rs`, `surface_plan.rs` | Surface and swapchain handling. |
| `diagnostics.rs`, `upload_metrics.rs` | Renderer diagnostics and upload metrics. |

## nova-gfx Frame Path

`NovaRenderer::draw(render_plan)` runs this sequence:

1. Observe the render plan for metrics.
2. Resolve full redraw or partial surface plan.
3. Determine backdrop blur quality.
4. Encode the GPUI scene into `NovaFrameUpload`.
5. Ensure backdrop blur targets if needed.
6. Ensure custom 3D mesh pipelines for the current backend.
7. Call `draw_present(upload, render_plan)`.

`draw_present` then:

1. Prepares the backend for frame submission.
2. Syncs atlas textures.
3. Ensures custom 3D mesh cache resources.
4. Determines partial present scissor eligibility.
5. Builds draw steps, path mask steps, and backdrop blur source steps.
6. Records GPU pass metrics.
7. Uploads frame buffers.
8. Uploads pending atlas pages.
9. Runs offscreen path-mask passes and refreshes backdrop blur only when its source changed.
10. Renders the main scene directly to the swapchain.
11. Presents the frame through the swapchain.
12. Records diagnostics.

## Frame Upload Buckets

`NovaFrameUpload::encode` groups scene data into upload buckets:

- globals;
- text raster parameters;
- quads;
- shadows;
- path rasterization vertices;
- path sprites;
- monochrome sprites;
- polychrome sprites;
- underlines;
- backdrop blur pass descriptors;
- backdrop blur primitives;
- animation bindings and values;
- custom 3D mesh parameters.

The renderer writes only non-empty buckets where possible. Atlas uploads are
handled separately through the GPUI sprite atlas and backend atlas textures.

## GPU Passes

The nova path may run these GPU passes:

| Pass | Purpose |
| --- | --- |
| Path mask pass | Rasterizes vector path masks to an offscreen texture. |
| Backdrop source pass | Captures source content for blur sampling. |
| Backdrop blur passes | Builds downsampled and blurred textures for backdrop blur primitives. |
| Main pass | Draws quads, shadows, paths, sprites, text, underlines, custom mesh content, and composited blur. |
Nova keeps every rotating back buffer coherent by rendering directly to the
swapchain. It does not allocate a full-size retained present texture and does
not run a second full-screen present-copy pass.

When the active backend exposes native presentation damage, Nova forwards
GPUI's unioned dirty region as presentation metadata without scissoring the main
render pass. DX12 uses `Present1` dirty rectangles for DirectComposition
`FLIP_SEQUENTIAL` swapchains. Vulkan uses `VK_KHR_incremental_present` when the
device advertises it. Unsupported swapchains keep the same direct-render path
and perform a regular full present.

Backdrop blur remains a source, downsample, blur, upsample, and composite render
pipeline. Those render-target passes are distinct from texture upload copies.
GPUI conservatively expands source damage across the complete multi-level
Dual-Kawase sampling footprint before passing the region to native presentation.
Nova retains the filtered blur pyramid across frames. A titlebar or list animation
painted above the first blur primitive reuses that pyramid; source scene changes,
blur parameter or quality changes, atlas pixel uploads, resize, alpha-mode changes,
and target recreation invalidate it. This preserves the same blur shader and
quality while avoiding repeated source/downsample/upsample passes for unrelated
foreground animation.

## Presentation-Only Frames

`present_framebuffer_only()` is used when GPUI needs presentation without a new
layout or paint pass. Nova re-encodes the retained scene and presents it through
the direct swapchain path because it no longer keeps a second full-size present
cache.

This path is important for event-driven rendering because it allows GPU output
or platform presentation to happen without forcing a CPU scene rebuild.

## Retained Resources And Memory Trim

GPUI keeps renderer resources across frames:

- shader modules and render pipelines;
- sprite atlas textures;
- frame upload buffers;
- draw step scratch buffers;
- backdrop blur targets;
- custom mesh pipeline and mesh buffers;
- text layout and glyph atlas state.

Idle windows advance trim policy from no trim to light and moderate levels.
Trim may shrink retained CPU buffers, atlas capacity, custom mesh caches, and
backend memory. It must not change application state.

## Diagnostics

Useful diagnostics include:

- `performance_metrics_snapshot()` for frame decisions, draw/present/skip
  counts, layout metrics, scene metrics, image cache state, atlas usage, and
  renderer backend details;
- renderer startup logs for selected backend and first-frame data;
- frame budget warnings from `window/frame_lifecycle.rs`;
- upload metrics for frame buffers, atlas pages, and custom mesh buffers.

When changing renderer code, record what metric proves the change works. Do
not weaken rendering correctness to hit an arbitrary memory or CPU number.

## BMCBL Change Rules

Framework-level changes are appropriate when they affect generic GPUI behavior:

- frame coalescing;
- renderer option parsing;
- dirty region or presentation logic;
- generic image pipeline behavior;
- generic element, layout, text, scene, or platform behavior;
- nova-gfx backend integration.

BMCBL-level changes belong in application code when they reference:

- configured launcher renderer backend;
- BMCBL window size, transparency, title, or chrome;
- default backgrounds, fonts, or images;
- UI routes and pages;
- Minecraft, CurseForge, EasyTier, downloads, updates, plugins, or music;
- product diagnostics screens.

If the application needs a new renderer knob, add a neutral GPUI option and set
the BMCBL default from `src/app.rs`.

## Reference Files

Framework docs:

- `crates/gpui/docs/rendering.zh-CN.md`
- `crates/gpui/docs/renderer_backend.zh-CN.md`
- `crates/gpui/docs/windows_renderer_backend.zh-CN.md`
- `crates/gpui/docs/performance_pipeline.zh-CN.md`
- `crates/gpui/docs/element_lifecycle.zh-CN.md`

Implementation entry points:

- `src/app.rs`
- `crates/gpui/src/gpui.rs`
- `crates/gpui/src/render_pipeline/renderer_backend.rs`
- `crates/gpui/src/window/frame_scheduling.rs`
- `crates/gpui/src/window/frame_lifecycle.rs`
- `crates/gpui/src/window/draw.rs`
- `crates/gpui/src/window/layout.rs`
- `crates/gpui/src/window/paint.rs`
- `crates/gpui/src/scene.rs`
- `crates/gpui/src/platform/nova.rs`
- `crates/gpui/src/platform/nova/nova_renderer.rs`
- `crates/gpui/src/platform/nova/nova_renderer/present.rs`
- `crates/gpui/src/platform/nova/frame_upload`

## Review Checklist

Before merging GPUI or renderer changes:

- The change does not reference BMCBL product modules from `crates/gpui`.
- `RequestFrameOptions` semantics are preserved.
- Static idle windows remain event-driven.
- Presentation-only frames do not rebuild layout unless required.
- Partial present has a safe full-redraw fallback.
- Dirty region changes account for backdrop blur and unsupported primitives.
- Renderer resource retention has a trim path.
- New GPU resources are included in diagnostics or are intentionally omitted.
- Errors propagate or are logged with enough context.
- A focused GPUI check or application check has been run.
