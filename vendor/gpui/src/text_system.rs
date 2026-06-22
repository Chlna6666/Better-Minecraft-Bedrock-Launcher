mod font_fallbacks;
mod font_features;
mod line;
mod line_layout;
mod line_wrapper;

pub use font_fallbacks::*;
pub use font_features::*;
pub use line::*;
pub use line_layout::*;
pub use line_wrapper::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    BackgroundExecutor, Bounds, DevicePixels, GlyphRasterization, GpuiMemoryTrimLevel, Hsla,
    Pixels, PlatformTextSystem, Point, Result, SharedString, Size, StrikethroughStyle,
    TextRenderingMode, UnderlineStyle, px, record_text_background_warmup,
};
use anyhow::{Context as _, anyhow};
use collections::FxHashMap;
use core::fmt;
use derive_more::{Add, Deref, FromStr, Sub};
use itertools::Itertools;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use smallvec::{SmallVec, smallvec};
use std::{
    borrow::Cow,
    cmp,
    collections::VecDeque,
    fmt::{Debug, Display, Formatter},
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut, Range},
    path::PathBuf,
    sync::Arc,
};

/// An opaque identifier for a specific font.
#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
#[repr(C)]
pub struct FontId(pub usize);

/// An opaque identifier for a specific font family.
#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
pub struct FontFamilyId(pub usize);

pub(crate) const SUBPIXEL_VARIANTS_X: u8 = 4;

pub(crate) const SUBPIXEL_VARIANTS_Y: u8 =
    if cfg!(target_os = "windows") || cfg!(target_os = "linux") {
        1
    } else {
        SUBPIXEL_VARIANTS_X
    };

const MAX_WRAPPER_POOL_KEYS: usize = 128;
const MAX_WRAPPERS_PER_KEY: usize = 4;
const MAX_FONT_RUNS_POOL_SIZE: usize = 128;
const BACKGROUND_WARM_UP_FONT_SIZE: Pixels = px(16.);
const BACKGROUND_WARM_UP_SAMPLE_TEXT: &[&str] = &[
    "abcdefghijklmnopqrstuvwxyz",
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "0123456789 .,;:/\\-_()[]{}",
    "Hello GPUI text layout",
    "图形界面文字布局",
    "🙂",
];
const BACKGROUND_WARM_UP_ADVANCE_CHARS: &[char] =
    &['m', '0', ' ', 'a', 'A', '1', '.', '/', '-', '_', '图', '🙂'];

/// The GPUI text rendering sub system.
pub struct TextSystem {
    platform_text_system: Arc<dyn PlatformTextSystem>,
    background_line_layout_cache: Arc<BackgroundLineLayoutCache>,
    system_font_family: RwLock<Option<SharedString>>,
    font_decision_logged: RwLock<bool>,
    font_ids_by_font: RwLock<FxHashMap<Font, Result<FontId>>>,
    font_metrics: RwLock<FxHashMap<FontId, FontMetrics>>,
    raster_bounds: RwLock<FxHashMap<RenderGlyphParams, Bounds<DevicePixels>>>,
    wrapper_pool: Mutex<FxHashMap<FontIdWithSize, VecDeque<LineWrapper>>>,
    font_runs_pool: Mutex<VecDeque<Vec<FontRun>>>,
    fallback_font_stack: SmallVec<[Font; 2]>,
}

impl TextSystem {
    pub(crate) fn new(platform_text_system: Arc<dyn PlatformTextSystem>) -> Self {
        TextSystem {
            platform_text_system,
            background_line_layout_cache: Arc::new(BackgroundLineLayoutCache::default()),
            system_font_family: RwLock::default(),
            font_decision_logged: RwLock::new(false),
            font_metrics: RwLock::default(),
            raster_bounds: RwLock::default(),
            font_ids_by_font: RwLock::default(),
            wrapper_pool: Mutex::default(),
            font_runs_pool: Mutex::default(),
            fallback_font_stack: smallvec![
                // TODO: Remove this when Linux have implemented setting fallbacks.
                font(".ZedMono"),
                font(".ZedSans"),
                font("Helvetica"),
                font("Segoe UI"),     // Windows
                font("Ubuntu"),       // Gnome (Ubuntu)
                font("Adwaita Sans"), // Gnome 47
                font("Cantarell"),    // Gnome
                font("Noto Sans"),    // KDE
                font("DejaVu Sans"),
                font("Arial"), // macOS, Windows
            ],
        }
    }

    /// Get a list of all available font names from the operating system.
    pub fn all_font_names(&self) -> Vec<String> {
        let mut names = self.platform_text_system.all_font_names();
        names.extend(
            self.fallback_font_stack
                .iter()
                .map(|font| font.family.to_string()),
        );
        names.push(".SystemUIFont".to_string());
        names.sort();
        names.dedup();
        names
    }

    /// Add a font's data to the text system.
    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.platform_text_system.add_fonts(fonts)?;
        self.clear_caches();
        Ok(())
    }

    /// Add font files to the text system by path.
    pub fn add_font_paths(&self, paths: Vec<PathBuf>) -> Result<()> {
        self.platform_text_system.add_font_paths(paths)?;
        self.clear_caches();
        Ok(())
    }

    pub(crate) fn platform_font_family(&self) -> SharedString {
        self.platform_text_system.platform_font_family()
    }

    pub(crate) fn warm_up_background(&self) {
        self.platform_text_system.warm_up_background();
        let warmed_layouts = self.warm_up_text_layout_data();
        record_text_background_warmup(warmed_layouts);
    }

    fn warm_up_text_layout_data(&self) -> usize {
        let mut warmed_layouts = 0usize;
        let mut fonts = Vec::with_capacity(self.fallback_font_stack.len() + 1);
        fonts.push(font(".SystemUIFont"));
        fonts.extend(self.fallback_font_stack.iter().cloned());

        for font in fonts {
            let Ok(font_id) = self.font_id(&font) else {
                continue;
            };

            self.read_metrics(font_id, |_| ());
            for ch in BACKGROUND_WARM_UP_ADVANCE_CHARS {
                let _ = self.advance(font_id, BACKGROUND_WARM_UP_FONT_SIZE, *ch);
            }

            for sample in BACKGROUND_WARM_UP_SAMPLE_TEXT {
                let layout = self.platform_text_system.layout_line(
                    sample,
                    BACKGROUND_WARM_UP_FONT_SIZE,
                    &[FontRun {
                        len: sample.len(),
                        font_id,
                    }],
                );
                if layout.len == sample.len() {
                    self.background_line_layout_cache.insert_line_layout(
                        SharedString::from(*sample),
                        BACKGROUND_WARM_UP_FONT_SIZE,
                        &[FontRun {
                            len: sample.len(),
                            font_id,
                        }],
                        None,
                        Arc::new(layout),
                    );
                    warmed_layouts = warmed_layouts.saturating_add(1);
                }
            }
        }

        warmed_layouts
    }

    pub(crate) fn log_platform_default_font_once(&self) {
        let mut logged = self.font_decision_logged.write();
        if *logged {
            return;
        }

        log::info!(
            "GPUI text font: using platform default \"{}\"",
            self.platform_font_family()
        );
        *logged = true;
    }

    pub(crate) fn log_application_default_font(&self, family: &SharedString) {
        log::info!("GPUI text font: using application default \"{}\"", family);
        *self.font_decision_logged.write() = true;
    }

    pub(crate) fn log_application_default_font_fallback(
        &self,
        family: &SharedString,
        error: &anyhow::Error,
    ) {
        log::warn!(
            "GPUI text font: failed to use application default \"{}\" ({}); falling back to platform default \"{}\"",
            family,
            error,
            self.platform_font_family()
        );
        *self.font_decision_logged.write() = true;
    }

    fn font_with_default_fallbacks(&self, font: &Font) -> Option<Font> {
        if font.fallbacks.is_some() {
            return None;
        }

        let fallback_families = self
            .fallback_font_stack
            .iter()
            .filter(|fallback| fallback.family != font.family)
            .map(|fallback| fallback.family.to_string())
            .collect::<Vec<_>>();
        if fallback_families.is_empty() {
            None
        } else {
            Some(Font {
                fallbacks: Some(FontFallbacks::from_fonts(fallback_families)),
                ..font.clone()
            })
        }
    }

    pub(crate) fn set_system_font_family(&self, family: SharedString) {
        let mut system_font_family = self.system_font_family.write();
        if system_font_family.as_ref() == Some(&family) {
            return;
        }

        self.platform_text_system
            .set_application_font_family(family.clone());
        *system_font_family = Some(family);
        drop(system_font_family);
        self.clear_caches();
    }

    pub(crate) fn preload_font_family(&self, family: SharedString) -> Result<()> {
        let mut font = font(family);
        font.weight = FontWeight::NORMAL;
        self.font_id(&font)?;
        Ok(())
    }

    pub(crate) fn clear_caches(&self) {
        self.font_ids_by_font.write().clear();
        self.font_metrics.write().clear();
        self.raster_bounds.write().clear();
        self.background_line_layout_cache.clear();
        self.wrapper_pool.lock().clear();
        self.font_runs_pool.lock().clear();
    }

    pub(crate) fn trim_global_caches(&self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light => {
                self.font_ids_by_font.write().shrink_to_fit();
                self.font_metrics.write().shrink_to_fit();
                self.raster_bounds.write().shrink_to_fit();
                self.wrapper_pool.lock().shrink_to_fit();
                self.font_runs_pool.lock().shrink_to_fit();
            }
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive => {
                self.clear_caches();
                self.font_ids_by_font.write().shrink_to_fit();
                self.font_metrics.write().shrink_to_fit();
                self.raster_bounds.write().shrink_to_fit();
                self.wrapper_pool.lock().shrink_to_fit();
                self.font_runs_pool.lock().shrink_to_fit();
            }
        }
    }

    /// Get the FontId for the configure font family and style.
    fn font_id(&self, font: &Font) -> Result<FontId> {
        let resolved_font;
        let font = if font.family == ".SystemUIFont" {
            if let Some(system_font_family) = self.system_font_family.read().as_ref().cloned() {
                resolved_font = Font {
                    family: system_font_family,
                    ..font.clone()
                };
                &resolved_font
            } else {
                font
            }
        } else {
            font
        };
        let default_fallback_font;
        let font = if let Some(font_with_fallbacks) = self.font_with_default_fallbacks(font) {
            default_fallback_font = font_with_fallbacks;
            &default_fallback_font
        } else {
            font
        };

        fn clone_font_id_result(font_id: &Result<FontId>) -> Result<FontId> {
            match font_id {
                Ok(font_id) => Ok(*font_id),
                Err(err) => Err(anyhow!("{err}")),
            }
        }

        let font_id = self
            .font_ids_by_font
            .read()
            .get(font)
            .map(clone_font_id_result);
        if let Some(font_id) = font_id {
            font_id
        } else {
            let font_id = self.platform_text_system.font_id(font);
            self.font_ids_by_font
                .write()
                .insert(font.clone(), clone_font_id_result(&font_id));
            font_id
        }
    }

    /// Get the Font for the Font Id.
    pub fn get_font_for_id(&self, id: FontId) -> Option<Font> {
        let lock = self.font_ids_by_font.read();
        lock.iter()
            .filter_map(|(font, result)| match result {
                Ok(font_id) if *font_id == id => Some(font.clone()),
                _ => None,
            })
            .next()
    }

    /// Resolves the specified font, falling back to the default font stack if
    /// the font fails to load.
    ///
    /// # Panics
    ///
    /// Panics if the font and none of the fallbacks can be resolved.
    pub fn resolve_font(&self, font: &Font) -> FontId {
        if let Ok(font_id) = self.font_id(font) {
            return font_id;
        }
        for fallback in &self.fallback_font_stack {
            if let Ok(font_id) = self.font_id(fallback) {
                return font_id;
            }
        }

        panic!(
            "failed to resolve font '{}' or any of the fallbacks: {}",
            font.family,
            self.fallback_font_stack
                .iter()
                .map(|fallback| &fallback.family)
                .join(", ")
        );
    }

    /// Get the bounding box for the given font and font size.
    /// A font's bounding box is the smallest rectangle that could enclose all glyphs
    /// in the font. superimposed over one another.
    pub fn bounding_box(&self, font_id: FontId, font_size: Pixels) -> Bounds<Pixels> {
        self.read_metrics(font_id, |metrics| metrics.bounding_box(font_size))
    }

    /// Get the typographic bounds for the given character, in the given font and size.
    pub fn typographic_bounds(
        &self,
        font_id: FontId,
        font_size: Pixels,
        character: char,
    ) -> Result<Bounds<Pixels>> {
        let glyph_id = self
            .platform_text_system
            .glyph_for_char(font_id, character)
            .with_context(|| format!("glyph not found for character '{character}'"))?;
        let bounds = self
            .platform_text_system
            .typographic_bounds(font_id, glyph_id)?;
        Ok(self.read_metrics(font_id, |metrics| {
            (bounds / metrics.units_per_em as f32 * font_size.0).map(px)
        }))
    }

    /// Get the advance width for the given character, in the given font and size.
    pub fn advance(&self, font_id: FontId, font_size: Pixels, ch: char) -> Result<Size<Pixels>> {
        let glyph_id = self
            .platform_text_system
            .glyph_for_char(font_id, ch)
            .with_context(|| format!("glyph not found for character '{ch}'"))?;
        let result = self.platform_text_system.advance(font_id, glyph_id)?
            / self.units_per_em(font_id) as f32;

        Ok(result * font_size)
    }

    /// Returns the width of an `em`.
    ///
    /// Uses the width of the `m` character in the given font and size.
    pub fn em_width(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        Ok(self.typographic_bounds(font_id, font_size, 'm')?.size.width)
    }

    /// Returns the advance width of an `em`.
    ///
    /// Uses the advance width of the `m` character in the given font and size.
    pub fn em_advance(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        Ok(self.advance(font_id, font_size, 'm')?.width)
    }

    /// Returns the width of an `ch`.
    ///
    /// Uses the width of the `0` character in the given font and size.
    pub fn ch_width(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        Ok(self.typographic_bounds(font_id, font_size, '0')?.size.width)
    }

    /// Returns the advance width of an `ch`.
    ///
    /// Uses the advance width of the `0` character in the given font and size.
    pub fn ch_advance(&self, font_id: FontId, font_size: Pixels) -> Result<Pixels> {
        Ok(self.advance(font_id, font_size, '0')?.width)
    }

    /// Get the number of font size units per 'em square',
    /// Per MDN: "an abstract square whose height is the intended distance between
    /// lines of type in the same type size"
    pub fn units_per_em(&self, font_id: FontId) -> u32 {
        self.read_metrics(font_id, |metrics| metrics.units_per_em)
    }

    /// Get the height of a capital letter in the given font and size.
    pub fn cap_height(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.cap_height(font_size))
    }

    /// Get the height of the x character in the given font and size.
    pub fn x_height(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.x_height(font_size))
    }

    /// Get the recommended distance from the baseline for the given font
    pub fn ascent(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.ascent(font_size))
    }

    /// Get the recommended distance below the baseline for the given font,
    /// in single spaced text.
    pub fn descent(&self, font_id: FontId, font_size: Pixels) -> Pixels {
        self.read_metrics(font_id, |metrics| metrics.descent(font_size))
    }

    /// Get the recommended baseline offset for the given font and line height.
    pub fn baseline_offset(
        &self,
        font_id: FontId,
        font_size: Pixels,
        line_height: Pixels,
    ) -> Pixels {
        let ascent = self.ascent(font_id, font_size);
        let descent = self.descent(font_id, font_size);
        let padding_top = (line_height - ascent - descent) / 2.;
        padding_top + ascent
    }

    fn read_metrics<T>(&self, font_id: FontId, read: impl FnOnce(&FontMetrics) -> T) -> T {
        let lock = self.font_metrics.upgradable_read();

        if let Some(metrics) = lock.get(&font_id) {
            read(metrics)
        } else {
            let mut lock = RwLockUpgradableReadGuard::upgrade(lock);
            let metrics = lock
                .entry(font_id)
                .or_insert_with(|| self.platform_text_system.font_metrics(font_id));
            read(metrics)
        }
    }

    /// Returns a handle to a line wrapper, for the given font and font size.
    pub fn line_wrapper(self: &Arc<Self>, font: Font, font_size: Pixels) -> LineWrapperHandle {
        let lock = &mut self.wrapper_pool.lock();
        let font_id = self.resolve_font(&font);
        let key = FontIdWithSize { font_id, font_size };
        let wrapper = lock
            .get_mut(&key)
            .and_then(VecDeque::pop_back)
            .unwrap_or_else(|| {
                LineWrapper::new(font_id, font_size, self.platform_text_system.clone())
            });

        LineWrapperHandle {
            wrapper: Some(wrapper),
            text_system: self.clone(),
        }
    }

    /// Get the rasterized size and location of a specific, rendered glyph.
    pub(crate) fn raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        let raster_bounds = self.raster_bounds.upgradable_read();
        if let Some(bounds) = raster_bounds.get(params) {
            Ok(*bounds)
        } else {
            let mut raster_bounds = RwLockUpgradableReadGuard::upgrade(raster_bounds);
            let bounds = self.platform_text_system.glyph_raster_bounds(params)?;
            raster_bounds.insert(params.clone(), bounds);
            Ok(bounds)
        }
    }

    pub(crate) fn rasterize_glyph(&self, params: &RenderGlyphParams) -> Result<GlyphRasterization> {
        let raster_bounds = self.raster_bounds(params)?;
        self.platform_text_system
            .rasterize_glyph(params, raster_bounds)
    }

    pub(crate) fn recommended_rendering_mode(
        &self,
        font_id: FontId,
        font_size: Pixels,
    ) -> TextRenderingMode {
        self.platform_text_system
            .recommended_rendering_mode(font_id, font_size)
    }

    pub(crate) fn glyph_dilation_for_color(&self, color: Hsla) -> u8 {
        self.platform_text_system.glyph_dilation_for_color(color)
    }

    fn recycle_font_runs_pool(&self, font_runs: Vec<FontRun>) {
        let mut pool = self.font_runs_pool.lock();
        if pool.len() >= MAX_FONT_RUNS_POOL_SIZE {
            pool.pop_front();
        }
        pool.push_back(font_runs);
    }
}

/// The GPUI text layout subsystem.
#[derive(Deref)]
pub struct WindowTextSystem {
    line_layout_cache: LineLayoutCache,
    #[deref]
    text_system: Arc<TextSystem>,
}

impl WindowTextSystem {
    pub(crate) fn new(
        text_system: Arc<TextSystem>,
        background_executor: Option<BackgroundExecutor>,
    ) -> Self {
        Self {
            line_layout_cache: LineLayoutCache::with_background_cache(
                text_system.platform_text_system.clone(),
                text_system.background_line_layout_cache.clone(),
                background_executor,
            ),
            text_system,
        }
    }

    pub(crate) fn layout_index(&self) -> LineLayoutIndex {
        self.line_layout_cache.layout_index()
    }

    pub(crate) fn reuse_layouts(&self, index: Range<LineLayoutIndex>) {
        self.line_layout_cache.reuse_layouts(index)
    }

    pub(crate) fn truncate_layouts(&self, index: LineLayoutIndex) {
        self.line_layout_cache.truncate_layouts(index)
    }

    pub(crate) fn clear_layout_cache(&self) {
        self.line_layout_cache.clear()
    }

    pub(crate) fn clear_raster_cache(&self) {
        self.raster_bounds.write().clear();
    }

    pub(crate) fn trim_window_caches(&self, level: GpuiMemoryTrimLevel) {
        self.line_layout_cache
            .trim_retained_capacity_for_level(level);
    }

    /// Shape the given line, at the given font_size, for painting to the screen.
    /// Subsets of the line can be styled independently with the `runs` parameter.
    ///
    /// Note that this method can only shape a single line of text. It will panic
    /// if the text contains newlines. If you need to shape multiple lines of text,
    /// use [`Self::shape_text`] instead.
    pub fn shape_line(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        force_width: Option<Pixels>,
    ) -> ShapedLine {
        debug_assert!(
            text.find('\n').is_none(),
            "text argument should not contain newlines"
        );

        let mut decoration_runs = SmallVec::<[DecorationRun; 32]>::new();
        for run in runs {
            if let Some(last_run) = decoration_runs.last_mut()
                && last_run.color == run.color
                && last_run.underline == run.underline
                && last_run.strikethrough == run.strikethrough
                && last_run.background_color == run.background_color
                && last_run.background_corner_radius == run.background_corner_radius
                && last_run.background_padding == run.background_padding
            {
                last_run.len += run.len as u32;
                continue;
            }
            decoration_runs.push(DecorationRun {
                len: run.len as u32,
                color: run.color,
                background_color: run.background_color,
                background_corner_radius: run.background_corner_radius,
                background_padding: run.background_padding,
                underline: run.underline,
                strikethrough: run.strikethrough,
            });
        }

        let layout = self.layout_line(&text, font_size, runs, force_width);

        ShapedLine {
            layout,
            text,
            decoration_runs,
        }
    }

    /// Shape a multi line string of text, at the given font_size, for painting to the screen.
    /// Subsets of the text can be styled independently with the `runs` parameter.
    /// If `wrap_width` is provided, the line breaks will be adjusted to fit within the given width.
    pub fn shape_text(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
    ) -> Result<SmallVec<[WrappedLine; 1]>> {
        let mut runs = runs.iter().filter(|run| run.len > 0).cloned().peekable();
        let mut font_runs = self.font_runs_pool.lock().pop_back().unwrap_or_default();

        let mut lines = SmallVec::new();
        let mut line_start = 0;
        let mut max_wrap_lines = line_clamp.unwrap_or(usize::MAX);
        let mut wrapped_lines = 0;

        let mut process_line = |line_text: SharedString| {
            font_runs.clear();
            let line_end = line_start + line_text.len();

            let mut decoration_runs = SmallVec::<[DecorationRun; 32]>::new();
            let mut run_start = line_start;
            while run_start < line_end {
                let Some(run) = runs.peek_mut() else {
                    break;
                };

                let run_len_within_line = cmp::min(line_end, run_start + run.len) - run_start;

                let decoration_changed = if let Some(last_run) = decoration_runs.last_mut()
                    && last_run.color == run.color
                    && last_run.underline == run.underline
                    && last_run.strikethrough == run.strikethrough
                    && last_run.background_color == run.background_color
                    && last_run.background_corner_radius == run.background_corner_radius
                    && last_run.background_padding == run.background_padding
                {
                    last_run.len += run_len_within_line as u32;
                    false
                } else {
                    decoration_runs.push(DecorationRun {
                        len: run_len_within_line as u32,
                        color: run.color,
                        background_color: run.background_color,
                        background_corner_radius: run.background_corner_radius,
                        background_padding: run.background_padding,
                        underline: run.underline,
                        strikethrough: run.strikethrough,
                    });
                    true
                };

                let font_id = self.resolve_font(&run.font);
                if let Some(font_run) = font_runs.last_mut()
                    && font_id == font_run.font_id
                    && !decoration_changed
                {
                    font_run.len += run_len_within_line;
                } else {
                    font_runs.push(FontRun {
                        len: run_len_within_line,
                        font_id,
                    });
                }

                if run_len_within_line == run.len {
                    runs.next();
                } else {
                    // Preserve the remainder of the run for the next line
                    run.len -= run_len_within_line;
                }
                run_start += run_len_within_line;
            }

            let layout = self.line_layout_cache.layout_wrapped_line(
                &line_text,
                font_size,
                &font_runs,
                wrap_width,
                Some(max_wrap_lines - wrapped_lines),
            );
            wrapped_lines += layout.wrap_boundaries.len();

            lines.push(WrappedLine {
                layout,
                decoration_runs,
                text: line_text,
            });

            // Skip `\n` character.
            line_start = line_end + 1;
            if let Some(run) = runs.peek_mut() {
                run.len -= 1;
                if run.len == 0 {
                    runs.next();
                }
            }
        };

        let mut split_lines = text.split('\n');
        let mut processed = false;

        if let Some(first_line) = split_lines.next()
            && let Some(second_line) = split_lines.next()
        {
            processed = true;
            process_line(SharedString::new(first_line));
            process_line(SharedString::new(second_line));
            for line_text in split_lines {
                process_line(SharedString::new(line_text));
            }
        }

        if !processed {
            process_line(text);
        }

        self.text_system.recycle_font_runs_pool(font_runs);

        Ok(lines)
    }

    pub(crate) fn finish_frame(&self) {
        self.line_layout_cache.finish_frame()
    }

    /// Layout the given line of text, at the given font_size.
    /// Subsets of the line can be styled independently with the `runs` parameter.
    /// Generally, you should prefer to use [`Self::shape_line`] instead, which
    /// can be painted directly.
    pub fn layout_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
        force_width: Option<Pixels>,
    ) -> Arc<LineLayout> {
        let mut last_run = None::<&TextRun>;
        let mut last_font: Option<FontId> = None;
        let mut font_runs = self.font_runs_pool.lock().pop_back().unwrap_or_default();
        font_runs.clear();

        for run in runs.iter() {
            let decoration_changed = if let Some(last_run) = last_run
                && last_run.color == run.color
                && last_run.underline == run.underline
                && last_run.strikethrough == run.strikethrough
            // we do not consider differing background color relevant, as it does not affect glyphs
            // && last_run.background_color == run.background_color
            {
                false
            } else {
                last_run = Some(run);
                true
            };

            if let Some(font_run) = font_runs.last_mut()
                && Some(font_run.font_id) == last_font
                && !decoration_changed
            {
                font_run.len += run.len;
            } else {
                let font_id = self.resolve_font(&run.font);
                last_font = Some(font_id);
                font_runs.push(FontRun {
                    len: run.len,
                    font_id,
                });
            }
        }

        let layout = self.line_layout_cache.layout_line(
            &SharedString::new(text),
            font_size,
            &font_runs,
            force_width,
        );

        self.text_system.recycle_font_runs_pool(font_runs);

        layout
    }
}

#[derive(Hash, Eq, PartialEq)]
struct FontIdWithSize {
    font_id: FontId,
    font_size: Pixels,
}

/// A handle into the text system, which can be used to compute the wrapped layout of text
pub struct LineWrapperHandle {
    wrapper: Option<LineWrapper>,
    text_system: Arc<TextSystem>,
}

impl Drop for LineWrapperHandle {
    fn drop(&mut self) {
        let mut state = self.text_system.wrapper_pool.lock();
        let wrapper = self.wrapper.take().unwrap();
        let key = FontIdWithSize {
            font_id: wrapper.font_id,
            font_size: wrapper.font_size,
        };
        if let Some(wrappers) = state.get_mut(&key) {
            if wrappers.len() >= MAX_WRAPPERS_PER_KEY {
                wrappers.pop_front();
            }
            wrappers.push_back(wrapper);
        } else if state.len() < MAX_WRAPPER_POOL_KEYS {
            let mut wrappers = VecDeque::new();
            wrappers.push_back(wrapper);
            state.insert(key, wrappers);
        }
    }
}

impl Deref for LineWrapperHandle {
    type Target = LineWrapper;

    fn deref(&self) -> &Self::Target {
        self.wrapper.as_ref().unwrap()
    }
}

impl DerefMut for LineWrapperHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.wrapper.as_mut().unwrap()
    }
}

/// The degree of blackness or stroke thickness of a font. This value ranges from 100.0 to 900.0,
/// with 400.0 as normal.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize, Add, Sub, FromStr)]
#[serde(transparent)]
pub struct FontWeight(pub f32);

impl Display for FontWeight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<f32> for FontWeight {
    fn from(weight: f32) -> Self {
        FontWeight(weight)
    }
}

impl Default for FontWeight {
    #[inline]
    fn default() -> FontWeight {
        FontWeight::NORMAL
    }
}

impl Hash for FontWeight {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(u32::from_be_bytes(self.0.to_be_bytes()));
    }
}

impl Eq for FontWeight {}

impl FontWeight {
    /// Thin weight (100), the thinnest value.
    pub const THIN: FontWeight = FontWeight(100.0);
    /// Extra light weight (200).
    pub const EXTRA_LIGHT: FontWeight = FontWeight(200.0);
    /// Light weight (300).
    pub const LIGHT: FontWeight = FontWeight(300.0);
    /// Normal (400).
    pub const NORMAL: FontWeight = FontWeight(400.0);
    /// Medium weight (500, higher than normal).
    pub const MEDIUM: FontWeight = FontWeight(500.0);
    /// Semibold weight (600).
    pub const SEMIBOLD: FontWeight = FontWeight(600.0);
    /// Bold weight (700).
    pub const BOLD: FontWeight = FontWeight(700.0);
    /// Extra-bold weight (800).
    pub const EXTRA_BOLD: FontWeight = FontWeight(800.0);
    /// Black weight (900), the thickest value.
    pub const BLACK: FontWeight = FontWeight(900.0);

    /// All of the font weights, in order from thinnest to thickest.
    pub const ALL: [FontWeight; 9] = [
        Self::THIN,
        Self::EXTRA_LIGHT,
        Self::LIGHT,
        Self::NORMAL,
        Self::MEDIUM,
        Self::SEMIBOLD,
        Self::BOLD,
        Self::EXTRA_BOLD,
        Self::BLACK,
    ];
}

impl schemars::JsonSchema for FontWeight {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FontWeight".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        use schemars::json_schema;
        json_schema!({
            "type": "number",
            "minimum": Self::THIN,
            "maximum": Self::BLACK,
            "default": Self::default(),
            "description": "Font weight value between 100 (thin) and 900 (black)"
        })
    }
}

/// Allows italic or oblique faces to be selected.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize, JsonSchema)]
pub enum FontStyle {
    /// A face that is neither italic not obliqued.
    #[default]
    Normal,
    /// A form that is generally cursive in nature.
    Italic,
    /// A typically-sloped version of the regular face.
    Oblique,
}

impl Display for FontStyle {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

/// A styled run of text, for use in [`crate::TextLayout`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRun {
    /// A number of utf8 bytes
    pub len: usize,
    /// The font to use for this run.
    pub font: Font,
    /// The color
    pub color: Hsla,
    /// The background color (if any)
    pub background_color: Option<Hsla>,
    /// The corner radius for the background (if any)
    pub background_corner_radius: Option<Pixels>,
    /// The padding for the background (if any)
    pub background_padding: Option<TextBackgroundPadding>,
    /// The underline style (if any)
    pub underline: Option<UnderlineStyle>,
    /// The strikethrough style (if any)
    pub strikethrough: Option<StrikethroughStyle>,
}

#[cfg(all(target_os = "macos", test))]
impl TextRun {
    fn with_len(&self, len: usize) -> Self {
        let mut this = self.clone();
        this.len = len;
        this
    }
}

/// Padding applied around a text run's painted background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextBackgroundPadding {
    /// Padding above the text background.
    pub top: Pixels,
    /// Padding to the right of the text background.
    pub right: Pixels,
    /// Padding below the text background.
    pub bottom: Pixels,
    /// Padding to the left of the text background.
    pub left: Pixels,
}

/// An identifier for a specific glyph, as returned by [`WindowTextSystem::layout_line`].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct GlyphId(pub(crate) u32);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderGlyphParams {
    pub(crate) font_id: FontId,
    pub(crate) glyph_id: GlyphId,
    pub(crate) font_size: Pixels,
    pub(crate) subpixel_variant: Point<u8>,
    pub(crate) scale_factor: f32,
    pub(crate) is_emoji: bool,
    pub(crate) is_cjk: bool,
    pub(crate) subpixel_rendering: bool,
    pub(crate) dilation: u8,
}

impl Eq for RenderGlyphParams {}

impl Hash for RenderGlyphParams {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.font_id.0.hash(state);
        self.glyph_id.0.hash(state);
        self.font_size.0.to_bits().hash(state);
        self.subpixel_variant.hash(state);
        self.scale_factor.to_bits().hash(state);
        self.is_emoji.hash(state);
        self.is_cjk.hash(state);
        self.subpixel_rendering.hash(state);
        self.dilation.hash(state);
    }
}

/// The configuration details for identifying a specific font.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Font {
    /// The font family name.
    ///
    /// The special name ".SystemUIFont" is used to identify the system UI font, which varies based on platform.
    pub family: SharedString,

    /// The font features to use.
    pub features: FontFeatures,

    /// The fallbacks fonts to use.
    pub fallbacks: Option<FontFallbacks>,

    /// The font weight.
    pub weight: FontWeight,

    /// The font style.
    pub style: FontStyle,
}

/// Get a [`Font`] for a given name.
pub fn font(family: impl Into<SharedString>) -> Font {
    Font {
        family: family.into(),
        features: FontFeatures::default(),
        weight: FontWeight::default(),
        style: FontStyle::default(),
        fallbacks: None,
    }
}

impl Font {
    /// Set this Font to be bold
    pub fn bold(mut self) -> Self {
        self.weight = FontWeight::BOLD;
        self
    }

    /// Set this Font to be italic
    pub fn italic(mut self) -> Self {
        self.style = FontStyle::Italic;
        self
    }
}

/// A struct for storing font metrics.
/// It is used to define the measurements of a typeface.
#[derive(Clone, Copy, Debug)]
pub struct FontMetrics {
    /// The number of font units that make up the "em square",
    /// a scalable grid for determining the size of a typeface.
    pub(crate) units_per_em: u32,

    /// The vertical distance from the baseline of the font to the top of the glyph covers.
    pub(crate) ascent: f32,

    /// The vertical distance from the baseline of the font to the bottom of the glyph covers.
    pub(crate) descent: f32,

    /// The recommended additional space to add between lines of type.
    pub(crate) line_gap: f32,

    /// The suggested position of the underline.
    pub(crate) underline_position: f32,

    /// The suggested thickness of the underline.
    pub(crate) underline_thickness: f32,

    /// The height of a capital letter measured from the baseline of the font.
    pub(crate) cap_height: f32,

    /// The height of a lowercase x.
    pub(crate) x_height: f32,

    /// The outer limits of the area that the font covers.
    /// Corresponds to the xMin / xMax / yMin / yMax values in the OpenType `head` table
    pub(crate) bounding_box: Bounds<f32>,
}

impl FontMetrics {
    /// Returns the vertical distance from the baseline of the font to the top of the glyph covers in pixels.
    pub fn ascent(&self, font_size: Pixels) -> Pixels {
        Pixels((self.ascent / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the vertical distance from the baseline of the font to the bottom of the glyph covers in pixels.
    pub fn descent(&self, font_size: Pixels) -> Pixels {
        Pixels((self.descent / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the recommended additional space to add between lines of type in pixels.
    pub fn line_gap(&self, font_size: Pixels) -> Pixels {
        Pixels((self.line_gap / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the suggested position of the underline in pixels.
    pub fn underline_position(&self, font_size: Pixels) -> Pixels {
        Pixels((self.underline_position / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the suggested thickness of the underline in pixels.
    pub fn underline_thickness(&self, font_size: Pixels) -> Pixels {
        Pixels((self.underline_thickness / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the height of a capital letter measured from the baseline of the font in pixels.
    pub fn cap_height(&self, font_size: Pixels) -> Pixels {
        Pixels((self.cap_height / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the height of a lowercase x in pixels.
    pub fn x_height(&self, font_size: Pixels) -> Pixels {
        Pixels((self.x_height / self.units_per_em as f32) * font_size.0)
    }

    /// Returns the outer limits of the area that the font covers in pixels.
    pub fn bounding_box(&self, font_size: Pixels) -> Bounds<Pixels> {
        (self.bounding_box / self.units_per_em as f32 * font_size.0).map(px)
    }
}

#[allow(unused)]
pub(crate) fn font_name_with_fallbacks<'a>(name: &'a str, system: &'a str) -> &'a str {
    // Note: the "Zed Plex" fonts were deprecated as we are not allowed to use "Plex"
    // in a derived font name. They are essentially indistinguishable from IBM Plex/Lilex,
    // and so retained here for backward compatibility.
    match name {
        ".SystemUIFont" => system,
        ".ZedSans" | "Zed Plex Sans" => "IBM Plex Sans",
        ".ZedMono" | "Zed Plex Mono" => "Lilex",
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::size;
    use std::{borrow::Cow, sync::Mutex as StdMutex};

    #[derive(Default)]
    struct RecordingTextSystem {
        requested_fonts: StdMutex<Vec<Font>>,
        layout_requests: StdMutex<Vec<String>>,
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
            self.layout_requests.lock().unwrap().push(text.to_string());
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
        assert!(
            platform_text_system
                .requested_fonts
                .lock()
                .unwrap()
                .iter()
                .all(|font| font.fallbacks.is_some())
        );
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
        assert!(requested_fonts[0].fallbacks.is_some());
    }

    #[test]
    fn default_fallback_stack_is_attached_to_fonts_without_explicit_fallbacks() {
        let platform_text_system = Arc::new(RecordingTextSystem::default());
        let text_system = TextSystem::new(platform_text_system.clone());

        text_system.font_id(&font("Primary Sans")).unwrap();

        let requested_fonts = platform_text_system.requested_fonts.lock().unwrap();
        let fallback_list = requested_fonts[0]
            .fallbacks
            .as_ref()
            .map(FontFallbacks::fallback_list)
            .unwrap_or_default();
        assert!(fallback_list.iter().any(|family| family == "Segoe UI"));
        assert!(fallback_list.iter().any(|family| family == "Noto Sans"));
        assert!(!fallback_list.iter().any(|family| family == "Primary Sans"));
    }

    #[test]
    fn explicit_fallbacks_are_not_replaced_by_default_stack() {
        let platform_text_system = Arc::new(RecordingTextSystem::default());
        let text_system = TextSystem::new(platform_text_system.clone());
        let mut primary = font("Primary Sans");
        primary.fallbacks = Some(FontFallbacks::from_fonts(vec!["Explicit Sans".to_string()]));

        text_system.font_id(&primary).unwrap();

        let requested_fonts = platform_text_system.requested_fonts.lock().unwrap();
        assert_eq!(
            requested_fonts[0]
                .fallbacks
                .as_ref()
                .map(FontFallbacks::fallback_list),
            Some(&["Explicit Sans".to_string()][..])
        );
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
    fn background_warm_up_primes_text_layout_data() {
        let platform_text_system = Arc::new(RecordingTextSystem::default());
        let text_system = TextSystem::new(platform_text_system.clone());
        let before = crate::performance_metrics_snapshot();

        text_system.warm_up_background();

        let after = crate::performance_metrics_snapshot();
        assert!(after.text_background_warmups > before.text_background_warmups);
        assert!(after.text_background_layouts > before.text_background_layouts);
        assert!(
            platform_text_system
                .layout_requests
                .lock()
                .unwrap()
                .iter()
                .any(|text| text == "Hello GPUI text layout")
        );
    }

    #[test]
    fn text_pools_are_bounded() {
        let text_system = Arc::new(TextSystem::new(Arc::new(RecordingTextSystem::default())));

        for index in 0..MAX_WRAPPER_POOL_KEYS + 16 {
            let _wrapper = text_system.line_wrapper(font(".SystemUIFont"), px(index as f32 + 1.0));
        }

        assert!(text_system.wrapper_pool.lock().len() <= MAX_WRAPPER_POOL_KEYS);

        for _ in 0..MAX_FONT_RUNS_POOL_SIZE + 16 {
            text_system.recycle_font_runs_pool(vec![FontRun {
                len: 1,
                font_id: FontId(1),
            }]);
        }

        assert!(text_system.font_runs_pool.lock().len() <= MAX_FONT_RUNS_POOL_SIZE);
    }
}
