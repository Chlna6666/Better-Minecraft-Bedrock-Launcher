use std::{
    borrow::Cow,
    ffi::{c_uint, c_void},
    mem::ManuallyDrop,
    path::PathBuf,
};

use crate::*;
use ::util::{ResultExt, maybe};
use anyhow::{Context, Result};
use collections::HashMap;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use windows::{
    Win32::{
        Foundation::*,
        Globalization::GetUserDefaultLocaleName,
        Graphics::{DirectWrite::*, Gdi::LOGFONTW},
        System::SystemServices::LOCALE_NAME_MAX_LENGTH,
        UI::WindowsAndMessaging::*,
    },
    core::*,
};

#[derive(Debug)]
struct FontInfo {
    font_family_h: HSTRING,
    font_face: IDWriteFontFace3,
    features: IDWriteTypography,
    fallbacks: Option<IDWriteFontFallback>,
    font_collection: IDWriteFontCollection1,
}

pub(crate) struct DirectWriteTextSystem {
    components: DirectWriteComponents,
    state: RwLock<DirectWriteState>,
    pending_glyph_analysis: Mutex<Option<PendingGlyphAnalysis>>,
}

struct PendingGlyphAnalysis {
    params: RenderGlyphParams,
    analysis: IDWriteGlyphRunAnalysis,
}

struct DirectWriteComponents {
    locale: HSTRING,
    factory: IDWriteFactory5,
    in_memory_loader: IDWriteInMemoryFontFileLoader,
    builder: IDWriteFontSetBuilder1,
    text_renderer: TextRendererWrapper,
    system_ui_font_name: SharedString,
    system_subpixel_rendering: bool,
}

impl Drop for DirectWriteComponents {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .factory
                .UnregisterFontFileLoader(&self.in_memory_loader);
        }
    }
}

struct DirectWriteState {
    system_font_collection: IDWriteFontCollection1,
    custom_font_collection: IDWriteFontCollection1,
    fonts: Vec<FontInfo>,
    font_to_font_id: HashMap<Font, FontId>,
    font_info_cache: HashMap<usize, FontId>,
    layout_line_scratch: Vec<u16>,
}

impl DirectWriteTextSystem {
    pub(crate) fn new() -> Result<Self> {
        let factory: IDWriteFactory5 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        // The `IDWriteInMemoryFontFileLoader` here is supported starting from
        // Windows 10 Creators Update, which consequently requires the entire
        // `DirectWriteTextSystem` to run on `win10 1703`+.
        let in_memory_loader = unsafe { factory.CreateInMemoryFontFileLoader()? };
        unsafe { factory.RegisterFontFileLoader(&in_memory_loader)? };
        let builder = unsafe { factory.CreateFontSetBuilder()? };
        let mut locale = [0u16; LOCALE_NAME_MAX_LENGTH as usize];
        unsafe { GetUserDefaultLocaleName(&mut locale) };
        let locale = HSTRING::from_wide(&locale);
        let text_renderer = TextRendererWrapper::new(locale.clone());

        let system_subpixel_rendering = get_system_subpixel_rendering();
        let system_ui_font_name = get_system_ui_font_name();
        let components = DirectWriteComponents {
            locale,
            factory,
            in_memory_loader,
            builder,
            text_renderer,
            system_ui_font_name,
            system_subpixel_rendering,
        };

        let system_font_collection = unsafe {
            let mut result = None;
            components
                .factory
                .GetSystemFontCollection(false, &mut result, true)?;
            result.context("Failed to get system font collection")?
        };
        let custom_font_set = unsafe { components.builder.CreateFontSet()? };
        let custom_font_collection = unsafe {
            components
                .factory
                .CreateFontCollectionFromFontSet(&custom_font_set)?
        };

        Ok(Self {
            components,
            state: RwLock::new(DirectWriteState {
                system_font_collection,
                custom_font_collection,
                fonts: Vec::new(),
                font_to_font_id: HashMap::default(),
                font_info_cache: HashMap::default(),
                layout_line_scratch: Vec::new(),
            }),
            pending_glyph_analysis: Mutex::new(None),
        })
    }
}

impl PlatformTextSystem for DirectWriteTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.state.write().add_fonts(&self.components, fonts)
    }

    fn add_font_paths(&self, paths: Vec<PathBuf>) -> Result<()> {
        let fonts = paths
            .into_iter()
            .map(std::fs::read)
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .map(Cow::Owned)
            .collect();
        self.add_fonts(fonts)
    }

    fn platform_font_family(&self) -> SharedString {
        self.components.system_ui_font_name.clone()
    }

    fn all_font_names(&self) -> Vec<String> {
        self.state.read().all_font_names(&self.components)
    }

    fn font_id(&self, font: &Font) -> Result<FontId> {
        let lock = self.state.upgradable_read();
        if let Some(font_id) = lock.font_to_font_id.get(font) {
            Ok(*font_id)
        } else {
            RwLockUpgradableReadGuard::upgrade(lock)
                .select_and_cache_font(&self.components, font)
                .with_context(|| format!("Failed to select font: {:?}", font))
        }
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.state.read().font_metrics(font_id)
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        self.state.read().get_typographic_bounds(font_id, glyph_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> anyhow::Result<Size<f32>> {
        self.state.read().get_advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.state.read().glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(
        &self,
        params: &RenderGlyphParams,
    ) -> anyhow::Result<Bounds<DevicePixels>> {
        let (analysis, bounds) = self
            .state
            .read()
            .glyph_analysis_and_bounds(&self.components, params)?;
        *self.pending_glyph_analysis.lock() = Some(PendingGlyphAnalysis {
            params: params.clone(),
            analysis,
        });
        Ok(bounds)
    }

    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> anyhow::Result<GlyphRasterization> {
        let analysis = self
            .pending_glyph_analysis
            .lock()
            .take()
            .filter(|pending| pending.params == *params)
            .map(|pending| pending.analysis);
        self.state.read().rasterize_glyph(
            &self.components,
            params,
            raster_bounds,
            analysis.as_ref(),
        )
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.state
            .write()
            .layout_line(&self.components, text, font_size, runs)
            .log_err()
            .unwrap_or(LineLayout {
                font_size,
                ..Default::default()
            })
    }
}

impl DirectWriteState {
    fn select_and_cache_font(
        &mut self,
        components: &DirectWriteComponents,
        font: &Font,
    ) -> Option<FontId> {
        let select_font = |this: &mut DirectWriteState, font: &Font| -> Option<FontId> {
            let info = [&this.custom_font_collection, &this.system_font_collection]
                .into_iter()
                .find_map(|font_collection| unsafe {
                    DirectWriteState::make_font_from_font_collection(
                        font,
                        font_collection,
                        &components.factory,
                        &this.custom_font_collection,
                        &this.system_font_collection,
                        &components.system_ui_font_name,
                    )
                })?;

            let font_id = FontId(this.fonts.len());
            let font_face_key = info.font_face.cast::<IUnknown>().unwrap().as_raw().addr();
            this.fonts.push(info);
            this.font_info_cache.insert(font_face_key, font_id);
            Some(font_id)
        };

        let mut font_id = select_font(self, font);
        if font_id.is_none() {
            // try updating system fonts and reselect
            let mut collection = None;
            let font_collection_updated = unsafe {
                components
                    .factory
                    .GetSystemFontCollection(false, &mut collection, true)
            }
            .log_err()
            .is_some();
            if font_collection_updated && let Some(collection) = collection {
                self.system_font_collection = collection;
            }
            font_id = select_font(self, font);
        };
        let font_id = font_id?;
        self.font_to_font_id.insert(font.clone(), font_id);
        Some(font_id)
    }

    fn add_fonts(
        &mut self,
        components: &DirectWriteComponents,
        fonts: Vec<Cow<'static, [u8]>>,
    ) -> Result<()> {
        for font_data in fonts {
            match font_data {
                Cow::Borrowed(data) => unsafe {
                    let font_file = components
                        .in_memory_loader
                        .CreateInMemoryFontFileReference(
                            &components.factory,
                            data.as_ptr().cast(),
                            data.len() as _,
                            None,
                        )?;
                    components.builder.AddFontFile(&font_file)?;
                },
                Cow::Owned(data) => unsafe {
                    let font_file = components
                        .in_memory_loader
                        .CreateInMemoryFontFileReference(
                            &components.factory,
                            data.as_ptr().cast(),
                            data.len() as _,
                            None,
                        )?;
                    components.builder.AddFontFile(&font_file)?;
                },
            }
        }
        let set = unsafe { components.builder.CreateFontSet()? };
        let collection = unsafe { components.factory.CreateFontCollectionFromFontSet(&set)? };
        self.custom_font_collection = collection;
        self.font_to_font_id.clear();
        self.font_info_cache.clear();

        Ok(())
    }

    fn generate_font_fallbacks(
        fallbacks: &FontFallbacks,
        factory: &IDWriteFactory5,
        custom_font_collection: &IDWriteFontCollection1,
        system_font_collection: &IDWriteFontCollection1,
    ) -> Result<Option<IDWriteFontFallback>> {
        let fallback_list = fallbacks.fallback_list();
        if fallback_list.is_empty() {
            return Ok(None);
        }
        unsafe {
            let builder = factory.CreateFontFallbackBuilder()?;
            for family_name in fallback_list {
                let family_name = HSTRING::from(family_name.as_str());
                for collection in [custom_font_collection, system_font_collection] {
                    if Self::add_font_fallback_mapping(&builder, &family_name, collection)
                        .log_err()
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            }
            let system_fallbacks = factory.GetSystemFontFallback()?;
            builder.AddMappings(&system_fallbacks)?;
            Ok(Some(builder.CreateFontFallback()?))
        }
    }

    unsafe fn add_font_fallback_mapping(
        builder: &IDWriteFontFallbackBuilder,
        family_name: &HSTRING,
        collection: &IDWriteFontCollection1,
    ) -> Result<bool> {
        let font_set = unsafe { collection.GetFontSet()? };
        let fonts = unsafe {
            font_set.GetMatchingFonts(
                family_name,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )?
        };
        if unsafe { fonts.GetFontCount() } == 0 {
            return Ok(false);
        }
        let font_face = unsafe { fonts.GetFontFaceReference(0)?.CreateFontFace()? };
        let mut count = 0;
        unsafe { font_face.GetUnicodeRanges(None, &mut count).ok() };
        if count == 0 {
            return Ok(false);
        }
        let mut unicode_ranges = vec![DWRITE_UNICODE_RANGE::default(); count as usize];
        unsafe { font_face.GetUnicodeRanges(Some(&mut unicode_ranges), &mut count)? };
        let collection = collection.cast::<IDWriteFontCollection>()?;
        unsafe {
            builder.AddMapping(
                &unicode_ranges,
                &[family_name.as_ptr()],
                &collection,
                None,
                None,
                1.0,
            )?;
        }
        Ok(true)
    }

    unsafe fn generate_font_features(
        factory: &IDWriteFactory5,
        font_features: &FontFeatures,
    ) -> Result<IDWriteTypography> {
        let direct_write_features = unsafe { factory.CreateTypography()? };
        apply_font_features(&direct_write_features, font_features)?;
        Ok(direct_write_features)
    }

    unsafe fn make_font_from_font_collection(
        &Font {
            ref family,
            ref features,
            ref fallbacks,
            weight,
            style,
        }: &Font,
        collection: &IDWriteFontCollection1,
        factory: &IDWriteFactory5,
        custom_font_collection: &IDWriteFontCollection1,
        system_font_collection: &IDWriteFontCollection1,
        system_ui_font_name: &SharedString,
    ) -> Option<FontInfo> {
        const SYSTEM_UI_FONT_NAME: &str = ".SystemUIFont";
        let family = if family == SYSTEM_UI_FONT_NAME {
            system_ui_font_name
        } else {
            font_name_with_fallbacks(family.as_str(), system_ui_font_name.as_str())
        };
        let fontset = unsafe { collection.GetFontSet().log_err()? };
        let font_family_h = HSTRING::from(family);
        let font = unsafe {
            fontset
                .GetMatchingFonts(
                    &font_family_h,
                    font_weight_to_dwrite(weight),
                    DWRITE_FONT_STRETCH_NORMAL,
                    font_style_to_dwrite(style),
                )
                .log_err()?
        };
        let total_number = unsafe { font.GetFontCount() };
        for index in 0..total_number {
            let res = maybe!({
                let font_face_ref = unsafe { font.GetFontFaceReference(index).log_err()? };
                let font_face = unsafe { font_face_ref.CreateFontFace().log_err()? };
                let direct_write_features =
                    unsafe { Self::generate_font_features(factory, features).log_err()? };
                let fallbacks = fallbacks.as_ref().and_then(|fallbacks| {
                    Self::generate_font_fallbacks(
                        fallbacks,
                        factory,
                        custom_font_collection,
                        system_font_collection,
                    )
                    .log_err()
                    .flatten()
                });
                let font_info = FontInfo {
                    font_family_h: font_family_h.clone(),
                    font_face,
                    features: direct_write_features,
                    fallbacks,
                    font_collection: collection.clone(),
                };
                Some(font_info)
            });
            if res.is_some() {
                return res;
            }
        }
        None
    }

    fn layout_line(
        &mut self,
        components: &DirectWriteComponents,
        text: &str,
        font_size: Pixels,
        font_runs: &[FontRun],
    ) -> Result<LineLayout> {
        if font_runs.is_empty() {
            return Ok(LineLayout {
                font_size,
                ..Default::default()
            });
        }
        unsafe {
            self.layout_line_scratch.clear();
            self.layout_line_scratch.extend(text.encode_utf16());
            let text_wide = &*self.layout_line_scratch;

            let mut utf8_offset = 0usize;
            let mut utf16_offset = 0u32;
            let text_layout = {
                let first_run = &font_runs[0];
                let font_info = &self.fonts[first_run.font_id.0];
                let collection = &font_info.font_collection;
                let format: IDWriteTextFormat1 = components
                    .factory
                    .CreateTextFormat(
                        &font_info.font_family_h,
                        collection,
                        font_info.font_face.GetWeight(),
                        font_info.font_face.GetStyle(),
                        DWRITE_FONT_STRETCH_NORMAL,
                        font_size.0,
                        &components.locale,
                    )?
                    .cast()?;
                if let Some(ref fallbacks) = font_info.fallbacks {
                    format.SetFontFallback(fallbacks)?;
                }

                let layout = components.factory.CreateTextLayout(
                    text_wide,
                    &format,
                    f32::INFINITY,
                    f32::INFINITY,
                )?;
                let run_start = utf8_run_start(text, utf8_offset);
                let run_end = utf8_run_end(text, run_start, first_run.len);
                let current_text = &text[run_start..run_end];
                utf8_offset = run_end;
                let current_text_utf16_length = current_text.encode_utf16().count() as u32;
                let text_range = DWRITE_TEXT_RANGE {
                    startPosition: utf16_offset,
                    length: current_text_utf16_length,
                };
                layout.SetTypography(&font_info.features, text_range)?;
                utf16_offset += current_text_utf16_length;

                layout
            };

            let (ascent, descent) = {
                let mut first_metrics = [DWRITE_LINE_METRICS::default(); 4];
                let mut line_count = 0u32;
                text_layout.GetLineMetrics(Some(&mut first_metrics), &mut line_count)?;
                (
                    px(first_metrics[0].baseline),
                    px(first_metrics[0].height - first_metrics[0].baseline),
                )
            };
            let mut break_ligatures = true;
            for run in &font_runs[1..] {
                let font_info = &self.fonts[run.font_id.0];
                let run_start = utf8_run_start(text, utf8_offset);
                let run_end = utf8_run_end(text, run_start, run.len);
                let current_text = &text[run_start..run_end];
                utf8_offset = run_end;
                let current_text_utf16_length = current_text.encode_utf16().count() as u32;

                let collection = &font_info.font_collection;
                let text_range = DWRITE_TEXT_RANGE {
                    startPosition: utf16_offset,
                    length: current_text_utf16_length,
                };
                utf16_offset += current_text_utf16_length;
                text_layout.SetFontCollection(collection, text_range)?;
                text_layout.SetFontFamilyName(&font_info.font_family_h, text_range)?;
                let font_size = if break_ligatures {
                    font_size.0.next_up()
                } else {
                    font_size.0
                };
                text_layout.SetFontSize(font_size, text_range)?;
                text_layout.SetFontStyle(font_info.font_face.GetStyle(), text_range)?;
                text_layout.SetFontWeight(font_info.font_face.GetWeight(), text_range)?;
                text_layout.SetTypography(&font_info.features, text_range)?;

                break_ligatures = !break_ligatures;
            }

            let mut runs = Vec::new();
            let mut renderer_context = RendererContext {
                text_system: self,
                components,
                index_converter: StringIndexConverter::new(text),
                runs: &mut runs,
                width: 0.0,
            };
            text_layout.Draw(
                Some((&raw mut renderer_context).cast::<c_void>().cast_const()),
                &components.text_renderer.0,
                0.0,
                0.0,
            )?;
            let width = px(renderer_context.width);

            Ok(LineLayout {
                font_size,
                width,
                ascent,
                descent,
                runs,
                len: text.len(),
            })
        }
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        unsafe {
            let font_info = &self.fonts[font_id.0];
            let mut metrics = std::mem::zeroed();
            font_info.font_face.GetMetrics(&mut metrics);

            FontMetrics {
                units_per_em: metrics.Base.designUnitsPerEm as _,
                ascent: metrics.Base.ascent as _,
                descent: -(metrics.Base.descent as f32),
                line_gap: metrics.Base.lineGap as _,
                underline_position: metrics.Base.underlinePosition as _,
                underline_thickness: metrics.Base.underlineThickness as _,
                cap_height: metrics.Base.capHeight as _,
                x_height: metrics.Base.xHeight as _,
                bounding_box: Bounds {
                    origin: Point {
                        x: metrics.glyphBoxLeft as _,
                        y: metrics.glyphBoxBottom as _,
                    },
                    size: Size {
                        width: (metrics.glyphBoxRight - metrics.glyphBoxLeft) as _,
                        height: (metrics.glyphBoxTop - metrics.glyphBoxBottom) as _,
                    },
                },
            }
        }
    }

    fn create_glyph_run_analysis(
        &self,
        components: &DirectWriteComponents,
        params: &RenderGlyphParams,
    ) -> Result<IDWriteGlyphRunAnalysis> {
        let font = &self.fonts[params.font_id.0];
        let glyph_id = [params.glyph_id.0 as u16];
        let advance = [0.0];
        let offset = [DWRITE_GLYPH_OFFSET::default()];
        let glyph_run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(unsafe { std::ptr::read(&***font.font_face) })),
            fontEmSize: params.font_size.0,
            glyphCount: 1,
            glyphIndices: glyph_id.as_ptr(),
            glyphAdvances: advance.as_ptr(),
            glyphOffsets: offset.as_ptr(),
            isSideways: BOOL(0),
            bidiLevel: 0,
        };
        let transform = DWRITE_MATRIX {
            m11: params.scale_factor,
            m12: 0.0,
            m21: 0.0,
            m22: params.scale_factor,
            dx: 0.0,
            dy: 0.0,
        };
        let baseline_origin_x =
            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor;
        let baseline_origin_y = params.subpixel_variant.y as f32
            / gpui::SUBPIXEL_VARIANTS_Y as f32
            / params.scale_factor;

        let mut rendering_mode = DWRITE_RENDERING_MODE1::default();
        let mut grid_fit_mode = DWRITE_GRID_FIT_MODE::default();
        unsafe {
            font.font_face.GetRecommendedRenderingMode(
                params.font_size.0,
                // Using 96 as scale is applied by the transform
                96.0,
                96.0,
                Some(&transform),
                false,
                DWRITE_OUTLINE_THRESHOLD_ANTIALIASED,
                DWRITE_MEASURING_MODE_NATURAL,
                None,
                &mut rendering_mode,
                &mut grid_fit_mode,
            )?;
        }
        let rendering_mode = match rendering_mode {
            DWRITE_RENDERING_MODE1_OUTLINE => DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
            m => m,
        };

        let antialias_mode = if should_use_subpixel_rendering(components, params) {
            DWRITE_TEXT_ANTIALIAS_MODE_CLEARTYPE
        } else {
            DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE
        };

        let glyph_analysis = unsafe {
            components.factory.CreateGlyphRunAnalysis(
                &glyph_run,
                Some(&transform),
                rendering_mode,
                DWRITE_MEASURING_MODE_NATURAL,
                grid_fit_mode,
                antialias_mode,
                baseline_origin_x,
                baseline_origin_y,
            )
        }?;
        Ok(glyph_analysis)
    }

    fn glyph_analysis_and_bounds(
        &self,
        components: &DirectWriteComponents,
        params: &RenderGlyphParams,
    ) -> Result<(IDWriteGlyphRunAnalysis, Bounds<DevicePixels>)> {
        let glyph_analysis = self.create_glyph_run_analysis(components, params)?;

        let texture_type = if should_use_subpixel_rendering(components, params) {
            DWRITE_TEXTURE_CLEARTYPE_3x1
        } else {
            DWRITE_TEXTURE_ALIASED_1x1
        };

        let bounds = unsafe { glyph_analysis.GetAlphaTextureBounds(texture_type)? };

        if bounds.right < bounds.left {
            Ok((
                glyph_analysis,
                Bounds {
                    origin: point(0.into(), 0.into()),
                    size: size(0.into(), 0.into()),
                },
            ))
        } else {
            Ok((
                glyph_analysis,
                Bounds {
                    origin: point(bounds.left.into(), bounds.top.into()),
                    size: size(
                        (bounds.right - bounds.left).into(),
                        (bounds.bottom - bounds.top).into(),
                    ),
                },
            ))
        }
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let font_info = &self.fonts[font_id.0];
        let codepoints = ch as u32;
        let mut glyph_indices = 0u16;
        unsafe {
            font_info
                .font_face
                .GetGlyphIndices(&raw const codepoints, 1, &raw mut glyph_indices)
                .log_err()
        }
        .map(|_| GlyphId(glyph_indices as u32))
    }

    fn rasterize_glyph(
        &self,
        components: &DirectWriteComponents,
        params: &RenderGlyphParams,
        glyph_bounds: Bounds<DevicePixels>,
        glyph_analysis: Option<&IDWriteGlyphRunAnalysis>,
    ) -> Result<GlyphRasterization> {
        if glyph_bounds.size.width.0 <= 0 || glyph_bounds.size.height.0 <= 0 {
            anyhow::bail!("glyph bounds are empty");
        }

        if params.is_emoji {
            let alpha =
                self.rasterize_grayscale(components, params, glyph_bounds, glyph_analysis)?;
            let fallback = GlyphBitmap {
                size: glyph_bounds.size,
                bytes: alpha.iter().flat_map(|alpha| [0, 0, 0, *alpha]).collect(),
            };
            if let Ok(layers) = self.rasterize_color_layers(components, params, glyph_bounds)
                && !layers.is_empty()
            {
                return Ok(GlyphRasterization::ColorLayers {
                    size: glyph_bounds.size,
                    layers,
                    fallback,
                });
            }
            return Ok(GlyphRasterization::Bitmap {
                size: fallback.size,
                bytes: fallback.bytes,
            });
        }

        let monochrome =
            self.rasterize_monochrome(components, params, glyph_bounds, glyph_analysis)?;
        Ok(GlyphRasterization::Bitmap {
            size: glyph_bounds.size,
            bytes: monochrome,
        })
    }

    fn rasterize_monochrome(
        &self,
        components: &DirectWriteComponents,
        params: &RenderGlyphParams,
        glyph_bounds: Bounds<DevicePixels>,
        glyph_analysis: Option<&IDWriteGlyphRunAnalysis>,
    ) -> Result<Vec<u8>> {
        if !should_use_subpixel_rendering(components, params) {
            return Ok(self
                .rasterize_grayscale(components, params, glyph_bounds, glyph_analysis)?
                .into_iter()
                .flat_map(|alpha| [alpha, alpha, alpha, 255])
                .collect());
        }

        let owned_analysis;
        let glyph_analysis = if let Some(glyph_analysis) = glyph_analysis {
            glyph_analysis
        } else {
            owned_analysis = self.create_glyph_run_analysis(components, params)?;
            &owned_analysis
        };
        let pixel_count = glyph_bounds.size.width.0 as usize * glyph_bounds.size.height.0 as usize;
        let mut clear_type = vec![0; pixel_count * 3];
        unsafe {
            glyph_analysis.CreateAlphaTexture(
                DWRITE_TEXTURE_CLEARTYPE_3x1,
                &rect_for_bounds(glyph_bounds),
                &mut clear_type,
            )?;
        }
        let mut bgra = Vec::with_capacity(pixel_count * 4);
        for channels in clear_type.chunks_exact(3) {
            bgra.extend_from_slice(&[channels[0], channels[1], channels[2], 255]);
        }
        Ok(bgra)
    }

    fn rasterize_grayscale(
        &self,
        components: &DirectWriteComponents,
        params: &RenderGlyphParams,
        glyph_bounds: Bounds<DevicePixels>,
        glyph_analysis: Option<&IDWriteGlyphRunAnalysis>,
    ) -> Result<Vec<u8>> {
        let owned_analysis;
        let glyph_analysis = if let Some(glyph_analysis) = glyph_analysis {
            glyph_analysis
        } else {
            owned_analysis = self.create_glyph_run_analysis(components, params)?;
            &owned_analysis
        };
        let mut bitmap =
            vec![0; glyph_bounds.size.width.0 as usize * glyph_bounds.size.height.0 as usize];
        unsafe {
            glyph_analysis.CreateAlphaTexture(
                DWRITE_TEXTURE_ALIASED_1x1,
                &rect_for_bounds(glyph_bounds),
                &mut bitmap,
            )?;
        }
        Ok(bitmap)
    }

    fn rasterize_color_layers(
        &self,
        components: &DirectWriteComponents,
        params: &RenderGlyphParams,
        glyph_bounds: Bounds<DevicePixels>,
    ) -> Result<Vec<ColorGlyphLayer>> {
        let subpixel_shift = params
            .subpixel_variant
            .map(|value| value as f32 / SUBPIXEL_VARIANTS_X as f32);
        let baseline_origin_x = subpixel_shift.x / params.scale_factor;
        let baseline_origin_y = subpixel_shift.y / params.scale_factor;
        let transform = DWRITE_MATRIX {
            m11: params.scale_factor,
            m12: 0.0,
            m21: 0.0,
            m22: params.scale_factor,
            dx: 0.0,
            dy: 0.0,
        };
        let font = &self.fonts[params.font_id.0];
        let glyph_id = [params.glyph_id.0 as u16];
        let advance = [glyph_bounds.size.width.0 as f32];
        let offset = [DWRITE_GLYPH_OFFSET {
            advanceOffset: -glyph_bounds.origin.x.0 as f32 / params.scale_factor,
            ascenderOffset: glyph_bounds.origin.y.0 as f32 / params.scale_factor,
        }];
        let glyph_run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(unsafe { std::ptr::read(&***font.font_face) })),
            fontEmSize: params.font_size.0,
            glyphCount: 1,
            glyphIndices: glyph_id.as_ptr(),
            glyphAdvances: advance.as_ptr(),
            glyphOffsets: offset.as_ptr(),
            isSideways: BOOL(0),
            bidiLevel: 0,
        };
        let enumerator = unsafe {
            components.factory.TranslateColorGlyphRun(
                windows_numerics::Vector2::new(baseline_origin_x, baseline_origin_y),
                &glyph_run,
                None,
                DWRITE_GLYPH_IMAGE_FORMATS_COLR,
                DWRITE_MEASURING_MODE_NATURAL,
                Some(&transform),
                0,
            )
        }?;

        let mut layers = Vec::new();
        loop {
            let color_run = unsafe { &*enumerator.GetCurrentRun()? };
            let analysis = unsafe {
                components.factory.CreateGlyphRunAnalysis(
                    &color_run.Base.glyphRun,
                    Some(&transform),
                    DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                    DWRITE_MEASURING_MODE_NATURAL,
                    DWRITE_GRID_FIT_MODE_DEFAULT,
                    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                    baseline_origin_x,
                    baseline_origin_y,
                )
            }?;
            let bounds = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) }?;
            let width = bounds.right - bounds.left;
            let height = bounds.bottom - bounds.top;
            if width > 0 && height > 0 {
                let mut alpha = vec![0; (width * height) as usize];
                unsafe {
                    analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut alpha)?;
                }
                let color = color_run.Base.runColor;
                layers.push(ColorGlyphLayer {
                    bounds: Bounds {
                        origin: point(
                            DevicePixels(bounds.left - glyph_bounds.origin.x.0),
                            DevicePixels(bounds.top - glyph_bounds.origin.y.0),
                        ),
                        size: size(DevicePixels(width), DevicePixels(height)),
                    },
                    color: Rgba {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                        a: color.a,
                    },
                    alpha,
                });
            }
            if !unsafe { enumerator.MoveNext() }?.as_bool() {
                break;
            }
        }
        Ok(layers)
    }

    fn get_typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        unsafe {
            let font = &self.fonts[font_id.0].font_face;
            let glyph_indices = [glyph_id.0 as u16];
            let mut metrics = [DWRITE_GLYPH_METRICS::default()];
            font.GetDesignGlyphMetrics(glyph_indices.as_ptr(), 1, metrics.as_mut_ptr(), false)?;

            let metrics = &metrics[0];
            let advance_width = metrics.advanceWidth as i32;
            let advance_height = metrics.advanceHeight as i32;
            let left_side_bearing = metrics.leftSideBearing;
            let right_side_bearing = metrics.rightSideBearing;
            let top_side_bearing = metrics.topSideBearing;
            let bottom_side_bearing = metrics.bottomSideBearing;
            let vertical_origin_y = metrics.verticalOriginY;

            let y_offset = vertical_origin_y + bottom_side_bearing - advance_height;
            let width = advance_width - (left_side_bearing + right_side_bearing);
            let height = advance_height - (top_side_bearing + bottom_side_bearing);

            Ok(Bounds {
                origin: Point {
                    x: left_side_bearing as f32,
                    y: y_offset as f32,
                },
                size: Size {
                    width: width as f32,
                    height: height as f32,
                },
            })
        }
    }

    fn get_advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        unsafe {
            let font = &self.fonts[font_id.0].font_face;
            let glyph_indices = [glyph_id.0 as u16];
            let mut metrics = [DWRITE_GLYPH_METRICS::default()];
            font.GetDesignGlyphMetrics(glyph_indices.as_ptr(), 1, metrics.as_mut_ptr(), false)?;

            let metrics = &metrics[0];

            Ok(Size {
                width: metrics.advanceWidth as f32,
                height: 0.0,
            })
        }
    }

    fn all_font_names(&self, components: &DirectWriteComponents) -> Vec<String> {
        let mut result =
            get_font_names_from_collection(&self.system_font_collection, &components.locale);
        result.extend(get_font_names_from_collection(
            &self.custom_font_collection,
            &components.locale,
        ));
        result
    }
}

struct TextRendererWrapper(IDWriteTextRenderer);

impl TextRendererWrapper {
    fn new(locale_str: HSTRING) -> Self {
        let inner = TextRenderer::new(locale_str);
        TextRendererWrapper(inner.into())
    }
}

#[implement(IDWriteTextRenderer)]
struct TextRenderer {
    locale: HSTRING,
}

impl TextRenderer {
    fn new(locale: HSTRING) -> Self {
        TextRenderer { locale }
    }
}

struct RendererContext<'t, 'a, 'b> {
    text_system: &'t mut DirectWriteState,
    components: &'a DirectWriteComponents,
    index_converter: StringIndexConverter<'a>,
    runs: &'b mut Vec<ShapedRun>,
    width: f32,
}

#[derive(Debug)]
struct ClusterAnalyzer<'t> {
    utf16_idx: usize,
    glyph_idx: usize,
    glyph_count: usize,
    cluster_map: &'t [u16],
}

impl<'t> ClusterAnalyzer<'t> {
    fn new(cluster_map: &'t [u16], glyph_count: usize) -> Self {
        ClusterAnalyzer {
            utf16_idx: 0,
            glyph_idx: 0,
            glyph_count,
            cluster_map,
        }
    }
}

impl Iterator for ClusterAnalyzer<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        if self.utf16_idx >= self.cluster_map.len() {
            return None; // No more clusters
        }
        let start_utf16_idx = self.utf16_idx;
        let current_glyph = self.cluster_map[start_utf16_idx] as usize;

        // Find the end of current cluster (where glyph index changes)
        let mut end_utf16_idx = start_utf16_idx + 1;
        while end_utf16_idx < self.cluster_map.len()
            && self.cluster_map[end_utf16_idx] as usize == current_glyph
        {
            end_utf16_idx += 1;
        }

        let utf16_len = end_utf16_idx - start_utf16_idx;

        // Calculate glyph count for this cluster
        let next_glyph = if end_utf16_idx < self.cluster_map.len() {
            self.cluster_map[end_utf16_idx] as usize
        } else {
            self.glyph_count
        };

        let glyph_count = next_glyph - current_glyph;

        // Update state for next call
        self.utf16_idx = end_utf16_idx;
        self.glyph_idx = next_glyph;

        Some((utf16_len, glyph_count))
    }
}

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for TextRenderer_Impl {
    fn IsPixelSnappingDisabled(
        &self,
        _clientdrawingcontext: *const ::core::ffi::c_void,
    ) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }

    fn GetCurrentTransform(
        &self,
        _clientdrawingcontext: *const ::core::ffi::c_void,
        transform: *mut DWRITE_MATRIX,
    ) -> windows::core::Result<()> {
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.0,
                m12: 0.0,
                m21: 0.0,
                m22: 1.0,
                dx: 0.0,
                dy: 0.0,
            };
        }
        Ok(())
    }

    fn GetPixelsPerDip(
        &self,
        _clientdrawingcontext: *const ::core::ffi::c_void,
    ) -> windows::core::Result<f32> {
        Ok(1.0)
    }
}

#[allow(non_snake_case)]
impl IDWriteTextRenderer_Impl for TextRenderer_Impl {
    fn DrawGlyphRun(
        &self,
        clientdrawingcontext: *const ::core::ffi::c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _measuringmode: DWRITE_MEASURING_MODE,
        glyphrun: *const DWRITE_GLYPH_RUN,
        glyphrundescription: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _clientdrawingeffect: windows::core::Ref<windows::core::IUnknown>,
    ) -> windows::core::Result<()> {
        let glyphrun = unsafe { &*glyphrun };
        let glyph_count = glyphrun.glyphCount as usize;
        if glyph_count == 0 {
            return Ok(());
        }
        let desc = unsafe { &*glyphrundescription };
        let context = unsafe { &mut *(clientdrawingcontext.cast::<RendererContext>().cast_mut()) };
        let Some(font_face) = glyphrun.fontFace.as_ref() else {
            return Ok(());
        };
        // This `cast()` action here should never fail since we are running on Win10+, and
        // `IDWriteFontFace3` requires Win10
        let Ok(font_face) = &font_face.cast::<IDWriteFontFace3>() else {
            return Err(Error::new(
                DWRITE_E_UNSUPPORTEDOPERATION,
                "Failed to cast font face",
            ));
        };

        let font_face_key = font_face.cast::<IUnknown>().unwrap().as_raw().addr();
        let font_id = context
            .text_system
            .font_info_cache
            .get(&font_face_key)
            .copied()
            // in some circumstances, we might be getting served a FontFace that we did not create ourselves
            // so create a new font from it and cache it accordingly. The usual culprit here seems to be Segoe UI Symbol
            .map_or_else(
                || {
                    let font = font_face_to_font(font_face, &self.locale)
                        .ok_or_else(|| Error::new(DWRITE_E_NOFONT, "Failed to create font"))?;
                    let font_id = match context.text_system.font_to_font_id.get(&font) {
                        Some(&font_id) => font_id,
                        None => context
                            .text_system
                            .select_and_cache_font(context.components, &font)
                            .ok_or_else(|| Error::new(DWRITE_E_NOFONT, "Failed to create font"))?,
                    };
                    context
                        .text_system
                        .font_info_cache
                        .insert(font_face_key, font_id);
                    windows::core::Result::Ok(font_id)
                },
                Ok,
            )?;

        let color_font = unsafe { font_face.IsColorFont().as_bool() };

        let glyph_ids = unsafe {
            slice_from_nullable(
                glyphrun.glyphIndices,
                glyph_count,
                "DirectWrite returned a null glyph indices array",
            )?
        };
        let glyph_advances = unsafe {
            slice_from_nullable(
                glyphrun.glyphAdvances,
                glyph_count,
                "DirectWrite returned a null glyph advances array",
            )?
        };
        let glyph_offsets = unsafe {
            slice_from_nullable(
                glyphrun.glyphOffsets,
                glyph_count,
                "DirectWrite returned a null glyph offsets array",
            )?
        };
        let cluster_map = unsafe {
            slice_from_nullable(
                desc.clusterMap,
                desc.stringLength as usize,
                "DirectWrite returned a null cluster map",
            )?
        };

        let cluster_analyzer = ClusterAnalyzer::new(cluster_map, glyph_count);
        let mut utf16_idx = desc.textPosition as usize;
        let mut glyph_idx = 0;
        let mut glyphs = Vec::with_capacity(glyph_count);
        for (cluster_utf16_len, cluster_glyph_count) in cluster_analyzer {
            context.index_converter.advance_to_utf16_ix(utf16_idx);
            utf16_idx += cluster_utf16_len;
            for (cluster_glyph_idx, glyph_id) in glyph_ids
                [glyph_idx..(glyph_idx + cluster_glyph_count)]
                .iter()
                .enumerate()
            {
                let id = GlyphId(*glyph_id as u32);
                let is_emoji =
                    color_font && is_color_glyph(font_face, id, &context.components.factory);
                let this_glyph_idx = glyph_idx + cluster_glyph_idx;
                glyphs.push(ShapedGlyph {
                    id,
                    position: point(
                        px(context.width + glyph_offsets[this_glyph_idx].advanceOffset),
                        px(-glyph_offsets[this_glyph_idx].ascenderOffset),
                    ),
                    render_offset: Point::default(),
                    font_size: px(glyphrun.fontEmSize),
                    index: context.index_converter.utf8_ix,
                    is_emoji,

                    is_cjk: false,
                });
                context.width += glyph_advances[this_glyph_idx];
            }
            glyph_idx += cluster_glyph_count;
        }
        context.runs.push(ShapedRun { font_id, glyphs });
        Ok(())
    }

    fn DrawUnderline(
        &self,
        _clientdrawingcontext: *const ::core::ffi::c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _underline: *const DWRITE_UNDERLINE,
        _clientdrawingeffect: windows::core::Ref<windows::core::IUnknown>,
    ) -> windows::core::Result<()> {
        Err(windows::core::Error::new(
            E_NOTIMPL,
            "DrawUnderline unimplemented",
        ))
    }

    fn DrawStrikethrough(
        &self,
        _clientdrawingcontext: *const ::core::ffi::c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _strikethrough: *const DWRITE_STRIKETHROUGH,
        _clientdrawingeffect: windows::core::Ref<windows::core::IUnknown>,
    ) -> windows::core::Result<()> {
        Err(windows::core::Error::new(
            E_NOTIMPL,
            "DrawStrikethrough unimplemented",
        ))
    }

    fn DrawInlineObject(
        &self,
        _clientdrawingcontext: *const ::core::ffi::c_void,
        _originx: f32,
        _originy: f32,
        _inlineobject: windows::core::Ref<IDWriteInlineObject>,
        _issideways: BOOL,
        _isrighttoleft: BOOL,
        _clientdrawingeffect: windows::core::Ref<windows::core::IUnknown>,
    ) -> windows::core::Result<()> {
        Err(windows::core::Error::new(
            E_NOTIMPL,
            "DrawInlineObject unimplemented",
        ))
    }
}

/// Interprets an optional DirectWrite array pointer as a slice, treating a
/// null pointer with a zero length as an empty slice. A null pointer with a
/// nonzero length fails with `null_error_message`.
///
/// # Safety
///
/// When `ptr` is non-null, the caller must guarantee that it points to a valid
/// array of at least `len` elements that outlives the returned slice.
unsafe fn slice_from_nullable<'a, T>(
    ptr: *const T,
    len: usize,
    null_error_message: &str,
) -> windows::core::Result<&'a [T]> {
    if ptr.is_null() {
        if len != 0 {
            return Err(Error::new(E_INVALIDARG, null_error_message));
        }
        Ok(&[])
    } else {
        Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
    }
}

struct StringIndexConverter<'a> {
    text: &'a str,
    utf8_ix: usize,
    utf16_ix: usize,
}

fn utf8_run_start(text: &str, start: usize) -> usize {
    let mut start = start.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    start
}

fn utf8_run_end(text: &str, start: usize, requested_len: usize) -> usize {
    let start = utf8_run_start(text, start);
    let requested_end = start.saturating_add(requested_len).min(text.len());
    if text.is_char_boundary(requested_end) {
        return requested_end;
    }
    let mut end = requested_end;
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

impl<'a> StringIndexConverter<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            utf8_ix: 0,
            utf16_ix: 0,
        }
    }

    #[allow(dead_code)]
    fn advance_to_utf8_ix(&mut self, utf8_target: usize) {
        for (ix, c) in self.text[self.utf8_ix..].char_indices() {
            if self.utf8_ix + ix >= utf8_target {
                self.utf8_ix += ix;
                return;
            }
            self.utf16_ix += c.len_utf16();
        }
        self.utf8_ix = self.text.len();
    }

    fn advance_to_utf16_ix(&mut self, utf16_target: usize) {
        for (ix, c) in self.text[self.utf8_ix..].char_indices() {
            if self.utf16_ix >= utf16_target {
                self.utf8_ix += ix;
                return;
            }
            self.utf16_ix += c.len_utf16();
        }
        self.utf8_ix = self.text.len();
    }
}

fn font_style_to_dwrite(style: FontStyle) -> DWRITE_FONT_STYLE {
    match style {
        FontStyle::Normal => DWRITE_FONT_STYLE_NORMAL,
        FontStyle::Italic => DWRITE_FONT_STYLE_ITALIC,
        FontStyle::Oblique => DWRITE_FONT_STYLE_OBLIQUE,
    }
}

fn font_style_from_dwrite(value: DWRITE_FONT_STYLE) -> FontStyle {
    match value.0 {
        0 => FontStyle::Normal,
        1 => FontStyle::Italic,
        2 => FontStyle::Oblique,
        _ => unreachable!(),
    }
}

fn font_weight_to_dwrite(weight: FontWeight) -> DWRITE_FONT_WEIGHT {
    DWRITE_FONT_WEIGHT(weight.0 as i32)
}

fn font_weight_from_dwrite(value: DWRITE_FONT_WEIGHT) -> FontWeight {
    FontWeight(value.0 as f32)
}

fn get_font_names_from_collection(
    collection: &IDWriteFontCollection1,
    locale: &HSTRING,
) -> Vec<String> {
    unsafe {
        let mut result = Vec::new();
        let family_count = collection.GetFontFamilyCount();
        for index in 0..family_count {
            let Some(font_family) = collection.GetFontFamily(index).log_err() else {
                continue;
            };
            let Some(localized_family_name) = font_family.GetFamilyNames().log_err() else {
                continue;
            };
            let Some(family_name) = get_name(localized_family_name, locale).log_err() else {
                continue;
            };
            result.push(family_name);
        }

        result
    }
}

fn font_face_to_font(font_face: &IDWriteFontFace3, locale: &HSTRING) -> Option<Font> {
    let localized_family_name = unsafe { font_face.GetFamilyNames().log_err() }?;
    let family_name = get_name(localized_family_name, locale).log_err()?;
    let weight = unsafe { font_face.GetWeight() };
    let style = unsafe { font_face.GetStyle() };
    Some(Font {
        family: family_name.into(),
        features: FontFeatures::default(),
        weight: font_weight_from_dwrite(weight),
        style: font_style_from_dwrite(style),
        fallbacks: None,
    })
}

// https://learn.microsoft.com/en-us/windows/win32/api/dwrite/ne-dwrite-dwrite_font_feature_tag
fn apply_font_features(
    direct_write_features: &IDWriteTypography,
    features: &FontFeatures,
) -> Result<()> {
    let tag_values = features.tag_value_list();
    if tag_values.is_empty() {
        return Ok(());
    }

    // All of these features are enabled by default by DirectWrite.
    // If you want to (and can) peek into the source of DirectWrite
    let mut feature_liga = make_direct_write_feature("liga", 1);
    let mut feature_clig = make_direct_write_feature("clig", 1);
    let mut feature_calt = make_direct_write_feature("calt", 1);

    for (tag, value) in tag_values {
        if tag.as_str() == "liga" && *value == 0 {
            feature_liga.parameter = 0;
            continue;
        }
        if tag.as_str() == "clig" && *value == 0 {
            feature_clig.parameter = 0;
            continue;
        }
        if tag.as_str() == "calt" && *value == 0 {
            feature_calt.parameter = 0;
            continue;
        }

        unsafe {
            direct_write_features.AddFontFeature(make_direct_write_feature(tag, *value))?;
        }
    }
    unsafe {
        direct_write_features.AddFontFeature(feature_liga)?;
        direct_write_features.AddFontFeature(feature_clig)?;
        direct_write_features.AddFontFeature(feature_calt)?;
    }

    Ok(())
}

#[inline]
const fn make_direct_write_feature(feature_name: &str, parameter: u32) -> DWRITE_FONT_FEATURE {
    let tag = make_direct_write_tag(feature_name);
    DWRITE_FONT_FEATURE {
        nameTag: tag,
        parameter,
    }
}

#[inline]
const fn make_open_type_tag(tag_name: &str) -> u32 {
    let bytes = tag_name.as_bytes();
    debug_assert!(bytes.len() == 4);
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[inline]
const fn make_direct_write_tag(tag_name: &str) -> DWRITE_FONT_FEATURE_TAG {
    DWRITE_FONT_FEATURE_TAG(make_open_type_tag(tag_name))
}

#[inline]
fn get_name(string: IDWriteLocalizedStrings, locale: &HSTRING) -> Result<String> {
    let mut locale_name_index = 0u32;
    let mut exists = BOOL(0);
    unsafe { string.FindLocaleName(locale, &mut locale_name_index, &mut exists as _)? };
    if !exists.as_bool() {
        unsafe {
            string.FindLocaleName(
                DEFAULT_LOCALE_NAME,
                &mut locale_name_index as _,
                &mut exists as _,
            )?
        };
        anyhow::ensure!(exists.as_bool(), "No localised string for {locale}");
    }

    let name_length = unsafe { string.GetStringLength(locale_name_index) }? as usize;
    let mut name_vec = vec![0u16; name_length + 1];
    unsafe {
        string.GetString(locale_name_index, &mut name_vec)?;
    }

    Ok(String::from_utf16_lossy(&name_vec[..name_length]))
}

fn get_system_subpixel_rendering() -> bool {
    let mut value = c_uint::default();
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETFONTSMOOTHINGTYPE,
            0,
            Some((&mut value) as *mut c_uint as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
    };
    if result.log_err().is_some() {
        value == FE_FONTSMOOTHINGCLEARTYPE
    } else {
        true
    }
}

fn should_use_subpixel_rendering(
    components: &DirectWriteComponents,
    params: &RenderGlyphParams,
) -> bool {
    should_use_subpixel_rendering_for_size(
        components.system_subpixel_rendering,
        params.is_emoji,
        params.font_size.0,
        params.scale_factor,
    )
}

fn should_use_subpixel_rendering_for_size(
    system_subpixel_rendering: bool,
    is_emoji: bool,
    font_size: f32,
    scale_factor: f32,
) -> bool {
    // Nova samples the subpixel atlas through a single-channel mono pipeline. For
    // small text, ClearType's three filtered channels otherwise accumulate into
    // visibly heavier stems after that reduction. Keep native ClearType for larger
    // glyphs while using grayscale coverage at UI-sized pixel heights.
    system_subpixel_rendering && !is_emoji && font_size * scale_factor >= 14.0
}

fn get_system_ui_font_name() -> SharedString {
    unsafe {
        let mut info: LOGFONTW = std::mem::zeroed();
        let font_family = if SystemParametersInfoW(
            SPI_GETICONTITLELOGFONT,
            std::mem::size_of::<LOGFONTW>() as u32,
            Some(&mut info as *mut _ as _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .log_err()
        .is_none()
        {
            // https://learn.microsoft.com/en-us/windows/win32/uxguide/vis-fonts
            // Segoe UI is the Windows font intended for user interface text strings.
            "Segoe UI".into()
        } else {
            let font_name = String::from_utf16_lossy(&info.lfFaceName);
            font_name.trim_matches(char::from(0)).to_owned().into()
        };
        log::info!("Use {} as UI font.", font_family);
        font_family
    }
}

// One would think that with newer DirectWrite method: IDWriteFontFace4::GetGlyphImageFormats
// but that doesn't seem to work for some glyphs, say ❤
fn is_color_glyph(
    font_face: &IDWriteFontFace3,
    glyph_id: GlyphId,
    factory: &IDWriteFactory5,
) -> bool {
    let glyph_run = DWRITE_GLYPH_RUN {
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
    unsafe {
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
    }
    .is_ok()
}

fn rect_for_bounds(bounds: Bounds<DevicePixels>) -> RECT {
    RECT {
        left: bounds.origin.x.0,
        top: bounds.origin.y.0,
        right: bounds.origin.x.0 + bounds.size.width.0,
        bottom: bounds.origin.y.0 + bounds.size.height.0,
    }
}

const DEFAULT_LOCALE_NAME: PCWSTR = windows::core::w!("en-US");

#[cfg(test)]
mod tests {
    use super::{
        ClusterAnalyzer, DirectWriteTextSystem, should_use_subpixel_rendering_for_size,
        utf8_run_end, utf8_run_start,
    };
    use crate::{
        FontRun, GlyphRasterization, PlatformTextSystem, RenderGlyphParams, font, point, px,
    };

    #[test]
    fn small_ui_text_uses_grayscale_coverage() {
        assert!(!should_use_subpixel_rendering_for_size(
            true, false, 12.5, 1.0
        ));
        assert!(!should_use_subpixel_rendering_for_size(
            true, true, 24.0, 1.0
        ));
        assert!(should_use_subpixel_rendering_for_size(
            true, false, 16.0, 1.0
        ));
        assert!(!should_use_subpixel_rendering_for_size(
            false, false, 16.0, 1.0
        ));
    }

    #[test]
    fn lays_out_and_rasterizes_system_text() -> anyhow::Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let font_id = text_system.font_id(&font("Segoe UI"))?;
        let layout = text_system.layout_line(
            "Hello",
            px(16.0),
            &[FontRun {
                len: "Hello".len(),
                font_id,
            }],
        );
        let glyph = layout
            .runs
            .first()
            .and_then(|run| run.glyphs.first())
            .ok_or_else(|| anyhow::anyhow!("DirectWrite produced no glyphs for system text"))?;
        let params = RenderGlyphParams {
            font_id: layout.runs[0].font_id,
            glyph_id: glyph.id,
            font_size: glyph.font_size,
            subpixel_variant: point(0, 0),
            scale_factor: 1.0,
            is_emoji: glyph.is_emoji,
            is_cjk: glyph.is_cjk,
        };
        let bounds = text_system.glyph_raster_bounds(&params)?;
        assert!(bounds.size.width.0 > 0);
        assert!(bounds.size.height.0 > 0);
        let rasterization = text_system.rasterize_glyph(&params, bounds)?;
        let GlyphRasterization::Bitmap { size, bytes } = rasterization else {
            anyhow::bail!("ordinary system text unexpectedly produced color layers");
        };
        assert_eq!(
            bytes.len(),
            size.width.0 as usize * size.height.0 as usize * 4
        );
        assert!(bytes.iter().any(|byte| *byte != 0));
        Ok(())
    }

    #[test]
    fn rasterizes_color_emoji_with_bgra_fallback() -> anyhow::Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        let text = "😀";
        let font_id = text_system.font_id(&font("Segoe UI Emoji"))?;
        let layout = text_system.layout_line(
            text,
            px(24.0),
            &[FontRun {
                len: text.len(),
                font_id,
            }],
        );
        let glyph = layout
            .runs
            .first()
            .and_then(|run| run.glyphs.first())
            .ok_or_else(|| anyhow::anyhow!("DirectWrite produced no glyph for color emoji"))?;
        assert!(glyph.is_emoji);
        let params = RenderGlyphParams {
            font_id: layout.runs[0].font_id,
            glyph_id: glyph.id,
            font_size: glyph.font_size,
            subpixel_variant: point(0, 0),
            scale_factor: 1.0,
            is_emoji: true,
            is_cjk: false,
        };
        let bounds = text_system.glyph_raster_bounds(&params)?;
        let rasterization = text_system.rasterize_glyph(&params, bounds)?;
        let GlyphRasterization::ColorLayers {
            size,
            layers,
            fallback,
        } = rasterization
        else {
            anyhow::bail!("color emoji did not produce DirectWrite color layers");
        };
        assert!(!layers.is_empty());
        assert_eq!(fallback.size, size);
        assert_eq!(
            fallback.bytes.len(),
            size.width.0 as usize * size.height.0 as usize * 4
        );
        assert!(
            layers
                .iter()
                .any(|layer| layer.alpha.iter().any(|alpha| *alpha != 0))
        );
        Ok(())
    }

    #[test]
    fn registering_font_data_invalidates_cached_selections() -> anyhow::Result<()> {
        let text_system = DirectWriteTextSystem::new()?;
        text_system.font_id(&font("Segoe UI"))?;
        assert!(!text_system.state.read().font_to_font_id.is_empty());

        let windows_directory =
            std::env::var_os("WINDIR").ok_or_else(|| anyhow::anyhow!("WINDIR is unavailable"))?;
        let font_path = std::path::PathBuf::from(windows_directory)
            .join("Fonts")
            .join("arial.ttf");
        text_system.add_font_paths(vec![font_path])?;

        assert!(text_system.state.read().font_to_font_id.is_empty());
        Ok(())
    }

    #[test]
    fn utf8_run_end_clamps_to_character_boundary() {
        let text = "诊".repeat(64);
        assert_eq!(utf8_run_end(&text, 0, 1), 0);
        assert_eq!(utf8_run_end(&text, 0, 3), 3);
        assert_eq!(utf8_run_start(&text, 1), 0);
        assert_eq!(utf8_run_end(&text, 3, 1), 3);
        assert!(text.is_char_boundary(utf8_run_end(&text, 0, 190)));
    }

    #[test]
    fn test_cluster_map() {
        let cluster_map = [0];
        let mut analyzer = ClusterAnalyzer::new(&cluster_map, 1);
        let next = analyzer.next();
        assert_eq!(next, Some((1, 1)));
        let next = analyzer.next();
        assert_eq!(next, None);

        let cluster_map = [0, 1, 2];
        let mut analyzer = ClusterAnalyzer::new(&cluster_map, 3);
        let next = analyzer.next();
        assert_eq!(next, Some((1, 1)));
        let next = analyzer.next();
        assert_eq!(next, Some((1, 1)));
        let next = analyzer.next();
        assert_eq!(next, Some((1, 1)));
        let next = analyzer.next();
        assert_eq!(next, None);
        // 👨‍👩‍👧‍👦👩‍💻
        let cluster_map = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4];
        let mut analyzer = ClusterAnalyzer::new(&cluster_map, 5);
        let next = analyzer.next();
        assert_eq!(next, Some((11, 4)));
        let next = analyzer.next();
        assert_eq!(next, Some((5, 1)));
        let next = analyzer.next();
        assert_eq!(next, None);
        // 👩‍💻
        let cluster_map = [0, 0, 0, 0, 0];
        let mut analyzer = ClusterAnalyzer::new(&cluster_map, 1);
        let next = analyzer.next();
        assert_eq!(next, Some((5, 1)));
        let next = analyzer.next();
        assert_eq!(next, None);
    }
}
