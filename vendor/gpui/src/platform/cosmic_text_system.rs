use crate::{
    Bounds, DevicePixels, Font, FontFallbacks, FontFeatures, FontId, FontMetrics, FontRun,
    FontStyle, FontWeight, GlyphId, GlyphRasterization, LineLayout, Pixels, PlatformTextSystem,
    Point, RenderGlyphParams, SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y, ShapedGlyph, ShapedRun,
    SharedString, Size, point, size,
};
use anyhow::{Context as _, Ok, Result};
use collections::HashMap;
use cosmic_text::{
    Attrs, AttrsList, CacheKey, CacheKeyFlags, Ellipsize, Family, Font as CosmicTextFont,
    FontFeatures as CosmicFontFeatures, FontSystem, Hinting, ShapeBuffer, ShapeLine, SwashCache,
    SwashContent, SwashImage,
    fontdb::{Query, Source, Stretch, Weight},
};

use parking_lot::RwLock;
use smallvec::SmallVec;
use std::{
    borrow::Cow,
    collections::{
        HashSet,
        hash_map::{DefaultHasher, Entry},
    },
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;
#[cfg(target_os = "windows")]
use windows::Win32::{
    Graphics::Gdi::LOGFONTW,
    UI::WindowsAndMessaging::{
        SPI_GETICONTITLELOGFONT, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    },
};

pub(crate) struct CosmicTextSystem(RwLock<CosmicTextSystemState>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontKey {
    family: SharedString,
    features: FontFeatures,
    fallbacks: Option<FontFallbacks>,
    automatic_fallbacks: bool,
    source_selection: FontSourceSelection,
}

impl FontKey {
    fn new(
        family: SharedString,
        features: FontFeatures,
        fallbacks: Option<FontFallbacks>,
        automatic_fallbacks: bool,
        source_selection: FontSourceSelection,
    ) -> Self {
        Self {
            family,
            features,
            fallbacks,
            automatic_fallbacks,
            source_selection,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FontSourceSelection {
    Any,
    SystemOnly,
}

impl FontSourceSelection {
    fn matches(self, source: &Source) -> bool {
        match self {
            Self::Any => true,
            Self::SystemOnly => !matches!(source, Source::Binary(_)),
        }
    }
}

#[derive(Clone, Debug)]
struct SystemFallbackFace {
    database_id: cosmic_text::fontdb::ID,
    family: SharedString,
    score: u32,
}

struct CosmicTextSystemState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    cjk_frame_bounds_cache: HashMap<CjkFrameKey, Bounds<DevicePixels>>,
    platform_font_family: SharedString,
    system_fonts_loaded: bool,
    system_coverage_fallback_face: Option<SystemFallbackFace>,
    system_coverage_fallback_computed: bool,
    coverage_best_fallback_logged: bool,
    scratch: ShapeBuffer,
    /// Contains all already loaded fonts, including all faces. Indexed by `FontId`.
    loaded_fonts: Vec<LoadedFont>,
    /// Caches the `FontId`s associated with a specific family to avoid iterating the font database
    /// for every font face in a family.
    font_ids_by_family_cache: HashMap<FontKey, SmallVec<[FontId; 4]>>,
    loaded_font_paths: HashSet<PathBuf>,
    loaded_embedded_font_hashes: HashSet<u64>,
}

struct LoadedFont {
    font: Arc<CosmicTextFont>,
    source_features: FontFeatures,
    features: CosmicFontFeatures,
    weight: Weight,
    cache_key_flags: CacheKeyFlags,
    is_known_emoji_font: bool,
    user_fallback_chain: Arc<[(FontId, SharedString)]>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CjkFrameKey {
    font_id: FontId,
    font_size_bits: u32,
    scale_factor_bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CjkRasterFrame {
    origin_y: DevicePixels,
    height: DevicePixels,
}

impl CosmicTextSystem {
    pub(crate) fn new() -> Self {
        // todo(linux) make font loading non-blocking
        let (mut font_system, system_fonts_loaded) = minimal_startup_font_system();
        let platform_font_family: SharedString = resolved_platform_font_name(&font_system).into();
        font_system
            .db_mut()
            .set_sans_serif_family(platform_font_family.to_string());

        Self(RwLock::new(CosmicTextSystemState {
            font_system,
            platform_font_family,
            swash_cache: SwashCache::new(),
            cjk_frame_bounds_cache: HashMap::default(),
            system_fonts_loaded,
            system_coverage_fallback_face: None,
            system_coverage_fallback_computed: false,
            coverage_best_fallback_logged: false,
            scratch: ShapeBuffer::default(),
            loaded_fonts: Vec::new(),
            font_ids_by_family_cache: HashMap::default(),
            loaded_font_paths: HashSet::default(),
            loaded_embedded_font_hashes: HashSet::default(),
        }))
    }
}

impl Default for CosmicTextSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformTextSystem for CosmicTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.0.write().add_fonts(fonts)
    }

    fn add_font_paths(&self, paths: Vec<PathBuf>) -> Result<()> {
        self.0.write().add_font_paths(paths)
    }

    fn set_application_font_family(&self, family: SharedString) {
        self.0.write().set_application_font_family(family);
    }

    fn platform_font_family(&self) -> SharedString {
        self.0.read().platform_font_family.clone()
    }

    fn all_font_names(&self) -> Vec<String> {
        let mut result = self
            .0
            .read()
            .font_system
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|family| family.0.clone()))
            .collect::<Vec<_>>();
        result.sort();
        result.dedup();
        result
    }

    fn font_id(&self, font: &Font) -> Result<FontId> {
        let mut state = self.0.write();
        let key = FontKey::new(
            font.family.clone(),
            font.features.clone(),
            font.fallbacks.clone(),
            true,
            FontSourceSelection::Any,
        );
        let candidates = if let Some(font_ids) = state.font_ids_by_family_cache.get(&key) {
            font_ids.clone()
        } else {
            let font_ids = state.load_family(
                &font.family,
                &font.features,
                font.fallbacks.as_ref(),
                true,
                FontSourceSelection::Any,
            )?;
            state.font_ids_by_family_cache.insert(key, font_ids.clone());
            font_ids
        };

        state.select_font_id(&candidates, font)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        let metrics = self
            .0
            .read()
            .loaded_font(font_id)
            .font
            .as_swash()
            .metrics(&[]);

        FontMetrics {
            units_per_em: metrics.units_per_em as u32,
            ascent: metrics.ascent,
            descent: -metrics.descent, // todo(linux) confirm this is correct
            line_gap: metrics.leading,
            underline_position: metrics.underline_offset,
            underline_thickness: metrics.stroke_size,
            cap_height: metrics.cap_height,
            x_height: metrics.x_height,
            // todo(linux): Compute this correctly
            bounding_box: Bounds {
                origin: point(0.0, 0.0),
                size: size(metrics.max_width, metrics.ascent + metrics.descent),
            },
        }
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        let lock = self.0.read();
        let glyph_metrics = lock.loaded_font(font_id).font.as_swash().glyph_metrics(&[]);
        let glyph_id = glyph_id.0 as u16;
        // todo(linux): Compute this correctly
        // see https://github.com/servo/font-kit/blob/master/src/loaders/freetype.rs#L614-L620
        Ok(Bounds {
            origin: point(0.0, 0.0),
            size: size(
                glyph_metrics.advance_width(glyph_id),
                glyph_metrics.advance_height(glyph_id),
            ),
        })
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        self.0.read().advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.0.read().glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        self.0.write().raster_bounds(params)
    }

    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<GlyphRasterization> {
        self.0.write().rasterize_glyph(params, raster_bounds)
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.0.write().layout_line(text, font_size, runs)
    }
}

impl CosmicTextSystemState {
    fn ensure_system_fonts_loaded(&mut self) {
        if self.system_fonts_loaded {
            return;
        }

        self.font_system.db_mut().load_system_fonts();
        self.system_fonts_loaded = true;
        self.font_ids_by_family_cache.clear();
        self.system_coverage_fallback_face = None;
        self.system_coverage_fallback_computed = false;
        self.coverage_best_fallback_logged = false;
    }

    fn loaded_font(&self, font_id: FontId) -> &LoadedFont {
        &self.loaded_fonts[font_id.0]
    }

    fn set_application_font_family(&mut self, family: SharedString) {
        self.platform_font_family = family.clone();
        self.font_system
            .db_mut()
            .set_sans_serif_family(family.to_string());
        self.font_ids_by_family_cache.clear();
        self.swash_cache = SwashCache::new();
        self.cjk_frame_bounds_cache.clear();
        self.system_coverage_fallback_face = None;
        self.system_coverage_fallback_computed = false;
        self.coverage_best_fallback_logged = false;
        log::info!("gpui_text_font: application_font_family=\"{}\"", family);
    }

    fn automatic_system_fallback_chain(
        &mut self,
        features: &FontFeatures,
        primary_family: &str,
    ) -> Arc<[(FontId, SharedString)]> {
        let platform_family = self.platform_font_family.clone();
        if !platform_family.eq_ignore_ascii_case(primary_family)
            && let Some(fallback) =
                self.load_system_fallback_family(platform_family.as_ref(), features)
            && self
                .loaded_font(fallback.0)
                .font
                .as_swash()
                .charmap()
                .map('图')
                != 0
        {
            return Arc::from(vec![fallback]);
        }

        if let Some(best_face) = self.system_coverage_fallback_face()
            && !best_face.family.eq_ignore_ascii_case(primary_family)
            && let Some(fallback) = self.load_system_fallback_face(&best_face, features)
        {
            return Arc::from(vec![fallback]);
        }

        Arc::from(Vec::new())
    }

    fn load_system_fallback_family(
        &mut self,
        family: &str,
        features: &FontFeatures,
    ) -> Option<(FontId, SharedString)> {
        let fallback_key = FontKey::new(
            SharedString::from(family.to_owned()),
            features.clone(),
            None,
            false,
            FontSourceSelection::SystemOnly,
        );
        let fallback_ids = if let Some(cached) = self.font_ids_by_family_cache.get(&fallback_key) {
            cached.clone()
        } else {
            let loaded = self
                .load_family(
                    family,
                    features,
                    None,
                    false,
                    FontSourceSelection::SystemOnly,
                )
                .unwrap_or_else(|error| {
                    log::warn!(
                        "gpui_system_font_fallback: failed to load system fallback family \"{}\": {}",
                        family,
                        error
                    );
                    SmallVec::new()
                });
            self.font_ids_by_family_cache
                .insert(fallback_key, loaded.clone());
            loaded
        };

        let fallback_id = fallback_ids.first().copied()?;
        let database_id = self.loaded_fonts[fallback_id.0].font.id();
        let face = self.font_system.db().face(database_id)?;
        let fallback_family = face
            .families
            .first()
            .map(|family| SharedString::from(family.0.clone()))?;
        Some((fallback_id, fallback_family))
    }

    fn load_system_fallback_face(
        &mut self,
        fallback_face: &SystemFallbackFace,
        features: &FontFeatures,
    ) -> Option<(FontId, SharedString)> {
        let font_id = self
            .font_id_for_database_id(fallback_face.database_id, features)
            .ok()?;
        let face = self.font_system.db().face(fallback_face.database_id)?;
        if !FontSourceSelection::SystemOnly.matches(&face.source)
            || check_is_known_emoji_font(&face.post_script_name)
            || is_icon_font_name(&face.post_script_name)
        {
            return None;
        }

        if !charmap_covers_system_text_sample(self.loaded_font(font_id).font.as_swash().charmap()) {
            log::warn!(
                "gpui_system_font_fallback: coverage fallback face did not retain text coverage after load family=\"{}\" postscript=\"{}\" face_id={:?}",
                fallback_face.family,
                face.post_script_name,
                face.id,
            );
            return None;
        }

        if !self.coverage_best_fallback_logged {
            log::info!(
                "gpui_system_font_fallback: coverage_best_font_used family=\"{}\" postscript=\"{}\" score={} face_id={:?}",
                fallback_face.family,
                face.post_script_name,
                fallback_face.score,
                face.id,
            );
            self.coverage_best_fallback_logged = true;
        }
        Some((font_id, fallback_face.family.clone()))
    }

    fn system_coverage_fallback_face(&mut self) -> Option<SystemFallbackFace> {
        if self.system_coverage_fallback_computed {
            return self.system_coverage_fallback_face.clone();
        }

        self.system_coverage_fallback_computed = true;
        self.system_coverage_fallback_face = best_system_text_fallback_face(&self.font_system);
        self.system_coverage_fallback_face.clone()
    }

    #[profiling::function]
    fn add_fonts(&mut self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        let db = self.font_system.db_mut();
        for bytes in fonts {
            let font_hash = font_bytes_hash(bytes.as_ref());
            if self.loaded_embedded_font_hashes.contains(&font_hash) {
                continue;
            }

            match bytes {
                Cow::Borrowed(embedded_font) => {
                    db.load_font_data(embedded_font.to_vec());
                }
                Cow::Owned(bytes) => {
                    db.load_font_data(bytes);
                }
            }
            self.loaded_embedded_font_hashes.insert(font_hash);
        }
        self.font_ids_by_family_cache.clear();
        self.swash_cache = SwashCache::new();
        self.cjk_frame_bounds_cache.clear();
        self.system_coverage_fallback_face = None;
        self.system_coverage_fallback_computed = false;
        self.coverage_best_fallback_logged = false;
        Ok(())
    }

    fn add_font_paths(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        let db = self.font_system.db_mut();
        for path in paths {
            let path = path.canonicalize().unwrap_or(path);
            if self.loaded_font_paths.contains(&path) {
                continue;
            }

            db.load_font_file(&path)
                .with_context(|| format!("loading default font from {}", path.display()))?;
            self.loaded_font_paths.insert(path);
        }
        self.font_ids_by_family_cache.clear();
        self.swash_cache = SwashCache::new();
        self.cjk_frame_bounds_cache.clear();
        self.system_coverage_fallback_face = None;
        self.system_coverage_fallback_computed = false;
        self.coverage_best_fallback_logged = false;
        Ok(())
    }

    #[profiling::function]
    fn load_family(
        &mut self,
        name: &str,
        features: &FontFeatures,
        fallbacks: Option<&FontFallbacks>,
        automatic_fallbacks: bool,
        source_selection: FontSourceSelection,
    ) -> Result<SmallVec<[FontId; 4]>> {
        let user_fallback_chain: Arc<[(FontId, SharedString)]> = match fallbacks {
            Some(fallbacks) if !fallbacks.fallback_list().is_empty() => {
                let mut chain = Vec::new();
                for fallback_name in fallbacks.fallback_list() {
                    let fallback_key = FontKey::new(
                        SharedString::from(fallback_name.clone()),
                        features.clone(),
                        None,
                        false,
                        FontSourceSelection::Any,
                    );
                    let fallback_ids =
                        if let Some(cached) = self.font_ids_by_family_cache.get(&fallback_key) {
                            cached.clone()
                        } else {
                            let loaded = self.load_family(
                                fallback_name,
                                features,
                                None,
                                false,
                                FontSourceSelection::Any,
                            )?;
                            self.font_ids_by_family_cache
                                .insert(fallback_key.clone(), loaded.clone());
                            loaded
                        };
                    let Some(&fallback_id) = fallback_ids.first() else {
                        continue;
                    };
                    let database_id = self.loaded_fonts[fallback_id.0].font.id();
                    if let Some(face) = self.font_system.db().face(database_id)
                        && let Some(family) = face.families.first()
                    {
                        chain.push((fallback_id, SharedString::from(family.0.clone())));
                    }
                }
                Arc::from(chain)
            }
            _ if automatic_fallbacks => self.automatic_system_fallback_chain(features, name),
            _ => Arc::from(Vec::new()),
        };

        let name =
            crate::text_system::font_name_with_fallbacks(name, self.platform_font_family.as_ref())
                .to_owned();

        let families = self
            .font_system
            .db()
            .faces()
            .filter(|face| {
                source_selection.matches(&face.source)
                    && face.families.iter().any(|family| *name == family.0)
            })
            .map(|face| (face.id, face.post_script_name.clone(), face.weight))
            .collect::<SmallVec<[_; 4]>>();

        if families.is_empty() && !self.system_fonts_loaded {
            self.ensure_system_fonts_loaded();
            return self.load_family(
                &name,
                features,
                fallbacks,
                automatic_fallbacks,
                source_selection,
            );
        }

        let mut loaded_font_ids = SmallVec::new();
        for (font_id, postscript_name, weight) in families {
            if let Some(existing_font_id) = self.loaded_font_id(
                font_id,
                weight,
                features,
                default_cache_key_flags(),
                &user_fallback_chain,
            ) {
                loaded_font_ids.push(existing_font_id);
                continue;
            }

            let font = self
                .font_system
                .get_font(font_id, weight)
                .context("Could not load font")?;

            // HACK: To let the storybook run and render Windows caption icons. We should actually do better font fallback.
            let allowed_bad_font_names = [
                "SegoeFluentIcons", // NOTE: Segoe fluent icons postscript name is inconsistent
                "Segoe Fluent Icons",
            ];

            if font.as_swash().charmap().map('m') == 0
                && !allowed_bad_font_names.contains(&postscript_name.as_str())
                && !charmap_covers_system_text_sample(font.as_swash().charmap())
            {
                self.font_system.db_mut().remove_face(font.id());
                continue;
            };

            let font_id = FontId(self.loaded_fonts.len());
            loaded_font_ids.push(font_id);
            self.loaded_fonts.push(LoadedFont {
                font,
                source_features: features.clone(),
                features: features.try_into()?,
                weight,
                cache_key_flags: default_cache_key_flags(),
                is_known_emoji_font: check_is_known_emoji_font(&postscript_name),
                user_fallback_chain: Arc::clone(&user_fallback_chain),
            });
        }

        Ok(loaded_font_ids)
    }

    fn loaded_font_id(
        &self,
        database_id: cosmic_text::fontdb::ID,
        weight: Weight,
        features: &FontFeatures,
        cache_key_flags: CacheKeyFlags,
        user_fallback_chain: &Arc<[(FontId, SharedString)]>,
    ) -> Option<FontId> {
        self.loaded_fonts
            .iter()
            .enumerate()
            .find(|(_, loaded_font)| {
                loaded_font.font.id() == database_id
                    && loaded_font.weight == weight
                    && loaded_font.source_features == *features
                    && loaded_font.cache_key_flags == cache_key_flags
                    && loaded_font.user_fallback_chain.as_ref() == user_fallback_chain.as_ref()
            })
            .map(|(index, _)| FontId(index))
    }

    fn select_font_id(&mut self, candidates: &[FontId], font: &Font) -> Result<FontId> {
        let selection_weight = font.weight.into();
        let names = [Family::Name(font.family.as_ref())];
        let query = Query {
            families: &names,
            weight: selection_weight,
            stretch: Stretch::Normal,
            style: font.style.into(),
        };

        if let Some(database_id) = self.font_system.db().query(&query)
            && candidates
                .iter()
                .copied()
                .any(|font_id| self.loaded_font(font_id).font.id() == database_id)
        {
            return self.font_id_for_database_id(database_id, &font.features);
        }

        let closest = candidates
            .iter()
            .copied()
            .min_by_key(|font_id| {
                let database_id = self.loaded_font(*font_id).font.id();
                let face = self.font_system.db().face(database_id);
                face.map_or(u16::MAX, |face| {
                    face.weight.0.abs_diff(font.weight.0 as u16)
                })
            })
            .context("requested font family contains no font matching the other parameters")?;

        let database_id = self.loaded_font(closest).font.id();
        self.font_id_for_database_id(database_id, &font.features)
    }

    fn font_id_for_database_id(
        &mut self,
        database_id: cosmic_text::fontdb::ID,
        features: &FontFeatures,
    ) -> Result<FontId> {
        if let Some(index) = self.loaded_fonts.iter().position(|loaded_font| {
            loaded_font.font.id() == database_id && loaded_font.source_features == *features
        }) {
            return Ok(FontId(index));
        }

        let Some(face) = self.font_system.db().face(database_id) else {
            anyhow::bail!("font face not found");
        };
        let weight = face.weight;
        let post_script_name = face.post_script_name.clone();
        let font = self
            .font_system
            .get_font(database_id, weight)
            .context("Could not load font")?;
        let font_id = FontId(self.loaded_fonts.len());
        self.loaded_fonts.push(LoadedFont {
            font,
            source_features: features.clone(),
            features: features.try_into()?,
            weight,
            cache_key_flags: default_cache_key_flags(),
            is_known_emoji_font: check_is_known_emoji_font(&post_script_name),
            user_fallback_chain: Arc::from(Vec::new()),
        });
        Ok(font_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        let glyph_metrics = self.loaded_font(font_id).font.as_swash().glyph_metrics(&[]);
        Ok(Size {
            width: glyph_metrics.advance_width(glyph_id.0 as u16),
            height: glyph_metrics.advance_height(glyph_id.0 as u16),
        })
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let glyph_id = self.loaded_font(font_id).font.as_swash().charmap().map(ch);
        if glyph_id == 0 {
            None
        } else {
            Some(GlyphId(glyph_id.into()))
        }
    }

    fn glyph_cache_key(&self, params: &RenderGlyphParams) -> CacheKey {
        let loaded_font = self.loaded_font(params.font_id);
        let subpixel_shift = point(
            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor,
            params.subpixel_variant.y as f32 / SUBPIXEL_VARIANTS_Y as f32 / params.scale_factor,
        );

        CacheKey::new(
            loaded_font.font.id(),
            params.glyph_id.0 as u16,
            (params.font_size * params.scale_factor).0,
            (subpixel_shift.x, subpixel_shift.y.trunc()),
            loaded_font.weight,
            loaded_font.cache_key_flags,
        )
        .0
    }

    fn glyph_image(&mut self, params: &RenderGlyphParams) -> Option<SwashImage> {
        let cache_key = self.glyph_cache_key(params);
        self.swash_cache
            .get_image(&mut self.font_system, cache_key)
            .clone()
    }

    fn glyph_image_bounds(&mut self, params: &RenderGlyphParams) -> Option<Bounds<DevicePixels>> {
        let image = self.glyph_image(params)?;
        Some(swash_image_bounds(&image))
    }

    fn stable_cjk_raster_frame(&mut self, params: &RenderGlyphParams) -> CjkRasterFrame {
        let key = CjkFrameKey {
            font_id: params.font_id,
            font_size_bits: params.font_size.0.to_bits(),
            scale_factor_bits: params.scale_factor.to_bits(),
        };
        if let Some(bounds) = self.cjk_frame_bounds_cache.get(&key).copied() {
            return CjkRasterFrame {
                origin_y: bounds.origin.y,
                height: bounds.size.height,
            };
        }

        let metrics = self
            .loaded_font(params.font_id)
            .font
            .as_swash()
            .metrics(&[]);
        let scale = params.font_size.0 * params.scale_factor / metrics.units_per_em as f32;
        let top = (metrics.ascent * scale).ceil() as i32;
        let bottom = (metrics.descent * scale).floor() as i32;
        let height = (top - bottom).max(1);
        let mut bounds = Bounds {
            origin: point(DevicePixels(0), DevicePixels(-top)),
            size: size(DevicePixels(0), DevicePixels(height)),
        };

        for sample in CJK_FRAME_SAMPLE_CHARS {
            if let Some(glyph_id) = self.glyph_for_char(params.font_id, *sample) {
                let mut sample_params = params.clone();
                sample_params.glyph_id = glyph_id;
                sample_params.is_cjk = false;
                sample_params.is_emoji = false;
                if let Some(sample_bounds) = self.glyph_image_bounds(&sample_params) {
                    bounds = union_cjk_frame_bounds(sample_bounds, bounds);
                }
            }
        }

        match self.cjk_frame_bounds_cache.entry(key) {
            Entry::Occupied(entry) => {
                let bounds = *entry.get();
                CjkRasterFrame {
                    origin_y: bounds.origin.y,
                    height: bounds.size.height,
                }
            }
            Entry::Vacant(entry) => {
                let bounds = *entry.insert(bounds);
                CjkRasterFrame {
                    origin_y: bounds.origin.y,
                    height: bounds.size.height,
                }
            }
        }
    }

    fn raster_bounds(&mut self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        let Some(glyph_bounds) = self.glyph_image_bounds(params) else {
            return Ok(Bounds::default());
        };
        if params.is_cjk && !params.is_emoji {
            let frame = self.stable_cjk_raster_frame(params);
            Ok(apply_cjk_vertical_frame(glyph_bounds, frame))
        } else {
            Ok(glyph_bounds)
        }
    }

    #[profiling::function]
    fn rasterize_glyph(
        &mut self,
        params: &RenderGlyphParams,
        glyph_bounds: Bounds<DevicePixels>,
    ) -> Result<GlyphRasterization> {
        if glyph_bounds.size.width.0 == 0 || glyph_bounds.size.height.0 == 0 {
            anyhow::bail!("glyph bounds are empty");
        } else {
            let bitmap_size = glyph_bounds.size;
            let image = self.glyph_image(params).with_context(|| {
                let font = &self.loaded_fonts[params.font_id.0].font;
                format!("no image for {params:?} in font {font:?}")
            })?;
            let bytes = if params.is_emoji {
                swash_image_to_polychrome_bitmap(image, bitmap_size)?
            } else if params.is_cjk {
                swash_image_to_cjk_monochrome_mask(image, glyph_bounds)?
            } else {
                swash_image_to_monochrome_mask(image, bitmap_size)?
            };

            Ok(GlyphRasterization::Bitmap {
                size: bitmap_size,
                bytes,
            })
        }
    }

    /// This is used when cosmic_text has chosen a fallback font instead of using the requested
    /// font, typically to handle some unicode characters. When this happens, `loaded_fonts` may not
    /// yet have an entry for this fallback font, and so one is added.
    ///
    /// Note that callers shouldn't use this `FontId` somewhere that will retrieve the corresponding
    /// `LoadedFont.features`, as it will have an arbitrarily chosen or empty value. The only
    /// current use of this field is for the *input* of `layout_line`, and so it's fine to use
    /// `font_id_for_cosmic_id` when computing the *output* of `layout_line`.
    fn font_id_for_cosmic_id(
        &mut self,
        id: cosmic_text::fontdb::ID,
        weight: Weight,
        cache_key_flags: CacheKeyFlags,
    ) -> Option<FontId> {
        if let Some(ix) = self.loaded_fonts.iter().position(|loaded_font| {
            loaded_font.font.id() == id
                && loaded_font.source_features == FontFeatures::default()
                && loaded_font.weight == weight
                && loaded_font.cache_key_flags == cache_key_flags
        }) {
            Some(FontId(ix))
        } else {
            let face = self.font_system.db().face(id)?;
            let post_script_name = face.post_script_name.clone();
            let font = self.font_system.get_font(id, weight)?;

            let font_id = FontId(self.loaded_fonts.len());
            self.loaded_fonts.push(LoadedFont {
                font,
                source_features: FontFeatures::default(),
                features: CosmicFontFeatures::new(),
                weight,
                cache_key_flags,
                is_known_emoji_font: check_is_known_emoji_font(&post_script_name),
                user_fallback_chain: Arc::from(Vec::new()),
            });

            Some(font_id)
        }
    }

    #[profiling::function]
    fn layout_line(&mut self, text: &str, font_size: Pixels, font_runs: &[FontRun]) -> LineLayout {
        let mut attrs_list = AttrsList::new(&Attrs::new());
        let mut offs = 0;
        for run in font_runs {
            let run_end = offs + run.len;
            let loaded_font = self.loaded_font(run.font_id);
            let Some(font) = self.font_system.db().face(loaded_font.font.id()) else {
                log::warn!(
                    "font face not found in database for font_id {:?}",
                    run.font_id
                );
                offs = run_end;
                continue;
            };
            let Some(first_family) = font.families.first() else {
                log::warn!(
                    "font face has no family names for font_id {:?}",
                    run.font_id
                );
                offs = run_end;
                continue;
            };

            let primary_family_name: SharedString = first_family.0.clone().into();
            let primary_stretch = font.stretch;
            let primary_style = font.style;
            let primary_weight = font.weight;
            let primary_cache_key_flags = loaded_font.cache_key_flags;
            let primary_features = loaded_font.features.clone();
            let fallback_chain = Arc::clone(&loaded_font.user_fallback_chain);
            let fallback_trace_families = if std::env::var_os("GPUI_CJK_TEXT_TRACE").is_some()
                && !fallback_chain.is_empty()
            {
                Some(
                    fallback_chain
                        .iter()
                        .map(|(_, family)| family.to_string())
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            };

            let primary_attrs = Attrs::new()
                .metadata(run.font_id.0)
                .family(Family::Name(&primary_family_name))
                .stretch(primary_stretch)
                .style(primary_style)
                .weight(primary_weight)
                .cache_key_flags(primary_cache_key_flags)
                .font_features(primary_features.clone());
            let fallback_attrs: SmallVec<[Attrs<'_>; 4]> = fallback_chain
                .iter()
                .map(|(fallback_id, fallback_name)| {
                    Attrs::new()
                        .metadata(fallback_id.0)
                        .family(Family::Name(fallback_name))
                        .stretch(primary_stretch)
                        .style(primary_style)
                        .weight(primary_weight)
                        .cache_key_flags(primary_cache_key_flags)
                        .font_features(primary_features.clone())
                })
                .collect();

            let spans = if fallback_chain.is_empty() {
                let mut spans = SmallVec::<[RunSpan; 4]>::new();
                spans.push(RunSpan {
                    start: offs,
                    end: run_end,
                    slot: None,
                    font_id: run.font_id,
                });
                spans
            } else {
                let loaded_fonts = &self.loaded_fonts;
                let covers = |id: FontId, ch: char| charmap_covers(loaded_fonts, id, ch);
                compute_run_spans(text, offs, run.len, run.font_id, &fallback_chain, &covers)
            };

            for span in spans {
                let attrs = match span.slot {
                    None => &primary_attrs,
                    Some(index) => &fallback_attrs[index],
                };
                if let Some(fallback_trace_families) = fallback_trace_families.as_ref()
                    && span.slot.is_some()
                    && text.get(span.start..span.end).is_some_and(is_cjk_text)
                {
                    let fallback_family = span
                        .slot
                        .and_then(|slot| fallback_trace_families.get(slot))
                        .map(String::as_str)
                        .unwrap_or("<unknown>");
                    log::info!(
                        "gpui_cjk_text_trace: fallback_span text={:?} primary_family=\"{}\" fallback_family=\"{}\" fallback_font_id={:?}",
                        text.get(span.start..span.end).unwrap_or_default(),
                        primary_family_name,
                        fallback_family,
                        span.font_id,
                    );
                }
                attrs_list.add_span(span.start..span.end, attrs);
            }
            offs = run_end;
        }

        let line = ShapeLine::new(
            &mut self.font_system,
            text,
            &attrs_list,
            cosmic_text::Shaping::Advanced,
            4,
        );
        let mut layout_lines = Vec::with_capacity(1);
        line.layout_to_buffer(
            &mut self.scratch,
            font_size.0,
            None, // We do our own wrapping
            cosmic_text::Wrap::None,
            Ellipsize::None,
            None,
            &mut layout_lines,
            None,
            Hinting::default(),
        );
        let layout = layout_lines.first().unwrap();

        let mut runs: Vec<ShapedRun> =
            Vec::with_capacity(layout.glyphs.len().min(font_runs.len().max(1)));
        let trace_cjk_text = std::env::var_os("GPUI_CJK_TEXT_TRACE").is_some();
        for glyph in &layout.glyphs {
            let mut font_id = FontId(glyph.metadata);
            let mut loaded_font = self.loaded_font(font_id);
            if loaded_font.font.id() != glyph.font_id
                || loaded_font.weight != glyph.font_weight
                || loaded_font.cache_key_flags != glyph.cache_key_flags
            {
                let Some(resolved_font_id) = self.font_id_for_cosmic_id(
                    glyph.font_id,
                    glyph.font_weight,
                    glyph.cache_key_flags,
                ) else {
                    continue;
                };
                font_id = resolved_font_id;
                loaded_font = self.loaded_font(font_id);
            }
            let is_emoji = loaded_font.is_known_emoji_font;

            // HACK: Prevent crash caused by variation selectors.
            if glyph.glyph_id == 3 && is_emoji {
                continue;
            }
            let is_cjk = is_cjk_text(text.get(glyph.start..glyph.end).unwrap_or_default());
            if trace_cjk_text
                && is_cjk
                && let Some(face) = self.font_system.db().face(loaded_font.font.id())
            {
                let family = face
                    .families
                    .first()
                    .map(|family| family.0.as_str())
                    .unwrap_or("<unknown>");
                log::info!(
                    "gpui_cjk_text_trace: text={:?} glyph_id={} font_id={:?} family=\"{}\" postscript=\"{}\" weight={:?} face_id={:?} layout_x={} layout_y={} glyph_font_size={}",
                    text.get(glyph.start..glyph.end).unwrap_or_default(),
                    glyph.glyph_id,
                    font_id,
                    family,
                    face.post_script_name,
                    face.weight,
                    face.id,
                    glyph.x,
                    glyph.y,
                    glyph.font_size,
                );
            }

            let shaped_glyph = ShapedGlyph {
                id: GlyphId(glyph.glyph_id as u32),
                position: point(glyph.x.into(), glyph.y.into()),
                render_offset: Point::default(),
                font_size: glyph.font_size.into(),
                index: glyph.start,
                is_emoji,
                is_cjk,
            };

            if let Some(last_run) = runs
                .last_mut()
                .filter(|last_run| last_run.font_id == font_id)
            {
                last_run.glyphs.push(shaped_glyph);
            } else {
                runs.push(ShapedRun {
                    font_id,
                    glyphs: vec![shaped_glyph],
                });
            }
        }

        let layout = LineLayout {
            font_size,
            width: layout.w.into(),
            ascent: layout.max_ascent.into(),
            descent: layout.max_descent.into(),
            runs,
            len: text.len(),
        };
        layout
    }
}

impl TryFrom<&FontFeatures> for CosmicFontFeatures {
    type Error = anyhow::Error;

    fn try_from(features: &FontFeatures) -> Result<Self> {
        let mut result = CosmicFontFeatures::new();
        for feature in features.0.iter() {
            let name_bytes: [u8; 4] = feature
                .0
                .as_bytes()
                .try_into()
                .context("Incorrect feature flag format")?;

            let tag = cosmic_text::FeatureTag::new(&name_bytes);

            result.set(tag, feature.1);
        }
        Ok(result)
    }
}

impl From<FontWeight> for cosmic_text::Weight {
    fn from(value: FontWeight) -> Self {
        cosmic_text::Weight(value.0 as u16)
    }
}

impl From<FontStyle> for cosmic_text::Style {
    fn from(style: FontStyle) -> Self {
        match style {
            FontStyle::Normal => cosmic_text::Style::Normal,
            FontStyle::Italic => cosmic_text::Style::Italic,
            FontStyle::Oblique => cosmic_text::Style::Oblique,
        }
    }
}

fn check_is_known_emoji_font(postscript_name: &str) -> bool {
    // TODO: Include other common emoji fonts
    matches!(
        postscript_name,
        "NotoColorEmoji" | "SegoeUIEmoji" | "AppleColorEmoji"
    )
}

fn default_cache_key_flags() -> CacheKeyFlags {
    CacheKeyFlags::empty()
}

fn font_bytes_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

const CJK_FRAME_SAMPLE_CHARS: &[char] = &[
    '\u{56fe}', '\u{8d44}', '\u{6e90}', '\u{5305}', '\u{6982}', '\u{89c8}', '\u{5bfc}', '\u{822a}',
    '\u{590d}', '\u{5236}', '\u{8def}', '\u{5f84}', '\u{65e5}', '\u{672c}', '\u{97e9}', '\u{9ad8}',
    '\u{4e2d}', '\u{6587}', '\u{56fd}', '\u{95e8}',
];

fn swash_image_bounds(image: &SwashImage) -> Bounds<DevicePixels> {
    Bounds {
        origin: point(image.placement.left.into(), (-image.placement.top).into()),
        size: size(image.placement.width.into(), image.placement.height.into()),
    }
}

fn union_cjk_frame_bounds(
    glyph_bounds: Bounds<DevicePixels>,
    frame_bounds: Bounds<DevicePixels>,
) -> Bounds<DevicePixels> {
    let origin = point(
        glyph_bounds.origin.x.min(frame_bounds.origin.x),
        glyph_bounds.origin.y.min(frame_bounds.origin.y),
    );
    let right = (glyph_bounds.origin.x.0 + glyph_bounds.size.width.0)
        .max(frame_bounds.origin.x.0 + frame_bounds.size.width.0);
    let bottom = (glyph_bounds.origin.y.0 + glyph_bounds.size.height.0)
        .max(frame_bounds.origin.y.0 + frame_bounds.size.height.0);
    Bounds {
        origin,
        size: size(
            DevicePixels((right - origin.x.0).max(1)),
            DevicePixels((bottom - origin.y.0).max(1)),
        ),
    }
}

fn apply_cjk_vertical_frame(
    glyph_bounds: Bounds<DevicePixels>,
    frame: CjkRasterFrame,
) -> Bounds<DevicePixels> {
    let top = glyph_bounds.origin.y.min(frame.origin_y);
    let bottom = (glyph_bounds.origin.y.0 + glyph_bounds.size.height.0)
        .max(frame.origin_y.0 + frame.height.0);
    Bounds {
        origin: point(glyph_bounds.origin.x, top),
        size: size(
            glyph_bounds.size.width,
            DevicePixels((bottom - top.0).max(1)),
        ),
    }
}

fn is_cjk_text(text: &str) -> bool {
    text.chars().any(is_cjk_char)
}

fn is_cjk_char(character: char) -> bool {
    matches!(
        character as u32,
        0x2E80..=0x2EFF
            | 0x2F00..=0x2FDF
            | 0x3000..=0x303F
            | 0x3040..=0x30FF
            | 0x3100..=0x312F
            | 0x3130..=0x318F
            | 0x31A0..=0x31BF
            | 0x31C0..=0x31EF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0x2CEB0..=0x2EBEF
            | 0x30000..=0x3134F
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RunSpan {
    start: usize,
    end: usize,
    slot: Option<usize>,
    font_id: FontId,
}

fn compute_run_spans(
    text: &str,
    run_offset: usize,
    run_len: usize,
    primary: FontId,
    fallback_chain: &[(FontId, SharedString)],
    covers: &impl Fn(FontId, char) -> bool,
) -> SmallVec<[RunSpan; 4]> {
    let mut spans = SmallVec::new();
    let run_end = run_offset + run_len;
    if run_end <= run_offset {
        return spans;
    }
    if fallback_chain.is_empty() {
        spans.push(RunSpan {
            start: run_offset,
            end: run_end,
            slot: None,
            font_id: primary,
        });
        return spans;
    }

    let run_text = &text[run_offset..run_end];
    let mut span_start = run_offset;
    let mut span_slot = None;
    let mut span_font_id = primary;

    for (grapheme_index, grapheme) in run_text.grapheme_indices(true) {
        let absolute_index = run_offset + grapheme_index;
        let character = grapheme.chars().next().unwrap_or('\0');
        let next_slot = pick_covering_slot(character, span_slot, primary, fallback_chain, covers);
        if next_slot == span_slot {
            continue;
        }
        if absolute_index > span_start {
            spans.push(RunSpan {
                start: span_start,
                end: absolute_index,
                slot: span_slot,
                font_id: span_font_id,
            });
        }
        span_start = absolute_index;
        span_slot = next_slot;
        span_font_id = slot_font_id(next_slot, primary, fallback_chain);
    }

    if span_start < run_end {
        spans.push(RunSpan {
            start: span_start,
            end: run_end,
            slot: span_slot,
            font_id: span_font_id,
        });
    }

    spans
}

fn slot_font_id(
    slot: Option<usize>,
    primary: FontId,
    fallback_chain: &[(FontId, SharedString)],
) -> FontId {
    match slot {
        None => primary,
        Some(index) => fallback_chain[index].0,
    }
}

fn pick_covering_slot(
    character: char,
    current: Option<usize>,
    primary: FontId,
    fallback_chain: &[(FontId, SharedString)],
    covers: &impl Fn(FontId, char) -> bool,
) -> Option<usize> {
    if (character as u32) <= 0x7F || covers(primary, character) {
        return None;
    }
    let current_id = slot_font_id(current, primary, fallback_chain);
    if covers(current_id, character) {
        return current;
    }
    for (index, (fallback_id, _)) in fallback_chain.iter().enumerate() {
        if covers(*fallback_id, character) {
            return Some(index);
        }
    }
    None
}

fn charmap_covers(loaded_fonts: &[LoadedFont], id: FontId, character: char) -> bool {
    loaded_fonts
        .get(id.0)
        .is_some_and(|loaded_font| loaded_font.font.as_swash().charmap().map(character) != 0)
}

fn charmap_covers_system_text_sample(charmap: swash::Charmap<'_>) -> bool {
    SYSTEM_FALLBACK_COVERAGE_SAMPLE
        .iter()
        .any(|(character, _)| !character.is_ascii() && charmap.map(*character) != 0)
}

fn best_system_text_fallback_face(font_system: &FontSystem) -> Option<SystemFallbackFace> {
    font_system
        .db()
        .faces()
        .filter(|face| FontSourceSelection::SystemOnly.matches(&face.source))
        .filter(|face| !check_is_known_emoji_font(&face.post_script_name))
        .filter(|face| !is_icon_font_name(&face.post_script_name))
        .filter_map(|face| {
            let score = system_text_coverage_score(font_system, face);
            let family = face.families.first()?.0.clone();
            (score > 0).then_some((score, normal_weight_distance(face.weight), family, face.id))
        })
        .max_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| right.2.cmp(&left.2))
        })
        .map(|(score, _, family, database_id)| SystemFallbackFace {
            database_id,
            family: SharedString::from(family),
            score,
        })
}

fn system_text_coverage_score(
    font_system: &FontSystem,
    face: &cosmic_text::fontdb::FaceInfo,
) -> u32 {
    if !FontSourceSelection::SystemOnly.matches(&face.source)
        || check_is_known_emoji_font(&face.post_script_name)
        || is_icon_font_name(&face.post_script_name)
    {
        return 0;
    }

    font_system
        .db()
        .with_face_data(face.id, |data, index| {
            let Some(font) = swash::FontRef::from_index(data, index as usize) else {
                return 0;
            };
            let charmap = font.charmap();
            let mut score = 0;
            for (character, weight) in SYSTEM_FALLBACK_COVERAGE_SAMPLE {
                if charmap.map(*character) != 0 {
                    score += *weight;
                }
            }
            score
        })
        .unwrap_or(0)
}

fn normal_weight_distance(weight: Weight) -> u16 {
    weight.0.abs_diff(Weight::NORMAL.0)
}

fn is_icon_font_name(postscript_name: &str) -> bool {
    let name = postscript_name.to_ascii_lowercase();
    name.contains("icon")
        || name.contains("symbol")
        || name.contains("emoji")
        || name.contains("codicon")
        || name.contains("fluent")
        || name.contains("material")
        || name.contains("awesome")
}

const SYSTEM_FALLBACK_COVERAGE_SAMPLE: &[(char, u32)] = &[
    ('A', 1),
    ('z', 1),
    ('0', 1),
    (' ', 1),
    ('-', 1),
    ('，', 2),
    ('。', 2),
    ('、', 2),
    ('图', 20),
    ('资', 20),
    ('源', 20),
    ('包', 20),
    ('概', 20),
    ('览', 20),
    ('导', 20),
    ('航', 20),
    ('復', 8),
    ('日', 8),
    ('本', 8),
    ('한', 8),
    ('글', 8),
    ('あ', 8),
    ('ア', 8),
];

fn swash_image_to_monochrome_mask(
    image: SwashImage,
    bitmap_size: Size<DevicePixels>,
) -> Result<Vec<u8>> {
    let pixel_count = glyph_pixel_count(bitmap_size)?;
    match image.content {
        SwashContent::Mask => {
            anyhow::ensure!(
                image.data.len() == pixel_count,
                "monochrome glyph mask has {} bytes for {} pixels",
                image.data.len(),
                pixel_count
            );
            Ok(image.data)
        }
        SwashContent::SubpixelMask => rgba_mask_to_alpha(image.data, pixel_count),
        SwashContent::Color => rgba_color_to_alpha(image.data, pixel_count),
    }
}

fn swash_image_to_cjk_monochrome_mask(
    image: SwashImage,
    frame_bounds: Bounds<DevicePixels>,
) -> Result<Vec<u8>> {
    let glyph_bounds = swash_image_bounds(&image);
    let glyph_mask = swash_image_to_monochrome_mask(image, glyph_bounds.size)?;
    copy_mask_to_frame(glyph_mask, glyph_bounds, frame_bounds)
}

fn copy_mask_to_frame(
    source: Vec<u8>,
    source_bounds: Bounds<DevicePixels>,
    frame_bounds: Bounds<DevicePixels>,
) -> Result<Vec<u8>> {
    let source_width =
        usize::try_from(source_bounds.size.width.0).context("invalid source glyph bitmap width")?;
    let source_height = usize::try_from(source_bounds.size.height.0)
        .context("invalid source glyph bitmap height")?;
    let frame_width =
        usize::try_from(frame_bounds.size.width.0).context("invalid CJK frame bitmap width")?;
    let frame_height =
        usize::try_from(frame_bounds.size.height.0).context("invalid CJK frame bitmap height")?;
    let expected_source_len = source_width
        .checked_mul(source_height)
        .context("source glyph bitmap is too large")?;
    anyhow::ensure!(
        source.len() == expected_source_len,
        "source glyph mask has {} bytes for {} pixels",
        source.len(),
        expected_source_len
    );

    let mut frame = vec![
        0;
        frame_width
            .checked_mul(frame_height)
            .context("CJK frame bitmap is too large")?
    ];
    let offset_x = source_bounds.origin.x.0 - frame_bounds.origin.x.0;
    let offset_y = source_bounds.origin.y.0 - frame_bounds.origin.y.0;
    anyhow::ensure!(
        offset_x >= 0
            && offset_y >= 0
            && offset_x + source_width as i32 <= frame_width as i32
            && offset_y + source_height as i32 <= frame_height as i32,
        "CJK frame {:?} does not contain source glyph bounds {:?}",
        frame_bounds,
        source_bounds
    );

    for source_y in 0..source_height {
        let target_y = offset_y + source_y as i32;
        for source_x in 0..source_width {
            let target_x = offset_x + source_x as i32;
            let source_index = source_y * source_width + source_x;
            let target_index = target_y as usize * frame_width + target_x as usize;
            frame[target_index] = source[source_index];
        }
    }

    Ok(frame)
}

fn swash_image_to_polychrome_bitmap(
    image: SwashImage,
    bitmap_size: Size<DevicePixels>,
) -> Result<Vec<u8>> {
    let pixel_count = glyph_pixel_count(bitmap_size)?;
    match image.content {
        SwashContent::Color => rgba_to_bgra(image.data, pixel_count),
        SwashContent::Mask => alpha_mask_to_bgra(image.data, pixel_count),
        SwashContent::SubpixelMask => {
            let alpha = rgba_mask_to_alpha(image.data, pixel_count)?;
            alpha_mask_to_bgra(alpha, pixel_count)
        }
    }
}

fn glyph_pixel_count(bitmap_size: Size<DevicePixels>) -> Result<usize> {
    let width = usize::try_from(bitmap_size.width.0).context("invalid glyph bitmap width")?;
    let height = usize::try_from(bitmap_size.height.0).context("invalid glyph bitmap height")?;
    width
        .checked_mul(height)
        .context("glyph bitmap is too large")
}

fn rgba_to_bgra(mut bytes: Vec<u8>, pixel_count: usize) -> Result<Vec<u8>> {
    ensure_rgba_len(bytes.len(), pixel_count, "color glyph bitmap")?;
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(bytes)
}

fn rgba_color_to_alpha(bytes: Vec<u8>, pixel_count: usize) -> Result<Vec<u8>> {
    ensure_rgba_len(bytes.len(), pixel_count, "color glyph bitmap")?;
    Ok(bytes.chunks_exact(4).map(|pixel| pixel[3]).collect())
}

fn rgba_mask_to_alpha(bytes: Vec<u8>, pixel_count: usize) -> Result<Vec<u8>> {
    ensure_rgba_len(bytes.len(), pixel_count, "subpixel glyph mask")?;
    Ok(bytes
        .chunks_exact(4)
        .map(|pixel| pixel[0].max(pixel[1]).max(pixel[2]).max(pixel[3]))
        .collect())
}

fn alpha_mask_to_bgra(bytes: Vec<u8>, pixel_count: usize) -> Result<Vec<u8>> {
    anyhow::ensure!(
        bytes.len() == pixel_count,
        "glyph alpha mask has {} bytes for {} pixels",
        bytes.len(),
        pixel_count
    );
    let mut output = Vec::with_capacity(pixel_count.saturating_mul(4));
    for alpha in bytes {
        output.extend_from_slice(&[0, 0, 0, alpha]);
    }
    Ok(output)
}

fn ensure_rgba_len(actual_len: usize, pixel_count: usize, description: &str) -> Result<()> {
    let expected_len = pixel_count
        .checked_mul(4)
        .context("glyph bitmap is too large")?;
    anyhow::ensure!(
        actual_len == expected_len,
        "{description} has {actual_len} bytes for {pixel_count} pixels"
    );
    Ok(())
}

#[cfg(target_os = "windows")]
fn platform_system_font_name_impl() -> String {
    let mut info = LOGFONTW::default();
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETICONTITLELOGFONT,
            std::mem::size_of::<LOGFONTW>() as u32,
            Some(&mut info as *mut _ as _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };

    if result.is_ok() {
        let font_name = String::from_utf16_lossy(&info.lfFaceName);
        let font_name = font_name.trim_matches(char::from(0)).trim();
        if !font_name.is_empty() {
            return font_name.to_owned();
        }
    }

    "Microsoft YaHei UI".to_owned()
}

#[cfg(target_os = "macos")]
fn platform_system_font_name_impl() -> String {
    ".AppleSystemUIFont".to_owned()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_system_font_name_impl() -> String {
    "IBM Plex Sans".to_owned()
}

fn resolved_platform_font_name(font_system: &FontSystem) -> String {
    let preferred = platform_system_font_name_impl();
    if font_family_exists(font_system, &preferred) {
        return preferred;
    }

    for fallback in platform_font_fallbacks() {
        if font_family_exists(font_system, fallback) {
            return (*fallback).to_owned();
        }
    }

    preferred
}

fn font_family_exists(font_system: &FontSystem, family_name: &str) -> bool {
    font_system
        .db()
        .faces()
        .any(|face| face.families.iter().any(|family| family.0 == family_name))
}

fn minimal_startup_font_system() -> (FontSystem, bool) {
    #[cfg(target_os = "windows")]
    {
        let locale = String::from("en-US");
        let mut db = cosmic_text::fontdb::Database::new();
        for path in windows_startup_font_paths() {
            let _ = db.load_font_file(path);
        }
        return (FontSystem::new_with_locale_and_db(locale, db), false);
    }

    #[cfg(not(target_os = "windows"))]
    {
        (FontSystem::new(), true)
    }
}

#[cfg(target_os = "windows")]
fn platform_font_fallbacks() -> &'static [&'static str] {
    &["Segoe UI", "Arial", "Microsoft YaHei UI", "Microsoft YaHei"]
}

#[cfg(target_os = "windows")]
fn windows_startup_font_paths() -> &'static [&'static str] {
    &[
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\segoeuib.ttf",
        "C:\\Windows\\Fonts\\segoeuii.ttf",
        "C:\\Windows\\Fonts\\segoeuiz.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
        "C:\\Windows\\Fonts\\ariali.ttf",
        "C:\\Windows\\Fonts\\arialbi.ttf",
    ]
}

#[cfg(target_os = "macos")]
fn platform_font_fallbacks() -> &'static [&'static str] {
    &[".AppleSystemUIFont", "Helvetica Neue", "Arial"]
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_font_fallbacks() -> &'static [&'static str] {
    &[
        "IBM Plex Sans",
        "Noto Sans",
        "Adwaita Sans",
        "Cantarell",
        "DejaVu Sans",
        "Arial",
    ]
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use std::hash::{Hash, Hasher};

    use super::*;
    use crate::{FontWeight, Point, RenderGlyphParams, font, px};

    #[test]
    fn cjk_layout_uses_common_horizontal_baseline() {
        let text_system = CosmicTextSystem::new();
        let text = "\u{6a21}\u{5757}\u{8d44}\u{6e90}\u{5730}\u{56fe}\u{622a}\u{56fe}";

        for weight in [FontWeight::NORMAL, FontWeight::SEMIBOLD, FontWeight::BOLD] {
            let mut font = font(".SystemUIFont");
            font.weight = weight;
            let font_id = text_system.font_id(&font).expect("system font loads");
            for font_size in [px(12.), px(18.), px(36.)] {
                let layout = text_system.layout_line(
                    text,
                    font_size,
                    &[FontRun {
                        len: text.len(),
                        font_id,
                    }],
                );
                let mut glyph_count = 0;
                let mut baseline = None;

                for run in &layout.runs {
                    for glyph in &run.glyphs {
                        glyph_count += 1;
                        let glyph_baseline = glyph.position.y + glyph.render_offset.y;
                        let expected_baseline = match baseline {
                            Some(expected_baseline) => expected_baseline,
                            None => *baseline.insert(glyph_baseline),
                        };
                        assert_eq!(
                            glyph_baseline, expected_baseline,
                            "glyph {:?} drifted from the common baseline at size {:?} and weight {:?}",
                            glyph.id, font_size, weight
                        );
                        assert_eq!(
                            glyph.render_offset,
                            Point::default(),
                            "glyph {:?} used an unexpected render offset at size {:?} and weight {:?}",
                            glyph.id,
                            font_size,
                            weight
                        );
                    }
                }

                assert!(glyph_count > 0);
            }
        }
    }

    #[test]
    fn cjk_glyphs_have_non_empty_raster_bounds_at_larger_sizes() {
        let text_system = CosmicTextSystem::new();
        let text = "\u{4e2d}\u{6587}\u{5b57}\u{5f62}\u{6e32}\u{67d3}";

        for weight in [FontWeight::NORMAL, FontWeight::SEMIBOLD, FontWeight::BOLD] {
            let mut font = font(".SystemUIFont");
            font.weight = weight;
            let font_id = text_system.font_id(&font).expect("system font loads");
            let runs = [FontRun {
                len: text.len(),
                font_id,
            }];

            for font_size in [px(12.), px(18.), px(36.), px(48.)] {
                let layout = text_system.layout_line(text, font_size, &runs);
                let mut visible_glyph_count = 0;

                for run in &layout.runs {
                    for glyph in &run.glyphs {
                        if glyph.is_emoji {
                            continue;
                        }

                        visible_glyph_count += 1;
                        let params = RenderGlyphParams {
                            font_id: run.font_id,
                            glyph_id: glyph.id,
                            font_size: glyph.font_size,
                            subpixel_variant: Point::default(),
                            scale_factor: 1.0,
                            is_emoji: false,
                            is_cjk: glyph.is_cjk,
                        };
                        let bounds = text_system
                            .glyph_raster_bounds(&params)
                            .expect("glyph raster bounds");

                        assert!(
                            bounds.size.width.0 > 0 && bounds.size.height.0 > 0,
                            "empty raster bounds for glyph {:?} at size {:?} and weight {:?}",
                            glyph.id,
                            font_size,
                            weight
                        );
                    }
                }

                assert!(visible_glyph_count > 0);
            }
        }
    }

    #[test]
    fn cjk_glyphs_use_common_vertical_raster_frame() {
        let text_system = CosmicTextSystem::new();
        let text = "\u{56fe}\u{8d44}\u{6e90}\u{5305}\u{6982}\u{89c8}\u{5bfc}\u{822a}";

        for weight in [FontWeight::NORMAL, FontWeight::SEMIBOLD, FontWeight::BOLD] {
            let mut font = font(".SystemUIFont");
            font.weight = weight;
            let font_id = text_system.font_id(&font).expect("system font loads");
            let runs = [FontRun {
                len: text.len(),
                font_id,
            }];

            for font_size in [px(12.), px(13.), px(18.)] {
                let layout = text_system.layout_line(text, font_size, &runs);
                let mut expected_origin_y = None;
                let mut expected_height = None;
                let mut glyph_count = 0;

                for run in &layout.runs {
                    for glyph in &run.glyphs {
                        assert!(glyph.is_cjk, "test glyph should be marked as CJK");
                        glyph_count += 1;
                        let params = RenderGlyphParams {
                            font_id: run.font_id,
                            glyph_id: glyph.id,
                            font_size: glyph.font_size,
                            subpixel_variant: Point::default(),
                            scale_factor: 1.0,
                            is_emoji: false,
                            is_cjk: true,
                        };
                        let bounds = text_system
                            .glyph_raster_bounds(&params)
                            .expect("glyph raster bounds");
                        let origin_y = match expected_origin_y {
                            Some(origin_y) => origin_y,
                            None => *expected_origin_y.insert(bounds.origin.y),
                        };
                        let height = match expected_height {
                            Some(height) => height,
                            None => *expected_height.insert(bounds.size.height),
                        };

                        assert_eq!(
                            bounds.origin.y, origin_y,
                            "CJK glyph {:?} used an unstable raster origin at size {:?} and weight {:?}",
                            glyph.id, font_size, weight
                        );
                        assert_eq!(
                            bounds.size.height, height,
                            "CJK glyph {:?} used an unstable raster height at size {:?} and weight {:?}",
                            glyph.id, font_size, weight
                        );
                    }
                }

                assert!(glyph_count > 0);
            }
        }
    }

    #[test]
    fn cjk_stable_frame_contains_exact_swash_bounds() {
        let text_system = CosmicTextSystem::new();
        let text = "\u{56fe}\u{8d44}\u{6e90}\u{5305}\u{6982}\u{89c8}\u{5bfc}\u{822a}";
        let font_id = text_system
            .font_id(&font(".SystemUIFont"))
            .expect("system font loads");
        let layout = text_system.layout_line(
            text,
            px(13.),
            &[FontRun {
                len: text.len(),
                font_id,
            }],
        );

        for run in &layout.runs {
            for glyph in &run.glyphs {
                let cjk_params = RenderGlyphParams {
                    font_id: run.font_id,
                    glyph_id: glyph.id,
                    font_size: glyph.font_size,
                    subpixel_variant: Point::default(),
                    scale_factor: 1.0,
                    is_emoji: false,
                    is_cjk: true,
                };
                let exact_params = RenderGlyphParams {
                    is_cjk: false,
                    ..cjk_params.clone()
                };
                let exact_bounds = text_system
                    .glyph_raster_bounds(&exact_params)
                    .expect("exact glyph raster bounds");
                let frame_bounds = text_system
                    .glyph_raster_bounds(&cjk_params)
                    .expect("CJK frame raster bounds");

                assert!(
                    frame_bounds.origin.y.0 <= exact_bounds.origin.y.0
                        && frame_bounds.origin.y.0 + frame_bounds.size.height.0
                            >= exact_bounds.origin.y.0 + exact_bounds.size.height.0,
                    "CJK frame {:?} clipped exact glyph bounds {:?}",
                    frame_bounds,
                    exact_bounds
                );
                assert_eq!(
                    frame_bounds.origin.x, exact_bounds.origin.x,
                    "CJK frame should keep the exact glyph x origin to avoid widening the sample region"
                );
                assert_eq!(
                    frame_bounds.size.width, exact_bounds.size.width,
                    "CJK frame should keep the exact glyph width to avoid horizontal halo"
                );
            }
        }
    }

    #[test]
    fn cjk_raster_key_is_distinct_from_non_cjk_key() {
        let mut non_cjk = RenderGlyphParams {
            font_id: FontId(0),
            glyph_id: GlyphId(42),
            font_size: px(13.),
            subpixel_variant: Point::default(),
            scale_factor: 1.0,
            is_emoji: false,
            is_cjk: false,
        };
        let cjk = {
            non_cjk.is_cjk = true;
            non_cjk.clone()
        };
        non_cjk.is_cjk = false;

        assert_ne!(non_cjk, cjk);
        let mut non_cjk_hasher = std::collections::hash_map::DefaultHasher::new();
        let mut cjk_hasher = std::collections::hash_map::DefaultHasher::new();
        non_cjk.hash(&mut non_cjk_hasher);
        cjk.hash(&mut cjk_hasher);
        assert_ne!(non_cjk_hasher.finish(), cjk_hasher.finish());
    }

    #[test]
    fn run_spans_use_primary_when_primary_covers_cjk() {
        let primary = FontId(0);
        let fallbacks = [(FontId(1), SharedString::from("Fallback"))];
        let covers = |font_id: FontId, character: char| {
            font_id == primary || (font_id == FontId(1) && character == '\u{56fe}')
        };

        let spans = compute_run_spans(
            "\u{56fe}",
            0,
            "\u{56fe}".len(),
            primary,
            &fallbacks,
            &covers,
        );

        assert_eq!(
            spans.as_slice(),
            &[RunSpan {
                start: 0,
                end: "\u{56fe}".len(),
                slot: None,
                font_id: primary,
            }]
        );
    }

    #[test]
    fn run_spans_move_cjk_to_configured_fallback() {
        let primary = FontId(0);
        let fallback = FontId(1);
        let fallbacks = [(fallback, SharedString::from("Fallback"))];
        let text = "abc\u{56fe}\u{8d44}def";
        let covers = |font_id: FontId, character: char| {
            font_id == primary && character.is_ascii()
                || font_id == fallback && !character.is_ascii()
        };

        let spans = compute_run_spans(text, 0, text.len(), primary, &fallbacks, &covers);

        assert_eq!(
            spans.as_slice(),
            &[
                RunSpan {
                    start: 0,
                    end: 3,
                    slot: None,
                    font_id: primary,
                },
                RunSpan {
                    start: 3,
                    end: 9,
                    slot: Some(0),
                    font_id: fallback,
                },
                RunSpan {
                    start: 9,
                    end: 12,
                    slot: None,
                    font_id: primary,
                },
            ]
        );
    }

    #[test]
    fn run_spans_keep_combining_marks_with_base() {
        let primary = FontId(0);
        let fallback = FontId(1);
        let fallbacks = [(fallback, SharedString::from("Fallback"))];
        let text = "\u{00e9}\u{0301}";
        let covers =
            |font_id: FontId, character: char| font_id == fallback && character == '\u{00e9}';

        let spans = compute_run_spans(text, 0, text.len(), primary, &fallbacks, &covers);

        assert_eq!(
            spans.as_slice(),
            &[RunSpan {
                start: 0,
                end: text.len(),
                slot: Some(0),
                font_id: fallback,
            }]
        );
    }

    #[test]
    fn run_spans_keep_emoji_zwj_sequence_together() {
        let primary = FontId(0);
        let fallback = FontId(1);
        let fallbacks = [(fallback, SharedString::from("Emoji"))];
        let text = "\u{1f469}\u{200d}\u{1f4bb}";
        let covers =
            |font_id: FontId, character: char| font_id == fallback && character == '\u{1f469}';

        let spans = compute_run_spans(text, 0, text.len(), primary, &fallbacks, &covers);

        assert_eq!(
            spans.as_slice(),
            &[RunSpan {
                start: 0,
                end: text.len(),
                slot: Some(0),
                font_id: fallback,
            }]
        );
    }

    #[test]
    fn layout_line_uses_configured_fallback_for_cjk_when_primary_lacks_coverage() {
        let text_system = CosmicTextSystem::new();
        let mut primary = font("Segoe UI");
        primary.fallbacks = Some(FontFallbacks::from_fonts(vec!["Microsoft YaHei UI".into()]));
        let fallback = font("Microsoft YaHei UI");
        let primary_id = text_system.font_id(&primary).expect("primary font loads");
        let fallback_id = text_system.font_id(&fallback).expect("fallback font loads");

        if text_system.glyph_for_char(primary_id, '\u{56fe}').is_some()
            || text_system
                .glyph_for_char(fallback_id, '\u{56fe}')
                .is_none()
        {
            return;
        }

        let text = "A\u{56fe}B";
        let layout = text_system.layout_line(
            text,
            px(13.),
            &[FontRun {
                len: text.len(),
                font_id: primary_id,
            }],
        );

        assert!(
            layout.runs.iter().any(|run| run.font_id == fallback_id),
            "configured fallback was not used for CJK text"
        );
    }

    #[test]
    fn explicit_user_fallbacks_override_system_global_fallback() {
        let text_system = CosmicTextSystem::new();
        let mut primary = font("Segoe UI");
        primary.fallbacks = Some(FontFallbacks::from_fonts(vec!["Microsoft YaHei UI".into()]));
        let primary_id = text_system.font_id(&primary).expect("primary font loads");
        let fallback_id = text_system
            .font_id(&font("Microsoft YaHei UI"))
            .expect("fallback font loads");

        let state = text_system.0.read();
        let loaded = state.loaded_font(primary_id);
        if loaded.font.as_swash().charmap().map('图') != 0 {
            return;
        }

        assert_eq!(
            loaded.user_fallback_chain.as_ref(),
            &[(fallback_id, SharedString::from("Microsoft YaHei UI"))]
        );
    }

    #[test]
    fn platform_ui_font_is_preferred_for_missing_cjk_when_it_covers_text() {
        let text_system = CosmicTextSystem::new();
        let primary_id = text_system
            .font_id(&font("Segoe UI"))
            .expect("primary font loads");
        let platform_id = text_system
            .font_id(&font(".SystemUIFont"))
            .expect("platform UI font loads");

        let state = text_system.0.read();
        if state
            .loaded_font(primary_id)
            .font
            .as_swash()
            .charmap()
            .map('图')
            != 0
            || state
                .loaded_font(platform_id)
                .font
                .as_swash()
                .charmap()
                .map('图')
                == 0
        {
            return;
        }

        let loaded = state.loaded_font(primary_id);
        assert_eq!(
            loaded
                .user_fallback_chain
                .first()
                .map(|(font_id, _)| *font_id),
            Some(platform_id)
        );
    }

    #[test]
    fn coverage_scored_system_fallback_is_used_when_platform_ui_font_does_not_cover_text() {
        let mut text_system = CosmicTextSystem::new();
        {
            let mut state = text_system.0.write();
            state.platform_font_family = "Segoe UI".into();
            state.font_system.db_mut().set_sans_serif_family("Segoe UI");
            state.font_ids_by_family_cache.clear();
            state.loaded_fonts.clear();
        }

        let primary_id = text_system
            .font_id(&font("Segoe UI"))
            .expect("primary font loads");
        let mut state = text_system.0.write();
        if state
            .loaded_font(primary_id)
            .font
            .as_swash()
            .charmap()
            .map('图')
            != 0
        {
            return;
        }
        let Some(best_face) = state.system_coverage_fallback_face() else {
            return;
        };

        let loaded = state.loaded_font(primary_id);
        let chain_debug = loaded
            .user_fallback_chain
            .iter()
            .map(|(font_id, family)| {
                let supports = state
                    .loaded_font(*font_id)
                    .font
                    .as_swash()
                    .charmap()
                    .map('图')
                    != 0;
                format!("{family}:{font_id:?}:supports_图={supports}")
            })
            .collect::<Vec<_>>();
        assert!(
            loaded.user_fallback_chain.iter().any(|(font_id, _)| state
                .loaded_font(*font_id)
                .font
                .as_swash()
                .charmap()
                .map('图')
                != 0),
            "coverage-scored fallback family {:?} did not add a CJK-capable system fallback; chain={:?}",
            best_face.family,
            // keep the local fallback chain visible when this machine has unusual fonts
            // or fontdb/cosmic loading disagrees with face metadata.
            // This is test-only diagnostics.
            // The assertion still verifies the behavior users need.
            // rustfmt keeps this compact enough for failure output.
            chain_debug
        );
    }

    #[test]
    fn automatic_system_fallback_returns_single_system_font() {
        let text_system = CosmicTextSystem::new();
        let primary_id = text_system
            .font_id(&font("Segoe UI"))
            .expect("primary font loads");
        let state = text_system.0.read();
        let loaded = state.loaded_font(primary_id);

        assert!(
            loaded.user_fallback_chain.len() <= 1,
            "automatic fallback should use a single unified system font, got {:?}",
            loaded
                .user_fallback_chain
                .iter()
                .map(|(_, family)| family.to_string())
                .collect::<Vec<_>>()
        );
        for (font_id, _) in loaded.user_fallback_chain.iter() {
            let database_id = state.loaded_font(*font_id).font.id();
            let Some(face) = state.font_system.db().face(database_id) else {
                continue;
            };
            assert!(
                FontSourceSelection::SystemOnly.matches(&face.source),
                "automatic fallback must not use embedded application fonts"
            );
        }
    }

    #[test]
    fn automatic_system_fallback_ignores_embedded_fonts() {
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/fonts/MiSans/MiSans-Medium.ttf"
            ))
            .to_vec(),
        );

        assert_ne!(
            best_system_text_fallback_face(&font_system)
                .as_ref()
                .map(|face| face.family.as_ref()),
            Some("MiSans"),
            "embedded font was selected as GPUI automatic system fallback"
        );
    }

    #[test]
    fn glyphs_keep_exact_swash_bounds() {
        let text_system = CosmicTextSystem::new();
        let font_id = text_system
            .font_id(&font(".SystemUIFont"))
            .expect("system font loads");
        let layout = text_system.layout_line(
            "A",
            px(13.),
            &[FontRun {
                len: "A".len(),
                font_id,
            }],
        );
        let run = layout.runs.first().expect("text shapes into a run");
        let glyph = run.glyphs.first().expect("text shapes into a glyph");
        let params = RenderGlyphParams {
            font_id: run.font_id,
            glyph_id: glyph.id,
            font_size: glyph.font_size,
            subpixel_variant: Point::default(),
            scale_factor: 1.0,
            is_emoji: false,
            is_cjk: glyph.is_cjk,
        };
        let mut state = text_system.0.write();
        let image = state.glyph_image(&params).expect("glyph image");
        let exact_bounds = Bounds {
            origin: point(image.placement.left.into(), (-image.placement.top).into()),
            size: size(image.placement.width.into(), image.placement.height.into()),
        };

        assert_eq!(
            state.raster_bounds(&params).expect("glyph raster bounds"),
            exact_bounds
        );
    }

    #[test]
    fn swash_color_bitmap_converts_to_bgra_for_polychrome_glyphs() {
        let image = SwashImage {
            content: SwashContent::Color,
            data: vec![10, 20, 30, 40, 50, 60, 70, 80],
            ..Default::default()
        };

        let bytes =
            swash_image_to_polychrome_bitmap(image, Size::new(DevicePixels(2), DevicePixels(1)))
                .expect("color bitmap converts");

        assert_eq!(bytes, vec![30, 20, 10, 40, 70, 60, 50, 80]);
    }

    #[test]
    fn swash_color_bitmap_uses_alpha_for_monochrome_glyphs() {
        let image = SwashImage {
            content: SwashContent::Color,
            data: vec![10, 20, 30, 40, 50, 60, 70, 80],
            ..Default::default()
        };

        let bytes =
            swash_image_to_monochrome_mask(image, Size::new(DevicePixels(2), DevicePixels(1)))
                .expect("color bitmap alpha extracts");

        assert_eq!(bytes, vec![40, 80]);
    }

    #[test]
    fn copy_mask_to_frame_keeps_padding_alpha_zero() {
        let source = vec![10, 20, 30, 40];
        let source_bounds = Bounds {
            origin: point(DevicePixels(3), DevicePixels(-6)),
            size: size(DevicePixels(2), DevicePixels(2)),
        };
        let frame_bounds = Bounds {
            origin: point(DevicePixels(3), DevicePixels(-7)),
            size: size(DevicePixels(2), DevicePixels(4)),
        };

        let frame =
            copy_mask_to_frame(source, source_bounds, frame_bounds).expect("frame copy succeeds");

        assert_eq!(frame, vec![0, 0, 10, 20, 30, 40, 0, 0]);
    }
}
