use super::system::{
    FONT_RUNS_MIN_RETAINED_CAPACITY, MAX_FONT_RUNS_POOL_SIZE, MAX_WRAPPER_POOL_KEYS,
};
use super::*;
use crate::{
    Bounds, DevicePixels, GlyphRasterization, Pixels, PlatformTextSystem, Point, Result,
    SharedString, Size, px, size,
};
use std::{
    borrow::Cow,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
};

#[derive(Default)]
struct RecordingTextSystem {
    requested_fonts: StdMutex<Vec<Font>>,
}

impl PlatformTextSystem for RecordingTextSystem {
    fn add_fonts(&self, _fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        Ok(())
    }

    fn add_font_paths(&self, _paths: Vec<PathBuf>) -> Result<()> {
        Ok(())
    }

    fn platform_font_family(&self) -> SharedString {
        ".SystemUIFont".into()
    }

    fn all_font_names(&self) -> Vec<String> {
        Vec::new()
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        self.requested_fonts
            .lock()
            .unwrap()
            .push(descriptor.clone());
        Ok(FontId(1))
    }

    fn font_metrics(&self, _font_id: FontId) -> FontMetrics {
        FontMetrics {
            units_per_em: 1000,
            ascent: 1025.0,
            descent: -275.0,
            line_gap: 0.0,
            underline_position: -95.0,
            underline_thickness: 60.0,
            cap_height: 698.0,
            x_height: 516.0,
            bounding_box: Bounds {
                origin: Point {
                    x: -260.0,
                    y: -245.0,
                },
                size: Size {
                    width: 1501.0,
                    height: 1364.0,
                },
            },
        }
    }

    fn typographic_bounds(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Bounds<f32>> {
        Ok(Bounds {
            origin: Point { x: 54.0, y: 0.0 },
            size: size(392.0, 528.0),
        })
    }

    fn advance(&self, _font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        Ok(size(600.0 * glyph_id.0 as f32, 0.0))
    }

    fn glyph_for_char(&self, _font_id: FontId, ch: char) -> Option<GlyphId> {
        Some(GlyphId(ch.len_utf16() as u32))
    }

    fn glyph_raster_bounds(&self, _params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        Ok(Default::default())
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

    fn layout_line(&self, text: &str, font_size: Pixels, _runs: &[FontRun]) -> LineLayout {
        LineLayout {
            font_size,
            width: px(0.),
            ascent: px(0.),
            descent: px(0.),
            runs: Vec::new(),
            len: text.len(),
        }
    }
}

#[test]
fn system_ui_font_uses_configured_default_family_and_clears_cache() {
    let platform_text_system = Arc::new(RecordingTextSystem::default());
    let text_system = TextSystem::new(platform_text_system.clone());
    let system_font = font(".SystemUIFont");

    assert_eq!(text_system.font_id(&system_font).unwrap(), FontId(1));
    text_system.set_system_font_family("Test Sans A".into());
    assert_eq!(text_system.font_id(&system_font).unwrap(), FontId(1));
    text_system.set_system_font_family("Test Sans B".into());
    assert_eq!(text_system.font_id(&system_font).unwrap(), FontId(1));

    let requested_families = platform_text_system
        .requested_fonts
        .lock()
        .unwrap()
        .iter()
        .map(|font| font.family.clone())
        .collect::<Vec<_>>();

    let expected_families: Vec<SharedString> = vec![
        ".SystemUIFont".into(),
        "Test Sans A".into(),
        "Test Sans B".into(),
    ];
    assert_eq!(requested_families, expected_families);
}

#[test]
fn startup_font_preload_only_resolves_normal_weight() {
    let platform_text_system = Arc::new(RecordingTextSystem::default());
    let text_system = TextSystem::new(platform_text_system.clone());

    text_system.preload_font_family("Test Sans".into()).unwrap();

    let requested_fonts = platform_text_system.requested_fonts.lock().unwrap();
    assert_eq!(requested_fonts.len(), 1);
    assert_eq!(requested_fonts[0].family, SharedString::from("Test Sans"));
    assert_eq!(requested_fonts[0].weight, FontWeight::NORMAL);
}

#[test]
fn platform_font_decision_is_logged_once() {
    let text_system = TextSystem::new(Arc::new(RecordingTextSystem::default()));

    assert!(!*text_system.font_decision_logged.read());
    text_system.log_platform_default_font_once();
    assert!(*text_system.font_decision_logged.read());
    text_system.log_platform_default_font_once();
    assert!(*text_system.font_decision_logged.read());
}

#[test]
fn application_font_decision_marks_log_state() {
    let text_system = TextSystem::new(Arc::new(RecordingTextSystem::default()));

    text_system.log_application_default_font(&"Test Sans".into());

    assert!(*text_system.font_decision_logged.read());
    assert_eq!(text_system.platform_font_family(), ".SystemUIFont");
}

#[test]
fn text_pools_are_bounded() {
    let text_system = Arc::new(TextSystem::new(Arc::new(RecordingTextSystem::default())));

    for index in 0..MAX_WRAPPER_POOL_KEYS + 16 {
        let _wrapper = text_system.line_wrapper(font(".SystemUIFont"), px(index as f32 + 1.0));
    }

    assert!(text_system.wrapper_pool_len() <= MAX_WRAPPER_POOL_KEYS);

    for _ in 0..MAX_FONT_RUNS_POOL_SIZE + 16 {
        text_system.recycle_font_runs_pool(vec![FontRun {
            len: 1,
            font_id: FontId(1),
        }]);
    }

    assert!(text_system.font_runs_pool_len() <= MAX_FONT_RUNS_POOL_SIZE);
}

#[test]
fn font_runs_pool_trims_oversized_buffers() {
    let text_system = TextSystem::new(Arc::new(RecordingTextSystem::default()));
    let mut font_runs = Vec::with_capacity(FONT_RUNS_MIN_RETAINED_CAPACITY * 8);
    font_runs.push(FontRun {
        len: 1,
        font_id: FontId(1),
    });

    text_system.recycle_font_runs_pool(font_runs);

    assert_eq!(text_system.font_runs_pool_len(), 1);
    assert!(text_system.font_runs_pool_max_capacity() <= FONT_RUNS_MIN_RETAINED_CAPACITY);
}
