use crate::FontId;

use super::ShapedGlyph;

/// A shaped text run using one font.
#[derive(Debug, Clone)]
pub struct ShapedRun {
    /// Font used by the run.
    pub font_id: FontId,
    /// Shaped glyphs in the run.
    pub glyphs: Vec<ShapedGlyph>,
}
