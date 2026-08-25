use super::{DecorationRun, background::paint_line_background, paint::paint_line};
use crate::{App, LineLayout, Pixels, Result, SharedString, TextAlign, Window};
use derive_more::{Deref, DerefMut};
use smallvec::SmallVec;
use std::sync::Arc;

/// A shaped and decorated line of text.
#[derive(Clone, Default, Debug, Deref, DerefMut)]
pub struct ShapedLine {
    #[deref]
    #[deref_mut]
    pub(crate) layout: Arc<LineLayout>,
    /// Original text.
    pub text: SharedString,
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 32]>,
}

impl ShapedLine {
    /// Return the UTF-8 byte length.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.layout.len
    }

    /// Override the rendered byte length.
    pub fn with_len(mut self, len: usize) -> Self {
        let layout = self.layout.as_ref();
        self.layout = Arc::new(LineLayout {
            font_size: layout.font_size,
            width: layout.width,
            ascent: layout.ascent,
            descent: layout.descent,
            runs: layout.runs.clone(),
            len,
        });
        self
    }

    /// Paint the line.
    pub fn paint(
        &self,
        origin: crate::Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        paint_line(
            origin,
            &self.layout,
            line_height,
            TextAlign::default(),
            None,
            &self.decoration_runs,
            &[],
            window,
            cx,
        )
    }

    /// Paint the line background.
    pub fn paint_background(
        &self,
        origin: crate::Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        paint_line_background(
            origin,
            &self.layout,
            line_height,
            TextAlign::default(),
            None,
            &self.decoration_runs,
            &[],
            window,
            cx,
        )
    }
}
