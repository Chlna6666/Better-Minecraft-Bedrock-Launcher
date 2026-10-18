//! Offscreen scene renderer with a multi-pass GPU render graph.
//!
//! `gpui-3d`'s GPUI scene view renders inside the host compositor pass through the
//! `RendererExtension` contract, which cannot create additional render targets. This module
//! provides the complementary, backend-neutral path for applications and tests that own a
//! Nova device and need full control over the frame: a directional shadow pass, a linear
//! PBR lighting pass, and a display-referred post pass resolved into an offscreen target that
//! can be read back or handed to a compositor.
//!
//! Ownership boundaries:
//!
//! - The caller creates and owns the Nova `Device`. `gpui-3d` never submits work outside the
//!   renderer methods and never creates a second window renderer or swapchain.
//! - The renderer owns the GPU buffers, textures, pipelines, render passes, and resource sets
//!   it creates on that device. `destroy` releases them; call it before destroying the device.
//! - CPU scene data and CPU decoded textures stay immutable after upload. Re-render calls only
//!   upload camera/transform/material changes, new meshes, and new textures.
//!
//! This module is available without the GPUI/Nova window features because it only needs
//! `gfx-core` types. Backend crates (`gfx-dx11`, `gfx-dx12`, `gfx-vulkan`, `gfx-opengl`) create
//! the device; examples and tests show the backend selection.
//!
//! # Example
//!
//! [`OffscreenRenderer::render_to_image`] renders one frame and returns tightly packed RGBA8
//! pixels in display (sRGB-encoded) space. The rendered image includes directional shadows,
//! metallic-roughness PBR shading, exposure, and tone mapping.
//!
//! ```ignore
//! // The device is created by the application-owned Nova backend, for example \
//! // `gfx_dx12::Dx12Device::new(&DeviceDescriptor::default())?`.
//! let mut renderer = OffscreenRenderer::new(&mut device, 1280, 720)?;
//! let image = renderer.render_to_image(
//!     &mut device,
//!     &scene,
//!     &textures,
//!     camera,
//!     &RenderOptions::default(),
//! )?;
//! ```

#![allow(
    clippy::cast_precision_loss,
    clippy::too_many_arguments,
    reason = "Render target and aspect conversion operates on validated u32 pixel extents, and the animated render entry point mirrors the static entry point's parameter list."
)]

mod artifacts;
mod gpu;

use crate::{
    AnimationScratch, Camera, CameraError, EvaluatedScene, MeshId, MeshPart, NodeId, RaycastError,
    RaycastHit, RaycastScratch, Scene, TextureAsset, TextureAssetId, TransformTrack, Vec2,
};
use gfx_core::{BackendKind, Device, MemoryTrimLevel, TextureTransferDevice};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tone mapping operator applied to the linear lighting result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tonemap {
    /// Clamp linear radiance to the display range without a filmic curve.
    None,
    /// Reinhard `c / (1 + c)` mapping.
    Reinhard,
    /// ACES filmic approximation; the default for lit captures.
    #[default]
    Aces,
}

impl Tonemap {
    pub(crate) const fn shader_value(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Reinhard => 1,
            Self::Aces => 2,
        }
    }
}

/// Debug visualization replacing the shaded surface output.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DebugView {
    /// PBR shaded output.
    #[default]
    Shaded,
    /// Base color (albedo) after texture and vertex-color modulation.
    Albedo,
    /// World-space surface normal mapped to the `0..1` range.
    Normal,
    /// Normalized zero-to-one depth.
    Depth,
    /// Mesh UV set zero.
    Uv,
    /// Directional shadow visibility (white is fully lit).
    Shadow,
    /// Stable per-instance color used for object identification.
    ObjectId,
}

impl DebugView {
    pub(crate) const fn shader_value(self) -> u32 {
        match self {
            Self::Shaded => 0,
            Self::Albedo => 1,
            Self::Normal => 2,
            Self::Depth => 3,
            Self::Uv => 4,
            Self::Shadow => 5,
            Self::ObjectId => 6,
        }
    }
}

/// Directional shadow-map settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowOptions {
    /// Whether the shadow pass runs. When disabled the scene is fully lit.
    pub enabled: bool,
    /// Shadow map edge length in texels; 1 to 8192. Higher values cost GPU memory.
    ///
    /// The default 1024 target needs about 8 MiB for the packed-depth color and depth targets;
    /// 2048 needs about 33 MiB and 4096 about 134 MiB, so raise it only when shadow detail
    /// justifies the memory and fill cost.
    pub resolution: u32,
    /// Constant depth bias in normalized light-space depth.
    pub depth_bias: f32,
    /// Slope-scaled depth bias applied to surfaces facing away from the light.
    pub normal_bias: f32,
    /// Shadow darkening from 0 (no visible shadow) to 1 (fully occluded).
    pub strength: f32,
    /// PCF filter radius multiplier; 1 keeps one shadow texel, higher values soften edges.
    ///
    /// Softness trades shadow-edge crispness for smoothness. Values above roughly 4 need a
    /// higher shadow resolution to stay stable.
    pub softness: f32,
}

impl Default for ShadowOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            resolution: 1024,
            depth_bias: 0.0025,
            normal_bias: 0.0025,
            strength: 1.0,
            softness: 1.0,
        }
    }
}

/// Bloom (light-bleed) settings applied after the HDR scene pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BloomOptions {
    /// Whether the bloom pass runs and the post pass adds its result.
    pub enabled: bool,
    /// Linear HDR luminance above which a pixel contributes to bloom.
    pub threshold: f32,
    /// Additive bloom weight in the post pass.
    pub strength: f32,
    /// Sample-radius multiplier for the blur.
    pub radius: f32,
}

impl Default for BloomOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.85,
            strength: 0.3,
            radius: 1.5,
        }
    }
}

/// Display grading applied by the post pass when post processing is enabled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostEffects {
    /// Darkening toward the frame corners from 0 (off) to 1 (strong).
    pub vignette: f32,
    /// Film-grain amplitude in display units from 0 (off) to 1 (strong).
    pub grain: f32,
    /// Ordered-dither amplitude in 8-bit steps to hide banding.
    pub dither: f32,
}

impl Default for PostEffects {
    fn default() -> Self {
        Self {
            vignette: 0.35,
            grain: 0.012,
            dither: 0.75,
        }
    }
}

/// Post-pass edge antialiasing mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AntialiasMode {
    /// No antialiasing.
    None,
    /// Luma-based FXAA-style edge blend in the post pass.
    #[default]
    Fxaa,
}

/// Coarse quality preset that fills `RenderOptions` with matching defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderQuality {
    /// Minimal shadow map, no bloom or FXAA; for integrated GPUs or tiny viewports.
    Low,
    /// 1024 shadow map, modest bloom, FXAA, grading.
    Medium,
    /// 2048 shadow map, stronger bloom, FXAA, grading.
    High,
    /// 4096 shadow map, strongest bloom, FXAA, grading.
    Ultra,
}

impl RenderQuality {
    /// Returns a complete option set for this quality level.
    #[must_use]
    pub fn render_options(self) -> RenderOptions {
        let mut options = RenderOptions::default();
        match self {
            Self::Low => {
                options.shadows.resolution = 512;
                options.bloom.enabled = false;
                options.antialias = AntialiasMode::None;
                options.post_effects = PostEffects {
                    vignette: 0.2,
                    grain: 0.0,
                    dither: 0.0,
                };
            }
            Self::Medium => {
                options.shadows.resolution = 1024;
                options.bloom.strength = 0.25;
            }
            Self::High => {
                options.shadows.resolution = 2048;
                options.bloom.strength = 0.35;
                options.post_effects.vignette = 0.35;
            }
            Self::Ultra => {
                options.shadows.resolution = 4096;
                options.bloom.strength = 0.45;
                options.post_effects.vignette = 0.4;
                options.post_effects.grain = 0.02;
            }
        }
        options
    }
}

/// Per-frame renderer settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    /// Directional shadow-map settings.
    pub shadows: ShadowOptions,
    /// Linear exposure multiplier applied before tone mapping.
    pub exposure: f32,
    /// Tone mapping operator.
    pub tonemap: Tonemap,
    /// Debug output view; [`DebugView::Shaded`] renders the lit surface.
    pub debug_view: DebugView,
    /// Linear HDR scene clear color (RGBA).
    ///
    /// The scene pass writes linear radiance into a 16-bit float target, so this value is tone
    /// mapped by the post pass instead of being display-referred.
    pub clear_color: [f32; 4],
    /// Whether the post pass applies exposure, tone mapping, bloom, FXAA, and grading.
    pub post_process: bool,
    /// Bloom (light-bleed) settings.
    pub bloom: BloomOptions,
    /// Display grading settings.
    pub post_effects: PostEffects,
    /// Post-pass edge antialiasing.
    pub antialias: AntialiasMode,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            shadows: ShadowOptions::default(),
            exposure: 1.0,
            tonemap: Tonemap::Aces,
            debug_view: DebugView::Shaded,
            clear_color: [0.012, 0.016, 0.025, 1.0],
            post_process: true,
            bloom: BloomOptions::default(),
            post_effects: PostEffects::default(),
            antialias: AntialiasMode::default(),
        }
    }
}

/// CPU-side per-frame rendering counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStatistics {
    /// Indexed draws submitted in the scene pass.
    pub draw_calls: u32,
    /// Indexed draws submitted in the shadow pass.
    pub shadow_draw_calls: u32,
    /// Triangles submitted in the scene pass.
    pub triangles: u64,
    /// Mesh instances submitted in the scene pass.
    pub instances: u64,
    /// Meshes uploaded from CPU data this frame.
    pub mesh_uploads: u32,
    /// Textures uploaded this frame.
    pub texture_uploads: u32,
    /// Bytes uploaded to the GPU this frame, including the frame uniform.
    pub upload_bytes: u64,
    /// CPU time spent preparing the scene and encoding commands.
    pub cpu_prepare_nanos: u64,
    /// CPU time spent in the backend submit call.
    pub submit_nanos: u64,
    /// True when the renderer served the frame from its retained static-frame cache without any
    /// CPU preparation, upload, or GPU submission.
    pub frame_reused: bool,
    /// True when only the camera changed, so the scene graph was not re-walked before culling.
    pub camera_only_update: bool,
}

/// Renderer-owned resource statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RendererStatistics {
    /// Live GPU mesh resources.
    pub live_meshes: usize,
    /// Live GPU texture resources.
    pub live_textures: usize,
    /// Cached per-batch resource sets reused across frames.
    pub resource_set_count: usize,
    /// GPU bytes owned by renderer buffers.
    pub resident_buffer_bytes: u64,
    /// GPU bytes owned by renderer textures, including shadow and scene targets.
    pub resident_texture_bytes: u64,
    /// Instanced draw-slot capacity in entries (one slot per submitted batch, not per instance).
    pub draw_capacity: usize,
    /// Instance capacity in entries.
    pub instance_capacity: usize,
    /// Light capacity in entries.
    pub light_capacity: usize,
    /// Current render target width in pixels.
    pub target_width: u32,
    /// Current render target height in pixels.
    pub target_height: u32,
    /// Internal HDR scene target width; differs from `target_width` when render scale is not 1.
    pub render_target_width: u32,
    /// Internal HDR scene target height.
    pub render_target_height: u32,
    /// Shadow map resolution in texels.
    pub shadow_resolution: u32,
    /// GPU bytes owned by scene, shadow, and post render targets.
    pub target_bytes: u64,
    /// GPU bytes owned by cached meshes.
    pub cached_mesh_bytes: u64,
    /// GPU bytes owned by cached textures.
    pub cached_texture_bytes: u64,
    /// Peak resident GPU bytes observed since renderer creation.
    pub peak_resident_bytes: u64,
    /// Total upload bytes across all frames, including uniforms and range uploads.
    pub upload_bytes_total: u64,
    /// Mesh cache hits across all frames.
    pub mesh_cache_hits: u64,
    /// Mesh cache misses across all frames.
    pub mesh_cache_misses: u64,
    /// Texture cache hits across all frames.
    pub texture_cache_hits: u64,
    /// Texture cache misses across all frames.
    pub texture_cache_misses: u64,
    /// Assets released by trim or by the memory budget since creation.
    pub evictions: u64,
    /// CPU bytes retained by incremental-upload hash tables.
    pub tracker_cpu_bytes: u64,
    /// Current soft GPU memory budget in bytes, when one is configured.
    pub memory_budget_bytes: Option<u64>,
    /// CPU bytes retained by reusable per-frame staging buffers.
    pub cpu_staging_bytes: u64,
    /// Number of live GPU buffers owned by the renderer.
    pub buffer_allocation_count: usize,
    /// Allocated-but-unused bytes across renderer-owned GPU buffers.
    pub buffer_slack_bytes: u64,
    /// GPU bytes held in the bounded mesh-buffer reuse pool.
    pub pooled_buffer_bytes: u64,
    /// Mesh buffers currently held for reuse.
    pub pooled_buffer_count: usize,
    /// Mesh buffer allocations that were served from the reuse pool.
    pub buffer_reuses: u64,
}

/// GPU-assisted object selection result from [`OffscreenRenderer::pick_object`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectPick {
    /// Scene node that produced the pixel.
    pub node: NodeId,
    /// Mesh asset identity.
    pub mesh: MeshId,
    /// Material part containing the picked triangle.
    pub part: MeshPart,
}

/// Error produced by viewport pick and projection helpers.
#[derive(Debug, thiserror::Error)]
pub enum PickError {
    /// The camera pose or NDC coordinates are invalid.
    #[error(transparent)]
    Camera(#[from] CameraError),
    /// The constructed ray is invalid.
    #[error(transparent)]
    Ray(#[from] RaycastError),
    /// Animation track sampling failed.
    #[error(transparent)]
    Animation(#[from] crate::AnimationError),
}

/// Converts a top-left-origin viewport pixel to a camera ray and picks the nearest surface.
///
/// `viewport` is the rendered pixel extent, so the result matches the frame produced by
/// [`OffscreenRenderer::render`] for the same camera and target size. CPU picking ignores alpha
/// masking and blended coverage, matching the scene raycast contract.
///
/// # Errors
///
/// Returns [`PickError`] when the camera cannot produce a ray for the pixel.
pub fn pick(
    scene: &Scene,
    camera: Camera,
    viewport: (u32, u32),
    pixel: (f32, f32),
    scratch: &mut RaycastScratch,
) -> Result<Option<RaycastHit>, PickError> {
    let ray = viewport_ray(camera, viewport, pixel)?;
    Ok(scene.raycast(ray, scratch)?)
}

/// Picks against an evaluated animation pose, matching what
/// [`OffscreenRenderer::render_animated`] submits for the same clip time.
///
/// # Errors
///
/// Returns [`PickError`] when the camera cannot produce a ray for the pixel.
pub fn pick_evaluated(
    evaluated: &EvaluatedScene<'_>,
    camera: Camera,
    viewport: (u32, u32),
    pixel: (f32, f32),
    scratch: &mut RaycastScratch,
) -> Result<Option<RaycastHit>, PickError> {
    let ray = viewport_ray(camera, viewport, pixel)?;
    Ok(evaluated.raycast(ray, scratch)?)
}

/// Projects a world-space point to top-left-origin pixels and normalized zero-to-one depth.
///
/// Returns `None` when the point is behind the camera or produces a non-finite projection.
///
/// # Errors
///
/// Returns [`CameraError`] when the camera pose, projection, or viewport aspect is invalid.
pub fn project(
    camera: Camera,
    viewport: (u32, u32),
    world: crate::Vec3,
) -> Result<Option<(f32, f32, f32)>, CameraError> {
    if viewport.0 == 0 || viewport.1 == 0 {
        return Ok(None);
    }
    let view_projection = camera.view_projection(viewport.0 as f32 / viewport.1 as f32)?;
    let clip = view_projection.transform4([world.x, world.y, world.z, 1.0]);
    if !clip[3].is_finite() || clip[3] <= 1.0e-6 {
        return Ok(None);
    }
    let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
    if !ndc.iter().all(|value| value.is_finite()) {
        return Ok(None);
    }
    Ok(Some((
        (ndc[0] * 0.5 + 0.5) * viewport.0 as f32,
        (0.5 - ndc[1] * 0.5) * viewport.1 as f32,
        ndc[2],
    )))
}

fn viewport_ray(
    camera: Camera,
    viewport: (u32, u32),
    pixel: (f32, f32),
) -> Result<crate::Ray, CameraError> {
    if viewport.0 == 0 || viewport.1 == 0 {
        return Err(CameraError::InvalidNdcPoint);
    }
    let ndc = Vec2::new(
        2.0 * pixel.0 / viewport.0 as f32 - 1.0,
        1.0 - 2.0 * pixel.1 / viewport.1 as f32,
    );
    camera.ray(ndc, viewport.0 as f32 / viewport.1 as f32)
}

/// Tightly packed RGBA8 image in display (sRGB-encoded) space.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl CapturedImage {
    /// Image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Tightly packed RGBA8 bytes in row-major order.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Consumes the image and returns its RGBA8 bytes.
    #[must_use]
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
}

/// Errors reported by the offscreen renderer.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The requested render target size is invalid.
    #[error("offscreen render target size is invalid: {0}")]
    InvalidSize(String),
    /// The camera pose, projection, or aspect ratio cannot produce a frame.
    #[error("scene camera preparation failed: {0}")]
    Scene(#[from] CameraError),
    /// A GPU resource could not be created, uploaded, or submitted.
    #[error("offscreen GPU operation failed: {0}")]
    Gpu(#[from] gfx_core::Error),
    /// Readback returned a pixel layout that does not match the bound target.
    #[error("offscreen readback failed: {0}")]
    Readback(String),
    /// CPU scene, mesh, or texture encoding failed before submission.
    #[error("offscreen render preparation failed: {0}")]
    Preparation(String),
    /// The renderer's GPU state was already destroyed.
    #[error("offscreen renderer has been destroyed")]
    Destroyed,
    /// GPU object selection returned an instance id outside the submitted draw list.
    #[error("offscreen object pick failed: {0}")]
    Pick(String),
}

/// Multi-pass offscreen scene renderer.
///
/// The renderer owns scene-pass, shadow-pass, and post-pass GPU resources for one device and
/// one target size. Call [`OffscreenRenderer::render_to_image`] for a captured frame or
/// [`OffscreenRenderer::render`] plus [`OffscreenRenderer::capture`] when the frame is needed
/// separately from the readback.
///
/// All methods must be called from the GPU owner thread with the same device that created the
/// renderer. CPU scene and texture data is immutable between calls; only changed camera,
/// material, mesh, and texture data is uploaded.
pub struct OffscreenRenderer {
    width: u32,
    height: u32,
    gpu: gpu::GpuState,
    prepared: crate::PreparedScene,
    prepared_key: Option<(u64, u64)>,
    last_frame: Option<(FrameReuseKey, FrameStatistics)>,
    animation_scratch: AnimationScratch,
    raycast_scratch: RaycastScratch,
    renderer_statistics: RendererStatistics,
}

/// Everything that can change a rendered frame while the prepared draw list stays valid.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FrameReuseKey {
    scene_id: u64,
    scene_version: u64,
    camera: Camera,
    options: RenderOptions,
    target: (u32, u32),
    render_scale: f32,
}

impl OffscreenRenderer {
    /// Creates a renderer for `width` x `height` drawable pixels.
    ///
    /// This allocates render targets, pipelines, and the initial uniform buffers on `device`.
    /// The initial shadow-map resolution is [`ShadowOptions::default`].
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidSize`] for a zero or oversized target and
    /// [`RenderError::Gpu`] when backend resource or pipeline creation fails.
    pub fn new<D: Device + ?Sized>(
        device: &mut D,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(RenderError::InvalidSize(format!("{width}x{height}")));
        }
        let gpu = gpu::GpuState::new(device, width, height)?;
        Ok(Self {
            width,
            height,
            gpu,
            prepared: crate::PreparedScene::default(),
            prepared_key: None,
            last_frame: None,
            animation_scratch: AnimationScratch::new(),
            raycast_scratch: RaycastScratch::new(),
            renderer_statistics: RendererStatistics::default(),
        })
    }

    /// Creates a renderer that renders HDR lighting at a fraction or multiple of the output size.
    ///
    /// Render scale below 1 reduces scene/bloom target memory and fill cost, then the post pass
    /// upscales to the output size. Scale above 1 supersamples the scene and downsamples in the
    /// post pass, approximating SSAA without multisample targets.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidSize`] for an invalid output size or render scale outside
    /// `0.25..=2.0`, and [`RenderError::Gpu`] when resource creation fails.
    pub fn with_render_scale<D: Device + TextureTransferDevice + ?Sized>(
        device: &mut D,
        width: u32,
        height: u32,
        render_scale: f32,
    ) -> Result<Self, RenderError> {
        let mut renderer = Self::new(device, width, height)?;
        renderer.set_render_scale(device, render_scale)?;
        Ok(renderer)
    }

    /// Backend used by the device this renderer was created with.
    #[must_use]
    pub fn backend(&self) -> BackendKind {
        self.gpu.backend()
    }

    /// Current internal render scale.
    #[must_use]
    pub fn render_scale(&self) -> f32 {
        self.gpu.render_scale()
    }

    /// Changes the internal HDR render scale and recreates scene/bloom targets.
    ///
    /// The output/post target size, readback size, and CPU picking viewport stay unchanged.
    /// `0.25..=2.0` is accepted; values below 1 trade sharpness for fill rate and memory, values
    /// above 1 supersample and downsample in the post pass.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidSize`] for a non-finite or out-of-range scale, or
    /// [`RenderError::Gpu`] when targets or resource sets cannot be recreated.
    pub fn set_render_scale<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        render_scale: f32,
    ) -> Result<(), RenderError> {
        self.gpu.set_render_scale(device, render_scale)?;
        self.last_frame = None;
        self.renderer_statistics = self.gpu.statistics();
        Ok(())
    }

    /// Current render target width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Current render target height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Resizes the scene and post targets. Shadow targets keep their resolution.
    ///
    /// This destroys and recreates size-dependent render targets, pipelines, and per-batch
    /// resource sets on the next frame. Cached meshes and textures stay resident.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidSize`] for a zero or oversized target and
    /// [`RenderError::Gpu`] when target or pipeline recreation fails.
    pub fn resize<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(RenderError::InvalidSize(format!("{width}x{height}")));
        }
        if self.width == width && self.height == height {
            return Ok(());
        }
        self.gpu.resize(device, width, height)?;
        self.width = width;
        self.height = height;
        self.last_frame = None;
        Ok(())
    }

    /// Renders one frame into the offscreen scene and post targets.
    ///
    /// `textures` resolves the `TextureAssetId` references stored in prepared materials. Assets
    /// that are referenced but absent from the table use a white fallback and do not fail the
    /// frame. Returns CPU-side counters; call [`OffscreenRenderer::capture`] to read pixels.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Scene`] when the camera cannot produce a valid projection and
    /// [`RenderError::Gpu`] when uploads or submission fail.
    pub fn render<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        scene: &Scene,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
    ) -> Result<FrameStatistics, RenderError> {
        let aspect = self.width as f32 / self.height as f32;
        let started = Instant::now();
        let key = FrameReuseKey {
            scene_id: scene.instance_id(),
            scene_version: scene.version(),
            camera,
            options: *options,
            target: (self.width, self.height),
            render_scale: self.gpu.render_scale(),
        };
        if let Some((last_key, last_statistics)) = self.last_frame
            && last_key == key
        {
            let mut statistics = last_statistics;
            statistics.frame_reused = true;
            statistics.camera_only_update = false;
            statistics.mesh_uploads = 0;
            statistics.texture_uploads = 0;
            statistics.upload_bytes = 0;
            statistics.submit_nanos = 0;
            statistics.cpu_prepare_nanos =
                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.renderer_statistics = self.gpu.statistics();
            return Ok(statistics);
        }
        let camera_only = self.prepared_key == Some((key.scene_id, key.scene_version))
            && self.prepared.camera() != Some(camera)
            && !self.prepared.uses_lod();
        if camera_only {
            self.prepared.update_camera(camera, aspect)?;
        } else {
            self.prepared.update(scene, camera, aspect)?;
        }
        self.prepared_key = Some((key.scene_id, key.scene_version));
        let mut statistics = self.gpu.render(
            device,
            &self.prepared,
            textures,
            camera,
            options,
            (self.width, self.height),
            started,
            !camera_only,
            scene.environment(),
        )?;
        statistics.camera_only_update = camera_only;
        self.renderer_statistics = self.gpu.statistics();
        self.last_frame = Some((key, statistics));
        Ok(statistics)
    }

    /// Renders one animation-sampled frame without mutating the authored scene.
    ///
    /// The caller supplies the clip tracks and clip time, matching the GPUI scene-view contract:
    /// gpui-3d owns no playback clock. Repeated samples reuse retained animation and preparation
    /// storage on this renderer.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Preparation`] when track sampling fails, [`RenderError::Scene`]
    /// when the camera cannot project the sampled pose, and [`RenderError::Gpu`] when submission
    /// fails.
    pub fn render_animated<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        scene: &Scene,
        tracks: &[TransformTrack],
        animation_time: Duration,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
    ) -> Result<FrameStatistics, RenderError> {
        let aspect = self.width as f32 / self.height as f32;
        let started = Instant::now();
        let evaluated = scene
            .evaluate_with(tracks, animation_time, &mut self.animation_scratch)
            .map_err(|error| RenderError::Preparation(format!("animation evaluation: {error}")))?;
        self.prepared.update_evaluated(&evaluated, camera, aspect)?;
        self.prepared_key = None;
        self.last_frame = None;
        let statistics = self.gpu.render(
            device,
            &self.prepared,
            textures,
            camera,
            options,
            (self.width, self.height),
            started,
            true,
            scene.environment(),
        )?;
        self.renderer_statistics = self.gpu.statistics();
        Ok(statistics)
    }

    /// Renders one crossfaded animation frame without mutating the authored scene.
    ///
    /// Samples two clips at their own absolute clip times and blends them with `blend` as the
    /// secondary weight (`0` = primary pose, `1` = secondary pose). Nodes animated by only one clip
    /// fade toward their authored rest pose, so callers can crossfade between locomotion clips and
    /// idle without an explicit idle track. Reuses the renderer's retained animation storage and
    /// preparation buffers.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Preparation`] when clip sampling or blending fails,
    /// [`RenderError::Scene`] when the camera cannot project the sampled pose, and
    /// [`RenderError::Gpu`] when submission fails.
    #[allow(
        clippy::too_many_arguments,
        reason = "Clip pair, times, blend weight, assets, camera, and options are the complete frame input; splitting them would create a redundant struct for one call site."
    )]
    pub fn render_blended<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        scene: &Scene,
        primary: &[TransformTrack],
        primary_time: Duration,
        secondary: &[TransformTrack],
        secondary_time: Duration,
        blend: f32,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
    ) -> Result<FrameStatistics, RenderError> {
        let aspect = self.width as f32 / self.height as f32;
        let started = Instant::now();
        let evaluated = scene
            .evaluate_blended_with(
                primary,
                primary_time,
                secondary,
                secondary_time,
                blend,
                &mut self.animation_scratch,
            )
            .map_err(|error| RenderError::Preparation(format!("animation blend: {error}")))?;
        self.prepared.update_evaluated(&evaluated, camera, aspect)?;
        self.prepared_key = None;
        self.last_frame = None;
        let statistics = self.gpu.render(
            device,
            &self.prepared,
            textures,
            camera,
            options,
            (self.width, self.height),
            started,
            true,
            scene.environment(),
        )?;
        self.renderer_statistics = self.gpu.statistics();
        Ok(statistics)
    }

    /// Reads the last rendered frame back into tightly packed RGBA8 pixels.
    ///
    /// The backend waits for the submitted frame before the copy, so the returned pixels match
    /// the most recent [`OffscreenRenderer::render`] call.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Gpu`] when the readback copy fails and
    /// [`RenderError::Readback`] when the backend returns an unexpected layout.
    pub fn capture<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<CapturedImage, RenderError> {
        self.gpu.capture(device)
    }

    /// Polls full GPU completion of the most recently rendered frame without blocking.
    ///
    /// Backends with a deferred offscreen submission path (currently DX12) return
    /// Ok(Some(false)) while work is still in flight and Ok(Some(true)) once it is done, so a
    /// caller can overlap CPU work with rendering before calling OffscreenRenderer::capture.
    /// Backends without that path return Ok(None); capture already waits for them, so callers
    /// should simply skip polling.
    ///
    /// Polling consumes the submission handle on completion; a later capture still reads the
    /// latest frame and does not wait again.
    ///
    /// # Errors
    ///
    /// Returns RenderError::Gpu when submission polling reports a device failure.
    pub fn poll_render<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<Option<bool>, RenderError> {
        self.gpu.poll_pending_submission(device)
    }

    /// Renders one frame and reads it back.
    ///
    /// Convenience for capture-oriented tools and golden-image tests.
    ///
    /// # Errors
    ///
    /// Returns any error from [`OffscreenRenderer::render`] or
    /// [`OffscreenRenderer::capture`].
    pub fn render_to_image<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        scene: &Scene,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
    ) -> Result<CapturedImage, RenderError> {
        self.render(device, scene, textures, camera, options)?;
        self.capture(device)
    }

    /// Releases cached GPU assets according to a GPUI memory-trim level.
    ///
    /// - `Light` keeps all caches.
    /// - `Moderate` releases meshes, textures, and resource sets no frame used recently.
    /// - `Aggressive` releases every cached mesh, texture, and resource set.
    ///
    /// Render targets and pipelines stay resident because the next frame needs them.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Gpu`] when a resource cannot be destroyed.
    pub fn trim<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        level: MemoryTrimLevel,
    ) -> Result<(), RenderError> {
        self.gpu.trim(device, level)?;
        self.last_frame = None;
        self.renderer_statistics = self.gpu.statistics();
        Ok(())
    }

    /// Current renderer-owned resource statistics.
    #[must_use]
    pub fn statistics(&self) -> RendererStatistics {
        self.renderer_statistics
    }

    /// Selects the nearest scene surface under a top-left-origin viewport pixel.
    ///
    /// Uses the CPU BVH and the camera pose supplied by the caller; it does not submit GPU work
    /// and does not require a device. For blending/mask-accurate selection use
    /// [`OffscreenRenderer::pick_object`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`PickError`] when the camera cannot produce a ray for the pixel.
    pub fn pick(
        &mut self,
        scene: &Scene,
        camera: Camera,
        pixel: (f32, f32),
    ) -> Result<Option<RaycastHit>, PickError> {
        pick(
            scene,
            camera,
            (self.width, self.height),
            pixel,
            &mut self.raycast_scratch,
        )
    }

    /// Selects the nearest surface of an animation-sampled pose under a viewport pixel.
    ///
    /// # Errors
    ///
    /// Returns [`PickError`] when animation sampling produces a stale or invalid pose, or when the
    /// camera cannot produce a ray.
    pub fn pick_animated(
        &mut self,
        scene: &Scene,
        tracks: &[TransformTrack],
        animation_time: Duration,
        camera: Camera,
        pixel: (f32, f32),
    ) -> Result<Option<RaycastHit>, PickError> {
        let viewport = (self.width, self.height);
        let ray = viewport_ray(camera, viewport, pixel)?;
        let evaluated = scene.evaluate_with(tracks, animation_time, &mut self.animation_scratch)?;
        Ok(evaluated.raycast(ray, &mut self.raycast_scratch)?)
    }

    /// Selects the submitted instance under a viewport pixel from the GPU object-id output.
    ///
    /// This re-renders the current scene with [`DebugView::ObjectId`], reads the resolved frame
    /// back, decodes the submitted instance index, and maps it to the prepared draw. Unlike CPU
    /// picking it follows the exact submitted pose, culling, and blend ordering, so it matches
    /// what the GPU drew. The renderer's last frame remains the object-id frame.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] when rendering, readback, or the instance mapping fails.
    pub fn pick_object<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        scene: &Scene,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
        pixel: (u32, u32),
    ) -> Result<Option<ObjectPick>, RenderError> {
        let mut object_options = *options;
        object_options.debug_view = DebugView::ObjectId;
        object_options.post_process = false;
        self.render(device, scene, textures, camera, &object_options)?;
        let image = self.capture(device)?;
        // The object-id frame is a debugging side effect; never serve it as a later shaded frame.
        self.last_frame = None;
        let x = pixel.0.min(self.width.saturating_sub(1));
        let y = pixel.1.min(self.height.saturating_sub(1));
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        let rgba = image.rgba();
        let id = u32::from(rgba[offset])
            | (u32::from(rgba[offset + 1]) << 8)
            | (u32::from(rgba[offset + 2]) << 16);
        if id == 0 {
            return Ok(None);
        }
        let index = (id - 1) as usize;
        let draw = self.prepared.draws.get(index).ok_or_else(|| {
            RenderError::Pick(format!(
                "object id {id} maps outside {} prepared draws",
                self.prepared.draws.len()
            ))
        })?;
        Ok(Some(ObjectPick {
            node: draw.node,
            mesh: draw.mesh.id(),
            part: draw.part,
        }))
    }

    /// Sets a soft GPU memory budget for cached assets.
    ///
    /// After every frame the renderer evicts least-recently-used assets that the frame did not
    /// reference until the resident total fits the budget. Assets referenced by the current frame
    /// are never evicted, so a single frame whose working set exceeds the budget stays over
    /// budget. [`None`] disables automatic trimming.
    pub fn set_memory_budget(&mut self, budget_bytes: Option<u64>) {
        self.gpu.set_memory_budget(budget_bytes);
        self.last_frame = None;
        self.renderer_statistics = self.gpu.statistics();
    }

    /// Current soft GPU memory budget in bytes.
    #[must_use]
    pub fn memory_budget(&self) -> Option<u64> {
        self.gpu.memory_budget()
    }

    /// Evicts least-recently-used cached assets until the resident total fits the budget.
    ///
    /// Returns the number of GPU bytes released. Assets referenced by the most recent frame are
    /// retained, so the call can return zero while still over budget when the current working set
    /// alone exceeds the budget.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Gpu`] when a resource cannot be destroyed.
    pub fn trim_to_budget<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<u64, RenderError> {
        let released = self.gpu.enforce_memory_budget(device)?;
        self.last_frame = None;
        self.renderer_statistics = self.gpu.statistics();
        Ok(released)
    }

    /// Releases every GPU resource owned by this renderer.
    ///
    /// Call this before destroying the device. The renderer cannot allocate a new frame after
    /// this call; drop it afterwards.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Gpu`] when a resource cannot be destroyed. Remaining resources
    /// stay tracked and will be released by later calls.
    pub fn destroy<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<(), RenderError> {
        self.gpu.destroy(device)
    }
}
