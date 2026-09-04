use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};

use crate::{
    AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels, Rgba, ScaledPixels,
    TransitionProperty,
};

use super::{DrawOrder, PaintGpuMesh3d, Path, Scene, TransformationMatrix};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    PolychromeSprite,
    Surface,
    BackdropBlur,
    Blur,
    GpuMesh3d,
}

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

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
    StartBlur(BlurCapture),
    EndBlur,
}

#[derive(Clone)]
pub(crate) enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
    BackdropBlur(PaintBackdropBlur),
    Blur(PaintBlur),
    GpuMesh3d(PaintGpuMesh3d),
}

impl Primitive {
    pub(crate) fn visually_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Shadow(left), Self::Shadow(right)) => left == right,
            (Self::Quad(left), Self::Quad(right)) => left == right,
            (Self::Path(left), Self::Path(right)) => left == right,
            (Self::Underline(left), Self::Underline(right)) => left == right,
            (Self::MonochromeSprite(left), Self::MonochromeSprite(right)) => left == right,
            (Self::PolychromeSprite(left), Self::PolychromeSprite(right)) => left == right,
            (Self::BackdropBlur(left), Self::BackdropBlur(right)) => left.visually_eq(right),
            (Self::Blur(left), Self::Blur(right)) => left == right,
            (Self::Surface(_), Self::Surface(_)) | (Self::GpuMesh3d(_), Self::GpuMesh3d(_)) => {
                false
            }
            _ => false,
        }
    }

    pub(crate) fn visual_bounds(&self) -> Bounds<ScaledPixels> {
        let bounds = match self {
            Self::Shadow(shadow) => {
                let radius = shadow.blur_radius.0.abs();
                let margin = if radius.is_finite() {
                    ScaledPixels(radius * 3.0)
                } else {
                    ScaledPixels(0.0)
                };
                shadow.bounds.dilate(margin)
            }
            Self::Blur(blur) => blur.bounds,
            _ => *self.bounds(),
        };
        bounds.intersect(&self.content_mask().bounds)
    }

    pub(crate) fn order(&self) -> DrawOrder {
        match self {
            Primitive::Shadow(shadow) => shadow.order,
            Primitive::Quad(quad) => quad.order,
            Primitive::Path(path) => path.order,
            Primitive::Underline(underline) => underline.order,
            Primitive::MonochromeSprite(sprite) => sprite.order,
            Primitive::PolychromeSprite(sprite) => sprite.order,
            Primitive::Surface(surface) => surface.order,
            Primitive::BackdropBlur(blur) => blur.order,
            Primitive::Blur(blur) => blur.order,
            Primitive::GpuMesh3d(mesh) => mesh.order,
        }
    }

    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
            Primitive::BackdropBlur(blur) => &blur.bounds,
            Primitive::Blur(blur) => &blur.bounds,
            Primitive::GpuMesh3d(mesh) => &mesh.bounds,
        }
    }

    pub(crate) fn set_order(&mut self, order: DrawOrder) {
        match self {
            Primitive::Shadow(shadow) => shadow.order = order,
            Primitive::Quad(quad) => quad.order = order,
            Primitive::Path(path) => path.order = order,
            Primitive::Underline(underline) => underline.order = order,
            Primitive::MonochromeSprite(sprite) => sprite.order = order,
            Primitive::PolychromeSprite(sprite) => sprite.order = order,
            Primitive::Surface(surface) => surface.order = order,
            Primitive::BackdropBlur(blur) => blur.order = order,
            Primitive::Blur(blur) => blur.order = order,
            Primitive::GpuMesh3d(mesh) => mesh.order = order,
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
            Primitive::BackdropBlur(blur) => &blur.content_mask,
            Primitive::Blur(blur) => &blur.content_mask,
            Primitive::GpuMesh3d(mesh) => &mesh.content_mask,
        }
    }

    pub(crate) fn animation_id(&self) -> Option<SceneAnimationId> {
        match self {
            Primitive::Shadow(shadow) => shadow.animation_id,
            Primitive::Quad(quad) => quad.animation_id,
            Primitive::MonochromeSprite(sprite) => sprite.animation_id,
            Primitive::PolychromeSprite(sprite) => sprite.animation_id,
            Primitive::BackdropBlur(blur) => blur.animation_id,
            Primitive::Blur(blur) => blur.animation_id,
            Primitive::Path(_)
            | Primitive::Underline(_)
            | Primitive::Surface(_)
            | Primitive::GpuMesh3d(_) => None,
        }
    }

    pub(crate) fn set_animation_id(&mut self, animation_id: SceneAnimationId) {
        match self {
            Primitive::Shadow(shadow) => shadow.animation_id = Some(animation_id),
            Primitive::Quad(quad) => quad.animation_id = Some(animation_id),
            Primitive::MonochromeSprite(sprite) => sprite.animation_id = Some(animation_id),
            Primitive::PolychromeSprite(sprite) => sprite.animation_id = Some(animation_id),
            Primitive::BackdropBlur(blur) => blur.animation_id = Some(animation_id),
            Primitive::Blur(blur) => blur.animation_id = Some(animation_id),
            Primitive::Path(_)
            | Primitive::Underline(_)
            | Primitive::Surface(_)
            | Primitive::GpuMesh3d(_) => {}
        }
    }
}

impl PaintOperation {
    pub(crate) fn visually_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Primitive(left), Self::Primitive(right)) => left.visually_eq(right),
            // Paint layers are batching/order metadata and do not produce pixels themselves. Their
            // spatial bounds may move with a layout animation while every actual pixel-producing
            // primitive remains outside a backdrop sampling region. Treating that metadata motion
            // as a visual mismatch prematurely terminates the unchanged prefix and makes all later
            // primitives look dirty to backdrop/present damage tracking.
            (Self::StartLayer(_), Self::StartLayer(_)) => true,
            (Self::EndLayer, Self::EndLayer) => true,
            (Self::StartBlur(left), Self::StartBlur(right)) => left == right,
            (Self::EndBlur, Self::EndBlur) => true,
            _ => false,
        }
    }

    pub(crate) fn visual_bounds(&self) -> Option<Bounds<ScaledPixels>> {
        match self {
            Self::Primitive(primitive) => Some(primitive.visual_bounds()),
            // A paint layer is batching metadata, not a pixel-producing operation. Its bounds may
            // cover the entire window while the actual primitives inside it occupy a small region.
            // Counting the layer rectangle as visual damage makes unrelated backdrop filters
            // re-sample whenever a layout/route animation moves a full-window batching container.
            Self::StartLayer(_) => None,
            Self::StartBlur(blur) => Some(
                blur.bounds
                    .dilate(blur_influence_radius(blur.radius))
                    .intersect(&blur.content_mask.bounds),
            ),
            Self::EndLayer | Self::EndBlur => None,
        }
    }
}

/// Capture parameters for a CSS `filter: blur(...)` element group.
///
/// `radius` is the CSS Gaussian standard deviation (sigma), expressed in scaled pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BlurCapture {
    pub(crate) animation_id: Option<SceneAnimationId>,
    pub(crate) bounds: Bounds<ScaledPixels>,
    pub(crate) content_mask: ContentMask<ScaledPixels>,
    pub(crate) radius: ScaledPixels,
    pub(crate) opacity: f32,
}

pub(crate) fn blur_influence_radius(radius: ScaledPixels) -> ScaledPixels {
    let sigma = radius.0.abs();
    if !sigma.is_finite() || sigma <= 0.0 {
        return ScaledPixels(0.0);
    }

    // CSS blur uses the Gaussian standard deviation. Three sigma covers the practical filter
    // support; the extra half pixel accounts for linear filtering at the target edge.
    ScaledPixels(sigma * 3.0 + 0.5)
}

/// A scene subtree rendered through a CSS element blur filter.
#[derive(Clone)]
pub(crate) struct PaintBlur {
    pub order: DrawOrder,
    /// Renderer-owned visual animation promoted from the captured subtree. The child scene keeps
    /// its raster/filter geometry static; only this final composite primitive is sampled per frame.
    pub animation_id: Option<SceneAnimationId>,
    /// The capture bounds including the blur's 3-sigma and linear filtering support.
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    /// CSS Gaussian standard deviation (sigma), in scaled pixels.
    pub radius: ScaledPixels,
    pub opacity: f32,
    pub content: Arc<Scene>,
}

impl fmt::Debug for PaintBlur {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaintBlur")
            .field("order", &self.order)
            .field("animation_id", &self.animation_id)
            .field("bounds", &self.bounds)
            .field("content_mask", &self.content_mask)
            .field("radius", &self.radius)
            .field("opacity", &self.opacity)
            .field("content", &self.content.len())
            .finish()
    }
}

impl PartialEq for PaintBlur {
    fn eq(&self, other: &Self) -> bool {
        self.order == other.order
            && self.animation_id == other.animation_id
            && self.bounds == other.bounds
            && self.content_mask == other.content_mask
            && self.radius == other.radius
            && self.opacity == other.opacity
            && (Arc::ptr_eq(&self.content, &other.content)
                || (self.content.paint_operations.len() == other.content.paint_operations.len()
                    && self
                        .content
                        .paint_operations
                        .iter()
                        .zip(&other.content.paint_operations)
                        .all(|(left, right)| left.visually_eq(right))))
    }
}

impl From<PaintBlur> for Primitive {
    fn from(blur: PaintBlur) -> Self {
        Primitive::Blur(blur)
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
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

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Clone, PartialEq)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub pad: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Rgba,
    pub thickness: ScaledPixels,
    pub wavy: u32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
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

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
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

#[derive(Clone, Debug, PartialEq)]
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

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub grayscale: bool,
    pub opacity: f32,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
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

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
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

impl From<PaintBackdropBlur> for Primitive {
    fn from(blur: PaintBackdropBlur) -> Self {
        Primitive::BackdropBlur(blur)
    }
}
