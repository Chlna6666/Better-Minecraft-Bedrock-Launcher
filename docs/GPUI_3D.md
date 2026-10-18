# GPUI 3D Architecture And API

This document defines the workspace boundary and current contract for `gpui-3d`. It is the source
for 3D API ownership, scene preparation, scene-view and offscreen renderer lifetimes, glTF asset
import, examples, test assets, and GPU verification scope.

The crate now contains two complementary rendering paths over one scene/asset/material model:

1. The GPUI **scene-view** path renders inside the host compositor pass through the generic
   `RendererExtension` contract. It is the in-UI path used by Map Viewer and Skin Pack.
2. The **offscreen renderer** (`gpui_3d::OffscreenRenderer`) renders a multi-pass GPU frame owned by
   the application: directional shadow map, linear HDR PBR scene pass, half-resolution bloom, and an
   HDR post resolve (exposure, tone mapping, FXAA, grading), resolved into an offscreen texture that
   can be read back or handed to a compositor. It uses the public Nova device passed by the caller
   and never creates a second window renderer, swapchain, cache owner, or render queue.

A glTF 2.0 / GLB importer (`gpui_3d::load_path` / `load_bytes`) converts licensed test models and
application assets into the same `Scene`, `Mesh`, `Material`, `TextureAsset`, light, camera, and
animation-track types used by both rendering paths. The importer is CPU-only and validated against
malformed input, missing limits, path escape attempts, and unsupported required extensions.

## Ownership

```text
BMCBL map / skin preview data
    -> src/ui converts domain geometry and materials
    -> crates/gpui-3d Scene, Mesh, Camera, Material, SceneView
    -> crates/gpui RendererExtension lifecycle
    -> nova-gfx indexed draw steps and backend resources
```

- `crates/gpui-3d` owns backend-neutral scene objects, geometry validation, cameras, materials,
  animation sampling, CPU culling, ray queries, and the Nova-backed GPUI scene view.
- `crates/gpui` owns generic element, window, input, renderer-extension scheduling, and native
  backend integration. It must not define Minecraft meshes, BMCBL preview materials, or 3D product
  policy.
- `src/ui` owns conversion from BMCBL data and preview behavior. `gpui-3d` must not depend on
  `src/`, BMCBL crates, Minecraft asset names, or page state.
- `crates/nova-gfx` owns device-neutral texture, buffer, pipeline, and draw resource contracts.

The [cradiy/gpui `gpui_3d` branch](https://github.com/cradiy/gpui/tree/main/crates%2Fgpui_3d) is a
feature and boundary reference, not an upstream dependency or a claim about the upstream GPUI API.
This workspace keeps its own crate and API surface.

## Feature status and completion gates

The current crate is a foundation, not a complete match for the reference branch. Keep these gaps
visible until an API, implementation, example, and focused validation exist for each supported
feature:

| Area | Current local API | Remaining 3D work |
| --- | --- | --- |
| Geometry | Immutable indexed meshes, material parts, area-weighted normals, MikkTSpace tangents, per-triangle edge masks, five primitives, bounds, BVH queries, automatic indexed instancing; glTF primitives (indexed, non-indexed, morph-target base geometry, skinned bind pose) | GPU skinning/morph deformation, GPU geometry partial updates, compressed mesh extensions |
| Scene graph | Hierarchical generational nodes, mutation versions for retained-frame invalidation, node/subtree visibility respected by rendering, shadows, picking, and bounds, distance-selected render LOD levels (rendering uses the level mesh; bounds/raycast keep the authored mesh) | Screen-space/HLOD selection, layers/culling masks, user metadata, gizmo/overlay lines |
| Camera and picking | Perspective/orthographic projection, orbit/pan/zoom, bounds framing, CPU ray queries, glTF cameras, top-left viewport pick/project helpers, GPU object-id picking that matches the submitted frame | Depth GPU picking and asynchronous readback |
| Materials and lighting | Metallic-roughness and unlit materials, albedo/normal/occlusion/**metallic-roughness** maps, four light kinds, three alpha modes; offscreen path adds directional shadows, equirectangular image-based lighting (mip-prefiltered diffuse/specular plus analytic split-sum BRDF), environment sky background, linear HDR rendering with exposure and ACES/Reinhard tone mapping, bloom, FXAA-style edge filtering, and vignette/film-grain/dither grading | Prefiltered cubemap/irradiance probes and a real BRDF LUT, clustered Forward+, point/spot shadows, MSAA/TAA |
| Render paths | Retained GPUI scene-view extension; **offscreen multi-pass renderer** (shadow, HDR PBR scene, half-resolution bloom, HDR post) on an application-owned Nova device with mesh/texture caches, resource-set reuse, readback, resize, trim/budget eviction, hash-tracked incremental uploads, reusable CPU staging buffers, a bounded mesh-buffer reuse pool, dynamic resolution/supersampled render scale, deferred GPU-completion polling before capture, and per-frame counters | GPUI compositor hand-off of an offscreen target (no CPU readback), GPU-driven indirect rendering, temporal effects |
| Scene scale and reuse | Retained draw list with frustum culling, instanced batches, static-frame reuse, camera-only re-cull without a scene walk, per-element hash-tracked uploads, verified 200,000-instance field in one indexed draw | GPU-driven indirect submission, LOD, occlusion culling, GPU-side scene traversal |
| Animation and deformation | Absolute-time TRS tracks, STEP/LINEAR/CUBICSPLINE glTF import, CPU pose evaluation, two-clip crossfade blending (`evaluate_blended` / `render_blended`), animated offscreen frame API (`render_animated`), instancing; animation clip sampling matches the authored hierarchy | Skeletal skinning, morph weights, multi-layer/synchronized clip graphs, IK |
| Asset pipeline | glTF 2.0 / GLB import with buffers, data URIs, external files, sparse accessors, materials, textures + generated mip chains, node/TRS/matrix transforms, KHR_lights_punctual, KHR_materials_unlit, KHR_materials_emissive_strength factors, cameras, transform animations; limits and path-escape checks | KHR_texture_transform, KHR_materials_clearcoat/transmission/ior/variants shading, KTX2/Basis, Draco/meshopt |
| Post-processing and quality | RGBA16Float linear HDR scene target, half-resolution bloom, FXAA-style edge blend, exposure/tone-mapping/gamma resolve, vignette, film grain, ordered dithering, Low/Medium/High/Ultra presets, dynamic-resolution and supersampled render scale; debug and object-id frames bypass grading | MSAA, TAA, motion blur, SSAO/SSR, depth of field, LUT color grading |
| Output and capture | GPUI scene-view element; offscreen PNG captures through `scene_lab`, readback pixels, deferred submission polling before readback, scriptable debug views (albedo/normal/depth/UV/shadow/object-id) | True asynchronous GPU-to-CPU readback and compositor hand-off, multi-frame trace export, GPU timestamped pass timings |
| Test assets | `test_assets/manifest.toml` + SHA-256 downloader + licenses, offline cache, ignored native tests, GPU pixel matrix | Additional licensed large-scene and animation assets |

These areas follow the feature categories in the [reference branch's crate guide](https://raw.githubusercontent.com/cradiy/gpui/main/crates/gpui_3d/README.md);
they are not implemented merely because they appear in this checklist. A feature is ready for
application use only after the generic API and renderer agree on ownership, validation, lifecycle,
and failure semantics, and a runnable example exercises the public path.

The source/build migration gate is complete: Map Viewer and Skin Pack use the new scene view, the old
GPUI mesh path is absent, and GPUI plus `gpui-3d` compile with the generic `RendererExtension`
boundary intact. No deprecated wrapper, alias, or parallel old/new rendering path remains. Native
DX12 and Vulkan pixel smoke checks pass for the scene-view example; frame-time evidence is still
missing. Shader compilation alone does not close that performance gate. Metal texture support
remains unavailable until upload and real-window rendering are implemented and checked.

## Scene and frame flow

1. Convert application geometry to immutable indexed [`Mesh`](../crates/gpui-3d/src/mesh.rs)
   assets and attach them to [`Node`](../crates/gpui-3d/src/scene.rs) values.
2. Attach `Material` values by mesh-part slot and lights to nodes. Parent transforms compose with
   child transforms; scene nodes contain no GPUI entity or application state.
3. Choose a [`Camera`](../crates/gpui-3d/src/camera.rs) and create a `SceneView` that owns an
   immutable `Arc<Scene>` snapshot plus its texture table.
4. Render with `gpui_3d::scene_view(Arc<SceneView>)`, which fills its available layout bounds, or
   pass the scene view to `window.paint_renderer_extension(...)` from an application-owned canvas.
5. GPUI routes the extension through its generic renderer lifecycle. `gpui-3d` prepares visible
   draw ranges, uploads changed mesh/texture assets, groups compatible adjacent draws into indexed
   instanced Nova steps, and keeps blended draws individually ordered.

`SceneView::with_scene`, `with_camera`, `with_textures`, `with_animation`, `with_animation_time`,
`with_projection_region`, `with_projection_inset`, `with_blend_edge_feather`, and `with_anisotropy`
preserve the scene-view identity. Keep it stable while updating a preview so retained GPU buffers
remain reusable. Creating a new scene view every render creates a new cache owner and prevents that
reuse. Replacing the scene clears animation tracks because they contain handles owned by the prior
scene; attach replacement tracks explicitly.
`with_projection_inset(max_pixels, fraction)` applies a per-axis inset of
`min(max_pixels, axis_length * fraction)` to the element bounds before intersecting the content mask.
`ProjectionRegion::ElementBounds` is the default. `VisibleContent` maps the camera projection to the
intersection of the extension bounds and the frame's rectangular `content_mask.bounds`; Nova still
applies the host scissor to every draw step. Rounded content-mask corners are not converted to 3D
coverage. `VisibleSquare` centers a square inside that same intersection for previews that need a
stable square camera region. `with_blend_edge_feather` multiplies fragment alpha only for blended
draws at the selected projection rectangle's edges; opaque and masked draws keep their existing
alpha and depth behavior. Its edge ramp is linear in pixels to match Map's existing transparent-edge
fade. A width of zero disables the fade.

`Mesh::with_edge_masks` attaches one [`TriangleEdgeMask`](../crates/gpui-3d/src/mesh.rs) to each
triangle in index-buffer order. The three bits select edges opposite triangle vertices 0, 1, and 2.
Importers can smooth outer edges while leaving triangulation seams untouched. Meshes with at least
one selected edge expand to three triangle-local GPU vertices per triangle so the shader can
interpolate barycentric coordinates; unmasked meshes retain the original shared indexed upload.
The CPU sidecar costs one byte per triangle. An enabled mesh uploads 64 bytes per index instead of
64 bytes per shared vertex; meshes with tangents add a separate 16-byte tangent frame per uploaded
vertex. Unmasked geometry keeps its shared indexed upload. The public `Vertex` layout and CPU
BVH/query order do not change.

## Geometry and transforms

`Mesh::new` validates finite vertex attributes, triangle-aligned indices, index bounds, and
contiguous material-part coverage. Use `Mesh::with_parts` when one vertex/index buffer contains
several material slots. Built-in geometry includes cube, plane, UV sphere, cylinder, and cone.
`Mesh::with_vertices` returns a new snapshot while keeping the vertex count, indices, material parts,
and triangle edge masks fixed. It revalidates the vertex attributes, rebuilds mesh and per-part bounds
plus the ray-query BVH, clears tangent frames, and assigns a fresh mesh identity. The source mesh
remains usable. When the replacement is submitted, the scene view uploads its geometry as a whole;
this API does not write into a buffer that may still be used by an in-flight frame. The `scene`
example shows replacing positions while retaining the original snapshot.
Vertices carry local position, normal, UV, and linear RGBA color. Imported geometry with missing
normals can call `Mesh::generate_normals`; it computes area-weighted normals per shared index and
rejects degenerate triangles or unreferenced vertices. Duplicate vertices preserve hard edges;
regenerating normals clears stored tangent frames, which must then be regenerated or reattached.
`Mesh::generate_tangents` computes MikkTSpace tangent frames for UV set zero and splits vertices
when a corner needs a different frame, including mirrored UV seams. It preserves triangle order,
material parts, and edge masks, and returns an output-to-source vertex map for external attributes.
It requires at least one triangle with nonzero position and UV area, plus usable indexed normals;
MikkTSpace supplies tangent frames for the remaining corners.
Importers with authored tangent frames can attach them with `Mesh::with_tangents`; they must split
vertices at tangent seams and provide one handedness sign per triangle. Both APIs keep tangents as
an optional CPU and GPU stream, so meshes without normal maps do not retain or upload tangent data.

`Node::with_pixel_offset` shifts rasterized pixels after camera projection; positive X moves right
and positive Y moves down. `Node::with_depth_bias` changes normalized zero-to-one depth after
projection; positive values move toward the near plane. Both reject non-finite input and leave
scene transforms, world bounds, and CPU ray queries unchanged. Selected triangle-edge coverage is
applied only to blended materials, uses fragment barycentrics and screen-space derivatives, and is
stored independently from vertex alpha.

`Transform` is translation, quaternion rotation, and scale. Use parent nodes for shared placement
and articulated hierarchies. `Scene::evaluate` samples absolute-time translation, rotation, and
scale tracks without changing authored nodes. `SceneView::with_animation` validates and attaches
tracks, while `with_animation_time` selects a clip sample. Scene-view preparation and `SceneView::raycast` use
the same sampled pose. Repeated clip-time changes currently evaluate node transforms on the CPU,
reuse the renderer's transform-map capacity, rebuild prepared draw records, and upload draw data;
this API is suitable for static poses and scrubbing, but it is not an independent high-rate
presentation lane.

GPUI-visible animation samples must use the current `window.animation_time()` value. Capture event
time when starting or retargeting a motion; do not sample `Instant::now()` inside the render path.
`gpui-3d` deliberately has no playback clock or task that wakes GPUI each frame. The application
owns clip time and the GPUI/native frame scheduler owns presentation cadence. Because each changed
sample currently rebuilds prepared CPU data, use the track API for pose evaluation and scrubbing;
continuous high-rate motion needs GPU-side retained transform evaluation before it can meet the
presentation-lane contract. For repeated CPU sampling, `Scene::evaluate_with` accepts reusable
`AnimationScratch` storage and `AnimationScratch::trim` releases its retained map capacity.

## Camera and queries

The camera uses a right-handed view matrix and zero-to-one clip-space depth, matching Nova's DX12,
Vulkan, and Metal projection conventions. Camera rays accept normalized device coordinates with
`(-1, -1)` at lower-left and `(1, 1)` at upper-right. `SceneView::raycast` accepts pixels from the
viewport's top-left and performs the Y conversion.

`OrbitCamera` is a backend-neutral Y-up controller for pointer-driven camera behavior. `orbit`
accepts yaw/pitch deltas in radians, `pan` translates the target along camera right/up in world
units, and `zoom` changes perspective distance or orthographic view height. `fit_bounds` accepts a
world-space [`Aabb`](../crates/gpui-3d/src/math.rs), preserves the current orbit direction, centers
the target on the bounds, and fits all eight corners using the requested viewport aspect and padding
factor. Perspective fitting moves the eye; orthographic fitting adjusts view height. Both preserve
the existing near/far planes and return `BoundsOutsideClipRange` if they cannot contain every
corner. Use `Scene::bounds` or `EvaluatedScene::bounds` to get complete scene bounds for the authored
or sampled pose. Both include all non-empty meshes, including meshes outside the current camera
frustum.
These operations return a validated `Camera` snapshot; the application owns pointer gestures and
decides when to replace the scene-view camera. The `scene` example exercises framing together with
orbit, pan, and zoom without coupling the crate to GPUI input.

`Scene::bounds` composes parent transforms and unions every non-empty mesh's world-space bounds.
`EvaluatedScene::bounds` uses the sampled local transforms from that immutable pose, so camera
framing and draw preparation can use the same animation time. Empty scenes return `None`; invalid
transforms or bounds that overflow finite world coordinates return `SceneError::InvalidTransform`.

`PreparedScene` frustum-culls mesh parts and orders opaque/masked draws before blended draws;
blended draws are sorted back-to-front. `PreparedScene::draw_batches()` walks the prepared order
without allocating. It groups consecutive opaque or masked draws only when mesh identity and
generation, mesh part, and every material value match; the scene view submits each group as one indexed
instanced draw. Per-instance world and normal transforms, pixel offset, and depth bias live in the
instance buffer. Blended draws remain singletons because merging them could change compositing order.
The `scene` and `scene_view` examples show the API and a scene with shared mesh/material resources.
`Scene::raycast` and `EvaluatedScene::raycast` use the same mesh bounds/BVH query path. Reuse
`RaycastScratch` for direct scene queries. For viewport-local
pointer coordinates, `SceneView::raycast` converts top-left-origin pixels to camera NDC and uses the
scene view's animation sample; its size and position must be relative to the same projection rectangle
used for rendering. With `VisibleContent`, subtract that rectangle's origin from the pointer and use
the intersection's size; with `VisibleSquare`, use the centered square's origin and size. Reuse
`SceneViewRaycastScratch` to retain both traversal and animated transform storage across pointer events. CPU
queries do not yet account for alpha masks or blended surface visibility.

## Materials, alpha, and texture assets

- `Material::base_color` and emissive color are linear values. Metallic and roughness weights are
  clamped to `0..=1` during material validation.
- `SpotLight` uses a normalized local-space ray direction, positive finite range, and validated
  inner/outer cone angles in radians (`0 <= inner < outer < PI`). The node transform moves its
  position and direction and scales its range by the largest world-space axis scale. The shader
  combines point-light range attenuation with a smooth cosine falloff between the outer and inner
  cone; the inner cone has full spotlight intensity and the outer edge has zero intensity.
- `Material::occlusion_texture` samples the linear red channel using the mesh UVs. The
  `occlusion_strength` factor is clamped to `0..=1` and attenuates ambient illumination only; point,
  spot, and directional direct lighting is unchanged. A missing map binds a white fallback and does
  not add an AO texture sample.
- `ShadingModel::MetallicRoughness` uses the scene view's fixed microfacet shader; `Unlit` uses
  material and vertex colors without lights or tone mapping.
- `AlphaMode::Opaque` writes opaque color/depth. `Mask` discards fragments below `alpha_cutoff`.
  `Blend` uses premultiplied alpha and disables depth writes; the scene prepares blended draws in
  back-to-front order.
- A positive blended edge-feather width applies a linear pixel ramp to output alpha at the
  projection rectangle's edges only for `Blend` materials. Opaque and masked draws keep their
  normal replace pipeline and depth behavior.
- Create immutable RGBA8 textures with `TextureAsset::rgba8` for sRGB color or
  `TextureAsset::linear_rgba8` for data such as tangent-space normal maps. The
  `*_mip_chain` constructors accept level zero followed by caller-generated levels whose dimensions
  halve with floor rounding and clamp at one; incomplete chains are valid. Add the assets to the
  scene-view texture table and reference each asset ID from `Material`. GPUI-3D uploads every supplied
  level in one Nova batch, creates a view spanning the chain, and defaults to linear texel and
  mip-level filtering plus clamp-to-edge addressing. `SceneView::with_anisotropy(true)` opts into
  hardware anisotropic filtering; Vulkan uses the adapter's reported limit and falls back to the
  configured filters when sampler anisotropy is unavailable. Image decoding, color conversion before
  RGBA8 storage, mip generation, and asset caching remain application responsibilities.
- A lit `MetallicRoughness` material with `normal_texture` requires `Mesh::tangents()`. Normal-map
  assets must use linear color space; the renderer rejects sRGB normal maps. Call
  `Mesh::generate_tangents()`
  for UV set zero, or attach imported frames with `Mesh::with_tangents()`. The generator returns any
  seam-split vertices and the output-to-source map for application-owned attributes. The shader
  transforms and orthogonalizes the tangent frame before applying normal-map RGB.
- DX11, DX12, Vulkan and OpenGL Nova devices implement the texture upload path. `gfx-metal` does not currently
  implement pixel uploads; Metal compiles the scene-view WGSL but textured scene views are not supported
  there yet.

| Backend | GPUI 3D shader source compiles | RGBA8 texture and mip upload | Anisotropic sampling | Real scene-view proof in this work |
| --- | --- | --- | --- | --- |
| DX11 | Yes, build-time SM5.0 bytecode | Implemented; native texture/mip readback gates | Supported, opt-in | GPUI glyph/image pixels tested; strict-backend 3D example completed native present; 3D pixel inspection pending |
| OpenGL 4.5 | Yes, build-time GLSL 4.50 translation | Implemented; native texture/mip readback gates | Requires native anisotropic extension when requested | Windows GPUI glyph/image pixels tested; strict-backend 3D example completed native present; Linux and 3D pixel inspection pending |
| DX12 | Yes | Implemented, batched | Supported, opt-in | Geometry and shared-resource sphere instances visible in a native window; frame pacing and GPU draw count not measured |
| Vulkan | Yes | Implemented, batched | Opt-in when `samplerAnisotropy` is supported; otherwise uses requested min/mag filters | Geometry and shared-resource sphere instances visible in a native window; frame pacing and GPU draw count not measured |
| Metal | Yes | Not implemented | Not exercised; texture upload is unsupported | Pending; compile-only evidence |

The native `scene_view` example supplies a two-level sRGB albedo chain. DX12 and Vulkan transfer tests
write both the base image and a smaller mip in a single batch. Their `texture_write` benchmarks also
include a 128x128 RGBA8 eight-level upload case; it measures transfer cost, not frame pacing.

The current scene view has no shadow maps, environment-map lighting, skeletal deformation, morph
targets, application-defined material shader variants, or headless renderer. These are not silently
emulated by the BMCBL UI. Add them to `gpui-3d` with backend-independent scene/API semantics before
using them in application code.

## Resource lifetime and errors

Meshes and textures are immutable CPU assets addressed by generated IDs. `gpui-3d` caches their
GPU buffers and views per scene view, keyed by asset ID and mesh generation. Scene snapshots that
retain scene-view identity reuse compatible uploads; changed assets replace only their corresponding
resources. Each compatible opaque or masked draw batch owns one resource set bound to its first draw
slot; blended draws keep individual sets to preserve compositing order. A batch's first slot and the
scene's instance range are part of resource-set reuse checks.

Renderer-owned scene-view caches retire after 120 unused rendered frames, when a moderate trim finds
them idle for at least one second, or when an aggressive trim clears all scene-view resources. Light
trims preserve GPU scene resources. Shader and pipeline resources stay with the renderer so a
moderate trim does not force their recompilation.

Invalid scene material texture references fail scene-view preparation with an error. GPU allocation,
texture upload, shader compilation, and draw-resource creation errors propagate through GPUI's
renderer error path. Application code must surface failures through its existing preview state and
must not turn a failed 3D build into a blank success state.

## Examples and benchmarks

The crate examples are executable API guides. The `scene_view` element fills its parent bounds, so a
native example should give its containing layout a finite size (the included window example uses
`size_full()`):

```powershell
cargo run -p gpui-3d --example scene
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx11
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-opengl
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx12
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-vulkan
```

For an unattended native-present check, append `--auto-exit-ms=5000`. The example disables backend
fallback, reports the actual backend and observed native presents, and returns an error if no present
is observed within the deadline. A successful present does not validate the scene's pixels.

The BMCBL adapters show where product data crosses the boundary: [Map Viewer preview
conversion](../src/ui/window/map_viewer/preview_3d.rs) and [Skin Pack preview
conversion](../src/ui/window/skin_pack/mesh.rs). Keep these adapters in `src/ui`; the reusable
examples and crate must not import BMCBL types.

`scene` demonstrates shared mesh/material instances and reports visible draw and batch counts, along
with normal and tangent generation from imported triangle positions, authored and evaluated scene
bounds, framing from the sampled pose, orbit/pan/zoom controls, orthographic projection, repeated
absolute-time sampling with `AnimationScratch`, and both direct and viewport-local ray queries. Its
scene-view ray query follows the same sampled pose as rendering and reuses `SceneViewRaycastScratch`. `scene_view`
demonstrates a native GPUI window with a two-level albedo mip chain, three shared-resource sphere
instances, PBR and unlit materials, ambient/directional/point/spot lights, normal and occlusion maps, alpha blending, a responsive inset
before visible-content clipping, a one-pixel linear fade on blended edges, and a fixed sampled animation pose. The current window
example is not autonomous playback. Both examples are covered by
`cargo check --locked -p gpui-3d --all-targets --all-features`.

The Criterion benches measure CPU primitive mesh and MikkTSpace tangent generation, fixed-topology
vertex snapshot rebuilding for a 64-by-32 UV sphere, authored and evaluated scene bounds, scene
preparation/retained-capacity reuse, animated transform evaluation plus prepared-scene update,
compatible draw-batch discovery at 100/1,000/10,000 draws, and BVH ray queries. Run a focused group
when comparing these CPU costs:

```powershell
cargo bench -p gpui-3d --bench scene
cargo bench -p gpui-3d --bench scene -- scene/batching
cargo bench -p gpui-3d --bench scene -- scene/bounds
cargo bench -p gpui-3d --bench scene -- vertex-snapshot/uv-sphere-64x32
cargo bench -p gpui-3d --bench scene -- generate-tangents/uv-sphere-64x32
cargo bench -p gpui-3d --bench raycast
```

The `gpui-3d` benches measure CPU work; they do not measure GPU execution, native present latency,
input-to-photon delay, or frame pacing. Nova's backend benches measure an eight-level 128x128 RGBA8
upload batch and report native copy timing when GPU timestamps are available:

```powershell
cargo test --locked -p gfx-dx12 --test texture_transfer
cargo test --locked -p gfx-vulkan --test texture_transfer
cargo bench --locked -p gfx-dx12 --bench texture_write -- mip-chain
cargo bench --locked -p gfx-vulkan --bench texture_write -- mip-chain
```

These transfer measurements do not represent 3D scene frame pacing.
The animated scene case includes per-sample CPU evaluation and prepared draw updates; use its result
to track that cost before considering a retained GPU transform-animation path.
Record GPU/backend/driver, window dimensions, display refresh, scene size, warm-up, and per-frame
timings separately for DX12 and Vulkan real-window comparisons. Shader compilation is a format
check, not evidence of correct pixels or smooth presentation.

## Offscreen multi-pass renderer

`gpui_3d::OffscreenRenderer` renders one frame per public call and owns only that frame's GPU
resources. Ownership boundaries:

- The caller creates and owns the Nova `Device`; `gpui-3d` never submits work outside
  `render`/`render_animated`/`capture`, never creates a window surface or swapchain, and never reads
  CPU memory from a texture it did not create.
- GPU objects created by the renderer: shadow color/depth targets, HDR scene color/depth targets,
  bloom color target, post color target (readback source), eight shader modules, five pipelines
  (shadow, opaque, blended, bloom, post), four render passes, four resource-set layouts, four
  pipeline layouts, three samplers, a fallback texture, per-mesh vertex/index/tangent buffers,
  per-texture images, frame uniform/instance/light/draw buffers, and cached per-batch resource sets.
  Pipeline layouts are released with the renderer; earlier revisions leaked three layouts on every
  renderer drop/resize because they were not retained in the pipeline bundle.
- `destroy` releases every one of those objects; it must run before the device is destroyed.
  `trim(Light|Moderate|Aggressive)` drops cached meshes, textures, and resource sets without touching
  target/pipeline state. `resize` recreates size-dependent targets, pipelines, and resource sets while
  keeping mesh and texture caches resident.

Render graph (passes recorded into one backend submission through
`render_step_lists_to_textures_compat`). Scene and bloom targets use the internal render
resolution, controlled by the render-scale API; the post target and readback always stay at the
requested output size:

| Pass | Color | Depth | Load | Content |
| --- | --- | --- | --- | --- |
| Shadow | `Rgba8Unorm` packed 24-bit depth | `Depth32Float` | Clear white / 1.0 | Opaque and masked draws from a light-space orthographic camera fitted to scene bounds |
| Scene | `Rgba16Float` linear HDR | `Depth32Float` | Caller linear clear color / 1.0 | Indexed instanced PBR draws, shadow comparison with 3x3 PCF scaled by softness, linear radiance, debug views |
| Bloom | `Rgba8Unorm` at half resolution | None | Clear | Thresholded 9-tap bright-pass blur sampled from the HDR scene target |
| Post | `Rgba8Unorm` + `COPY_SRC` | None | Clear | Fullscreen HDR resolve: exposure, tone mapping, bloom add, FXAA, vignette, grain, dither, gamma; readback source |

Per-frame updates are diffed against the last uploaded bytes: a static scene with an unchanged camera
uploads 0 bytes and issues no mesh or texture upload after warm-up. Camera-only changes upload the
288-byte frame uniform. Per-batch resource sets are cached by mesh id/generation, mesh
part, draw index, instance count, material texture ids, and a target/frame-buffer epoch, so a static
scene reuses every set without per-frame descriptor creation.

## glTF 2.0 / GLB import

`gpui_3d::load_path` and `load_bytes` produce `GltfScene` with `scene`, `textures`, `animations`,
`cameras`, `warnings`, and `statistics`. Supported: GLB and JSON containers; embedded, data-URI, and
external buffers; interleaved and compact accessors; sparse accessors; POSITION/NORMAL/TANGENT/
TEXCOORD_0/COLOR_0; indexed and unindexed triangle primitives; material alpha modes and double-sided
flags; base-color/normal/occlusion/metallic-roughness textures decoded through `image` with generated
box-filtered mip chains; node TRS or matrix transforms; cameras; KHR_lights_punctual (directional,
point, spot); KHR_materials_unlit; KHR_materials_emissive_strength; and STEP/LINEAR/CUBICSPLINE
transform animation channels converted to absolute-time `TransformTrack`s.

Import limits are `GltfLoadOptions`: document bytes, decoded buffer bytes, node count, vertices per
primitive, texture dimension, texture decoding on/off, mip generation, and external-resource access.
Every accessor read is bounds-checked with checked arithmetic; node cycles, duplicate animation times,
out-of-range indices, non-finite values, and external paths that escape the document directory fail
with a typed `GltfError`. Extensions listed in `extensionsRequired` outside {KHR_lights_punctual,
KHR_materials_unlit} fail the import; unknown optional extensions are reported as warnings.

Feature status to keep honest: skins and morph targets are parsed only far enough to render the bind
pose and emit a warning; emissive textures are not sampled (the factor is ignored with a warning);
KHR_texture_transform UV transforms are not applied; KTX2/Basis, Draco, and meshopt payloads are not
decoded; KHR_materials_clearcoat/transmission/ior/variants are recorded as warnings and fall back to
the base metallic-roughness model.

## Test assets

`crates/gpui-3d/test_assets/manifest.toml` lists every external model with source URL, SHA-256, byte
size, license, attribution, and feature tags. `download.ps1` fetches missing files into `cache/`,
verifies all hashes, and fails on any error. The cache is git-ignored; after first preparation the
core regressions run fully offline. Verified assets:

| Asset | License | Features exercised |
| --- | --- | --- |
| DamagedHelmet.glb | CC BY 4.0 (ctxwing) | GLB, PBR, base-color/normal/occlusion/metallic-roughness textures |
| AnimatedCube.gltf + .bin + .png | CC0 (UX3D / Norbert Nopper) | JSON document, external buffer, external image, node rotation animation |
| BoxVertexColors.glb | CC0 (Marco Hutte) | GLB, COLOR_0 vertex colors, indexed geometry |

Tests that need the cache are marked `#[ignore]` and documented as requiring
`test_assets/download.ps1`; they fail (not skip) when run without verified assets. Synthetic GLB
fixtures generated inside `tests/gltf_import.rs` cover malformed containers, index overflow, node
cycles, required extensions, and path escape without any external file.

## GPU verification and measured results

`tests/offscreen_gpu.rs` creates real Nova devices, renders through the public API, reads pixels back,
and asserts: shaded output is non-uniform and lit; enabling directional shadows changes more than 100
pixels; resize keeps rendering at the new extent; a repeated static frame uploads 0 bytes and creates
no resources; aggressive trim forces re-upload; and DX11/Vulkan agree with the DX12 reference within 1%
of pixels. Run it explicitly:

```powershell
cargo test -p gpui-3d --test offscreen_gpu -- --ignored --test-threads=1
```

`scene_lab` provides the scriptable capture path (`--backend=all --out=... --json=...`). Measured on
this development machine (Windows 11 26100, AMD Radeon RX 7600M XT, 1280x720, 2048 shadow map, 8 draw
calls / 7 shadow draws / 18,806 triangles / 13 instances, scene cached after the first frame):

| Backend | First frame (prepare / submit) | Warm render p50 | Steady frame (prepare / submit) | Steady upload bytes | Capture p50 |
| --- | --- | --- | --- | --- | --- |
| DX12 | 92.4 ms / 1.6 ms | 1.16 ms | 1.16 ms / 1.10 ms | 0 | 5.10 ms |
| DX11 | 2.9 ms / 0.1 ms | 0.27 ms | 0.09 ms / 0.05 ms | 0 | 2.85 ms |
| Vulkan | 40.4 ms / 1.5 ms | 0.31 ms | 0.30 ms / 0.25 ms | 0 | 5.09 ms |
| OpenGL 4.5 | 0.9 ms / 0.1 ms | 0.12 ms | 0.09 ms / 0.05 ms | 0 | 1.85 ms |

The first-frame differences are shader/pipeline creation cost; after warm-up every backend reports
0 mesh uploads, 0 texture uploads, 0 upload bytes, and identical draw/triangle/instance counts. These
are CPU-side prepare/submit and readback timings, not GPU frame times: Nova still has no cross-backend
GPU timestamp contract, so no GPU pass time is claimed here. Captures are written to
`target/scene_lab/*.png`; DX11, DX12, Vulkan, and OpenGL images agree visually, and the GPU pixel test
enforces DX11/Vulkan agreement with DX12 numerically.

| Backend | GPUI scene-view shader | Offscreen shadow+PBR+post | Readback pixels | Cross-backend agreement |
| --- | --- | --- | --- | --- |
| DX12 | Build-time DXBC | Verified (scene_lab capture + GPU test) | Verified | Reference backend |
| DX11 | Build-time SM5.0 bytecode | Verified (scene_lab capture + GPU test) | Verified | Within 1% of DX12 |
| Vulkan | Build-time SPIR-V | Verified (scene_lab capture + GPU test) | Verified | Within 1% of DX12 |
| OpenGL 4.5 | Build-time GLSL 4.50 | Verified through scene_lab capture (WGL hidden-window bootstrap) | Verified | Visually matches; not part of the automated matrix test |
| Metal | Build-time MSL | Not implemented (no texture upload path) | Not supported | Pending |

## HDR post pipeline, bloom, FXAA, and quality presets

The offscreen renderer resolves light in linear HDR. Nova now exposes `Rgba16Float` through the
format set; DX12, DX11, Vulkan, and OpenGL 4.5 create and sample the HDR scene target (Metal maps the
format but is untested on this Windows host). The chain is:

1. Shadow pass: packed 24-bit depth in `Rgba8Unorm` plus `Depth32Float`.
2. Scene pass: linear radiance into `Rgba16Float` with a depth target; no exposure or gamma yet.
3. Bloom pass (optional): half-resolution `Rgba8Unorm` target, thresholded 9-tap bright-pass blur.
4. Post pass: full-screen resolve into the readback `Rgba8Unorm` target.

The post shader applies exposure, ACES/Reinhard tone mapping, additive bloom, FXAA-style edge
blending, vignette, film-grain noise from a stable pixel hash, 4x4 ordered dithering to reduce 8-bit
banding, and gamma. `RenderOptions::post_process == false` passes the scene bytes through without
grading (object-id selection depends on that); debug views bypass the HDR resolve for the same
reason. The post effects are deterministic: there is no frame-time input and no TAA.

| Preset | Shadow map | Bloom | FXAA | Grading |
| --- | --- | --- | --- | --- |
| `Low` | 512 | off | off | vignette only, no grain/dither |
| `Medium` | 1024 | strength 0.25 | on | vignette, grain, dither |
| `High` | 2048 | strength 0.35 | on | vignette, grain, dither |
| `Ultra` | 4096 | strength 0.45 | on | stronger vignette/grain |

`RenderOptions::bloom` controls threshold/strength/radius, `RenderOptions::post_effects` controls
vignette/grain/dither, and `RenderOptions::antialias` selects `None` or `Fxaa`. `RenderQuality::
render_options` fills a complete option set; callers can still override individual fields.
`ShadowOptions::softness` scales the PCF comparison radius without changing the shadow-map memory.

Measured on this machine (DX12, 1280x720, 10-frame orbit benchmark; render is CPU frame cost,
capture includes GPU completion and readback):

| Preset | Resident bytes | Render p50 | Capture p50 |
| --- | --- | --- | --- |
| `Low` | 18.1 MiB | 3.60 ms | 5.36 ms |
| `Medium` | 24.3 MiB | 3.02 ms | 4.38 ms |
| `High` | 48.8 MiB | 3.16 ms | 4.75 ms |
| `Ultra` | 147.1 MiB | 3.38 ms | 5.19 ms |

Bloom and FXAA cost on this GPU at 720p is within roughly 1 ms of driver/queue noise in the capture
timing, while shadow-map resolution dominates both memory and scene-pass fill. The HDR scene target
adds 4 bytes per pixel versus the previous display-referred target and the half-resolution bloom
target adds 1 byte per pixel; at 720p that is about 5.5 MiB. GPU timestamp queries are still absent,
so per-pass GPU time is not reported.

Remaining post-processing gaps: MSAA/SSAA, TAA, motion blur, SSAO/SSR, depth of field, multi-scale
bloom, and LUT color grading. FXAA is an luma edge heuristic, not a resolved multisample coverage.

## Render scale, visibility, and animation crossfades

### Render scale (dynamic resolution and supersampling)

`OffscreenRenderer::with_render_scale` / `set_render_scale` render the HDR scene and bloom targets
at `render_scale * output_size`, while the post target, readback, and viewport stay at the requested
output size. Accepted scales are `0.25..=2.0`:

- Below 1 reduces scene/bloom fill cost and GPU target memory, then the post pass upsamples with a
  four-tap footprint filter.
- Above 1 supersamples the scene and downsamples in the post pass, approximating SSAA for edges,
  specular highlights, and shadows without multisample render targets.
- Changing scale recreates size-dependent targets/resource sets and invalidates the retained static
  frame; meshes/textures stay cached. Debug views and object-id selection read the scene with a
  single unfiltered texel so ids keep decoding at any scale.

Measured 1280x720 studio, quality Medium, DX12 (post/bloom/shadow included):

| Render scale | Internal HDR size | Target bytes | Resident |
| --- | --- | --- | --- |
| 0.5 | 640x360 | 14.4 MiB | 15.1 MiB |
| 1.0 | 1280x720 | 22.9 MiB | 23.7 MiB |
| 2.0 | 2560x1440 | 57.2 MiB | 58.0 MiB |

Render scale is the practical alternative to MSAA until Nova exposes multisample targets: scale 2
costs more fill/memory than MSAA but needs no new backend contract. FXAA remains enabled at scale 1
by default; combining scale 2 with FXAA is valid but usually unnecessary.

### Node visibility

`Node::visible` defaults to `true`, `Node::with_visible` sets it while building, and
`Scene::set_visible` toggles it with retained-snapshot invalidation. Hiding a node hides its entire
subtree: traversal skips it for scene preparation (draws and shadows), CPU ray picking, and scene
bounds, so a hidden subtree cannot cast shadows or be picked. Visibility does not mutate the
hierarchy, transforms, or visible siblings' prepared draw order.

### Animation crossfades

`gpui_3d::blend_transform` blends two local poses (linear translation/scale, shortest-arc spherical
rotation, clamped weight). `Scene::evaluate_blended` / `evaluate_blended_with` sample two clips at
independent absolute times and blend them per node: `0` is the primary pose, `1` the secondary pose,
and nodes animated by only one clip fade toward their authored rest pose. The renderer exposes the
same operation as `OffscreenRenderer::render_blended`. Invalid blend weights, duplicate tracks
within one clip, stale handles, and invalid authored poses return typed errors. This is CPU clip
blending; skeletal and morph deformation are still unimplemented.

### Image-based lighting and sky

`EnvironmentMap` wraps an equirectangular texture with an intensity and yaw rotation.
`EnvironmentMap::new` validates parameters, `load_path` decodes a PNG/JPEG panorama (sRGB,
downscaled to 4096 max, box-filtered mip chain), and `EnvironmentMap::procedural_studio` builds a
licensed-asset-free studio panorama. Attach it with `Scene::set_environment` / clear it with
`None`; the change bumps the scene version so retained frames are not reused.

The scene shader uses the map for both ambient terms: diffuse samples a heavily blurred mip
(whole-environment average) and specular samples `roughness * (mip_count - 1)` along the
reflection direction, combined with the Karis analytic split-sum BRDF approximation. A fullscreen
sky triangle is drawn first in the scene pass with depth comparison but no depth write, so geometry
overwrites it; debug and object-id passes skip it. This is mip-based prefiltering of an
equirectangular LDR texture, not a prefiltered cubemap plus irradiance probe and BRDF LUT; very
glossy metals therefore use an approximation, and HDR radiance is limited to what fits in the
texture chain. Environment textures are cached by asset id, counted in statistics, invalidated,
and released by aggressive trim or renderer destruction.

### Distance LOD

`Node::with_lod` accepts `LodLevel` values ordered from nearest to farthest. Each level carries a
mesh, optional materials, and `max_view_distance`; preparation picks the first level whose distance
covers the camera-to-node distance, otherwise the last level. A node may consist only of LOD
levels, while `Node::mesh` remains the bounds/raycast representation so callers should keep the
highest-detail mesh there for accurate queries. LOD selection depends on the camera, so the
camera-only retained fast path is disabled whenever preparation used a LOD level; camera moves then
re-walk and re-select. Screen-space LOD, HLOD, and layer/culling masks are not implemented.

### Deferred GPU-completion polling

Nova's `PresentationDevice` now exposes `render_step_lists_to_textures_deferred`: backends with a
deferred offscreen path return a Nova `SubmissionId`, and the default returns `None` after the
synchronous compatibility submission. DX12 implements the deferred path. The offscreen renderer
stores the handle and exposes `OffscreenRenderer::poll_render`, which reports `Some(false)` while
in flight, `Some(true)` on completion, and `None` when the backend has no handle. Capture and
maintenance waits consume the handle, and the common maintenance wait no longer pays a full post
texture readback just to establish completion. The pixel readback itself is still a synchronous
copy after completion; mapping a GPU buffer asynchronously and compositing the offscreen target
without CPU readback remain future work.

### Tests covering this batch

- Library tests cover hidden-subtree preparation, picking, bounds, transform blending, crossfade
  endpoints/midpoints, fade-to-rest, duplicate/stale/weight errors, and animation-scratch reuse.
- `renderer_memory` now has 14 native tests: the additions verify that blended poses and visibility
  change the submitted frame, that render scale keeps the output size while changing target
  memory/sampling and preserving object-id picking, that an environment map lights the scene and
  draws the sky with cache/trim lifecycle, and that a deferred submission can be polled to
  completion before capture.

## GPU memory and cache lifecycle

The renderer reports its own allocation accounting through `RendererStatistics`. These are
engine-logical bytes measured at GPU resource creation time (Nova allocation accounting); they are
not OS process RSS and not driver-reported residency, and the renderer does not claim them as such.

| Field | Meaning |
| --- | --- |
| `resident_buffer_bytes` / `resident_texture_bytes` | Live GPU bytes owned by the renderer |
| `target_bytes` | Shadow/scene/post render targets |
| `cached_mesh_bytes` / `cached_texture_bytes` | Asset caches (textures include the fallback and their mip chains) |
| `peak_resident_bytes` | Peak resident total observed after each frame |
| `upload_bytes_total` | Cumulative uploaded bytes, including uniforms and partial range writes |
| `mesh_cache_hits/misses`, `texture_cache_hits/misses` | Per-frame asset cache outcomes, accumulated |
| `evictions` | Assets released by trim or the memory budget |
| `tracker_cpu_bytes` | CPU memory retained to detect changed upload ranges |
| `memory_budget_bytes` | Configured soft budget |

`submission_unobserved` is an internal state: because the Nova compatibility submission API exposes
no fence handle, `render` marks the frame unobserved and `capture` clears it. Maintenance calls
(`render`, `resize`, `trim`, `trim_to_budget`, `destroy`) first drain an unobserved submission by
reading the post target back. Callers that always capture never pay this extra wait. There is no
deferred-retirement queue, so `pending_retirement_bytes` is intentionally absent rather than
fabricated.

Memory defaults and controls:

- Shadow map default is 1024 (about 8 MiB for the packed-depth color + depth targets). 2048 costs
  about 33 MiB and 4096 about 134 MiB; raise `ShadowOptions::resolution` only when the shadow detail
  is worth it.
- glTF import caps texture dimensions at 2048 by default and downscales oversized images before
  generating mip chains. `GltfLoadOptions::max_texture_dimension` and
  `GltfLoadOptions::downscale_textures` control the cap and fallback.
- Texture cache keys are the source image index plus color space, so several glTF texture entries
  sharing one image upload one GPU texture.
- `OffscreenRenderer::set_memory_budget(Some(bytes))` enables soft-budget eviction: after each frame,
  least-recently-used assets not referenced by that frame are evicted until the resident total fits.
  `trim_to_budget` forces the same pass on demand.
- `trim(Light|Moderate|Aggressive)` releases idle assets; Aggressive additionally shrinks
  over-allocated streaming buffers, clears the mesh-buffer reuse pool, and shrinks CPU staging
  buffers.

Allocator churn and fragmentation controls:

- Per-frame draw/instance/light/frame encoding reuses four CPU staging buffer capacities instead of
  allocating them on every dirty frame (`RendererStatistics::cpu_staging_bytes`).
- Evicted mesh buffers enter a bounded reuse pool (16 MiB / 64 entries, exact usage matching,
  at-most-2x waste). A later mesh of the same size class takes the allocation back through
  `Device::create_buffer`-free reuse, so scene switching and budget trimming do not fragment GPU
  heaps with repeated create/destroy cycles. The pool is excluded from the soft asset budget,
  bounded independently (released above half its limit), and cleared by aggressive trim. Metrics:
  `pooled_buffer_bytes`, `pooled_buffer_count`, `buffer_reuses`, `buffer_allocation_count`,
  `buffer_slack_bytes`.
- Pipeline layouts are now retained and destroyed with the pipeline bundle; earlier revisions leaked
  them on every renderer drop and resize.
- Streaming buffers grow geometrically and shrink only on explicit aggressive trim, avoiding
  alternating realloc patterns that fragment heaps.

Measured VRAM effects on this machine (engine-logical bytes):

| Configuration | Resident | Notes |
| --- | --- | --- |
| Studio, 720p, `Low` (512 shadow, no bloom/FXAA) | 18.1 MiB | HDR scene + bloom targets ~17 MiB |
| Studio, 720p, `Medium` (1024 shadow, bloom, FXAA) | 24.3 MiB | +6.2 MiB for 1024 shadow map |
| Studio, 720p, `High` (2048 shadow, bloom, FXAA) | 48.8 MiB | shadow targets dominate |
| Studio, 720p, `Ultra` (4096 shadow) | 147.1 MiB | ~134 MiB shadow targets |
| DamagedHelmet, 720p, `High`, textures 2048 | 114.7 MiB | 64 MiB textures + 48 MiB targets |
| DamagedHelmet, 720p, `High`, `--max-texture=1024` | 63.0 MiB | 16 MiB textures; 49 MiB less VRAM |

A cached 720p studio scene reports 17 GPU buffer allocations, 0 bytes of buffer slack, 7 KiB of CPU
staging capacity, and 0 pooled bytes once all caches are live; a budget-trimmed scene cycles buffers
through the pool and reports real reuse before aggressive trim releases it to 4 frame buffers.

Upload granularity is tracked per element with a 64-bit content hash per draw slot, instance, light,
and frame uniform, so there is no retained CPU byte copy of these buffers. An animation frame that
moves one of 64 instances uploads one 144-byte instance record (asserted in `renderer_memory` as less
than 512 bytes out of a 9 KiB instance table) while the mesh, textures, and draw slots upload
nothing.

Native memory tests (`tests/renderer_memory.rs`, 14 ignored tests) assert: soft-budget eviction
releases idle meshes and textures and brings resident bytes back under the budget; an 8-scene asset
cycle stays bounded and aggressive trim clears every cache/resource set; a 60-frame three-scene
cycling session under a 2 MiB asset budget stays under the resident envelope, keeps resource-set
counts bounded, and records repeated evictions; two renderers can share one device with independent
targets/caches and the survivor keeps rendering after the other is destroyed; resize keeps the mesh
cache warm while retargeting render targets; animation uploads only changed instance ranges; CPU/GPU
picking agree; a 10,000-instance field keeps one draw slot, reuses unchanged static frames with zero
uploads, skips the scene walk for camera-only moves, and uploads one instance record for a single
`node_mut` change; the HDR/bloom/FXAA path changes output pixels, quality presets apply, and changing
quality invalidates the retained frame; a bounded mesh-buffer pool reports real reuses and aggressive
trim releases it; blended poses and node visibility change the submitted frame; render scale keeps
the output size while changing HDR target memory and sampling, and object-id picking still decodes
when the scene is supersampled; an environment map lights the scene, draws the sky, survives
resource invalidation, and is released by aggressive trim; a deferred submission can be polled to
completion before capture.

## Retained frame reuse and scene scale

Frame statistics expose two additional CPU-side fast paths:

- `FrameStatistics::frame_reused`: the scene identity, mutation version, camera pose, options, and
  target size all match the last submitted frame. The renderer returns the retained statistics
  without walking the scene, encoding buffers, uploading, or submitting GPU work; a following
  `capture` reads the still-valid render target. Static frames therefore cost zero CPU preparation
  and zero upload bytes.
- `FrameStatistics::camera_only_update`: only the camera moved. `PreparedScene::update_camera`
  re-culls and re-sorts the retained candidate draws without re-walking the scene graph, and only
  the 288-byte frame uniform is uploaded because draw, instance, and light content hashes are unchanged.

`Scene::version` is a monotonic mutation counter bumped by insert, remove, reparent, and
`node_mut` (conservatively). The renderer validates both the scene's process-unique identity and
the version, so a dropped and recreated scene can never inherit a cached frame.

Instanced batching also fixes draw-buffer growth: the offscreen renderer writes one draw-parameter
slot per instanced batch instead of one per draw. A 200,000-node field that shares one mesh and
material submits a single indexed draw with 200,000 instances and a 512-byte draw slot instead of
200,000 duplicated 512-byte slots (about 100 MiB saved). The lab exposes this with
`--instances=N`. Measured on this machine, 1280x720, shadow map 1024, static camera:

| Nodes | Draw calls | Instances | Triangles | Buffer bytes | Target bytes | Resident | First frame prepare | Steady frame |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 5,000 | 1 | 5,000 | 60,000 | 704 KiB | 18.5 MiB | 19.7 MiB | 29.7 ms | reused, 0 upload |
| 50,000 | 1 | 50,000 | 600,000 | 7.0 MiB | 18.5 MiB | 25.7 MiB | 44 ms | reused, 0 upload |
| 200,000 | 1 | 200,000 | 2,400,000 | 28.1 MiB | 18.5 MiB | 47.1 MiB | 242 ms | reused, 0 upload |

Before the per-batch draw slot change the 200,000-node field reported 128 MiB of buffers, 147 MiB
resident, and 353 ms first-frame preparation. Remaining costs are the initial CPU scene walk and
GPU submission of the full instance range; there is still no GPU-driven indirect submission,
clustered lighting, or LOD, so the field renders every instance every time the camera or scene
changes.

## Picking and projection

`pick` / `pick_evaluated` convert a top-left-origin viewport pixel to a camera ray and return
`RaycastHit` (stable node and mesh ids, material part, distance, position, geometric normal, UV).
`project` maps a world point back to pixels and normalized depth for gizmos or overlays.
`OffscreenRenderer::pick` uses the renderer viewport and retains traversal storage;
`pick_animated` samples the supplied clip time before raycasting.

`OffscreenRenderer::pick_object` renders the scene with the object-id debug view, reads the resolved
frame back, and decodes a one-based 24-bit instance id from RGB. The id maps to the prepared draw
that the GPU actually submitted, so culling, blending order, and animation pose match the frame. It
requires a device that implements `TextureTransferDevice` because the compatibility submission API
has no asynchronous readback path yet; the readback is synchronous and the renderer last frame
remains the object-id frame afterward. CPU and GPU picking agree in `renderer_memory`, and the scene
lab prints both results with `--pick=x,y`.

## Validation

```powershell
# Every file changed by this work is rustfmt-clean (verified with rustfmt --check per file).
# The repository as a whole is not currently rustfmt-clean: `cargo fmt --all --check` reports 728
# pre-existing diffs in files untouched by this work, so it is not used as a gate here.
cargo test -p gpui-3d
cargo test -p gpui-3d --test picking
cargo test -p gpui-3d --test renderer_memory -- --ignored --test-threads=1
cargo test -p gpui-3d -- --ignored --test-threads=1   # test assets + native GPU matrix
cargo check -p gpui-3d --all-targets --all-features
cargo clippy -p gpui-3d --all-targets --all-features --no-deps
cargo run -p gpui-3d --example scene_lab -- --backend=all --width=1280 --height=720 --frames=4 --quality=high --out=target/scene_lab/studio.png --json=target/scene_lab/studio.json
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --frames=8 --quality=high --orbit-degrees=2.0 --json=target/scene_lab/bench.json
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --quality=low --bloom=off --fxaa=off --out=target/scene_lab/plain.png
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --model=crates/gpui-3d/test_assets/cache/DamagedHelmet.glb --quality=high --exposure=2.0 --max-texture=1024 --budget-mb=160 --pick=640,360 --out=target/scene_lab/helmet.png
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --instances=200000 --frames=4 --quality=medium --out=target/scene_lab/instances.png --json=target/scene_lab/instances.json
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --quality=high --render-scale=2.0 --out=target/scene_lab/ssaa.png
cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --width=1280 --height=720 --quality=high --env-map=studio --out=target/scene_lab/ibl.png
```

For a backend change, also launch the `scene_view` example on that backend and inspect pixels and
frame behavior on a real window. The Metal texture limitation above remains until the backend upload
implementation changes and is revalidated.


## Remaining gap audit and optimization backlog

These are the highest-value items that are still missing or unoptimized, with the concrete blocker
for each. They are prioritized by user-visible effect first, then API/backend cost.

| Area | Missing or unoptimized | Blocker / effort |
| --- | --- | --- |
| Rendering effects | MSAA, TAA, motion blur, SSAO, SSR, depth of field, multi-scale bloom, LUT grading | MSAA needs multisample render-pass state in Nova; the rest are new passes/targets plus history buffers. Render scale 2 already provides approximate SSAA. |
| Lighting | Prefiltered cubemap/irradiance probes and a real BRDF LUT, point/spot shadow maps, clustered Forward+ | The current equirect mip approximation covers IBL; prefiltered probes need cube targets and a convolution pass, and point/spot shadows need atlas targets plus multi-light culling. |
| Deformation | GPU skeletal skinning, morph targets, pose graphs, IK | Importer already recognizes skinned/morph primitives but keeps base geometry; needs joint/weight vertex attributes, bone buffers, and the skinning pipeline. |
| glTF completeness | `KHR_texture_transform`, `TEXCOORD_1`, clearcoat/transmission/ior/variants, `KHR_texture_basisu`, Draco/meshopt | Material texture transforms and UV1 need extra draw parameters; compressed extensions need new decoders. |
| Scene graph | Screen-space/HLOD selection, layers/culling masks, user metadata, gizmo/debug line rendering | Distance LOD is implemented; screen-space size selection needs projection-aware metrics, and lines need a dedicated vertex/pipeline path. |
| Async/latency | True asynchronous GPU-to-CPU readback, compositor hand-off without CPU copy, GPU timestamped pass timings, frame pacing | DX12 deferred submission now provides a completion handle and poll/wait path, but the readback copy itself is still synchronous; Nova has transfer timestamps but no per-pass query handle yet. |
| GPU-driven pipeline | Indirect draws, GPU culling, GPU-side scene traversal, mesh shaders | Nova phase-1 API has no indirect draw descriptor or mesh pipeline. |
| Memory/perf backlog | Mesh arena/compaction, texture streaming/compression, bindless-style descriptor reuse, parallel scene preparation, shadow-frustum culling, pipeline cache warm start, async uploads | Mostly engine-side work; parallel preparation must not regress deterministic ordering, and arena/shared-buffer designs invalidate per-batch resource sets on growth. |
| Platform coverage | Metal runtime validation, WebGL backend | Metal cannot be validated on this Windows host; WebGL lacks the storage-buffer/indirect features the renderer uses. |

Already optimized and verified in this phase: HDR target memory, shadow defaults, texture caps and
downscaling, incremental element uploads, static-frame reuse, camera-only re-cull, per-batch draw
slots, staging-buffer reuse, bounded mesh-buffer reuse pool, budget-driven eviction, render scale,
IBL environment caching, distance LOD, deferred completion polling, and pipeline-layout lifetime. The next highest-return step is IBL (visual impact, modest graph
work), followed by LOD groups (large-scene CPU/GPU cost) and async readback (latency).

## Completion record

This record closes the GPUI-3D work landed on `master` for the current phase. It is intentionally
committed without a `[skip ci]` marker so the repository's Compile Check workflow validates the
pushed tree.

Delivered in this phase:

- `gpui-3d` public API: scene graph, meshes/tangents, materials, cameras, ray queries, glTF 2.0/GLB
  import, retained `PreparedScene`, offscreen multi-pass renderer, animation sampling, project/pick
  helpers, GPU object-id picking, statistics and resource lifecycle APIs.
- `nova-gfx`: linear HDR `Rgba16Float` format across DX11/DX12/Vulkan/OpenGL (Metal mapping added,
  not runtime-validated on this host).
- Offscreen render graph: packed-depth shadow pass, linear HDR PBR scene pass, half-resolution bloom
  pass, HDR post resolve with exposure/tone mapping/FXAA/vignette/grain/dither, readback target.
- Memory and performance: per-element hash-tracked incremental uploads, exponential streaming
  buffers with trim, soft asset budget with LRU eviction, texture caching/downscaling, static-frame
  reuse, camera-only re-cull, per-batch draw slots, reusable CPU staging buffers, bounded mesh-buffer
  reuse pool, allocation/fragmentation metrics, pipeline-layout leak fix, dynamic-resolution and
  supersampled render scale.
- Scene graph and animation: node/subtree visibility respected by rendering, shadows, picking, and
  bounds, distance-selected render LOD, plus CPU two-clip crossfade evaluation and `render_blended`.
- Environment and submission: equirectangular mip-prefiltered IBL with procedural sky background
  and environment texture lifecycle, plus Nova deferred offscreen submission handles and DX12
  completion polling before synchronous capture.
- Verification assets and tooling: SHA-256-pinned licensed test-asset manifest and downloader, four
  backend pixel matrix, 12 native memory/lifecycle tests, scene lab with quality presets, render
  scale, orbit benchmarks, debug views, JSON statistics, and this documentation.

Latest local verification before this record:

- `cargo test -p gpui-3d`: 107 library tests, 8 glTF import tests, 5 CPU picking tests, 2 documentation
  tests pass.
- `cargo test -p gpui-3d -- --ignored --test-threads=1`: cached real-model imports, DX12→DX11→Vulkan
  pixel agreement matrix, and 14 native renderer memory/lifecycle tests pass.
- `cargo check --lib` (repository default features) and
  `cargo check -p bmcbl --no-default-features` pass; changed Rust files are rustfmt-clean and add no
  clippy warnings (the repository has a pre-existing formatting baseline unrelated to this work).
- Scene lab captures were regenerated on DX12 with HDR + bloom + FXAA at quality presets, plus DX11,
  Vulkan, and OpenGL 4.5 backends for the pixel matrix.

Explicitly not implemented in this phase (documented, not claimed): GPU skinning/morph deformation,
prefiltered cubemap irradiance probes and a real BRDF LUT, clustered Forward+, point/spot shadow
maps, MSAA/TAA, SSAO/SSR/motion blur/depth of field, multi-scale bloom, LUT grading, GPU timestamped
pass timings, GPU-driven indirect submission, screen-space/HLOD selection, layer masks, occlusion
culling, true asynchronous GPU-to-CPU readback, compositor hand-off of an offscreen target without
CPU readback, and Metal runtime validation. Render scale 2 approximates supersampling and equirect
mip-based IBL covers environment lighting; distance LOD and DX12 deferred completion polling are
implemented.
