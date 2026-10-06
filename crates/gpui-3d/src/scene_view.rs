use crate::{
    AnimationError, AnimationScratch, Camera, CameraError, Mat4, Ray, RaycastError, RaycastScratch,
    Scene, TextureAsset, TextureAssetId, TransformTrack, Vec2, Vec3,
};
use gfx_core::ExtensionDevice;
use gpui::{
    IntoElement, RendererExtension, RendererExtensionContext, RendererExtensionRenderer, Styled,
    Window, canvas,
};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

pub(super) const SHADER: &str = include_str!("scene_view.wgsl");
pub(super) const PACKED_VERTEX_STRIDE: u32 = 64;
pub(super) const DRAW_PARAMS_STRIDE: u32 = 48;
pub(super) const DRAW_SLOT_STRIDE: usize = 512;
pub(super) const FRAME_PARAMS_STRIDE: usize = 112;
pub(super) const INSTANCE_STRIDE: u32 = 144;
pub(super) const LIGHT_STRIDE: u32 = 80;
pub(super) const VERTEX_BINDING: u32 = 0;
pub(super) const DRAW_BINDING: u32 = 1;
pub(super) const LIGHT_BINDING: u32 = 2;
pub(super) const ALBEDO_BINDING: u32 = 3;
pub(super) const SAMPLER_BINDING: u32 = 4;
pub(super) const NORMAL_MAP_BINDING: u32 = 5;
pub(super) const OCCLUSION_BINDING: u32 = 6;
pub(super) const TANGENT_BINDING: u32 = 7;
pub(super) const INSTANCE_BINDING: u32 = 8;
pub(super) const FRAME_BINDING: u32 = 9;

static NEXT_SCENE_VIEW_ID: AtomicU64 = AtomicU64::new(1);

mod artifacts;
mod gpu;
mod projection;
mod renderer;
mod resources;
mod scene;
/// A prepared scene and camera snapshot rendered as a GPUI layout element through Nova.
///
/// A scene view owns immutable scene, camera, and texture snapshots. Use
/// [`SceneView::with_scene`], [`SceneView::with_camera`], or [`SceneView::with_textures`] to submit
/// updated snapshots while retaining its renderer resources. Material texture IDs resolve against
/// this scene view's texture assets.
#[derive(Clone, Debug)]
pub struct SceneView {
    pub(super) id: SceneViewId,
    pub(super) scene: Arc<Scene>,
    pub(super) scene_transform: Mat4,
    pub(super) camera: Camera,
    pub(super) textures: Arc<HashMap<TextureAssetId, Arc<TextureAsset>>>,
    pub(super) animation: Arc<[TransformTrack]>,
    pub(super) animation_time: Duration,
    pub(super) projection_region: ProjectionRegion,
    pub(super) inset_max_pixels: f32,
    pub(super) inset_fraction: f32,
    pub(super) blend_edge_feather: f32,
    pub(super) anisotropy_enabled: bool,
    pub(super) sampling: crate::TextureSampling,
}

/// Selects the rectangle used for the viewport's camera projection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProjectionRegion {
    /// Use the renderer extension element's full layout bounds.
    #[default]
    ElementBounds,
    /// Use the intersection of the element bounds and the current GPUI content mask.
    VisibleContent,
    /// Fit a centered square inside the intersection of the element bounds and content mask.
    VisibleSquare,
}

/// Reusable transform and traversal storage for scene-view pixel raycasts.
#[derive(Debug, Default)]
pub struct SceneViewRaycastScratch {
    query: RaycastScratch,
    animation: AnimationScratch,
}

impl SceneViewRaycastScratch {
    /// Creates empty raycast storage.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Releases retained query and animation capacity.
    pub fn trim(&mut self) {
        self.query.trim();
        self.animation.trim();
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct SceneViewId(u64);

impl SceneView {
    /// Creates a scene view for an immutable scene and camera.
    #[must_use]
    pub fn new(scene: Arc<Scene>, camera: Camera) -> Self {
        Self {
            id: SceneViewId(NEXT_SCENE_VIEW_ID.fetch_add(1, Ordering::Relaxed)),
            scene,
            scene_transform: Mat4::IDENTITY,
            camera,
            textures: Arc::default(),
            animation: Arc::from([]),
            animation_time: Duration::ZERO,
            projection_region: ProjectionRegion::default(),
            inset_max_pixels: 0.0,
            inset_fraction: 0.0,
            blend_edge_feather: 0.0,
            anisotropy_enabled: false,
            sampling: crate::TextureSampling::Linear,
        }
    }

    /// Returns a scene view with a new scene and the same scene-view identity.
    ///
    /// Scene-bound animation tracks are cleared because node handles belong to their source
    /// scene. Attach tracks for the replacement scene with [`SceneView::with_animation`].
    #[must_use]
    pub fn with_scene(&self, scene: Arc<Scene>) -> Self {
        Self {
            id: self.id,
            scene,
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: Arc::from([]),
            animation_time: Duration::ZERO,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        }
    }

    /// Returns a scene view with a new camera and the same scene-view identity.
    #[must_use]
    pub fn with_camera(&self, camera: Camera) -> Self {
        Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        }
    }

    /// Returns a snapshot with a transform applied to the complete scene.
    ///
    /// The transform affects rendering, frustum culling, lighting, and ray queries. It must be a
    /// finite, invertible matrix.
    ///
    /// # Errors
    ///
    /// Returns [`SceneViewConfigError::InvalidSceneTransform`] when the matrix is non-finite or
    /// singular.
    pub fn with_scene_transform(
        &self,
        scene_transform: Mat4,
    ) -> Result<Self, SceneViewConfigError> {
        if scene_transform.inverse().is_none() {
            return Err(SceneViewConfigError::InvalidSceneTransform);
        }
        Ok(Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        })
    }

    /// Returns a scene view projected into the selected frame-local rectangle.
    #[must_use]
    pub fn with_projection_region(&self, projection_region: ProjectionRegion) -> Self {
        Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        }
    }

    /// Returns a scene view with a responsive inset applied before content-mask clipping.
    ///
    /// For each axis, the inset is `min(max_pixels, axis_length * fraction)` on both sides.
    /// Passing `(0.0, 0.0)` disables the inset. This can preserve projection margins across
    /// differently sized previews without putting UI-specific layout rules in the renderer.
    ///
    /// # Errors
    ///
    /// Returns [`SceneViewConfigError`] if either value is negative or non-finite, or if
    /// `fraction` is at least `0.5`.
    pub fn with_projection_inset(
        &self,
        max_pixels: f32,
        fraction: f32,
    ) -> Result<Self, SceneViewConfigError> {
        if !max_pixels.is_finite()
            || max_pixels < 0.0
            || !fraction.is_finite()
            || fraction < 0.0
            || fraction >= 0.5
        {
            return Err(SceneViewConfigError::InvalidProjectionInset);
        }
        Ok(Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region: self.projection_region,
            inset_max_pixels: max_pixels,
            inset_fraction: fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        })
    }

    /// Returns a scene view that fades blended draws at its projected rectangle's edges.
    ///
    /// The width is measured in target pixels. A width of zero disables edge fading. Opaque and
    /// masked draws keep their original alpha and depth behavior.
    ///
    /// # Errors
    ///
    /// Returns [`SceneViewConfigError`] if `pixels` is negative or not finite.
    pub fn with_blend_edge_feather(&self, pixels: f32) -> Result<Self, SceneViewConfigError> {
        if !pixels.is_finite() || pixels < 0.0 {
            return Err(SceneViewConfigError::InvalidEdgeFeather);
        }
        Ok(Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: pixels,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        })
    }

    /// Returns a scene view that requests anisotropic texture filtering.
    ///
    /// DX12 supports the request directly. Vulkan uses the selected adapter's maximum when it
    /// exposes sampler anisotropy, and otherwise falls back to the configured linear filters.
    /// Metal does not currently support GPUI 3D texture uploads.
    #[must_use]
    pub fn with_anisotropy(&self, enabled: bool) -> Self {
        Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: enabled,
            sampling: self.sampling,
        }
    }

    /// Returns a scene view that samples its texture-table albedo assets with the given filtering.
    ///
    /// Atlas images whose UV regions must not bleed together, such as Minecraft skin and map
    /// atlases, need [`crate::TextureSampling::Nearest`]. This is a scene-view-wide sampler choice;
    /// per-asset filtering is not supported.
    #[must_use]
    pub fn with_sampling(&self, sampling: crate::TextureSampling) -> Self {
        Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling,
        }
    }

    /// Returns a scene view that samples scene-node tracks at an absolute clip time.
    ///
    /// The scene view does not own a playback clock. Applications choose `time` using their
    /// animation lifecycle; frame-visible animation should use the current GPUI frame timestamp.
    /// The source scene and track list remain immutable, and this snapshot keeps the same renderer
    /// identity.
    ///
    /// # Errors
    ///
    /// Returns [`AnimationError`] when a track targets a stale or duplicate node, contains
    /// invalid key data, or cannot sample the requested time.
    pub fn with_animation(
        &self,
        tracks: impl Into<Arc<[TransformTrack]>>,
        time: Duration,
    ) -> Result<Self, AnimationError> {
        let animation = tracks.into();
        self.scene.evaluate(&animation, time)?;
        Ok(Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation,
            animation_time: time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        })
    }

    /// Returns a scene view sampled at another absolute clip time.
    ///
    /// This preserves the authored scene, track list, texture table, and scene-view identity.
    #[must_use]
    pub fn with_animation_time(&self, time: Duration) -> Self {
        Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: self.textures.clone(),
            animation: self.animation.clone(),
            animation_time: time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        }
    }

    /// Returns a scene view with a new texture table and the same scene-view identity.
    ///
    /// Texture assets are immutable. Their generated IDs are the keys referenced by material
    /// `albedo_texture`, `normal_texture`, and `occlusion_texture` fields. Replacing the table
    /// releases assets that are no longer used after the renderer retires their GPU resources.
    ///
    /// # Errors
    ///
    /// Returns [`SceneViewTextureError::DuplicateId`] when multiple assets use the same ID.
    pub fn with_textures(
        &self,
        textures: impl IntoIterator<Item = Arc<TextureAsset>>,
    ) -> Result<Self, SceneViewTextureError> {
        let mut table = HashMap::new();
        for texture in textures {
            if table.insert(texture.id(), texture).is_some() {
                return Err(SceneViewTextureError::DuplicateId);
            }
        }
        Ok(Self {
            id: self.id,
            scene: self.scene.clone(),
            scene_transform: self.scene_transform,
            camera: self.camera,
            textures: Arc::new(table),
            animation: self.animation.clone(),
            animation_time: self.animation_time,
            projection_region: self.projection_region,
            inset_max_pixels: self.inset_max_pixels,
            inset_fraction: self.inset_fraction,
            blend_edge_feather: self.blend_edge_feather,
            anisotropy_enabled: self.anisotropy_enabled,
            sampling: self.sampling,
        })
    }

    pub(super) fn texture(&self, id: TextureAssetId) -> Option<&Arc<TextureAsset>> {
        self.textures.get(&id)
    }

    /// Current immutable scene snapshot.
    #[must_use]
    pub fn scene(&self) -> &Arc<Scene> {
        &self.scene
    }

    /// Current camera snapshot.
    #[must_use]
    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// Transform applied to all scene nodes before the camera view.
    #[must_use]
    pub fn scene_transform(&self) -> Mat4 {
        self.scene_transform
    }

    /// Casts a pixel position relative to the projected rectangle into the scene and returns its
    /// nearest hit.
    ///
    /// `size` must match the projected rectangle used for rendering. With
    /// [`ProjectionRegion::VisibleContent`], use the visible intersection's size and express
    /// `position` relative to that rectangle's top-left corner. Keep `scratch` between pointer
    /// events to reuse traversal and animated-transform storage. The query uses the same
    /// animation snapshot and absolute clip time as scene-view rendering.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError`] for an invalid viewport size or camera, [`RaycastError`] when the
    /// generated ray is invalid, or [`AnimationError`] when the animation snapshot cannot be
    /// evaluated.
    pub fn raycast(
        &self,
        position: Vec2,
        size: Vec2,
        scratch: &mut SceneViewRaycastScratch,
    ) -> Result<Option<crate::RaycastHit>, SceneViewRaycastError> {
        if !size.x.is_finite() || !size.y.is_finite() || size.x <= 0.0 || size.y <= 0.0 {
            return Err(CameraError::InvalidProjection.into());
        }
        let aspect = size.x / size.y;
        let ndc = Vec2::new(
            position.x / size.x * 2.0 - 1.0,
            1.0 - position.y / size.y * 2.0,
        );
        let ray = self.camera.ray(ndc, aspect)?;
        let inverse = self
            .scene_transform
            .inverse()
            .ok_or(SceneViewRaycastError::InvalidSceneTransform)?;
        let scene_ray = Ray::new(
            inverse.transform_point(ray.origin),
            inverse.transform_vector(ray.direction),
        )
        .ok_or(RaycastError::InvalidRay)?;
        let SceneViewRaycastScratch { query, animation } = scratch;
        let mut hit = if self.animation.is_empty() {
            self.scene.raycast(scene_ray, query)?
        } else {
            self.scene
                .evaluate_with(&self.animation, self.animation_time, animation)?
                .raycast(scene_ray, query)?
        };
        if let Some(hit) = &mut hit {
            hit.position = self.scene_transform.transform_point(hit.position);
            hit.normal = transform_normal(inverse, hit.normal)
                .ok_or(SceneViewRaycastError::InvalidSceneTransform)?;
            hit.distance = (hit.position - ray.origin).length();
        }
        Ok(hit)
    }
}

fn transform_normal(inverse: Mat4, normal: Vec3) -> Option<Vec3> {
    let columns = inverse.columns();
    Vec3::new(
        columns[0][0] * normal.x + columns[0][1] * normal.y + columns[0][2] * normal.z,
        columns[1][0] * normal.x + columns[1][1] * normal.y + columns[1][2] * normal.z,
        columns[2][0] * normal.x + columns[2][1] * normal.y + columns[2][2] * normal.z,
    )
    .normalized()
}

/// Errors produced while configuring a scene view.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SceneViewConfigError {
    /// Projection inset values must be finite and non-negative, with a fraction below 0.5.
    #[error(
        "viewport projection inset must be finite and non-negative; fraction must be below 0.5"
    )]
    InvalidProjectionInset,
    /// Edge feather width must be a finite, non-negative number of target pixels.
    #[error("blended scene-view edge feather must be finite and non-negative")]
    InvalidEdgeFeather,
    /// Scene transform matrix must be finite and invertible.
    #[error("scene-view transform must be finite and invertible")]
    InvalidSceneTransform,
}

/// Errors produced while resolving scene-view texture assets.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SceneViewTextureError {
    /// Two supplied texture assets have the same ID.
    #[error("scene-view texture assets contain a duplicate ID")]
    DuplicateId,
}

/// Errors produced while converting projected scene-view coordinates into a scene ray.
#[derive(Debug, thiserror::Error)]
pub enum SceneViewRaycastError {
    /// The scene view's animation snapshot could not be evaluated.
    #[error(transparent)]
    Animation(#[from] AnimationError),
    /// Camera projection or pose is invalid.
    #[error(transparent)]
    Camera(#[from] CameraError),
    /// The generated ray is invalid for scene querying.
    #[error(transparent)]
    Raycast(#[from] RaycastError),
    /// Scene transform cannot map a query back to world space.
    #[error("scene-view transform is not invertible")]
    InvalidSceneTransform,
}

/// Creates a GPUI element that fills the available layout bounds and paints a scene view there.
pub fn scene_view(scene_view: Arc<SceneView>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window: &mut Window, _| {
            window.paint_renderer_extension(bounds, scene_view.clone());
        },
    )
    .size_full()
}

impl RendererExtension for SceneView {
    fn create_renderer(
        &self,
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
    ) -> gpui::Result<Box<dyn RendererExtensionRenderer>> {
        Ok(Box::new(renderer::Renderer::new(device, context)?))
    }
}

#[cfg(test)]
mod tests {
    use super::gpu::{encode_draws, encode_frame_params, encode_instances, encode_lights, transpose};
    use super::*;
    use crate::{
        Keyframe, Mat4, Material, Mesh, Node, PreparedLight, PreparedScene, Projection,
        TextureAssetId, Vec2, Vec3, Vec3Track, Vertex,
    };
    use gfx_core::{BackendKind, ShaderStage};
    use gpui::Bounds;

    #[test]
    fn empty_scene_encodes_minimum_draw_and_light_slots() {
        let draw = PreparedScene::default();
        let bytes = encode_draws(&draw).unwrap();
        assert_eq!(bytes.len(), DRAW_SLOT_STRIDE);
        assert_eq!(encode_lights(&[]).len(), LIGHT_STRIDE as usize);
        assert_eq!(
            encode_frame_params(
                Mat4::IDENTITY,
                Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 10.0),
                Bounds::default(),
                gfx_core::Extent2d::new(1, 1).unwrap(),
                0.0,
            )
            .len(),
            FRAME_PARAMS_STRIDE
        );
        assert_eq!(transpose(Mat4::IDENTITY), Mat4::IDENTITY);
    }

    #[test]
    fn packed_spot_light_cone_matches_shader_cosine_layout() {
        let bytes = encode_lights(&[PreparedLight::Spot {
            position: Vec3::new(1.0, 2.0, 3.0),
            direction: -Vec3::Z,
            color: [0.5, 0.6, 0.7],
            intensity: 4.0,
            range: 8.0,
            inner_angle: 0.2,
            outer_angle: 0.5,
        }]);
        let read_u32 =
            |offset: usize| u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let read_f32 =
            |offset: usize| f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());

        assert_eq!(bytes.len(), LIGHT_STRIDE as usize);
        assert_eq!(read_u32(0), 3);
        assert_eq!([read_f32(16), read_f32(20), read_f32(24)], [1.0, 2.0, 3.0]);
        assert_eq!(read_f32(28), 8.0);
        assert_eq!([read_f32(32), read_f32(36), read_f32(40)], [0.0, 0.0, -1.0]);
        assert_eq!(read_f32(44), 0.0);
        assert_eq!(
            [read_f32(48), read_f32(52), read_f32(56), read_f32(60)],
            [0.5, 0.6, 0.7, 4.0]
        );
        assert!((read_f32(64) - 0.5_f32.cos()).abs() < f32::EPSILON);
        assert!((read_f32(68) - 0.2_f32.cos()).abs() < f32::EPSILON);
    }

    #[test]
    fn packed_draw_enables_material_textures_and_occlusion_strength() {
        let mut material = Material::new();
        material.albedo_texture = Some(TextureAssetId(17));
        material.normal_texture = Some(TextureAssetId(18));
        material.occlusion_texture = Some(TextureAssetId(19));
        material.occlusion_strength = 0.65;
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        let bytes = encode_draws(&prepared).unwrap();
        // DrawParams: base_color(0), emissive(16), metallic(32), roughness(36), alpha_cutoff(40),
        // normal_mapping_enabled(44), occlusion_strength(48), alpha_mode(52), light_count(56),
        // shading_model(60), texture_flags(64). The scene adds no lights.
        assert_eq!(f32::from_ne_bytes(bytes[32..36].try_into().unwrap()), 0.0);
        assert_eq!(f32::from_ne_bytes(bytes[36..40].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_ne_bytes(bytes[40..44].try_into().unwrap()), 0.5);
        assert_eq!(u32::from_ne_bytes(bytes[44..48].try_into().unwrap()), 1);
        assert!(
            (f32::from_ne_bytes(bytes[48..52].try_into().unwrap()) - 0.65).abs() < f32::EPSILON
        );
        assert_eq!(u32::from_ne_bytes(bytes[52..56].try_into().unwrap()), 0);
        assert_eq!(u32::from_ne_bytes(bytes[56..60].try_into().unwrap()), 0);
        assert_eq!(u32::from_ne_bytes(bytes[60..64].try_into().unwrap()), 0);
        assert_eq!(
            u32::from_ne_bytes(bytes[64..68].try_into().unwrap()),
            1 | 2 | 4,
            "albedo, occlusion, and UV flags are set for UV-mapped cube geometry",
        );
        assert_eq!(f32::from_ne_bytes(bytes[8..12].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_ne_bytes(bytes[12..16].try_into().unwrap()), 1.0);
    }

    #[test]
    fn texture_mapped_mesh_sets_the_uv_texture_flag() {
        let mut material = Material::new();
        material.albedo_texture = Some(TextureAssetId(17));
        let vertices = [Vec3::ZERO, Vec3::X, Vec3::Y]
            .into_iter()
            .enumerate()
            .map(|(index, position)| Vertex {
                position,
                normal: Vec3::Z,
                uv: Vec2::new(index as f32, 1.0),
                color: [1.0; 4],
            })
            .collect::<Vec<_>>();
        let mesh = Mesh::new(vertices, [0, 1, 2]).unwrap();
        assert!(mesh.uses_uv_regions());

        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(mesh))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        let bytes = encode_draws(&prepared).unwrap();

        assert_eq!(
            u32::from_ne_bytes(bytes[64..68].try_into().unwrap()),
            1 | 4,
            "UV-backed draws must request texture sampling",
        );
    }

    #[test]
    fn camera_only_updates_keep_the_retained_draw_list() {
        let mut scene = Scene::new();
        scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let mut prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.len(), 1);

        let orbit = Camera {
            eye: Vec3::new(3.0, 0.0, 0.0),
            ..camera
        };
        assert!(prepared.update_camera(orbit, 1.0).unwrap());
        assert_eq!(prepared.draws.len(), 1);
        assert_eq!(prepared.camera(), Some(orbit));

        // A camera far outside the mesh frustum-culls the retained draw without re-walking.
        let away = Camera {
            eye: Vec3::new(0.0, 0.0, 40.0),
            ..camera
        };
        assert!(prepared.update_camera(away, 1.0).unwrap());
        assert!(prepared.draws.is_empty());
    }

    #[test]
    fn packed_instance_records_keep_each_world_transform() {
        let mesh = Arc::new(Mesh::cube());
        let mut scene = Scene::new();
        for x in [-1.0, 1.0] {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh.clone())
                        .with_transform(crate::Transform {
                            translation: Vec3::new(x, 0.0, 0.0),
                            ..crate::Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            20.0,
        );
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        let bytes = encode_instances(&prepared).unwrap();
        let mut translations = (0..prepared.draws.len())
            .map(|index| {
                let offset = index * INSTANCE_STRIDE as usize + 48;
                f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
            })
            .collect::<Vec<_>>();
        translations.sort_by(f32::total_cmp);

        assert_eq!(bytes.len(), 2 * INSTANCE_STRIDE as usize);
        assert_eq!(translations, [-1.0, 1.0]);
    }

    #[test]
    fn scene_view_updates_keep_identity_for_gpu_cache_reuse() {
        let scene = Arc::new(Scene::new());
        let camera = Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 10.0);
        let scene_view = SceneView::new(scene.clone(), camera);
        let updated = scene_view.with_camera(Camera {
            projection: Projection::Orthographic {
                height: 5.0,
                near: 0.1,
                far: 10.0,
            },
            ..camera
        });
        assert_eq!(scene_view.id, updated.id);
        assert!(Arc::ptr_eq(scene_view.scene(), updated.scene()));
    }

    #[test]
    fn scene_transform_updates_keep_identity_and_reject_singular_matrices() {
        let scene_view = SceneView::new(
            Arc::new(Scene::new()),
            Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 10.0),
        );
        let translated = scene_view.with_scene_transform(translation_x(1.0)).unwrap();

        assert_eq!(scene_view.id, translated.id);
        assert_eq!(translated.scene_transform(), translation_x(1.0));
        assert!(matches!(
            scene_view.with_scene_transform(Mat4::from_columns([
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ])),
            Err(SceneViewConfigError::InvalidSceneTransform)
        ));
    }

    #[test]
    fn replacing_scene_clears_scene_bound_animation_tracks() {
        let mut scene = Scene::new();
        let node = scene.insert(None, Node::new()).unwrap();
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([Keyframe::new(std::time::Duration::ZERO, Vec3::ZERO)]).unwrap(),
        );
        let scene_view = SceneView::new(
            Arc::new(scene),
            Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 10.0),
        )
        .with_projection_region(ProjectionRegion::VisibleContent)
        .with_projection_inset(6.0, 0.08)
        .unwrap()
        .with_blend_edge_feather(0.75)
        .unwrap()
        .with_animation([track], std::time::Duration::ZERO)
        .unwrap();
        let updated = scene_view.with_scene(Arc::new(Scene::new()));

        assert_eq!(scene_view.id, updated.id);
        assert!(updated.animation.is_empty());
        assert_eq!(updated.animation_time, std::time::Duration::ZERO);
        assert_eq!(updated.projection_region, ProjectionRegion::VisibleContent);
        assert_eq!(updated.inset_max_pixels, 6.0);
        assert_eq!(updated.inset_fraction, 0.08);
        assert_eq!(updated.blend_edge_feather, 0.75);
    }

    #[test]
    fn texture_updates_keep_scene_view_identity_and_reject_duplicate_assets() {
        let scene_view = SceneView::new(
            Arc::new(Scene::new()),
            Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 10.0),
        );
        let texture = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([1, 2, 3, 4])).unwrap());
        let updated = scene_view.with_textures([texture.clone()]).unwrap();
        assert_eq!(scene_view.id, updated.id);
        assert!(updated.texture(texture.id()).is_some());
        assert!(matches!(
            scene_view.with_textures([texture.clone(), texture]),
            Err(SceneViewTextureError::DuplicateId)
        ));
    }

    #[test]
    fn projection_and_blend_edge_feather_updates_keep_scene_view_identity() {
        let scene_view = SceneView::new(
            Arc::new(Scene::new()),
            Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 10.0),
        );
        let updated = scene_view
            .with_projection_region(ProjectionRegion::VisibleContent)
            .with_blend_edge_feather(1.0)
            .unwrap();

        assert_eq!(scene_view.id, updated.id);
        assert_eq!(updated.projection_region, ProjectionRegion::VisibleContent);
        assert_eq!(updated.blend_edge_feather, 1.0);
        assert!(matches!(
            scene_view.with_blend_edge_feather(f32::NAN),
            Err(SceneViewConfigError::InvalidEdgeFeather)
        ));
        assert!(matches!(
            scene_view.with_blend_edge_feather(-1.0),
            Err(SceneViewConfigError::InvalidEdgeFeather)
        ));
        assert!(matches!(
            scene_view.with_projection_inset(f32::NAN, 0.08),
            Err(SceneViewConfigError::InvalidProjectionInset)
        ));
        assert!(matches!(
            scene_view.with_projection_inset(6.0, 0.51),
            Err(SceneViewConfigError::InvalidProjectionInset)
        ));
    }

    #[test]
    fn anisotropy_is_opt_in_and_survives_scene_view_updates() {
        let scene_view = SceneView::new(
            Arc::new(Scene::new()),
            Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 10.0),
        );
        assert!(!scene_view.anisotropy_enabled);

        let updated = scene_view
            .with_anisotropy(true)
            .with_projection_region(ProjectionRegion::VisibleContent)
            .with_blend_edge_feather(1.0)
            .unwrap();

        assert_eq!(scene_view.id, updated.id);
        assert!(updated.anisotropy_enabled);
    }

    #[test]
    fn scene_view_raycast_uses_the_rendered_animation_sample() {
        let mut scene = Scene::new();
        let node = scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([
                Keyframe::new(std::time::Duration::ZERO, Vec3::ZERO),
                Keyframe::new(std::time::Duration::from_secs(1), Vec3::new(2.0, 0.0, 0.0)),
            ])
            .unwrap(),
        );
        let camera = Camera::perspective(
            Vec3::new(2.0, 0.0, 3.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_animation([track], std::time::Duration::from_secs(1))
            .unwrap();
        let mut scratch = SceneViewRaycastScratch::new();
        let center = Vec2::new(50.0, 50.0);
        let size = Vec2::new(100.0, 100.0);

        assert!(
            scene_view
                .with_animation_time(std::time::Duration::ZERO)
                .raycast(center, size, &mut scratch)
                .unwrap()
                .is_none()
        );
        assert!(
            scene_view
                .raycast(center, size, &mut scratch)
                .unwrap()
                .is_some()
        );
        let transform_capacity = scratch.animation.transforms.capacity();
        assert!(transform_capacity > 0);
        assert!(
            scene_view
                .raycast(center, size, &mut scratch)
                .unwrap()
                .is_some()
        );
        assert_eq!(scratch.animation.transforms.capacity(), transform_capacity);
        scratch.trim();
        assert_eq!(scratch.animation.transforms.capacity(), 0);
    }

    #[test]
    fn scene_view_raycast_applies_scene_transform_and_returns_world_hit() {
        let mut scene = Scene::new();
        scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(1.0, 0.0, 3.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_scene_transform(translation_x(1.0))
            .unwrap();
        let mut scratch = SceneViewRaycastScratch::new();
        let hit = scene_view
            .raycast(Vec2::new(50.0, 50.0), Vec2::new(100.0, 100.0), &mut scratch)
            .unwrap()
            .unwrap();

        assert!((hit.position.x - 1.0).abs() < 1.0e-5);
        assert!((hit.position.z - 0.5).abs() < 1.0e-5);
        assert!((hit.distance - 2.5).abs() < 1.0e-4);
    }

    fn translation_x(x: f32) -> Mat4 {
        Mat4::from_columns([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [x, 0.0, 0.0, 1.0],
        ])
    }

    #[test]
    fn scene_view_shader_compiles_for_supported_native_backends() {
        for backend in [BackendKind::Vulkan, BackendKind::Dx12, BackendKind::Metal] {
            for (stage, entry_point) in [
                (ShaderStage::Vertex, "vs_main"),
                (ShaderStage::Fragment, "fs_main"),
            ] {
                let binary = if backend == BackendKind::Metal {
                    gfx_shader::compile_wgsl_to_msl_with_version(
                        SHADER,
                        stage,
                        entry_point,
                        gfx_shader::MslVersion::V1_2,
                    )
                } else {
                    gfx_shader::compile_wgsl_for_backend(SHADER, backend, stage, entry_point)
                };
                binary.unwrap();
            }
        }
    }
}
