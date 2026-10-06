//! Reusable 3D scene data and preparation for GPUI renderer extensions.
//!
//! The crate owns generic geometry, camera, material, scene-graph, preparation, and query APIs.
//! Application crates convert domain data into these types; GPUI owns presentation and Nova owns
//! backend resources.
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
mod camera;
mod light;
mod material;
mod math;
mod mesh;
mod prepare;
mod raycast;
mod scene;
mod scene_view;

pub use animation::{
    AnimationError, AnimationScratch, Interpolation, Keyframe, RotationTrack, TransformTrack,
    Vec3Track,
};
pub use camera::{Camera, CameraError, OrbitCamera, Projection};
pub use light::{Light, LightError, PreparedLight, SpotCone, SpotLight};
pub use material::{
    AlphaMode, Material, MaterialId, ShadingModel, TextureAsset, TextureAssetId, TextureColorSpace,
    TextureError, TextureSampling,
};
pub use math::{Aabb, Mat4, Quat, Ray, Transform, Vec2, Vec3};
pub use mesh::{GeneratedTangents, Mesh, MeshError, MeshId, MeshPart, TriangleEdgeMask, Vertex};
pub use prepare::{DrawBatch, PreparedDraw, PreparedScene};
pub use raycast::{RaycastError, RaycastHit, RaycastScratch};
pub use scene::{EvaluatedScene, Node, NodeError, NodeId, Scene, SceneError};
pub use scene_view::{
    ProjectionRegion, SceneView, SceneViewConfigError, SceneViewRaycastError,
    SceneViewRaycastScratch, SceneViewTextureError, scene_view,
};
