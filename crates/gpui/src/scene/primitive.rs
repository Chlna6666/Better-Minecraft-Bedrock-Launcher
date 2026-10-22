use std::{fmt, sync::Arc};

use crate::{
    Bounds, ContentMask, DrawOrder, MonochromeSprite, PaintBackdropBlur, PaintSurface,
    PolychromeSprite, Quad, ScaledPixels, SceneAnimationId, Shadow, Underline,
};

use super::geometry::is_solid_quad;
use super::{PaintRendererExtension, Path, Scene};

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
    RendererExtension,
}

#[derive(Clone)]
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
    RendererExtension(PaintRendererExtension),
}

impl Primitive {
    pub(crate) fn has_same_batch_topology(&self, other: &Self) -> bool {
        if self.order() != other.order() {
            return false;
        }

        match (self, other) {
            (Self::Shadow(_), Self::Shadow(_))
            | (Self::Path(_), Self::Path(_))
            | (Self::Underline(_), Self::Underline(_))
            | (Self::Surface(_), Self::Surface(_))
            | (Self::BackdropBlur(_), Self::BackdropBlur(_))
            | (Self::Blur(_), Self::Blur(_))
            | (Self::RendererExtension(_), Self::RendererExtension(_)) => true,
            (Self::Quad(left), Self::Quad(right)) => is_solid_quad(left) == is_solid_quad(right),
            (Self::MonochromeSprite(left), Self::MonochromeSprite(right)) => {
                left.tile.texture_id == right.tile.texture_id && left.sampling() == right.sampling()
            }
            (Self::PolychromeSprite(left), Self::PolychromeSprite(right)) => {
                left.tile.texture_id == right.tile.texture_id
            }
            _ => false,
        }
    }

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
            (Self::RendererExtension(left), Self::RendererExtension(right)) => {
                left.visually_eq(right)
            }
            (Self::Surface(_), Self::Surface(_)) => false,
            _ => false,
        }
    }

    pub(crate) fn visual_bounds(&self) -> Bounds<ScaledPixels> {
        self.unclipped_visual_bounds()
            .intersect(&self.content_mask().bounds)
    }

    /// Pixel-producing geometry before the content mask. Translation must
    /// move this geometry before applying a fixed ancestor clip.
    pub(crate) fn unclipped_visual_bounds(&self) -> Bounds<ScaledPixels> {
        match self {
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
        }
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
            Primitive::RendererExtension(extension) => extension.order,
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
            Primitive::RendererExtension(extension) => &extension.bounds,
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
            Primitive::RendererExtension(extension) => extension.order = order,
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
            Primitive::RendererExtension(extension) => &extension.content_mask,
        }
    }

    pub(crate) fn animation_id(&self) -> Option<SceneAnimationId> {
        match self {
            Primitive::Shadow(shadow) => shadow.animation_id,
            Primitive::Quad(quad) => quad.animation_id,
            Primitive::Path(path) => path.animation_id,
            Primitive::Underline(underline) => underline.animation_id,
            Primitive::MonochromeSprite(sprite) => sprite.animation_id,
            Primitive::PolychromeSprite(sprite) => sprite.animation_id,
            Primitive::BackdropBlur(blur) => blur.animation_id,
            Primitive::Blur(blur) => blur.animation_id,
            Primitive::RendererExtension(extension) => extension.animation_id,
            Primitive::Surface(_) => None,
        }
    }

    pub(crate) fn set_animation_id(&mut self, animation_id: SceneAnimationId) {
        match self {
            Primitive::Shadow(shadow) => shadow.animation_id = Some(animation_id),
            Primitive::Quad(quad) => quad.animation_id = Some(animation_id),
            Primitive::Path(path) => path.animation_id = Some(animation_id),
            Primitive::Underline(underline) => underline.animation_id = Some(animation_id),
            Primitive::MonochromeSprite(sprite) => sprite.animation_id = Some(animation_id),
            Primitive::PolychromeSprite(sprite) => sprite.animation_id = Some(animation_id),
            Primitive::BackdropBlur(blur) => blur.animation_id = Some(animation_id),
            Primitive::Blur(blur) => blur.animation_id = Some(animation_id),
            Primitive::RendererExtension(extension) => extension.animation_id = Some(animation_id),
            Primitive::Surface(_) => {}
        }
    }
}

impl PaintOperation {
    pub(crate) fn same_paint(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Primitive(left), Self::Primitive(right)) => left.visually_eq(right),
            // Unlike damage comparison, Scene revision equality includes structural layer bounds.
            (Self::StartLayer(left), Self::StartLayer(right)) => left == right,
            (Self::EndLayer, Self::EndLayer) | (Self::EndBlur, Self::EndBlur) => true,
            (Self::StartBlur(left), Self::StartBlur(right)) => left == right,
            _ => false,
        }
    }

    pub(crate) fn has_same_batch_topology(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Primitive(left), Self::Primitive(right)) => left.has_same_batch_topology(right),
            (Self::StartLayer(_), Self::StartLayer(_))
            | (Self::EndLayer, Self::EndLayer)
            | (Self::StartBlur(_), Self::StartBlur(_))
            | (Self::EndBlur, Self::EndBlur) => true,
            _ => false,
        }
    }

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
    /// Structural retained identity. None when an element has no stable path.
    pub(crate) layer_id: Option<crate::GlobalElementId>,
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
    /// Structural retained identity independent of the layer's paint-order index.
    pub layer_id: Option<crate::GlobalElementId>,
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
            .field("layer_id", &self.layer_id)
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
            && self.layer_id == other.layer_id
            && self.bounds == other.bounds
            && self.content_mask == other.content_mask
            && self.radius == other.radius
            && self.opacity == other.opacity
            && (Arc::ptr_eq(&self.content, &other.content)
                || self
                    .content
                    .paint_operations
                    .matches_by(&other.content.paint_operations, |left, right| {
                        left.visually_eq(right)
                    }))
    }
}

impl From<PaintBlur> for Primitive {
    fn from(blur: PaintBlur) -> Self {
        Primitive::Blur(blur)
    }
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

impl From<PaintBackdropBlur> for Primitive {
    fn from(blur: PaintBackdropBlur) -> Self {
        Primitive::BackdropBlur(blur)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

impl From<PaintRendererExtension> for Primitive {
    fn from(extension: PaintRendererExtension) -> Self {
        Self::RendererExtension(extension)
    }
}
