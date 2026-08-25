use super::{DecorationRun, background::paint_line_background, paint::paint_line};
use crate::{App, Bounds, Pixels, Result, SharedString, TextAlign, Window, WrappedLineLayout};
use derive_more::{Deref, DerefMut};
use smallvec::SmallVec;
use std::sync::Arc;

/// A shaped, decorated, and wrapped line of text.
#[derive(Clone, Default, Debug, Deref, DerefMut)]
pub struct WrappedLine {
    #[deref]
    #[deref_mut]
    pub(crate) layout: Arc<WrappedLineLayout>,
    /// Original text.
    pub text: SharedString,
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 32]>,
}

impl WrappedLine {
    /// Return the unwrapped UTF-8 byte length.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.layout.len()
    }

    /// Paint the wrapped line.
    pub fn paint(
        &self,
        origin: crate::Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        bounds: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        let width = bounds.map_or(self.layout.wrap_width, |bounds| Some(bounds.size.width));
        paint_line(
            origin,
            &self.layout.unwrapped_layout,
            line_height,
            align,
            width,
            &self.decoration_runs,
            &self.wrap_boundaries,
            window,
            cx,
        )
    }

    /// Paint the wrapped line background.
    pub fn paint_background(
        &self,
        origin: crate::Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        bounds: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        let width = bounds.map_or(self.layout.wrap_width, |bounds| Some(bounds.size.width));
        paint_line_background(
            origin,
            &self.layout.unwrapped_layout,
            line_height,
            align,
            width,
            &self.decoration_runs,
            &self.wrap_boundaries,
            window,
            cx,
        )
    }
}
