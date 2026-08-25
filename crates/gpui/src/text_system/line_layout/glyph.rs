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
    /// Whether the glyph belongs to a CJK script.
    pub is_cjk: bool,
}
