use super::animation::AnimationProperty;
use crate::{Pixels, TransformOrigin, px};

/// Describes a visual-only font-size animation that keeps text shaping and glyph rasterization
/// stable for the duration of the animation.
///
/// The text is rasterized once at `raster_font_size()` and the renderer scales that retained
/// result between the requested endpoint sizes. This is intended for transient UI motion where
/// intermediate frames must not reflow text. If changing the font size is expected to change line
/// wrapping, baselines, intrinsic width, or parent layout, animate the actual `text_size` on the
/// layout path instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualTextSizeAnimation {
    raster_font_size: Pixels,
    property: AnimationProperty,
}

impl VisualTextSizeAnimation {
    /// Create a visual-only font-size animation.
    ///
    /// The larger finite endpoint is selected as the raster size so the animation normally
    /// downscales cached glyphs rather than magnifying a smaller raster. Callers should render the
    /// text at [`Self::raster_font_size`] and attach [`Self::property`] to a renderer-owned
    /// animation or retained composite layer.
    pub fn between(from: Pixels, to: Pixels, origin: TransformOrigin) -> Self {
        let from = finite_non_negative(from.0);
        let to = finite_non_negative(to.0);
        let raster = from.max(to).max(1.0 / 4096.0);

        Self {
            raster_font_size: px(raster),
            property: AnimationProperty::scale_opacity(
                from / raster,
                to / raster,
                1.0,
                1.0,
                origin,
            ),
        }
    }

    /// Font size that should be used for the single cached/rasterized text representation.
    pub fn raster_font_size(self) -> Pixels {
        self.raster_font_size
    }

    /// Renderer-owned scale property that visually maps the retained raster between endpoints.
    pub fn property(self) -> AnimationProperty {
        self.property
    }
}

impl AnimationProperty {
    /// Animate visual scale without changing layout or text raster identity.
    pub fn scale(from: f32, to: f32, origin: TransformOrigin) -> Self {
        Self::scale_opacity(from, to, 1.0, 1.0, origin)
    }
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_text_size_uses_larger_endpoint_as_raster_size() {
        let animation = VisualTextSizeAnimation::between(
            px(14.0),
            px(18.0),
            TransformOrigin::CENTER,
        );

        assert_eq!(animation.raster_font_size(), px(18.0));
        assert_eq!(
            animation.property(),
            AnimationProperty::scale_opacity(
                14.0 / 18.0,
                1.0,
                1.0,
                1.0,
                TransformOrigin::CENTER,
            )
        );
    }

    #[test]
    fn visual_text_size_sanitizes_invalid_endpoints() {
        let animation = VisualTextSizeAnimation::between(
            px(f32::NAN),
            px(-4.0),
            TransformOrigin::CENTER,
        );

        assert!(animation.raster_font_size().0.is_finite());
        assert!(animation.raster_font_size().0 > 0.0);
    }

    #[test]
    fn scale_is_a_scale_only_renderer_property() {
        assert_eq!(
            AnimationProperty::scale(0.8, 1.0, TransformOrigin::CENTER),
            AnimationProperty::scale_opacity(
                0.8,
                1.0,
                1.0,
                1.0,
                TransformOrigin::CENTER,
            )
        );
    }
}
