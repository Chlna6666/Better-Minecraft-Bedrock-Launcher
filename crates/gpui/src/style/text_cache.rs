use super::text::TextStyle;

/// Returns whether two text styles have the same shaping/layout identity.
///
/// Paint-only decorations are intentionally excluded. This predicate is the boundary used by
/// retained caches to decide whether previously measured/shaped text geometry can stay valid even
/// when the visual decoration must be repainted.
pub(crate) fn text_layout_style_eq(left: &TextStyle, right: &TextStyle) -> bool {
    left.font_family == right.font_family
        && left.font_features == right.font_features
        && left.font_fallbacks == right.font_fallbacks
        && left.font_size == right.font_size
        && left.line_height == right.line_height
        && left.font_weight == right.font_weight
        && left.font_style == right.font_style
        && left.white_space == right.white_space
        && left.text_overflow == right.text_overflow
        && left.text_align == right.text_align
        && left.line_clamp == right.line_clamp
}

/// Returns whether two text styles have the same paint/decorative identity.
///
/// These fields can change the emitted paint primitives without changing glyph shaping or text
/// measurement. A retained cache must not replay an old paint range when this predicate is false,
/// but it may still reuse layout when [`text_layout_style_eq`] is true.
pub(crate) fn text_paint_style_eq(left: &TextStyle, right: &TextStyle) -> bool {
    left.color == right.color
        && left.background_color == right.background_color
        && left.background_corner_radius == right.background_corner_radius
        && left.background_padding == right.background_padding
        && left.underline == right.underline
        && left.strikethrough == right.strikethrough
}

/// Returns whether the only semantic text-style change is the inherited foreground color.
///
/// This is intentionally stricter than "layout identity matches": backgrounds and text
/// decorations also emit paint primitives and therefore still require an ordinary repaint. Cached
/// views use this predicate only together with per-glyph provenance proving that their retained
/// scene range is safe to recolor.
pub(crate) fn text_foreground_only_change(left: &TextStyle, right: &TextStyle) -> bool {
    left.color != right.color
        && text_layout_style_eq(left, right)
        && left.background_color == right.background_color
        && left.background_corner_radius == right.background_corner_radius
        && left.background_padding == right.background_padding
        && left.underline == right.underline
        && left.strikethrough == right.strikethrough
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TextStyle, blue, px, red};

    #[test]
    fn foreground_color_is_paint_only() {
        let left = TextStyle::default();
        let mut right = left.clone();
        right.color = red();

        assert!(text_layout_style_eq(&left, &right));
        assert!(!text_paint_style_eq(&left, &right));
        assert!(text_foreground_only_change(&left, &right));
    }

    #[test]
    fn font_size_changes_layout_identity() {
        let left = TextStyle::default();
        let mut right = left.clone();
        right.font_size = px(18.0).into();

        assert!(!text_layout_style_eq(&left, &right));
        assert!(text_paint_style_eq(&left, &right));
        assert!(!text_foreground_only_change(&left, &right));
    }

    #[test]
    fn background_decoration_is_paint_only() {
        let left = TextStyle::default();
        let mut right = left.clone();
        right.background_color = Some(blue());

        assert!(text_layout_style_eq(&left, &right));
        assert!(!text_paint_style_eq(&left, &right));
        assert!(!text_foreground_only_change(&left, &right));
    }

    #[test]
    fn both_identities_equal_implies_full_text_style_equality() {
        let left = TextStyle::default();
        let right = left.clone();

        assert!(text_layout_style_eq(&left, &right));
        assert!(text_paint_style_eq(&left, &right));
        assert!(!text_foreground_only_change(&left, &right));
        assert_eq!(left, right);
    }
}
