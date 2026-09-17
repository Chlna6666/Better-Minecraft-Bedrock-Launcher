use super::{line_layout::ShapedGlyph, primitives::RenderGlyphParams};

impl ShapedGlyph {
    /// Whether this glyph should use the stable vertical raster frame policy.
    ///
    /// This policy is intentionally independent from Unicode script identity. Dense square-script
    /// families may opt in, while all other shaping/fallback decisions continue to use script
    /// metadata from `text_system::script`.
    #[inline]
    pub(crate) fn uses_stable_vertical_raster_frame(&self) -> bool {
        self.is_cjk && !self.is_emoji
    }
}

impl RenderGlyphParams {
    /// Whether rasterization should normalize this glyph to a stable vertical frame.
    #[inline]
    pub(crate) fn uses_stable_vertical_raster_frame(&self) -> bool {
        self.is_cjk && !self.is_emoji
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, GlyphId, Point, px};

    #[test]
    fn stable_vertical_frame_policy_is_independent_from_emoji() {
        let mut params = RenderGlyphParams {
            font_id: FontId(0),
            glyph_id: GlyphId(1),
            font_size: px(13.),
            subpixel_variant: Point::default(),
            scale_factor: 1.0,
            grayscale_antialiasing: false,
            is_emoji: false,
            is_cjk: true,
        };
        assert!(params.uses_stable_vertical_raster_frame());

        params.is_emoji = true;
        assert!(!params.uses_stable_vertical_raster_frame());
    }
}
