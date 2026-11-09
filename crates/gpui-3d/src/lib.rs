//! Game-oriented 3D rendering engine with a GPU-resident native offscreen path.
//!
//! The core owns geometry, camera, materials, lighting, scene preparation,
//! GPU draw plans and native offscreen submission through gfx-core. GPUI is
//! only an optional scene-view/Sidecar consumer (feature `gpui-sidecar`);
//! standalone games can use `--no-default-features` and provide their own
//! nova-gfx device and presentation lifecycle.
//!
//! # CPU scene quick start
//!
//! Build and prepare a scene without opening a native window:
//!
//! ```rust
//! use gpui_3d::{Camera, Mesh, Node, PreparedScene, Scene, Vec3};
//! use std::sync::Arc;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut scene = Scene::new();
//! scene.insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))?;
//! let camera = Camera::perspective(
//!     Vec3::new(0.0, 0.0, 3.0),
//!     Vec3::ZERO,
//!     Vec3::Y,
//!     0.9,
//!     0.1,
//!     100.0,
//! );
//! let prepared = PreparedScene::new(&scene, camera, 16.0 / 9.0)?;
//! assert_eq!(prepared.draws.len(), 1);
//! # Ok(())
//! # }
//! ```
//!
//! # Triangle edge coverage
//!
//! Mark silhouette edges independently from vertex color alpha. Edge bits are ordered by the
//! triangle vertex opposite each edge; the shared quad diagonal remains unmarked:
//!
//! ```rust
//! use gpui_3d::{Mesh, Node, TriangleEdgeMask, Vec2};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mesh = Mesh::plane(1.0, 1.0)?.with_edge_masks([
//!     TriangleEdgeMask::new([true, true, false]),
//!     TriangleEdgeMask::new([true, false, true]),
//! ])?;
//! let node = Node::new()
//!     .with_mesh(std::sync::Arc::new(mesh))
//!     .with_pixel_offset(Vec2::new(0.45, -0.45))?
//!     .with_depth_bias(0.002)?;
//! # let _ = node;
//! # Ok(())
//! # }
//! ```

#![allow(
    clippy::must_use_candidate,
    reason = "The math API uses ordinary value getters; fallible operations already return must-use Result or Option values."
)]

mod animation;
mod asset;
mod camera;
mod fingerprint;
mod light;
mod material;
pub use gfx_core::FrontFace;
mod math;
mod mesh;
mod native_offscreen;
mod voxel_mesh;
mod gpu_submission_trace;
mod gpu_encoding;
mod upload_feedback;
pub use upload_feedback::{GpuUploadCostEstimate, gpu_upload_cost_estimate};
pub use voxel_mesh::{
    VOXEL_OCCLUDER, VoxelFaceTemplate, VoxelFusionInput, VoxelMeshInstance, VoxelMeshSource, voxel_ambient_light,
};
mod prepare;
mod raycast;
mod render;
mod scene;
#[cfg(feature = "gpui-sidecar")]
mod scene_view;

pub use animation::{
    AnimationError, AnimationScratch, Interpolation, Keyframe, RotationTrack, TransformTrack,
    Vec3Track, blend_transform,
};
pub use asset::{
    EnvironmentError, EnvironmentMap, GltfAnimation, GltfCamera, GltfError, GltfLoadOptions,
    GltfScene, GltfStatistics, load_bytes, load_path,
};
pub use asset::{ImportedScene, ImportedTriangle, load_imported_bytes, load_imported_path};
pub use camera::{Camera, CameraError, OrbitCamera, Projection};
pub use light::{Light, LightError, PreparedLight, SpotCone, SpotLight};
pub use material::{
    AlphaMode, Material, MaterialId, PbrTextureLayout, ShadingModel, SurfaceKind, TextureAsset,
    TextureAssetId, TextureColorSpace, TextureError, TextureSampling,
};
pub use math::{Aabb, Mat4, Quat, Ray, Transform, Vec2, Vec3};
pub use mesh::{GeneratedTangents, Mesh, MeshError, MeshId, MeshPart, TriangleEdgeMask, Vertex};
pub use native_offscreen::{NativeOffscreenPass, submit_native_graphics, submit_native_offscreen_passes};
pub use prepare::{DrawBatch, PreparedDraw, PreparedScene};
pub use raycast::{RaycastError, RaycastHit, RaycastScratch};
pub use render::{
    AntialiasMode, BloomOptions, CapturedImage, DebugView, FrameStatistics, ObjectPick,
    OffscreenRenderer, PickError, PostEffects, RenderError, RenderOptions, RenderQuality,
    RendererStatistics, ShadowOptions, Tonemap, pick, pick_evaluated, project,
};
pub use scene::{EvaluatedScene, LodLevel, Node, NodeError, NodeId, Occluder, Scene, SceneError};
#[cfg(feature = "gpui-sidecar")]
pub use scene_view::{
    DistanceFog, FlipbookTexture, ProjectionRegion, SceneView, SceneViewConfigError,
    SceneViewRaycastError, SceneViewRaycastScratch, SceneViewTextureError, ScreenEffects,
    VoxelDisplay, VoxelIllumination, VoxelShading, VoxelShadingError, scene_view,
};
