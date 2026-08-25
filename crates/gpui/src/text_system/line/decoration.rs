use crate::{Hsla, Pixels, StrikethroughStyle, TextBackgroundPadding, UnderlineStyle};

/// Visual decoration applied to a run of text.
#[derive(Debug, Clone)]
pub struct DecorationRun {
    /// UTF-8 byte length.
    pub len: u32,
    /// Foreground color.
    pub color: Hsla,
    /// Optional background color.
    pub background_color: Option<Hsla>,
    /// Background corner radius.
    pub background_corner_radius: Option<Pixels>,
    /// Background padding.
    pub background_padding: Option<TextBackgroundPadding>,
    /// Underline style.
    pub underline: Option<UnderlineStyle>,
    /// Strikethrough style.
    pub strikethrough: Option<StrikethroughStyle>,
}
