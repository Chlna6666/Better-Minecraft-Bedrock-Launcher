//! Color classification belongs to one exact FontInfo face, not a font family or text run.
use crate::GlyphId;
use collections::HashMap;
use std::mem::ManuallyDrop;
use windows::{
    Win32::{Foundation::DWRITE_E_NOCOLOR, Graphics::DirectWrite::*},
    core::BOOL,
};

#[derive(Debug, Default)]
pub(super) struct ColorGlyphs {
    entries: HashMap<GlyphId, bool>,
    #[cfg(test)]
    pub(super) queries: usize,
}

impl ColorGlyphs {
    pub(super) fn is_color(
        &mut self,
        font_face: &IDWriteFontFace3,
        glyph_id: GlyphId,
        factory: &IDWriteFactory5,
    ) -> bool {
        if let Some(is_color) = self.entries.get(&glyph_id) {
            return *is_color;
        }
        #[cfg(test)]
        {
            self.queries += 1;
        }
        match classify(font_face, glyph_id, factory) {
            Ok(is_color) => {
                self.entries.insert(glyph_id, is_color);
                is_color
            }
            Err(error) => {
                // Do not cache transient DirectWrite failures as a permanent monochrome result.
                log::debug!("failed to classify DirectWrite color glyph: {error}");
                false
            }
        }
    }
}

fn classify(
    font_face: &IDWriteFontFace3,
    glyph_id: GlyphId,
    factory: &IDWriteFactory5,
) -> windows::core::Result<bool> {
    let glyph_run = DWRITE_GLYPH_RUN {
        // SAFETY: the call only borrows this live face; ManuallyDrop avoids releasing it.
        fontFace: ManuallyDrop::new(Some(unsafe { std::ptr::read(&****font_face) })),
        fontEmSize: 14.0,
        glyphCount: 1,
        glyphIndices: &(glyph_id.0 as u16),
        glyphAdvances: &0.0,
        glyphOffsets: &DWRITE_GLYPH_OFFSET {
            advanceOffset: 0.0,
            ascenderOffset: 0.0,
        },
        isSideways: BOOL(0),
        bidiLevel: 0,
    };
    // SAFETY: the face and the single-glyph arrays remain live until this synchronous query returns.
    match unsafe {
        factory.TranslateColorGlyphRun(
            windows_numerics::Vector2::default(),
            &glyph_run as _,
            None,
            DWRITE_GLYPH_IMAGE_FORMATS_COLR
                | DWRITE_GLYPH_IMAGE_FORMATS_SVG
                | DWRITE_GLYPH_IMAGE_FORMATS_PNG
                | DWRITE_GLYPH_IMAGE_FORMATS_JPEG
                | DWRITE_GLYPH_IMAGE_FORMATS_PREMULTIPLIED_B8G8R8A8,
            DWRITE_MEASURING_MODE_NATURAL,
            None,
            0,
        )
    } {
        Ok(_) => Ok(true),
        Err(error) if error.code() == DWRITE_E_NOCOLOR => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::super::DirectWriteTextSystem;
    use crate::{FontRun, GlyphId, PlatformTextSystem, RendererCapabilities, font, px};

    #[test]
    fn repeated_emoji_shaping_queries_each_face_glyph_once() -> anyhow::Result<()> {
        let text_system = DirectWriteTextSystem::new(RendererCapabilities::default())?;
        let font_id = text_system.font_id(&font("Segoe UI Emoji"))?;
        let text = "😀😀😀";
        let runs = [FontRun {
            font_id,
            len: text.len(),
        }];
        for font_size in [24.0, 30.0] {
            let layout = text_system.layout_line(text, px(font_size), &runs);
            assert_eq!(
                layout
                    .runs
                    .iter()
                    .map(|run| run.glyphs.len())
                    .sum::<usize>(),
                3
            );
            assert!(
                layout
                    .runs
                    .iter()
                    .flat_map(|run| &run.glyphs)
                    .all(|glyph| glyph.is_emoji)
            );
            let queries: usize = text_system
                .state
                .read()
                .fonts
                .iter()
                .map(|font| font.color_glyphs.queries)
                .sum();
            assert_eq!(
                queries, 1,
                "repeated text and size changes reuse color classification"
            );
        }
        Ok(())
    }

    #[test]
    fn monochrome_glyph_in_color_font_is_cached_too() -> anyhow::Result<()> {
        let text_system = DirectWriteTextSystem::new(RendererCapabilities::default())?;
        let font_id = text_system.font_id(&font("Segoe UI Emoji"))?;
        let mut state = text_system.state.write();
        let info = &mut state.fonts[font_id.0];
        for _ in 0..2 {
            assert!(!info.color_glyphs.is_color(
                &info.font_face,
                GlyphId(0),
                &text_system.components.factory,
            ));
        }
        assert_eq!(info.color_glyphs.queries, 1);
        Ok(())
    }
}
