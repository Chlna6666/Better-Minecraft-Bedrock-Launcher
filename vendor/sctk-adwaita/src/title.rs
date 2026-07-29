use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

#[cfg(any(feature = "crossfont", feature = "ab_glyph"))]
mod config;
#[cfg(any(feature = "crossfont", feature = "ab_glyph"))]
mod font_preference;

#[cfg(feature = "crossfont")]
mod crossfont_renderer;

#[cfg(all(not(feature = "crossfont"), feature = "ab_glyph"))]
mod ab_glyph_renderer;

#[cfg(all(not(feature = "crossfont"), not(feature = "ab_glyph")))]
mod dumb;

#[derive(Debug)]
pub struct TitleText {
    #[cfg(feature = "crossfont")]
    imp: crossfont_renderer::CrossfontTitleText,
    #[cfg(all(not(feature = "crossfont"), feature = "ab_glyph"))]
    imp: ab_glyph_renderer::AbGlyphTitleText,
    #[cfg(all(not(feature = "crossfont"), not(feature = "ab_glyph")))]
    imp: dumb::DumbTitleText,
    external: Option<ExternalTitleText>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleMask {
    width: u32,
    height: u32,
    alpha: Vec<u8>,
}

impl TitleMask {
    pub fn new(width: u32, height: u32, alpha: Vec<u8>) -> Option<Self> {
        let pixel_count = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        if width == 0 || height == 0 || alpha.len() != pixel_count {
            return None;
        }

        Some(Self {
            width,
            height,
            alpha,
        })
    }
}

#[derive(Debug)]
struct ExternalTitleText {
    mask: TitleMask,
    color: Color,
    pixmap: Pixmap,
}

impl ExternalTitleText {
    fn new(mask: TitleMask, color: Color) -> Option<Self> {
        let pixmap = colorize_mask(&mask, color)?;
        Some(Self {
            mask,
            color,
            pixmap,
        })
    }

    fn update_color(&mut self, color: Color) {
        if self.color == color {
            return;
        }
        if let Some(pixmap) = colorize_mask(&self.mask, color) {
            self.color = color;
            self.pixmap = pixmap;
        }
    }
}

fn colorize_mask(mask: &TitleMask, color: Color) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(mask.width, mask.height)?;
    for (pixel, alpha) in pixmap.pixels_mut().iter_mut().zip(&mask.alpha) {
        let alpha = (color.alpha() * f32::from(*alpha) / 255.0).clamp(0.0, 1.0);
        let color = Color::from_rgba(color.red(), color.green(), color.blue(), alpha)?;
        *pixel = PremultipliedColorU8::from_rgba(
            (color.red() * color.alpha() * 255.0).round() as u8,
            (color.green() * color.alpha() * 255.0).round() as u8,
            (color.blue() * color.alpha() * 255.0).round() as u8,
            (color.alpha() * 255.0).round() as u8,
        )?;
    }
    Some(pixmap)
}

impl TitleText {
    pub fn new(color: Color) -> Option<Self> {
        #[cfg(feature = "crossfont")]
        return crossfont_renderer::CrossfontTitleText::new(color)
            .ok()
            .map(|imp| Self {
                imp,
                external: None,
            });

        #[cfg(all(not(feature = "crossfont"), feature = "ab_glyph"))]
        return Some(Self {
            imp: ab_glyph_renderer::AbGlyphTitleText::new(color),
            external: None,
        });

        #[cfg(all(not(feature = "crossfont"), not(feature = "ab_glyph")))]
        {
            let _ = color;
            return None;
        }
    }

    pub fn update_scale(&mut self, scale: u32) {
        if self.external.is_none() {
            self.imp.update_scale(scale);
        }
    }

    pub fn update_title(&mut self, title: impl Into<String>) {
        self.imp.update_title(title)
    }

    pub fn update_color(&mut self, color: Color) {
        if let Some(external) = self.external.as_mut() {
            external.update_color(color);
        } else {
            self.imp.update_color(color);
        }
    }

    pub fn set_mask(&mut self, mask: TitleMask, color: Color) -> bool {
        let Some(external) = ExternalTitleText::new(mask, color) else {
            return false;
        };
        self.external = Some(external);
        true
    }

    pub fn clear_mask(&mut self) -> bool {
        self.external.take().is_some()
    }

    pub fn pixmap(&self) -> Option<&Pixmap> {
        self.external
            .as_ref()
            .map(|external| &external.pixmap)
            .or_else(|| self.imp.pixmap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_mask_rejects_invalid_dimensions() {
        assert!(TitleMask::new(0, 1, Vec::new()).is_none());
        assert!(TitleMask::new(1, 0, Vec::new()).is_none());
        assert!(TitleMask::new(2, 2, vec![255; 3]).is_none());
    }

    #[test]
    fn external_title_mask_preserves_coverage_when_colored() {
        let mask = TitleMask::new(2, 1, vec![0, 128]).expect("valid title mask");
        let pixmap =
            colorize_mask(&mask, Color::from_rgba8(200, 100, 50, 255)).expect("valid pixmap");
        let pixels = pixmap.pixels();

        assert_eq!(pixels[0].alpha(), 0);
        assert_eq!(pixels[1].alpha(), 128);
        assert!(pixels[1].red() > pixels[1].green());
        assert!(pixels[1].green() > pixels[1].blue());
    }
}
