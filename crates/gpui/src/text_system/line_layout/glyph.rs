use crate::{GlyphId, Pixels, Point};

/// A shaped glyph ready to paint.
#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    /// Glyph identifier assigned by the text system.
    pub id: GlyphId,
    /// Position in the containing line.
    pub position: Point<Pixels>,
    /// Offset applied during rasterization.
    pub render_offset: Point<Pixels>,
    /// Font size selected for this glyph.
    pub font_size: Pixels,
    /// UTF-8 byte index in the original text.
    pub index: usize,
    /// Whether the glyph is an emoji.
    pub is_emoji: bool,
    /// Compatibility storage for the stable vertical raster-frame policy.
    ///
    /// New text code must use `uses_stable_vertical_raster_frame()` instead of interpreting this
    /// bit as a regional/script classification. The field remains temporarily to keep the shaping
    /// backends source-compatible while they migrate to script-aware metadata.
    pub is_cjk: bool,
}
