use std::{cell::Cell, rc::Rc};

use crate::{App, Global, TextRenderingMode};

#[derive(Clone)]
struct GlobalTextRenderingMode(Rc<Cell<TextRenderingMode>>);

impl Default for GlobalTextRenderingMode {
    fn default() -> Self {
        Self(Rc::new(Cell::new(TextRenderingMode::default())))
    }
}

impl Global for GlobalTextRenderingMode {}

impl App {
    /// Returns the application-wide text rendering mode used by newly painted glyphs.
    pub fn text_rendering_mode(&self) -> TextRenderingMode {
        self.try_global::<GlobalTextRenderingMode>()
            .map(|state| state.0.get())
            .unwrap_or_default()
    }

    /// Changes the application-wide text rendering mode and rebuilds retained window scenes once.
    ///
    /// Existing glyph atlas entries remain reusable because the antialiasing mode is already part
    /// of each glyph raster key; only retained scene ownership needs to be refreshed.
    pub fn set_text_rendering_mode(&mut self, mode: TextRenderingMode) {
        if self.text_rendering_mode() == mode {
            return;
        }

        self.text_rendering_mode_cell().set(mode);
        self.refresh_windows();
    }

    pub(crate) fn text_rendering_mode_cell(&mut self) -> Rc<Cell<TextRenderingMode>> {
        if let Some(state) = self.try_global::<GlobalTextRenderingMode>() {
            return state.0.clone();
        }

        let state = GlobalTextRenderingMode::default();
        let mode = state.0.clone();
        self.set_global(state);
        mode
    }
}
