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
| `crates/gpui/src/scene` | UI scene primitives, path data, batches, prepared scene data, bounds trees, transforms, and generic renderer extensions. |
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

## Presentation lane vs UI commit lane

GPUI treats renderer-owned animation presentation and UI scene generation as two different lanes.

```text
UI / entity state                         retained presentation
      │                                           │
      ├─ notify / dirty                            ├─ AnimationEngine tick
      │                                           ├─ update compact scene animation values
      ├─ View render / layout / paint              └─ present retained committed scene
      │
      └─ commit next immutable Scene snapshot
```

When a platform callback contains both a renderer-owned animation tick and dirty UI work, presentation runs first. The last committed retained scene is submitted with the newest animation values before callbacks, View rendering, layout, text shaping, or scene rebuilding run. Dirty UI work then builds the next scene snapshot for a later presentation.

Windows and Linux/FreeBSD Nova windows share one dedicated GPU owner thread. It creates devices, swapchains, window targets and extension renderers, encodes scenes, submits GPU work and destroys window resources. The thread-local device and pipeline registries are shared across windows on this owner. The UI produces immutable packets; native window owners hold producer proxies and native handles, without owning a Nova renderer.

Windows keeps winit windows, native input and DWM frame pacing on its native event loop. Wayland and X11 keep protocol objects, callbacks and native surfaces on their platform thread. Only owned handle descriptors cross to the GPU owner; the native surface remains alive until the window's GPU destruction barrier completes. macOS's native Metal renderer is outside this Nova owner path.

Linux/FreeBSD currently runs native protocol dispatch and GPUI UI work on the same event loop.
The GPU owner consumes submitted work independently, but a synchronous UI Render block also
delays new native presentation ticks. This GPU ownership split does not provide Windows's separate
native/UI host behavior on Linux; that requires its own native event-loop separation and validation.

The first visibility handshake waits for an actual submitted frame. Later packets enter a latest-wins queue without waiting for encoding or submission. Replaced and backend-deferred packets carry forward every unsubmitted scene and backdrop-source damage region. Resize, transparency and memory trim are command-order barriers; drawable-size acceptance queues an extent, and GPU target recreation happens on the next presentation. The owner processes one command per window dispatch so one busy window cannot monopolize queued work for other windows. A window's destruction waits for its GPU resources to be released; application shutdown drains windows and joins the GPU thread.

Presentation samples use the GPU frame's monotonic timestamp after queueing, without running UI layout or rebuilding its committed display list. Animation completions are returned only after successful submission. Windows, Wayland and X11 declare scene-animation ownership through `owns_scene_animations()`; queued/deferred timelines wait for owner completion reports. Backend readiness wakes the native frame lane when supported. A pending frame continues through native pacing; a GPU error requests a fresh UI commit. A static completed scene schedules no continuous owner work.

The blocked-Render lab checks whether animation presentation progresses while UI `Render` is blocked. This does not establish nominal-refresh continuity or physical scanout timing.

A UI commit produced after an early presentation sets needs_present and requests a follow-up presentation; it is not synchronously presented at the tail of the same expensive render callback. Dirty-to-present latency accounting therefore remains attached to the presentation that actually contains the committed UI state.

Engine-owned animation samples live in `PresentationState`, not in the committed `Scene`. `PresentationPacket` therefore carries a retained scene and a separate dynamic animation-value slice. Presentation ticks update only the dynamic state; they do not mutate display-list ownership or scene revision.

UI scene storage is Arc-backed only at the commit boundary. A completed UI scene is published into a latest-wins pending presentation slot; the presentation phase promotes the newest pending snapshot before renderer-owned animation sampling or GPU submission. Scratch storage may reuse the previous Scene allocation only after presentation ownership releases it; while an older snapshot is shared, scratch reset detaches to a fresh empty Scene instead of cloning or mutating the committed display list.

If multiple UI commits arrive before a successful submission, presentation damage is cumulative rather than relative only to the immediately previous UI frame. Dirty regions and backdrop-blur source damage preserve every unsubmitted transition, so skipping intermediate pending snapshots remains pixel-correct for partial presentation.

## End-To-End Frame Path

```mermaid
flowchart TD
    "Entity or window state changes" --> "cx.notify / window.refresh / animation request"
    "cx.notify / window.refresh / animation request" --> "Window schedules PlatformFrameRequest"
    "Window schedules PlatformFrameRequest" --> "Platform delivers frame callback"
    "Platform delivers frame callback" --> "Window::run_platform_frame"
    "Window::run_platform_frame" --> "Frame work decision"
    "Frame work decision" -->|"draw frame"| "Window::draw"
    "Frame work decision" -->|"present only"| "present_framebuffer_only"
    "Window::draw" --> "prepaint/layout"
    "prepaint/layout" --> "paint"
    "paint" --> "Scene + PresentationPacket"
    "Scene + PresentationPacket" --> "platform_window.draw"
    "platform_window.draw" --> "platform presentation mailbox or owner queue"
    "platform presentation mailbox or owner queue" --> "NovaRenderer::draw on presentation owner"
    "NovaRenderer::draw" --> "FrameUpload::encode"
    "FrameUpload::encode" --> "GPU buffers and atlas upload"
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

## PlatformFrameRequest

Platform wakeups carry two independent typed work domains: `UiCommitRequest` and
`PresentationRequest`. Callers do not manipulate boolean rendering-policy fields.

| Constructor | Meaning |
| --- | --- |
| `PlatformFrameRequest::ui_commit()` | Request fresh UI generation: layout, prepaint, paint, and a new retained scene commit. |
| `PlatformFrameRequest::presentation()` | Request presentation of the last committed retained scene without forcing UI generation. |
| `PlatformFrameRequest::ui_commit_and_presentation()` | Coalesce both domains into one platform wakeup while preserving their separate execution phases. |
| `PlatformFrameRequest::default()` | No work; used only as an empty pending slot. |

The combined constructor is a transport/coalescing representation, not a compatibility mode:
presentation semantics are never inferred from UI commit semantics, and UI generation is never
inferred from a presentation request.

Frame requests are coalesced before platform wakeup. GPUI also arms a watchdog
for a stalled callback; recovery reissues the same coalesced request through the
window's platform frame source. Animation sampling and presentation therefore
remain paced by that window's native frame callback instead of the watchdog
timer. Its deadline follows the configured window cadence or recent native
callback/presentation cadence and explicit inactive-redraw cadence. The first
deadline allows two missed intervals; repeated misses back off to a two-second
cap. Recovery pauses for completed frames whenever a window is hidden or
minimized, and pauses inactive windows unless inactive rendering is enabled.
It resumes when the window is visible, not minimized, and active or opted into
inactive rendering.

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
- `PlatformFrameRequest`;
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
- generic renderer extensions submitted by extension implementations.

Scene ownership lives under `crates/gpui/src/scene`:

| Area | Role |
| --- | --- |
| `primitive.rs` | Primitive data types for renderer upload. |
| `batch.rs` | Primitive batching and batch metadata. |
| `prepared.rs` | Prepared frame data. |
| `path.rs`, `path_builder.rs` | Path storage and path geometry. |
| `renderer_extension.rs` | Generic renderer-extension payloads and frame ordering. |
| `bounds_tree.rs` | Spatial data for bounds and dirty region support. |
| `transform.rs` | Transformation matrices. |

Scene data is generic. BMCBL-specific panels, pages, Minecraft concepts, or
asset names must not appear in this layer.

## PresentationPacket

After a successful draw, GPUI snapshots one lifetime-free presentation packet containing:

- an `Arc<Scene>` to the immutable retained scene;
- renderer-owned animation samples;
- the accumulated dirty region;
- backdrop-blur source damage;
- partial or full present mode.

The packet is moved across the Window/platform boundary. It does not borrow `Window`, `Frame`,
or `PresentationState`. On Windows/Linux/FreeBSD the type is compile-time checked as
`Send + Sync` and is used by the Windows scene mailbox and Linux presentation owner queue.
macOS is intentionally excluded from that cross-thread contract until CoreVideo surface
attachments are separated from the generic Scene.

Dirty region behavior:

- full redraw is used for the first frame, forced redraws, unsupported partial
  cases, and large coalesced regions;
- partial present is used when dirty retained scene segments can be bounded
  safely;
- backdrop blur can expand the dirty region because it samples previous
  content;
- animation dirty bounds can request partial redraw;
- unsupported batches force a safe fallback to full redraw.

`Window::present` calls `platform_window.draw(packet)`. Presentation-only frames call
`platform_window.present_framebuffer_only(packet)`.

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
| `renderer.rs` | Renderer state, draw entry point, retained resources, memory trim, and backend-independent orchestration. |
| `renderer/init.rs` | Surface, swapchain, and per-window resource initialization on a device and renderer core resolved from the sharing registries. |
| `renderer/draw_steps.rs` | Conversion from frame upload data to render step descriptors. |
| `renderer/present.rs` | Buffer upload, atlas upload, offscreen passes, direct swapchain rendering, and submission. |
| `renderer/submission.rs` | GPU submission and pending submission handling. |
| `renderer/surface_lifecycle.rs` | Resize and surface lifecycle behavior. |
| `frame_upload` | CPU packing of scene primitives into GPU upload buffers. |
| `resources` | Buffer, texture, depth, shader, pipeline, and resource set creation. |
| `resources/core.rs` | Cache of the renderer core shared by windows that agree on device and color format. |
| `device.rs` | Device keys and the registry that shares one backend device between windows. |
| `shader_artifacts.rs` | Build-generated shader artifact table included from `OUT_DIR`. |
| `shader.rs`, `shaders/*.wgsl` | Generated artifact lookup; WGSL source constants are compiled only for shader regression tests. |
| `atlas.rs`, `atlas_resources.rs` | Sprite atlas management and GPU atlas synchronization. |
| `swapchain.rs`, `surface.rs`, `surface_plan.rs` | Surface and swapchain handling. |
| `diagnostics.rs`, `upload_metrics.rs` | Renderer diagnostics and upload metrics. |

### Build-Time Shader Artifacts

Components declare their WGSL in a build script instead of compiling shaders at
runtime. `gfx-shader-build` provides the reusable surface: `ShaderSet::new`,
`Shader::wgsl_file`/`wgsl`/`entry`, `BackendSelection`, and `ShaderSet::emit`,
which writes a generated table plus the backend payloads (`dxbc`, `spv`, `msl`)
into `OUT_DIR` and registers `cargo:rerun-if-changed`. Both `crates/gpui` and
`crates/gpui-3d` use it, so a component adds shaders without reimplementing the
pipeline.

`gfx_core::EmbeddedShader` is the runtime side. Each generated table exposes
`{name}_{backend}_shader(entry_point) -> Option<EmbeddedShader>`, and
`EmbeddedShader::to_binary(stage, entry_point)` returns the compiled bytes for
the running backend.

BMCBL GPUI and gpui-3d sets use `Dx12ArtifactPolicy::RequireBytecode`: Windows
artifacts must complete WGSL -> HLSL -> DXBC during the build. Cross-building a
Windows target on a host that cannot run `D3DCompile` is a hard build error rather
than an HLSL fallback. The production `gfx-dx12` dependency also leaves its
`shader-compiler` feature disabled, so an accidentally supplied HLSL module is
rejected instead of calling FXC during renderer creation. Only explicit shader
tools/examples opt into that feature. A missing generated artifact remains an
error rather than a silent runtime translation path. Production shader lookup now
constructs `ShaderBinaries` directly from the generated artifact table; the old
source-plus-compiler callback and WGSL source constants are test-only, so release
startup does not carry the WGSL translator path through Nova initialization.

`gfx-shader-build` parses and validates each WGSL bundle once, then reuses the validated Naga module for every entry point and enabled backend. On a Windows build host it invokes the minimal FXC binding directly rather than depending on the complete `gfx-dx12` runtime crate. This keeps device, swapchain, allocator, and presentation code out of the shader build-script dependency graph.

### Shared Device And Compiled Pipelines

Windows of one process render through one backend device and one set of compiled
pipelines when they agree on adapter and surface format.

`device.rs` keys devices by `DeviceKey` (`backend`, `adapter_name`,
`power_preference`) and hands out `Arc<Mutex<NovaBackend>>`. `NovaRenderer` holds
that handle and locks it for one backend operation at a time. `resources/core.rs`
caches the renderer core — resource and pipeline layouts, the render pass, the
compiled shader modules, and the render pipelines — per device key and color
format, and `create_renderer_resources` adds only the parts sized to one window
(path mask target, depth texture, frame resources). The renderer core is
size-independent: DX12 and Vulkan both ignore the viewport extent at pipeline
creation and drive viewport and scissor as dynamic state, so windows of different
sizes share it.

Renderer initialization runs on a dedicated `gpui-renderer-init` thread
(`crates/gpui/src/platform/windows/renderer_init.rs`) instead of the shared
background executor. Initialization must be serialized onto one thread because
the registries are keyed per creating thread; the finished renderer is still
handed to the window's own thread, which draws.

Two limits are intentional and current:

- The registries are thread local because a DX12 device is not `Send`: it holds
  `HANDLE`, `IUnknown`, and mapped-upload `NonNull` pointers. A process-wide
  registry would require an `unsafe` `Send` assertion for the device.
- Linux gives each window its own presentation thread, so windows there do not
  share a device yet.

### Measured Frame Cost

`GPUI_NOVA_RENDER_DIAGNOSTICS` enables the per-frame copy attribution line, which
reports where a frame actually moves bytes and pixels. Measured on BMCBL's main
window plus its debug window:

| Quantity | Steady-state frame | First frame |
| --- | --- | --- |
| Atlas texture upload | ~3-5 KB in 1-6 regions | 4.9 MB in 45 regions |
| Mapped frame upload | ~9 KB | ~10 KB |
| Blur pixels actually processed | ~5 K | ~131 K |
| Blur render passes | 3 | 7 |

The steady-state figures are the ones to optimize against; the first frame
populates the whole atlas and is a startup cost, not a recurring one.

Points that were measured rather than assumed, so that they are not re-opened as
speculative work:

- The only explicit copy in a frame is the atlas texture upload. The mapped frame
  upload is a CPU write into host-visible memory
  (`mapped_frame_upload_is_gpu_copy=false`), so packing scene data into a staging
  vector and then copying it into the mapped page costs one extra pass over about
  100 KB in a dense frame, roughly 0.1% of a 60 Hz frame budget. A zero-copy
  encoder is not worth the backend API it would require.
- The frame path creates no resources: `present.rs`, `draw_steps.rs`, and
  `renderer.rs` call no `create_*` and no descriptor `validate()`, so pipeline
  labels and creation-time validation never run per frame.
- The blur path is damage local (`blur_source_mode=damage-local-retained-filter`);
  `blur_full_target_pixels` is the size of the full target, not the work done.
- Atlas uploads are gated on the atlas texture-set generation and uploaded as
  regions, and the DX12 upload ring reuses pages and trims idle ones through
  `MemoryTrimLevel` rather than reallocating per upload.



## nova-gfx Frame Path

`NovaRenderer::draw(packet)` and `present_framebuffer_only(packet)` run this sequence on the GPU owner:

1. Apply pending drawable-size changes.
2. Observe packet damage and resolve the full or partial surface plan.
3. Determine backdrop blur quality and encode the retained scene into `FrameUpload`.
4. Ensure the path-mask target and prepare renderer-extension draw steps.
5. Update the damage-aware blur cache plan and ensure blur targets if needed.
6. Call `draw_present(upload, packet)`.

The per-window `RendererRegistry` keys instances by `RendererExtension::renderer_type()`. Inputs
remain immutable scene nodes; their shared mutable renderer stays on the GPU owner, including
creation, trim and removal when the active retained scene no longer uses that renderer type.

`FilterRegistry` owns backdrop target variants, quality and cache validity. `RenderTarget` groups
a texture with its view. Source-damage provenance, isolated blur sources and partial-redraw
eligibility remain in the existing damage plan and draw-step logic. Cache validity is recorded
only after successful submission; a deferred refresh remains invalid.

`RendererOptions` expresses application intent. `BackendCapabilities` describes backend facts;
`PresentationCapabilities` queries a live swapchain's native partial-presentation and readiness
notification support. Missing or stale swapchains return unsupported capabilities. These facts
do not turn a requested present mode, a fixed queue latency or an unqueried MSAA limit into a
reported hardware capability.

`draw_present` then:

1. Prepares the backend for frame submission.
2. Syncs atlas textures.
3. Determines partial present scissor eligibility.
4. Builds draw steps, path mask steps, and backdrop blur source steps.
5. Records GPU pass metrics.
6. Uploads frame buffers.
7. Uploads pending atlas pages.
8. Runs offscreen path-mask passes and refreshes backdrop blur only when its source changed.
9. Renders the main scene directly to the swapchain.
10. Presents the frame through the swapchain.
11. Records diagnostics.

## Frame Upload Buckets

`FrameUpload::encode` groups scene data into upload buckets:

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
- renderer-extension input references and ordered batch descriptors. Extension renderers prepare
  their draw steps before the frame is submitted; those resources are not packed into GPUI upload
  buffers.

The renderer writes only non-empty buckets where possible. Atlas uploads are
handled separately through the GPUI sprite atlas and backend atlas textures.

## GPU Passes

The nova path may run these GPU passes:

| Pass | Purpose |
| --- | --- |
| Path mask pass | Rasterizes vector path masks to an offscreen texture. |
| Backdrop source pass | Captures source content for blur sampling. |
| Backdrop blur passes | Builds downsampled and blurred textures for backdrop blur primitives. |
| Main pass | Draws GPUI quads, shadows, paths, sprites, text, underlines, and composited blur. Renderer extensions submit their own draw work through the generic extension lifecycle. |
Nova keeps every rotating back buffer coherent by rendering directly to the
swapchain. It does not allocate a full-size retained present texture and does
not run a second full-screen present-copy pass.

When the active backend exposes native presentation damage, Nova forwards
GPUI's unioned dirty region as presentation metadata without scissoring the main
render pass. DX12 uses `Present1` dirty rectangles for DirectComposition
`FLIP_SEQUENTIAL` swapchains. Vulkan uses `VK_KHR_incremental_present` when the
device advertises it. Unsupported swapchains keep the same direct-render path
and perform a regular full present.

Backdrop blur remains a source, horizontal filter, vertical filter, and composite
render pipeline. Those render-target passes are distinct from texture upload copies.
Nova uses a two-pass separable Gaussian filter; the CPU precomputes the weights and
axis-specific offsets once per active configuration in the retained frame upload.
GPUI conservatively expands source damage across the complete sampling footprint
before passing the region to native presentation. Nova retains filtered blur targets
across frames. A titlebar or list animation painted above the first blur primitive
reuses those targets; source scene changes, blur parameter or quality changes, atlas
pixel uploads, resize, alpha-mode changes,
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

## Retained Packed Chunks

The retained element semantic generation is also the source of truth for packed
chunk identity. GPUI records only exact, stable subtree paths; Nova does not
derive chunk identity from a shortened hash or an application-provided ID.
Nested candidates are collapsed to the largest safe span.

The first production slice promotes a span only when it contains at least 32
static quads, has an exclusive draw-order interval, and contains no layer, blur,
surface, renderer-extension, animation, or mixed-pipeline barrier. On a partial dirty
frame, a replayed chunk with the same identity and generation reuses its packed
quad bytes. Static signature construction combines the cached chunk token and
hashes only uncached byte spans. A generation change, reordered/nonexclusive
draw order, or any barrier uses normal encoding and whole-stream fallback.

Each frame-resource slot also retains the exact identity, generation, byte range,
and packed-content hash of the quad chunks successfully presented through that slot. When a later
frame keeps a chunk at the same range, Nova writes only the dirty gaps around the
resident chunk. A generation change dirties that chunk; a changed position,
invalid range, or uninitialized slot conservatively falls back to the complete
quad stream. Slot selection waits for the submission that owns the slot before
any range is overwritten, so DX12 and Vulkan share the same fence-safe rule.

This fixed-layout residency is the first GPU slab slice. It avoids uploading a
clean chunk when a sibling changes without changing draw offsets or painter
order. It is not yet a movable/free-list arena: compacting or relocating chunks,
reusing retired holes, and drawing noncontiguous allocations require explicit
submission retirement and draw-step cost validation before they become a
production path.

## Retained Resources And Memory Trim

GPUI keeps renderer resources across frames:

- shader modules and render pipelines;
- sprite atlas textures;
- frame upload buffers;
- retained packed quad chunks;
- draw step scratch buffers;
- backdrop blur targets;
- text layout and glyph atlas state.

Renderer extensions own their GPU resources and trimming policy. GPUI calls the
`RendererExtensionRenderer::trim_memory` hook for moderate and aggressive trims
after pending submissions drain; light trims do not call extensions. The default
hook is a no-op, so extensions release only resources they own and can recreate
from the current extension input.

On Windows, glyph antialiasing follows the destination window surface. Opaque
surfaces may use DirectWrite RGB ClearType coverage. Transparent or blurred
surfaces must request DirectWrite grayscale coverage and store it in the
monochrome atlas; converting an already rasterized ClearType mask to grayscale
in the fragment shader is not equivalent and produces jagged vertical curves
and small digits. The surface choice is part of the glyph atlas key so changing
window appearance cannot reuse coverage generated for the other mode. Nova
DX12 and Nova Vulkan share this rule.

Idle windows advance trim policy from no trim to light and moderate levels.
Trim may shrink retained CPU buffers, atlas capacity, and backend memory. Extension
implementations own the lifetime and trimming policy of their resources through the
trim hook. Trim must not change application state.

## Diagnostics

Useful diagnostics include:

- `performance_metrics_snapshot()` for frame decisions, draw/present/skip
  counts, layout metrics, scene metrics, image cache state, atlas usage, and
  renderer backend details;
- renderer startup logs for selected backend and first-frame data;
- frame budget warnings from `window/frame_lifecycle.rs`;
- upload metrics for GPUI frame buffers and atlas pages. GPUI does not collect
  3D viewport resource counts; those resources are owned by `gpui-3d`.

When changing renderer code, record what metric proves the change works. Do
not weaken rendering correctness to hit an arbitrary memory or CPU number.

### GPUI performance lab

`gpui_perf_lab` opens a real Nova window and writes one JSON report to stdout.
Run the same scenario separately for DX12 and Vulkan; do not substitute the
headless test platform for backend measurements.

```powershell
cargo run --manifest-path crates/gpui/Cargo.toml --example gpui_perf_lab --no-default-features --features nova-gfx-dx12 -- --backend=nova-dx12 --scenario=single-dirty --refresh-rate=120 --frames=600
cargo run --manifest-path crates/gpui/Cargo.toml --example gpui_perf_lab --no-default-features --features nova-gfx-vulkan -- --backend=nova-vulkan --scenario=single-dirty --refresh-rate=120 --frames=600
```

Available scenarios are `static-idle`, `single-dirty`, `scroll-10k`,
`cjk-cold`, `cjk-hot`, `texture-stress`, `overdraw-modal`, `effects`, and
`animation`. Except for the deliberately cold CJK run, the lab discards 120
warm-up frames. Reports contain raw samples and p50/p95/p99/max summaries.
Stage durations are CPU wall times: `layout` is the accumulated Taffy compute
time, `prepaint` includes layout work, and `backend_draw` covers Nova packing,
uploads, command submission, and presentation. GPU pass time still requires
PIX or RenderDoc, and frame pacing still requires PresentMon/ETW.

`static_stream_hits` and `static_stream_misses` count retained upload-mask
entries, not GPU write calls. A retained-key hit means the complete static
upload signature matched. Hashed bytes include static primitive streams and
the animation-topology signature input, excluding packed bytes represented by a
reused chunk token. `retained_chunk_hits`, `retained_chunk_misses`, and
`retained_chunk_reused_bytes` expose the chunk path separately. These definitions
must remain stable across before/after reports. `quad_upload_bytes` records bytes
actually written after fixed-layout retained ranges are removed, rather than the
full logical quad stream length.

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
- `crates/gpui/src/platform/nova/renderer.rs`
- `crates/gpui/src/platform/nova/renderer/present.rs`
- `crates/gpui/src/platform/nova/frame_upload`

## Review Checklist

Before merging GPUI or renderer changes:

- The change does not reference BMCBL product modules from `crates/gpui`.
- `PlatformFrameRequest` semantics are preserved.
- Static idle windows remain event-driven.
- Presentation-only frames do not rebuild layout unless required.
- Partial present has a safe full-redraw fallback.
- Dirty region changes account for backdrop blur and unsupported primitives.
- Renderer resource retention has a trim path.
- New GPU resources are included in diagnostics or are intentionally omitted.
- Errors propagate or are logged with enough context.
- A focused GPUI check or application check has been run.
