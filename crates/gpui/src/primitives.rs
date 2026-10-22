//! Pixel-producing primitives and filter parameters, independent of window display-list storage.
//! UI painting and backend encoding share these canonical types without display-list ownership.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels, Rgba, ScaledPixels,
    TransitionProperty,
};

mod extension_device;
mod transform;
pub use extension_device::ExtensionDevice;
pub use transform::TransformationMatrix;
mod path;
mod path_builder;
mod renderer_extension;
pub use path::Path;
pub(crate) use path::{PathCacheId, PathGeometryGeneration, PathId, PathVertex_ScaledPixels};
pub use path_builder::*;
pub(crate) use renderer_extension::PaintRendererExtension;
pub use renderer_extension::{
    MemoryTrimLevel, RendererExtension, RendererExtensionContext, RendererExtensionRenderer,
};

pub(crate) type DrawOrder = u32;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct SceneAnimationId(pub(crate) u32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SceneAnimationValue {
    pub(crate) animation_id: SceneAnimationId,
    pub(crate) property: TransitionProperty,
    pub(crate) progress: f32,
    pub(crate) from: [f32; 4],
    pub(crate) to: [f32; 4],
}

#[derive(Default, Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub(crate) struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Rgba,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub pad: u32,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
    pub thickness: ScaledPixels,
    pub wavy: u32,
}

#[derive(Default, Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub(crate) struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum MonochromeSpriteSampling {
    Glyph = 0,
    Linear = 1,
}

impl MonochromeSprite {
    pub(crate) fn sampling(&self) -> MonochromeSpriteSampling {
        match self.pad {
            1 => MonochromeSpriteSampling::Linear,
            _ => MonochromeSpriteSampling::Glyph,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    /// Image sampling mode; 1 selects the nearest source texel.
    pub sampling: u32,
    pub grayscale: bool,
    pub opacity: f32,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

/// The backing content for a painted surface.
#[derive(Clone, Debug)]
pub(crate) enum SurfaceContent {
    #[cfg(target_os = "macos")]
    CoreVideo(core_video::pixel_buffer::CVPixelBuffer),
    #[cfg(not(target_os = "macos"))]
    Unsupported,
}

#[derive(Clone, Debug)]
pub(crate) struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub content: SurfaceContent,
}

/// Controls whether compatible overlapping blur primitives reuse one filtered target.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum BackdropBlurOverlapMode {
    /// Reuse one Gaussian result for compatible overlapping primitives. This is the default.
    #[default]
    Reuse,
    /// Recompute every blur primitive independently, including pixels covered by another blur.
    Recompute,
}

/// Parameters for GPU-backed backdrop blur.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BackdropBlurStyle {
    /// CSS Gaussian blur sigma (standard deviation) in logical pixels.
    pub radius: Pixels,
    /// Downsample factor used by backends that implement a separable GPU blur.
    pub downsample: u8,
    /// Number of filter levels requested by the backend.
    pub levels: u8,
    /// Saturation multiplier applied after blur.
    pub saturation: f32,
    /// Optional tint color blended over the blurred backdrop.
    pub tint: Option<Hsla>,
    /// Controls whether compatible overlap is reused or recomputed independently.
    pub overlap_mode: BackdropBlurOverlapMode,
}

impl BackdropBlurStyle {
    /// Creates a blur style with conservative defaults for interactive UI.
    pub fn new(radius: Pixels) -> Self {
        Self {
            radius,
            downsample: 2,
            levels: 3,
            saturation: 1.0,
            tint: None,
            overlap_mode: BackdropBlurOverlapMode::Reuse,
        }
    }

    /// Selects an efficient blur configuration for the configured radius.
    pub fn auto_quality(mut self) -> Self {
        let radius = self.radius.0.abs();
        let (downsample, levels) = if radius < 1.0 {
            (1, 2)
        } else if radius < 6.0 {
            (1, 2)
        } else if radius <= 12.0 {
            (2, 2)
        } else {
            (2, 3)
        };
        self.downsample = downsample;
        self.levels = levels;
        self
    }

    /// Sets the downsample factor. Values lower than one are clamped to one.
    pub fn downsample(mut self, downsample: u8) -> Self {
        self.downsample = downsample.max(1);
        self
    }

    /// Sets the number of filter levels. Values are clamped to `1..=6`.
    pub fn levels(mut self, levels: u8) -> Self {
        self.levels = levels.clamp(1, 6);
        self
    }

    /// Sets the saturation multiplier.
    pub fn saturation(mut self, saturation: f32) -> Self {
        self.saturation = saturation.max(0.0);
        self
    }

    /// Sets a tint color blended over the blurred backdrop.
    pub fn tint(mut self, tint: Hsla) -> Self {
        self.tint = Some(tint);
        self
    }

    /// Selects how compatible overlapping blur primitives are evaluated.
    pub fn overlap_mode(mut self, overlap_mode: BackdropBlurOverlapMode) -> Self {
        self.overlap_mode = overlap_mode;
        self
    }
}

impl From<Pixels> for BackdropBlurStyle {
    fn from(radius: Pixels) -> Self {
        Self::new(radius)
    }
}

impl From<f32> for BackdropBlurStyle {
    fn from(radius: f32) -> Self {
        Self::new(radius.into())
    }
}

impl From<f64> for BackdropBlurStyle {
    fn from(radius: f64) -> Self {
        Self::new(radius.into())
    }
}

/// Backdrop blur primitive emitted into the scene.
///
/// `PartialEq` intentionally represents the filter/source identity used by backdrop cache
/// invalidation. Composite-only state (draw order, animation id, clipping corner geometry,
/// saturation, opacity and tint) remains part of `visually_eq` so scene/static upload retention
/// still notices those visual changes without throwing away an otherwise reusable Gaussian result.
#[derive(Clone, Debug)]
pub(crate) struct PaintBackdropBlur {
    pub order: DrawOrder,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub radius: ScaledPixels,
    pub downsample: u8,
    pub levels: u8,
    pub saturation: f32,
    pub opacity: f32,
    pub tint: Option<Hsla>,
    pub recompute_overlap: bool,
}

impl PaintBackdropBlur {
    pub(crate) fn visually_eq(&self, other: &Self) -> bool {
        self.order == other.order
            && self.animation_id == other.animation_id
            && self.bounds == other.bounds
            && self.content_mask == other.content_mask
            && self.corner_radii == other.corner_radii
            && self.radius == other.radius
            && self.downsample == other.downsample
            && self.levels == other.levels
            && self.saturation == other.saturation
            && self.opacity == other.opacity
            && self.tint == other.tint
            && self.recompute_overlap == other.recompute_overlap
    }
}

impl PartialEq for PaintBackdropBlur {
    fn eq(&self, other: &Self) -> bool {
        self.bounds == other.bounds
            && self.content_mask.bounds == other.content_mask.bounds
            && self.radius == other.radius
            && self.downsample == other.downsample
            && self.levels == other.levels
            && self.recompute_overlap == other.recompute_overlap
    }
}
