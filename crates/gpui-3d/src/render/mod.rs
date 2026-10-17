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
    AnimationScratch, Camera, CameraError, Scene, TextureAsset, TextureAssetId, TransformTrack,
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
    pub resolution: u32,
    /// Constant depth bias in normalized light-space depth.
    pub depth_bias: f32,
    /// Slope-scaled depth bias applied to surfaces facing away from the light.
    pub normal_bias: f32,
    /// Shadow darkening from 0 (no visible shadow) to 1 (fully occluded).
    pub strength: f32,
}

impl Default for ShadowOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            resolution: 2048,
            depth_bias: 0.0025,
            normal_bias: 0.0025,
            strength: 1.0,
        }
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
    /// Display-referred clear color for the scene pass (RGBA, 0-1).
    pub clear_color: [f32; 4],
    /// Whether the post pass applies vignette grading.
    pub post_process: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            shadows: ShadowOptions::default(),
            exposure: 1.0,
            tonemap: Tonemap::Aces,
            debug_view: DebugView::Shaded,
            clear_color: [0.045, 0.055, 0.075, 1.0],
            post_process: true,
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
    /// Draw-slot capacity in entries.
    pub draw_capacity: usize,
    /// Instance capacity in entries.
    pub instance_capacity: usize,
    /// Light capacity in entries.
    pub light_capacity: usize,
    /// Current render target width in pixels.
    pub target_width: u32,
    /// Current render target height in pixels.
    pub target_height: u32,
    /// Shadow map resolution in texels.
    pub shadow_resolution: u32,
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
    animation_scratch: AnimationScratch,
    renderer_statistics: RendererStatistics,
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
            animation_scratch: AnimationScratch::new(),
            renderer_statistics: RendererStatistics::default(),
        })
    }

    /// Backend used by the device this renderer was created with.
    #[must_use]
    pub fn backend(&self) -> BackendKind {
        self.gpu.backend()
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
    pub fn resize<D: Device + ?Sized>(
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
    pub fn render<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        scene: &Scene,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
    ) -> Result<FrameStatistics, RenderError> {
        let aspect = self.width as f32 / self.height as f32;
        let started = Instant::now();
        self.prepared.update(scene, camera, aspect)?;
        let statistics = self.gpu.render(
            device,
            &self.prepared,
            textures,
            camera,
            options,
            (self.width, self.height),
            started,
        )?;
        self.renderer_statistics = self.gpu.statistics();
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
    pub fn render_animated<D: Device + ?Sized>(
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
        let statistics = self.gpu.render(
            device,
            &self.prepared,
            textures,
            camera,
            options,
            (self.width, self.height),
            started,
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
    pub fn trim<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        level: MemoryTrimLevel,
    ) -> Result<(), RenderError> {
        self.gpu.trim(device, level)?;
        self.renderer_statistics = self.gpu.statistics();
        Ok(())
    }

    /// Current renderer-owned resource statistics.
    #[must_use]
    pub fn statistics(&self) -> RendererStatistics {
        self.renderer_statistics
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
    pub fn destroy<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<(), RenderError> {
        self.gpu.destroy(device)
    }
}
