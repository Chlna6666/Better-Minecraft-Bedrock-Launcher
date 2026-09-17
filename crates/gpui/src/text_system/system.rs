use crate::{
    Bounds, DevicePixels, GlyphRasterization, GpuiMemoryTrimLevel, Pixels, PlatformTextSystem,
    Result, SharedString, Size, px,
};
use anyhow::{Context as _, anyhow};
use collections::FxHashMap;
use derive_more::Deref;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use smallvec::{SmallVec, smallvec};
use std::{
    borrow::Cow,
    cmp,
    collections::VecDeque,
    hash::Hash,
    ops::{Deref, DerefMut, Range},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use super::{
    DecorationRun, Font, FontId, FontMetrics, FontRun, FontWeight, LineLayout, LineLayoutCache,
    LineLayoutFrameMetrics, LineLayoutIndex, LineWrapper, RenderGlyphParams, ShapedLine, TextRun,
    WrappedLine, font, font_catalog::FontCatalog,
};

pub(super) const MAX_WRAPPER_POOL_KEYS: usize = 128;
const MAX_WRAPPERS_PER_KEY: usize = 4;
pub(super) const MAX_FONT_RUNS_POOL_SIZE: usize = 128;
pub(super) const FONT_RUNS_MIN_RETAINED_CAPACITY: usize = 32;
const FONT_RUNS_TRIM_WATERMARK_MULTIPLIER: usize = 4;
const TEXT_CACHE_MIN_RETAINED_CAPACITY: usize = 64;
const TEXT_CACHE_TRIM_WATERMARK_MULTIPLIER: usize = 4;
// Raster-bounds entries are cheap, but a fixed byte or glyph-count cap is a poor fit for IDE/editor
// workloads. Use epoch-LRU aging instead: cache hits update a relaxed last-used epoch under the
// read lock, while insertion/pressure paths evict entries that are genuinely cold.
const RASTER_BOUNDS_LRU_SWEEP_DIVISOR: usize = 4;
const RASTER_BOUNDS_LRU_MIN_SWEEP_INSERTS: usize = 256;

/// The GPUI text rendering sub system.
pub struct TextSystem {
    pub(super) platform_text_system: Arc<dyn PlatformTextSystem>,
    system_font_family: RwLock<Option<SharedString>>,
    pub(super) font_decision_logged: RwLock<bool>,
    font_id_cache: RwLock<FontIdCache>,
    font_cache_generation: AtomicU64,
    font_metrics: RwLock<FxHashMap<FontId, FontMetrics>>,
    raster_bounds: RwLock<RasterBoundsCache>,
    wrapper_pool: Mutex<FxHashMap<FontIdWithSize, VecDeque<LineWrapper>>>,
    font_runs_pool: Mutex<VecDeque<Vec<FontRun>>>,
    font_catalog: FontCatalog,
}

#[derive(Default)]
struct FontIdCache {
    ids_by_font: FxHashMap<Font, Result<FontId>>,
    fonts_by_id: FxHashMap<FontId, Font>,
}

struct RasterBoundsEntry {
    bounds: Bounds<DevicePixels>,
    last_used_epoch: AtomicU64,
}

impl RasterBoundsEntry {
    fn new(bounds: Bounds<DevicePixels>, epoch: u64) -> Self {
        Self {
            bounds,
            last_used_epoch: AtomicU64::new(epoch),
        }
    }

    #[inline]
    fn touch(&self, epoch: u64) -> Bounds<DevicePixels> {
        self.last_used_epoch.store(epoch, Ordering::Relaxed);
        self.bounds
    }

    #[inline]
    fn last_used_epoch(&self) -> u64 {
        self.last_used_epoch.load(Ordering::Relaxed)
    }
}

struct RasterBoundsCache {
    entries: FxHashMap<RenderGlyphParams, RasterBoundsEntry>,
    next_epoch: AtomicU64,
    lookups_since_sweep: AtomicUsize,
    hits_since_sweep: AtomicUsize,
    inserts_since_sweep: usize,
}

impl Default for RasterBoundsCache {
    fn default() -> Self {
        Self {
            entries: FxHashMap::default(),
            next_epoch: AtomicU64::new(0),
            lookups_since_sweep: AtomicUsize::new(0),
            hits_since_sweep: AtomicUsize::new(0),
            inserts_since_sweep: 0,
        }
    }
}

impl RasterBoundsCache {
    fn get(&self, params: &RenderGlyphParams) -> Option<Bounds<DevicePixels>> {
        self.lookups_since_sweep.fetch_add(1, Ordering::Relaxed);
        let epoch = self.next_access_epoch();
        let entry = self.entries.get(params)?;
        self.hits_since_sweep.fetch_add(1, Ordering::Relaxed);
        Some(entry.touch(epoch))
    }

    fn insert_if_absent(
        &mut self,
        params: RenderGlyphParams,
        bounds: Bounds<DevicePixels>,
    ) -> Bounds<DevicePixels> {
        let epoch = self.next_access_epoch();
        if let Some(existing) = self.entries.get(&params) {
            self.lookups_since_sweep.fetch_add(1, Ordering::Relaxed);
            self.hits_since_sweep.fetch_add(1, Ordering::Relaxed);
            return existing.touch(epoch);
        }

        self.lookups_since_sweep.fetch_add(1, Ordering::Relaxed);
        self.entries
            .insert(params, RasterBoundsEntry::new(bounds, epoch));
        self.inserts_since_sweep = self.inserts_since_sweep.saturating_add(1);
        self.sweep_if_needed();
        bounds
    }

    #[inline]
    fn next_access_epoch(&self) -> u64 {
        self.next_epoch
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    #[inline]
    fn current_epoch(&self) -> u64 {
        self.next_epoch.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn sweep_if_needed(&mut self) {
        let threshold = self
            .entries
            .len()
            .saturating_div(RASTER_BOUNDS_LRU_SWEEP_DIVISOR)
            .max(RASTER_BOUNDS_LRU_MIN_SWEEP_INSERTS);
        if self.inserts_since_sweep < threshold {
            return;
        }

        self.inserts_since_sweep = 0;
        self.evict_idle_entries();
    }

    fn evict_idle_entries(&mut self) {
        let live_len = self.entries.len();
        if live_len <= TEXT_CACHE_MIN_RETAINED_CAPACITY {
            self.take_hit_ratio_percent();
            return;
        }

        let hit_ratio = self.take_hit_ratio_percent();
        let divisor = match hit_ratio {
            90..=usize::MAX => 1,
            70..=89 => 2,
            40..=69 => 4,
            _ => 8,
        };
        let recent_access_window = live_len
            .saturating_div(divisor)
            .max(TEXT_CACHE_MIN_RETAINED_CAPACITY) as u64;
        let current_epoch = self.current_epoch();
        self.entries.retain(|_, entry| {
            current_epoch.saturating_sub(entry.last_used_epoch()) <= recent_access_window
        });
    }

    fn take_hit_ratio_percent(&self) -> usize {
        let lookups = self.lookups_since_sweep.swap(0, Ordering::Relaxed);
        let hits = self.hits_since_sweep.swap(0, Ordering::Relaxed).min(lookups);
        if lookups == 0 {
            100
        } else {
            hits.saturating_mul(100) / lookups
        }
    }

    fn evict_lru_to_len(&mut self, target_len: usize) {
        let live_len = self.entries.len();
        if live_len <= target_len {
            return;
        }

        let mut candidates = self
            .entries
            .iter()
            .map(|(params, entry)| (params.clone(), entry.last_used_epoch()))
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|(_, last_used_epoch)| *last_used_epoch);

        for (params, _) in candidates.into_iter().take(live_len - target_len) {
            self.entries.remove(&params);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.next_epoch.store(0, Ordering::Relaxed);
        self.lookups_since_sweep.store(0, Ordering::Relaxed);
        self.hits_since_sweep.store(0, Ordering::Relaxed);
        self.inserts_since_sweep = 0;
    }

    fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light | GpuiMemoryTrimLevel::Moderate => {
                let current_len = self.entries.len();
                let target_len = match level {
                    GpuiMemoryTrimLevel::Light => current_len.saturating_mul(3) / 4,
                    GpuiMemoryTrimLevel::Moderate => current_len / 2,
                    GpuiMemoryTrimLevel::Aggressive => unreachable!(),
                };
                self.evict_lru_to_len(target_len);

                let retained_capacity = self.entries.len();
                trim_map_capacity(
                    &mut self.entries,
                    retained_capacity,
                    TEXT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
            }
            GpuiMemoryTrimLevel::Aggressive => {
                self.clear();
                self.entries.shrink_to(0);
            }
        }
    }
}

impl TextSystem {
    pub(crate) fn new(platform_text_system: Arc<dyn PlatformTextSystem>) -> Self {
        TextSystem {
            platform_text_system,
            system_font_family: RwLock::default(),
            font_decision_logged: RwLock::new(false),
            font_metrics: RwLock::default(),
            raster_bounds: RwLock::default(),
            font_id_cache: RwLock::default(),
            font_cache_generation: AtomicU64::new(0),
            wrapper_pool: Mutex::default(),
            font_runs_pool: Mutex::default(),
            font_catalog: FontCatalog::default(),
        }
    }

    /// Returns a shared snapshot of all available font names.
    pub fn font_names(&self) -> Arc<[String]> {
        self.font_catalog
            .available_font_names(|| self.platform_text_system.all_font_names())
    }

    /// Get a list of all available font names from the operating system.
    pub fn all_font_names(&self) -> Vec<String> {
        self.font_names().as_ref().to_vec()
    }

    /// Returns whether a font family can be selected by the platform text system.
    pub fn is_font_family_available(&self, family: &str) -> bool {
        let family = family.trim();
        !family.is_empty() && self.font_id(&font(family.to_owned())).is_ok()
    }

    /// Returns the application default font family, or the platform default when unset.
    pub fn default_font_family(&self) -> SharedString {
        self.system_font_family
            .read()
            .clone()
            .unwrap_or_else(|| self.platform_font_family())
    }

    /// Returns the effective fallback families in resolution order.
    pub fn fallback_font_families(&self) -> Arc<[SharedString]> {
        self.font_catalog.fallback_families()
    }

    /// Add a font's data to the text system.
    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.platform_text_system.add_fonts(fonts)?;
        self.font_catalog.invalidate_available_names();
        self.clear_caches();
        Ok(())
    }

    /// Add font files to the text system by path.
    pub fn add_font_paths(&self, paths: Vec<PathBuf>) -> Result<()> {
        self.platform_text_system.add_font_paths(paths)?;
        self.font_catalog.invalidate_available_names();
        self.clear_caches();
        Ok(())
    }

    pub(crate) fn platform_font_family(&self) -> SharedString {
        self.platform_text_system.platform_font_family()
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

    pub(crate) fn set_fallback_font_families(&self, families: Vec<SharedString>) {
        if self.font_catalog.set_fallback_font_families(families) {
            self.clear_caches();
        }
    }

    pub(crate) fn preload_font_family(&self, family: SharedString) -> Result<()> {
        let mut font = font(family);
        font.weight = FontWeight::NORMAL;
        self.font_id(&font)?;
        Ok(())
    }

    pub(crate) fn clear_caches(&self) {
        let mut font_id_cache = self.font_id_cache.write();
        font_id_cache.ids_by_font.clear();
        font_id_cache.fonts_by_id.clear();
        self.font_cache_generation.fetch_add(1, Ordering::Release);
        drop(font_id_cache);
        self.font_metrics.write().clear();
        self.raster_bounds.write().clear();
        self.wrapper_pool.lock().clear();
        self.font_runs_pool.lock().clear();
    }

    /// Get the FontId for the configure font family and style.
    pub(super) fn font_id(&self, font: &Font) -> Result<FontId> {
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

        fn clone_font_id(font_id: &Result<FontId>) -> Result<FontId> {
            match font_id {
                Ok(font_id) => Ok(*font_id),
                Err(err) => Err(anyhow!("{err}")),
            }
        }

        loop {
            let generation = {
                let font_id_cache = self.font_id_cache.read();
                if let Some(font_id) = font_id_cache.ids_by_font.get(font).map(clone_font_id) {
                    return font_id;
                }
                self.font_cache_generation.load(Ordering::Acquire)
            };

            // Platform font discovery can enter DirectWrite/CoreText/fontdb and should not hold the
            // shared cache lock. Different font misses may resolve concurrently; a same-key race is
            // reconciled under the short write section below.
            let font_id = self.platform_text_system.font_id(font);
            let mut font_id_cache = self.font_id_cache.write();
            if let Some(cached) = font_id_cache.ids_by_font.get(font).map(clone_font_id) {
                return cached;
            }
            if self.font_cache_generation.load(Ordering::Acquire) != generation {
                drop(font_id_cache);
                continue;
            }

            font_id_cache
                .ids_by_font
                .insert(font.clone(), clone_font_id(&font_id));
            if let Ok(font_id) = font_id.as_ref() {
                font_id_cache
                    .fonts_by_id
                    .entry(*font_id)
                    .or_insert_with(|| font.clone());
            }
            return font_id;
        }
    }

    /// Get the Font for the Font Id.
    pub fn font_for_id(&self, id: FontId) -> Option<Font> {
        self.font_id_cache.read().fonts_by_id.get(&id).cloned()
    }

    /// Tries to resolve the specified font, preserving its weight, style, and
    /// OpenType features while checking configured fallback families.
    pub fn try_resolve_font(&self, font: &Font) -> Result<FontId> {
        let mut attempted_families = SmallVec::<[SharedString; 8]>::new();
        attempted_families.push(font.family.clone());
        let mut last_error = match self.font_id(font) {
            Ok(font_id) => return Ok(font_id),
            Err(error) => error,
        };

        let configured_fallbacks = self.fallback_font_families();
        let run_fallbacks = font
            .fallbacks
            .as_ref()
            .map(|fallbacks| fallbacks.fallback_list())
            .unwrap_or_default();
        for family in run_fallbacks.iter().chain(configured_fallbacks.iter()) {
            if attempted_families
                .iter()
                .any(|attempted| attempted.eq_ignore_ascii_case(family.as_ref()))
            {
                continue;
            }
            let family = family.clone();
            attempted_families.push(family.clone());
            let mut fallback = font.clone();
            fallback.family = family.clone();
            fallback.fallbacks = None;
            match self.font_id(&fallback) {
                Ok(font_id) => return Ok(font_id),
                Err(error) => last_error = error,
            }
        }

        let attempted_families = attempted_families
            .iter()
            .map(SharedString::as_ref)
            .collect::<Vec<_>>()
            .join(", ");
        Err(last_error.context(format!(
            "failed to resolve font '{}' using families [{attempted_families}]",
            font.family
        )))
    }

    /// Resolves the specified font, falling back to the configured font stack if
    /// the font fails to load.
    ///
    /// # Panics
    ///
    /// Panics if the font and none of the fallbacks can be resolved.
    pub fn resolve_font(&self, font: &Font) -> FontId {
        self.try_resolve_font(font)
            .unwrap_or_else(|error| panic!("{error}"))
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
    /// lines of type in the same text size"
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
        let font_id = self.resolve_font(&font);
        let key = FontIdWithSize { font_id, font_size };
        let wrapper = {
            let mut pool = self.wrapper_pool.lock();
            pool.get_mut(&key).and_then(VecDeque::pop_back)
        }
        .unwrap_or_else(|| LineWrapper::new(font_id, font_size, self.platform_text_system.clone()));

        LineWrapperHandle {
            wrapper: Some(wrapper),
            text_system: self.clone(),
        }
    }

    /// Get the rasterized size and location of a specific, rendered glyph.
    pub(crate) fn raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        if let Some(bounds) = self.raster_bounds.read().get(params) {
            return Ok(bounds);
        }

        // Platform glyph analysis can enter DirectWrite/FreeType and is much more expensive than
        // the cache operation itself. Do it without the global raster-bounds write lock so misses
        // for unrelated glyphs can proceed concurrently across windows. A same-key race is benign:
        // the first completed result is retained and all callers observe the same cached bounds.
        let computed = self.platform_text_system.glyph_raster_bounds(params)?;
        let mut raster_bounds = self.raster_bounds.write();
        Ok(raster_bounds.insert_if_absent(params.clone(), computed))
    }

    pub(crate) fn rasterize_glyph(&self, params: &RenderGlyphParams) -> Result<GlyphRasterization> {
        let raster_bounds = self.raster_bounds(params)?;
        self.platform_text_system
            .rasterize_glyph(params, raster_bounds)
    }

    pub(super) fn take_font_runs_pool(&self) -> Vec<FontRun> {
        let mut font_runs = self.font_runs_pool.lock().pop_back().unwrap_or_default();
        font_runs.clear();
        font_runs
    }

    pub(super) fn recycle_font_runs_pool(&self, mut font_runs: Vec<FontRun>) {
        font_runs.clear();
        trim_vec_capacity(
            &mut font_runs,
            FONT_RUNS_MIN_RETAINED_CAPACITY,
            FONT_RUNS_TRIM_WATERMARK_MULTIPLIER,
        );

        let mut pool = self.font_runs_pool.lock();
        if pool.len() >= MAX_FONT_RUNS_POOL_SIZE {
            pool.pop_front();
        }
        pool.push_back(font_runs);
    }

    pub(crate) fn trim_retained_capacity_for_level(&self, level: GpuiMemoryTrimLevel) {
        trim_wrapper_pool(&mut self.wrapper_pool.lock(), level);
        trim_font_runs_pool(&mut self.font_runs_pool.lock(), level);
        self.raster_bounds
            .write()
            .trim_retained_capacity_for_level(level);
    }

    #[cfg(test)]
    pub(super) fn wrapper_pool_len(&self) -> usize {
        self.wrapper_pool.lock().len()
    }

    #[cfg(test)]
    pub(super) fn font_runs_pool_len(&self) -> usize {
        self.font_runs_pool.lock().len()
    }

    #[cfg(test)]
    pub(super) fn font_runs_pool_max_capacity(&self) -> usize {
        self.font_runs_pool
            .lock()
            .iter()
            .map(Vec::capacity)
            .max()
            .unwrap_or_default()
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
    pub(crate) fn new(text_system: Arc<TextSystem>) -> Self {
        Self {
            line_layout_cache: LineLayoutCache::new(text_system.platform_text_system.clone()),
            text_system,
        }
    }

    pub(crate) fn layout_index(&self) -> LineLayoutIndex {
        self.line_layout_cache.layout_index()
    }

    pub(crate) fn can_reuse_layouts(&self, index: Range<LineLayoutIndex>) -> bool {
        self.line_layout_cache.can_reuse_layouts(index)
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

    pub(crate) fn trim_retained_capacity_for_level(&self, level: GpuiMemoryTrimLevel) {
        self.line_layout_cache
            .trim_retained_capacity_for_level(level);
        self.text_system.trim_retained_capacity_for_level(level);
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
        if text.find('\n').is_none() {
            let mut font_runs = self.text_system.take_font_runs_pool();
            let line = self.shape_single_line_text(
                text,
                font_size,
                runs,
                wrap_width,
                line_clamp,
                &mut font_runs,
            );
            self.text_system.recycle_font_runs_pool(font_runs);
            return Ok(smallvec![line]);
        }

        let mut runs = runs.iter().filter(|run| run.len > 0).cloned().peekable();
        let mut font_runs = self.text_system.take_font_runs_pool();

        let mut lines = SmallVec::new();
        let mut line_start = 0;
        let max_wrap_lines = line_clamp.unwrap_or(usize::MAX);
        let mut wrapped_lines = 0;

        let mut queue_line_layout = |line_text: SharedString| {
            font_runs.clear();
            let line_end = line_start + line_text.len();

            let mut decoration_runs = SmallVec::<[DecorationRun; 32]>::new();
            let mut run_start = line_start;
            while run_start < line_end {
                let Some(run) = runs.peek_mut() else {
                    break;
                };

                let run_len_within_line = cmp::min(line_end, run_start + run.len) - run_start;

                push_decoration_run(&mut decoration_runs, run, run_len_within_line);
                self.push_font_run(&mut font_runs, run, run_len_within_line);

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
            queue_line_layout(SharedString::new(first_line));
            queue_line_layout(SharedString::new(second_line));
            for line_text in split_lines {
                queue_line_layout(SharedString::new(line_text));
            }
        }

        if !processed {
            queue_line_layout(text);
        }

        self.text_system.recycle_font_runs_pool(font_runs);

        Ok(lines)
    }

    pub(crate) fn finish_frame(&self) -> LineLayoutFrameMetrics {
        self.line_layout_cache.finish_frame()
    }

    fn shape_single_line_text(
        &self,
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        wrap_width: Option<Pixels>,
        line_clamp: Option<usize>,
        font_runs: &mut Vec<FontRun>,
    ) -> WrappedLine {
        font_runs.clear();
        let mut decoration_runs = SmallVec::<[DecorationRun; 32]>::new();

        let line_end = text.len();
        let mut run_start = 0;
        for run in runs.iter().filter(|run| run.len > 0) {
            if run_start >= line_end {
                break;
            }

            let run_len_within_line = cmp::min(line_end, run_start + run.len) - run_start;
            push_decoration_run(&mut decoration_runs, run, run_len_within_line);
            self.push_font_run(font_runs, run, run_len_within_line);
            run_start += run_len_within_line;
        }

        let layout = self.line_layout_cache.layout_wrapped_line(
            &text,
            font_size,
            &*font_runs,
            wrap_width,
            Some(line_clamp.unwrap_or(usize::MAX)),
        );

        WrappedLine {
            layout,
            decoration_runs,
            text,
        }
    }

    fn push_font_run(&self, font_runs: &mut Vec<FontRun>, run: &TextRun, len: usize) {
        let font_id = self.resolve_font(&run.font);
        if let Some(font_run) = font_runs.last_mut()
            && font_id == font_run.font_id
        {
            font_run.len += len;
        } else {
            font_runs.push(FontRun { len, font_id });
        }
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
        let mut font_runs = self.text_system.take_font_runs_pool();
        font_runs.clear();

        for run in runs.iter().filter(|run| run.len > 0) {
            self.push_font_run(&mut font_runs, run, run.len);
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

fn push_decoration_run(
    decoration_runs: &mut SmallVec<[DecorationRun; 32]>,
    run: &TextRun,
    len: usize,
) {
    if let Some(last_run) = decoration_runs.last_mut()
        && last_run.color == run.color
        && last_run.underline == run.underline
        && last_run.strikethrough == run.strikethrough
        && last_run.background_color == run.background_color
        && last_run.background_corner_radius == run.background_corner_radius
        && last_run.background_padding == run.background_padding
    {
        last_run.len += len as u32;
    } else {
        decoration_runs.push(DecorationRun {
            len: len as u32,
            color: run.color,
            background_color: run.background_color,
            background_corner_radius: run.background_corner_radius,
            background_padding: run.background_padding,
            underline: run.underline,
            strikethrough: run.strikethrough,
        });
    }
}

fn trim_wrapper_pool(
    wrapper_pool: &mut FxHashMap<FontIdWithSize, VecDeque<LineWrapper>>,
    level: GpuiMemoryTrimLevel,
) {
    match level {
        GpuiMemoryTrimLevel::Light | GpuiMemoryTrimLevel::Moderate => {
            for wrappers in wrapper_pool.values_mut() {
                trim_vec_deque_capacity(
                    wrappers,
                    MAX_WRAPPERS_PER_KEY,
                    TEXT_CACHE_TRIM_WATERMARK_MULTIPLIER,
                );
            }

            let floor = match level {
                GpuiMemoryTrimLevel::Light => MAX_WRAPPER_POOL_KEYS,
                GpuiMemoryTrimLevel::Moderate => MAX_WRAPPER_POOL_KEYS / 2,
                GpuiMemoryTrimLevel::Aggressive => unreachable!(),
            };
            trim_map_capacity(wrapper_pool, floor, TEXT_CACHE_TRIM_WATERMARK_MULTIPLIER);
        }
        GpuiMemoryTrimLevel::Aggressive => {
            wrapper_pool.clear();
            wrapper_pool.shrink_to(0);
        }
    }
}

fn trim_font_runs_pool(font_runs_pool: &mut VecDeque<Vec<FontRun>>, level: GpuiMemoryTrimLevel) {
    match level {
        GpuiMemoryTrimLevel::Light | GpuiMemoryTrimLevel::Moderate => {
            let target_len = match level {
                GpuiMemoryTrimLevel::Light => MAX_FONT_RUNS_POOL_SIZE,
                GpuiMemoryTrimLevel::Moderate => MAX_FONT_RUNS_POOL_SIZE / 2,
                GpuiMemoryTrimLevel::Aggressive => unreachable!(),
            };

            while font_runs_pool.len() > target_len {
                font_runs_pool.pop_front();
            }

            for font_runs in font_runs_pool.iter_mut() {
                font_runs.clear();
                trim_vec_capacity(
                    font_runs,
                    FONT_RUNS_MIN_RETAINED_CAPACITY,
                    FONT_RUNS_TRIM_WATERMARK_MULTIPLIER,
                );
            }
            trim_vec_deque_capacity(
                font_runs_pool,
                target_len,
                TEXT_CACHE_TRIM_WATERMARK_MULTIPLIER,
            );
        }
        GpuiMemoryTrimLevel::Aggressive => {
            font_runs_pool.clear();
            font_runs_pool.shrink_to(0);
        }
    }
}

fn trim_map_capacity<K, V>(map: &mut FxHashMap<K, V>, floor: usize, multiplier: usize)
where
    K: Eq + Hash,
{
    let target = floor.max(map.len());
    if map.capacity() > target.saturating_mul(multiplier) {
        map.shrink_to(target);
    }
}

fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(vec.len());
    if vec.capacity() > target.saturating_mul(multiplier) {
        vec.shrink_to(target);
    }
}

fn trim_vec_deque_capacity<T>(vec: &mut VecDeque<T>, floor: usize, multiplier: usize) {
    let target = floor.max(vec.len());
    if vec.capacity() > target.saturating_mul(multiplier) {
        vec.shrink_to(target);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DevicePixels, FontId, GlyphId, GpuiMemoryTrimLevel, point, px, size};

    fn test_raster_bounds() -> Bounds<DevicePixels> {
        Bounds {
            origin: point(DevicePixels(0), DevicePixels(0)),
            size: size(DevicePixels(8), DevicePixels(12)),
        }
    }

    fn test_glyph_params(glyph_id: u32) -> RenderGlyphParams {
        RenderGlyphParams {
            font_id: FontId(1),
            glyph_id: GlyphId(glyph_id),
            font_size: px(14.),
            subpixel_variant: point(0_u8, 0_u8),
            scale_factor: 1.0,
            grayscale_antialiasing: false,
            is_emoji: false,
            is_cjk: false,
        }
    }

    #[test]
    fn raster_bounds_lru_records_hits_without_write_path() {
        let mut cache = RasterBoundsCache::default();
        let params = test_glyph_params(42);
        let bounds = test_raster_bounds();

        assert!(cache.get(&params).is_none());
        assert_eq!(cache.insert_if_absent(params.clone(), bounds), bounds);
        assert_eq!(cache.get(&params), Some(bounds));
        assert_eq!(cache.len(), 1);
        assert!(cache.take_hit_ratio_percent() > 0);
    }

    #[test]
    fn raster_bounds_lru_keeps_interleaved_hot_entries() {
        let mut cache = RasterBoundsCache::default();
        let bounds = test_raster_bounds();
        let hot_params = (0..32).map(test_glyph_params).collect::<Vec<_>>();

        for params in &hot_params {
            cache.insert_if_absent(params.clone(), bounds);
        }

        for glyph_id in 32..12_000 {
            cache.insert_if_absent(test_glyph_params(glyph_id), bounds);
            if glyph_id % 64 == 0 {
                for params in &hot_params {
                    assert_eq!(cache.get(params), Some(bounds));
                }
            }
        }
        cache.evict_idle_entries();

        for params in &hot_params {
            assert_eq!(cache.get(params), Some(bounds));
        }
        assert!(
            cache.len() < 12_000,
            "one-shot scan entries should not remain fully resident"
        );
    }

    #[test]
    fn raster_bounds_memory_pressure_keeps_most_recent_entries() {
        let mut cache = RasterBoundsCache::default();
        let bounds = test_raster_bounds();

        for glyph_id in 0..256 {
            cache.insert_if_absent(test_glyph_params(glyph_id), bounds);
        }
        for glyph_id in 224..256 {
            for _ in 0..4 {
                assert_eq!(cache.get(&test_glyph_params(glyph_id)), Some(bounds));
            }
        }

        let before = cache.len();
        cache.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Moderate);
        assert!(cache.len() < before);
        for glyph_id in 224..256 {
            assert_eq!(cache.get(&test_glyph_params(glyph_id)), Some(bounds));
        }
    }
}
