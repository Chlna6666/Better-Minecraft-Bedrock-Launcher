use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels, ScaledPixels,
    TransitionProperty,
};

use super::{DrawOrder, PaintGpuMesh3d, Path, TransformationMatrix};

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
    GpuMesh3d(PaintGpuMesh3d),
}

impl Primitive {
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
            Primitive::Path(_)
            | Primitive::Underline(_)
            | Primitive::Surface(_)
            | Primitive::GpuMesh3d(_) => {}
        }
    }
}

#[derive(Default, Debug, Clone)]
#[repr(C)]
pub(crate) struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: u32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
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

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub animation_id: Option<SceneAnimationId>,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
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

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
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

/// Parameters for GPU-backed backdrop blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackdropBlurStyle {
    /// Blur radius in logical pixels.
    pub radius: Pixels,
    /// Downsample factor used by backends that implement a separable GPU blur.
    pub downsample: u8,
    /// Number of Dual Kawase downsample/upsample levels.
    pub levels: u8,
    /// Saturation multiplier applied after blur.
    pub saturation: f32,
    /// Optional tint color blended over the blurred backdrop.
    pub tint: Option<Hsla>,
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
        }
    }

    /// Sets the downsample factor. Values lower than one are clamped to one.
    pub fn downsample(mut self, downsample: u8) -> Self {
        self.downsample = downsample.max(1);
        self
    }

    /// Sets the number of Dual Kawase blur levels. Values are clamped to `1..=6`.
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
    pub tint: Option<Hsla>,
}

impl From<PaintBackdropBlur> for Primitive {
    fn from(blur: PaintBackdropBlur) -> Self {
        Primitive::BackdropBlur(blur)
    }
}
