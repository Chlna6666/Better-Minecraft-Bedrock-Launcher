use super::{
    line_layout::ShapedGlyph, primitives::RenderGlyphParams, script::TextClusterProperties,
};

/// Raster-bounds policy selected after shaping and before glyph atlas lookup.
///
/// This is deliberately orthogonal to Unicode script identity. Most glyphs keep their exact
/// raster bounds, while dense square-script families may opt into a stable vertical frame to
/// avoid visible atlas-bound jumps between adjacent glyphs.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum GlyphRasterPolicy {
    /// Preserve the exact raster bounds returned by the platform text backend.
    #[default]
    ExactBounds,
    /// Normalize only the vertical raster extent while preserving the glyph's exact x bounds.
    StableVerticalFrame,
}

impl GlyphRasterPolicy {
    /// Derive the raster policy directly from Unicode cluster metadata produced during shaping.
    ///
    /// Emoji keep their exact bitmap/color-glyph bounds even when they are adjacent to a dense
    /// square script. This keeps script classification and atlas normalization as separate concerns.
    #[inline]
    pub(crate) fn for_cluster(properties: TextClusterProperties, is_emoji: bool) -> Self {
        if !is_emoji && properties.uses_stable_vertical_raster_frame() {
            Self::StableVerticalFrame
        } else {
            Self::ExactBounds
        }
    }

    #[inline]
    pub(crate) fn uses_stable_vertical_frame(self) -> bool {
        matches!(self, Self::StableVerticalFrame)
    }
}

impl ShapedGlyph {
    /// Raster policy carried from shaping into paint.
    ///
    /// The compatibility storage bit is intentionally interpreted only here so shaping/fallback
    /// code cannot accidentally treat the raster decision as Unicode script identity.
    #[inline]
    pub(crate) fn raster_policy(&self) -> GlyphRasterPolicy {
        if self.is_cjk && !self.is_emoji {
            GlyphRasterPolicy::StableVerticalFrame
        } else {
            GlyphRasterPolicy::ExactBounds
        }
    }

    /// Whether this glyph should use the stable vertical raster frame policy.
    #[inline]
    pub(crate) fn uses_stable_vertical_raster_frame(&self) -> bool {
        self.raster_policy().uses_stable_vertical_frame()
    }
}

impl RenderGlyphParams {
    /// Raster policy used to derive cache identity and platform raster bounds.
    #[inline]
    pub(crate) fn raster_policy(&self) -> GlyphRasterPolicy {
        if self.is_cjk && !self.is_emoji {
            GlyphRasterPolicy::StableVerticalFrame
        } else {
            GlyphRasterPolicy::ExactBounds
        }
    }

    /// Whether rasterization should normalize this glyph to a stable vertical frame.
    #[inline]
    pub(crate) fn uses_stable_vertical_raster_frame(&self) -> bool {
        self.raster_policy().uses_stable_vertical_frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, GlyphId, Point, ShapedGlyph, point, px};
    use crate::text_system::script::text_cluster_properties;

    fn test_render_params() -> RenderGlyphParams {
        RenderGlyphParams {
            font_id: FontId(0),
            glyph_id: GlyphId(1),
            font_size: px(13.),
            subpixel_variant: Point::default(),
            scale_factor: 1.0,
            grayscale_antialiasing: false,
            is_emoji: false,
            is_cjk: false,
        }
    }

    #[test]
    fn cluster_metadata_maps_to_narrow_raster_policy() {
        for (text, is_emoji, expected) in [
            ("中文", false, GlyphRasterPolicy::StableVerticalFrame),
            ("한글", false, GlyphRasterPolicy::StableVerticalFrame),
            ("العربية", false, GlyphRasterPolicy::ExactBounds),
            ("हिन्दी", false, GlyphRasterPolicy::ExactBounds),
            ("😀", true, GlyphRasterPolicy::ExactBounds),
        ] {
            assert_eq!(
                GlyphRasterPolicy::for_cluster(text_cluster_properties(text), is_emoji),
                expected,
                "text={text:?}",
            );
        }
    }

    #[test]
    fn emoji_flag_overrides_stable_vertical_script_policy() {
        let han = text_cluster_properties("中");
        assert_eq!(
            GlyphRasterPolicy::for_cluster(han, true),
            GlyphRasterPolicy::ExactBounds
        );
    }

    #[test]
    fn render_params_map_compatibility_bit_to_typed_policy() {
        let mut params = test_render_params();
        assert_eq!(params.raster_policy(), GlyphRasterPolicy::ExactBounds);

        params.is_cjk = true;
        assert_eq!(
            params.raster_policy(),
            GlyphRasterPolicy::StableVerticalFrame
        );
        assert!(params.uses_stable_vertical_raster_frame());
    }

    #[test]
    fn emoji_always_uses_exact_bounds() {
        let mut params = test_render_params();
        params.is_cjk = true;
        params.is_emoji = true;

        assert_eq!(params.raster_policy(), GlyphRasterPolicy::ExactBounds);
        assert!(!params.uses_stable_vertical_raster_frame());
    }

    #[test]
    fn shaped_glyph_and_render_params_share_policy_semantics() {
        let glyph = ShapedGlyph {
            id: GlyphId(1),
            position: point(px(0.), px(0.)),
            render_offset: Point::default(),
            font_size: px(13.),
            index: 0,
            is_emoji: false,
            is_cjk: true,
        };
        let mut params = test_render_params();
        params.is_cjk = true;

        assert_eq!(glyph.raster_policy(), params.raster_policy());
        assert_eq!(
            glyph.raster_policy(),
            GlyphRasterPolicy::StableVerticalFrame
        );
    }
}
