use super::*;
use crate::{
    Bounds, DevicePixels, GlyphRasterization, Hsla, Pixels, PlatformTextSystem, Point, Result,
    SharedString, Size, blue, font, px, red, size,
};
use std::{borrow::Cow, path::PathBuf, sync::{Arc, Mutex}};

#[derive(Default)]
struct RunRecordingTextSystem {
    shaped_runs: Mutex<Vec<Vec<FontRun>>>,
}

impl RunRecordingTextSystem {
    fn take_last_runs(&self) -> Vec<FontRun> {
        self.shaped_runs
            .lock()
            .unwrap()
            .pop()
            .expect("platform layout_line was not called")
    }
}

impl PlatformTextSystem for RunRecordingTextSystem {
    fn add_fonts(&self, _fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        Ok(())
    }

    fn add_font_paths(&self, _paths: Vec<PathBuf>) -> Result<()> {
        Ok(())
    }

    fn platform_font_family(&self) -> SharedString {
        "A".into()
    }

    fn all_font_names(&self) -> Vec<String> {
        vec!["A".to_owned(), "B".to_owned()]
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        Ok(match descriptor.family.as_ref() {
            "A" => FontId(1),
            "B" => FontId(2),
            other => panic!("unexpected font family {other}"),
        })
    }

    fn font_metrics(&self, _font_id: FontId) -> FontMetrics {
        FontMetrics {
            units_per_em: 1000,
            ascent: 800.0,
            descent: 200.0,
            line_gap: 0.0,
            underline_position: -100.0,
            underline_thickness: 50.0,
            cap_height: 700.0,
            x_height: 500.0,
            bounding_box: Bounds {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: 1000.0,
                    height: 1000.0,
                },
            },
        }
    }

    fn typographic_bounds(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Bounds<f32>> {
        Ok(Bounds {
            origin: Point { x: 0.0, y: 0.0 },
            size: size(500.0, 700.0),
        })
    }

    fn advance(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Size<f32>> {
        Ok(size(500.0, 0.0))
    }

    fn glyph_for_char(&self, _font_id: FontId, ch: char) -> Option<GlyphId> {
        Some(GlyphId(ch as u32))
    }

    fn glyph_raster_bounds(&self, _params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        Ok(Bounds::default())
    }

    fn rasterize_glyph(
        &self,
        _params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<GlyphRasterization> {
        Ok(GlyphRasterization::Bitmap {
            size: raster_bounds.size,
            bytes: Vec::new(),
        })
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.shaped_runs.lock().unwrap().push(runs.to_vec());
        LineLayout {
            font_size,
            width: px(0.0),
            ascent: px(0.0),
            descent: px(0.0),
            runs: Vec::new(),
            len: text.len(),
        }
    }
}

fn text_run(len: usize, family: &str, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font(family.to_owned()),
        color,
        background_color: None,
        background_corner_radius: None,
        background_padding: None,
        underline: None,
        strikethrough: None,
    }
}

#[test]
fn same_font_different_paint_decoration_is_one_shaping_run() {
    let platform = Arc::new(RunRecordingTextSystem::default());
    let text_system = Arc::new(TextSystem::new(platform.clone()));
    let window_text_system = WindowTextSystem::new(text_system);
    let runs = [text_run(2, "A", red()), text_run(3, "A", blue())];

    window_text_system.layout_line("hello", px(14.0), &runs, None);

    assert_eq!(
        platform.take_last_runs(),
        vec![FontRun {
            len: 5,
            font_id: FontId(1),
        }]
    );
}

#[test]
fn same_paint_decoration_different_fonts_keeps_shaping_boundary() {
    let platform = Arc::new(RunRecordingTextSystem::default());
    let text_system = Arc::new(TextSystem::new(platform.clone()));
    let window_text_system = WindowTextSystem::new(text_system);
    let runs = [text_run(2, "A", red()), text_run(3, "B", red())];

    window_text_system.layout_line("hello", px(14.0), &runs, None);

    assert_eq!(
        platform.take_last_runs(),
        vec![
            FontRun {
                len: 2,
                font_id: FontId(1),
            },
            FontRun {
                len: 3,
                font_id: FontId(2),
            },
        ]
    );
}

#[test]
fn zero_length_text_runs_do_not_reach_platform_shaper() {
    let platform = Arc::new(RunRecordingTextSystem::default());
    let text_system = Arc::new(TextSystem::new(platform.clone()));
    let window_text_system = WindowTextSystem::new(text_system);
    let runs = [
        text_run(2, "A", red()),
        text_run(0, "B", red()),
        text_run(3, "A", red()),
    ];

    window_text_system.layout_line("hello", px(14.0), &runs, None);

    assert_eq!(
        platform.take_last_runs(),
        vec![FontRun {
            len: 5,
            font_id: FontId(1),
        }]
    );
}
