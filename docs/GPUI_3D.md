# GPUI 3D Architecture And API

This document defines the workspace boundary and current contract for `gpui-3d`. It is the source
for 3D API ownership, scene preparation, scene-view resource lifetime, examples, and benchmark scope.

Map Viewer and Skin Pack now convert their preview data to `gpui-3d` scenes. The dedicated GPUI mesh
primitive, upload, cache, and draw path has been removed; GPUI retains the generic
`RendererExtension` contract. Source-token review, `gpui-3d` tests/all-target checks, and the focused
GPUI check pass; `cargo check --locked -p bmcbl --no-default-features` also passes, and the CPU
`scene` example runs successfully. Real-window DX12 and Vulkan smoke captures show the scene-view
geometry and three shared-resource sphere instances after the canvas was made to fill its available
layout bounds. Frame pacing and GPU draw counts have not been measured, and the feature matrix below
records remaining 3D work; this does not imply full feature
parity or that all performance issues are resolved.

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
| Geometry | Immutable indexed meshes, material parts, area-weighted normals, MikkTSpace tangent generation and authored tangent streams, optional per-triangle edge masks, fixed-topology vertex snapshot replacement, five primitives, bounds, BVH queries, automatic indexed instancing for compatible adjacent draws | Partial GPU geometry updates with an explicit in-flight resource lifetime contract |
| Camera and picking | Perspective and orthographic projection, orbit/pan/zoom controls, authored/evaluated world bounds, bounds framing, CPU ray queries | Picking from submitted-frame ID/depth output |
| Materials and lighting | Metallic-roughness and unlit materials, ambient/directional/point/spot lights, albedo, normal, and ambient-occlusion maps, caller-supplied mip chains, three alpha modes, opt-in anisotropic sampling | HDR environment lighting, directional shadows, linear/HDR output controls, and typed material bindings |
| Render extension | One retained Nova scene-view path with a fixed shader, indexed instancing, content-mask projection, selected triangle-edge coverage, pixel offsets, depth bias, and projection-edge feathering | Generic material programs, custom vertex streams, configurable mesh passes, and their resource/error contracts |
| Animation and deformation | Absolute-time translation, rotation, and scale tracks evaluated on CPU | Pose/weight blending, constraints and IK, morph/skin deformation, GPU deformation, and coupled geometry/bounds publication |
| Output and capture | GPUI scene-view layout element | Headless rendering, viewport capture, asynchronous readback, and object/label outputs |

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

## Validation

```powershell
cargo fmt --all --check
cargo test --locked -p gpui-3d
cargo check --locked -p gpui-3d --all-targets --all-features
cargo check --workspace --no-default-features
```

For a backend change, also launch the `scene_view` example on that backend and inspect pixels and
frame behavior on a real window. The Metal texture limitation above remains until the backend upload
implementation changes and is revalidated.
