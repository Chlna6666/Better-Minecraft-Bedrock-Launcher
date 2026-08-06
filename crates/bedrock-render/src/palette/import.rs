use crate::{BedrockRenderError, Result};
use bedrock_world::{BlockState, NbtTag};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, OnceLock};

const BUILTIN_BLOCK_COLOR_JSON: &str = include_str!("../../data/colors/bedrock-block-color.json");
const BUILTIN_BIOME_COLOR_JSON: &str = include_str!("../../data/colors/bedrock-biome-color.json");
include!(concat!(env!("OUT_DIR"), "/builtin_palette_tables.rs"));

/// An 8-bit RGBA color used by palettes and decoded render planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbaColor {
    /// Red channel.
    pub red: u8,
    /// Green channel.
    pub green: u8,
    /// Blue channel.
    pub blue: u8,
    /// Alpha channel.
    pub alpha: u8,
}

impl RgbaColor {
    /// Creates a new color from red, green, blue, and alpha channels.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// Returns the color as `[red, green, blue, alpha]`.
    #[must_use]
    pub const fn to_array(self) -> [u8; 4] {
        [self.red, self.green, self.blue, self.alpha]
    }
}

/// Import counters returned after merging palette data.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PaletteImportReport {
    /// Number of block colors imported or overwritten.
    pub block_colors: usize,
    /// Number of biome colors imported or overwritten.
    pub biome_colors: usize,
    /// Number of entries skipped because they were incomplete or unsupported.
    pub skipped_entries: usize,
}

/// Color palette used by block, biome, surface, height, and cave render modes.
#[derive(Debug, Clone)]
pub struct RenderPalette {
    biome_colors: HashMap<u32, RgbaColor>,
    biome_grass_colors: HashMap<u32, RgbaColor>,
    biome_foliage_colors: HashMap<u32, RgbaColor>,
    biome_water_colors: HashMap<u32, RgbaColor>,
    block_colors: HashMap<String, RgbaColor>,
    block_state_colors: HashMap<String, BlockStateColorRules>,
    block_variant_colors: HashMap<String, HashMap<String, RgbaColor>>,
    unknown_biome_color: RgbaColor,
    unknown_block_color: RgbaColor,
    missing_chunk_color: RgbaColor,
    void_color: RgbaColor,
    air_color: RgbaColor,
    default_grass_color: RgbaColor,
    default_foliage_color: RgbaColor,
    default_water_color: RgbaColor,
    cave_air_color: RgbaColor,
    cave_solid_color: RgbaColor,
    cave_water_color: RgbaColor,
    cave_lava_color: RgbaColor,
    min_height_color: RgbaColor,
    max_height_color: RgbaColor,
}

#[derive(Debug, Clone)]
struct BlockStateColorRules {
    rules: Vec<BlockStateColorRule>,
}

#[derive(Debug, Clone)]
struct BlockStateColorRule {
    state_name: String,
    selector: StateColorSelector,
    color: RgbaColor,
}

#[derive(Debug, Clone)]
enum StateColorSelector {
    IntRange { min: i32, max: i32 },
    StringValues(Vec<String>),
}

impl Default for RenderPalette {
    fn default() -> Self {
        Self::from_builtin_static_tables()
    }
}

impl RenderPalette {
    /// Returns the shared embedded palette without reparsing bundled JSON.
    #[must_use]
    pub fn builtin_shared() -> Arc<Self> {
        static PALETTE: OnceLock<Arc<RenderPalette>> = OnceLock::new();
        Arc::clone(PALETTE.get_or_init(|| Arc::new(RenderPalette::default())))
    }

    /// Creates the default embedded palette.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the embedded palette from the auditable JSON sources.
    ///
    /// # Errors
    ///
    /// Returns an error if either embedded JSON source is invalid.
    pub fn from_builtin_json_sources() -> Result<Self> {
        let mut palette = Self::empty_with_builtin_defaults();
        palette.insert_default_blocks();
        palette.merge_json_str(BUILTIN_BLOCK_COLOR_JSON)?;
        palette.merge_json_str(BUILTIN_BIOME_COLOR_JSON)?;
        Ok(palette)
    }

    /// Builds the embedded palette from build-time generated static tables.
    #[must_use]
    pub fn from_builtin_static_tables() -> Self {
        let mut palette = Self::empty_with_builtin_defaults();
        palette.insert_default_blocks();
        palette.default_grass_color = rgba_from_static(BUILTIN_BIOME_DEFAULT_GRASS);
        palette.default_foliage_color = rgba_from_static(BUILTIN_BIOME_DEFAULT_FOLIAGE);
        palette.default_water_color = rgba_from_static(BUILTIN_BIOME_DEFAULT_WATER);
        palette
            .block_colors
            .reserve(BUILTIN_BLOCK_COLORS.len().saturating_add(8));
        for entry in BUILTIN_BLOCK_COLORS {
            palette
                .block_colors
                .insert(entry.name.to_string(), rgba_from_static(entry.color));
        }
        palette.biome_colors.reserve(BUILTIN_BIOME_COLORS.len());
        palette
            .biome_grass_colors
            .reserve(BUILTIN_BIOME_COLORS.len());
        palette
            .biome_foliage_colors
            .reserve(BUILTIN_BIOME_COLORS.len());
        palette
            .biome_water_colors
            .reserve(BUILTIN_BIOME_COLORS.len());
        for entry in BUILTIN_BIOME_COLORS {
            palette
                .biome_colors
                .insert(entry.id, rgba_from_static(entry.color));
            palette
                .biome_grass_colors
                .insert(entry.id, rgba_from_static(entry.grass));
            palette
                .biome_foliage_colors
                .insert(entry.id, rgba_from_static(entry.foliage));
            palette
                .biome_water_colors
                .insert(entry.id, rgba_from_static(entry.water));
        }
        for rule in BUILTIN_STATE_RULES {
            let rules = palette
                .block_state_colors
                .entry(rule.block.to_string())
                .or_insert_with(|| BlockStateColorRules { rules: Vec::new() });
            rules.rules.push(BlockStateColorRule {
                state_name: rule.state.to_string(),
                selector: selector_from_static(rule.selector),
                color: rgba_from_static(rule.color),
            });
        }
        for variant in BUILTIN_VARIANT_COLORS {
            palette
                .block_variant_colors
                .entry(variant.block.to_string())
                .or_default()
                .insert(variant.variant.to_string(), rgba_from_static(variant.color));
        }
        palette
    }

    fn empty_with_builtin_defaults() -> Self {
        Self {
            biome_colors: HashMap::new(),
            biome_grass_colors: HashMap::new(),
            biome_foliage_colors: HashMap::new(),
            biome_water_colors: HashMap::new(),
            block_colors: HashMap::new(),
            block_state_colors: HashMap::new(),
            block_variant_colors: HashMap::new(),
            unknown_biome_color: RgbaColor::new(255, 0, 255, 180),
            unknown_block_color: RgbaColor::new(255, 0, 255, 255),
            missing_chunk_color: RgbaColor::new(0, 0, 0, 0),
            void_color: RgbaColor::new(0, 0, 0, 0),
            air_color: RgbaColor::new(0, 0, 0, 0),
            default_grass_color: RgbaColor::new(98, 151, 64, 255),
            default_foliage_color: RgbaColor::new(62, 124, 50, 255),
            default_water_color: RgbaColor::new(28, 76, 158, 255),
            cave_air_color: RgbaColor::new(12, 12, 14, 255),
            cave_solid_color: RgbaColor::new(116, 116, 116, 255),
            cave_water_color: RgbaColor::new(38, 82, 180, 255),
            cave_lava_color: RgbaColor::new(255, 92, 12, 255),
            min_height_color: RgbaColor::new(36, 52, 100, 255),
            max_height_color: RgbaColor::new(242, 244, 232, 255),
        }
    }

    /// Returns the embedded block-color JSON source.
    #[must_use]
    pub fn builtin_block_color_json() -> &'static str {
        BUILTIN_BLOCK_COLOR_JSON
    }

    /// Returns the embedded biome-color JSON source.
    #[must_use]
    pub fn builtin_biome_color_json() -> &'static str {
        BUILTIN_BIOME_COLOR_JSON
    }

    /// Adds or replaces a biome color and returns the updated palette.
    #[must_use]
    pub fn with_biome_color(mut self, id: u32, color: RgbaColor) -> Self {
        self.biome_colors.insert(id, color);
        self
    }

    /// Adds or replaces a block color and returns the updated palette.
    #[must_use]
    pub fn with_block_color(mut self, name: impl Into<String>, color: RgbaColor) -> Self {
        let name = normalize_block_name(&name.into());
        self.block_colors.insert(name.clone(), color);
        self.block_state_colors.remove(&name);
        self.block_variant_colors.remove(&name);
        self
    }

    /// Changes the fallback color used for unknown biome IDs.
    #[must_use]
    pub fn with_unknown_biome_color(mut self, color: RgbaColor) -> Self {
        self.unknown_biome_color = color;
        self
    }

    /// Changes the fallback color used for unknown block names.
    #[must_use]
    pub fn with_unknown_block_color(mut self, color: RgbaColor) -> Self {
        self.unknown_block_color = color;
        self
    }

    /// Changes the low and high colors used by height-map rendering.
    #[must_use]
    pub fn with_height_gradient(mut self, min_color: RgbaColor, max_color: RgbaColor) -> Self {
        self.min_height_color = min_color;
        self.max_height_color = max_color;
        self
    }

    /// Changes cave diagnostic colors for air, solid, water, and lava.
    #[must_use]
    pub fn with_cave_colors(
        mut self,
        air: RgbaColor,
        solid: RgbaColor,
        water: RgbaColor,
        lava: RgbaColor,
    ) -> Self {
        self.cave_air_color = air;
        self.cave_solid_color = solid;
        self.cave_water_color = water;
        self.cave_lava_color = lava;
        self
    }

    /// Merges palette entries from a JSON file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if the JSON shape is invalid.
    pub fn merge_json_file(&mut self, path: impl AsRef<Path>) -> Result<PaletteImportReport> {
        let path = path.as_ref();
        let content = fs::read_to_string(path).map_err(|error| {
            BedrockRenderError::io(
                format!("failed to read palette JSON {}", path.display()),
                error,
            )
        })?;
        self.merge_json_str(&content)
    }

    /// Merges palette entries from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON cannot be parsed or contains invalid required fields.
    pub fn merge_json_str(&mut self, content: &str) -> Result<PaletteImportReport> {
        let value = serde_json::from_str::<Value>(content).map_err(|error| {
            BedrockRenderError::Validation(format!("invalid palette JSON: {error}"))
        })?;
        let mut report = PaletteImportReport::default();
        self.merge_json_value(&value, &mut report)?;
        Ok(report)
    }

    /// Returns the configured biome color or the unknown-biome fallback.
    #[must_use]
    pub fn biome_color(&self, id: u32) -> RgbaColor {
        self.biome_colors
            .get(&id)
            .copied()
            .unwrap_or(self.unknown_biome_color)
    }

    /// Returns a deterministic diagnostic color for known biome IDs.
    #[must_use]
    pub fn raw_biome_color(&self, id: u32) -> RgbaColor {
        if self.biome_colors.contains_key(&id) {
            return biome_hash_color(id);
        }
        self.unknown_biome_color
    }

    /// Returns whether this palette has an explicit color for the biome ID.
    #[must_use]
    pub fn has_biome_color(&self, id: u32) -> bool {
        self.biome_colors.contains_key(&id)
    }

    /// Returns the configured block color or the unknown-block fallback.
    #[must_use]
    pub fn block_color(&self, name: &str) -> RgbaColor {
        if is_air_block(name) {
            return self.air_color;
        }
        self.find_block_color(name)
            .unwrap_or(self.unknown_block_color)
    }

    /// Returns a surface block color with optional biome tinting applied.
    #[must_use]
    pub fn surface_block_color(
        &self,
        name: &str,
        biome_id: Option<u32>,
        biome_tint: bool,
    ) -> RgbaColor {
        let color = self.block_color(name);
        if is_grass_tinted_block(name) {
            if is_surface_grass_block(name) {
                return self.surface_grass_block_color(color, biome_id, biome_tint);
            }
            let tint = if biome_tint {
                self.biome_grass_tint(biome_id)
            } else {
                None
            };
            return multiply_with_biome_tint(color, tint, self.default_grass_color);
        }
        if is_foliage_tinted_block(name) {
            let tint = if biome_tint {
                self.biome_foliage_tint(biome_id)
            } else {
                None
            };
            return multiply_with_biome_tint(color, tint, self.default_foliage_color);
        }
        if is_water_block(name) {
            let tint = if biome_tint {
                self.biome_water_tint(biome_id)
            } else {
                None
            };
            let tinted = multiply_with_biome_tint(color, tint, self.default_water_color);
            return blend_toward_color(tinted, tint.unwrap_or(self.default_water_color), 190);
        }
        with_alpha(color, 255)
    }

    /// Returns a block color with JSON-defined state overrides applied.
    #[must_use]
    pub(crate) fn block_state_color(&self, state: &BlockState) -> RgbaColor {
        if is_air_block(&state.name) {
            return self.air_color;
        }
        self.find_state_color(state)
            .unwrap_or_else(|| self.block_color(&state.name))
    }

    /// Returns a JSON-defined variant color for a block, if one is available.
    #[must_use]
    pub(crate) fn block_variant_color(&self, name: &str, variant: &str) -> Option<RgbaColor> {
        let normalized = normalize_block_name(name);
        self.block_variant_colors
            .get(&normalized)
            .and_then(|variants| variants.get(variant))
            .copied()
            .or_else(|| {
                name.strip_prefix("minecraft:")
                    .and_then(|short_name| self.block_variant_colors.get(short_name))
                    .and_then(|variants| variants.get(variant))
                    .copied()
            })
    }

    pub(crate) fn surface_block_state_color_with_legacy_biome(
        &self,
        state: &BlockState,
        biome_id: Option<u32>,
        legacy_biome_color: Option<RgbaColor>,
        biome_tint: bool,
    ) -> RgbaColor {
        let color = self.block_state_color(state);
        let legacy_tint = legacy_biome_color.map(|color| with_alpha(color, 255));
        if is_grass_tinted_block(&state.name) {
            if is_surface_grass_block(&state.name) {
                let tint = if biome_tint {
                    legacy_tint.or_else(|| self.biome_grass_tint(biome_id))
                } else {
                    None
                };
                return self.surface_grass_block_color_from_tint(color, tint);
            }
            let tint = if biome_tint {
                legacy_tint.or_else(|| self.biome_grass_tint(biome_id))
            } else {
                None
            };
            return multiply_with_biome_tint(color, tint, self.default_grass_color);
        }
        if is_foliage_tinted_block(&state.name) {
            let tint = if biome_tint {
                legacy_tint.or_else(|| self.biome_foliage_tint(biome_id))
            } else {
                None
            };
            return multiply_with_biome_tint(color, tint, self.default_foliage_color);
        }
        if is_water_block(&state.name) {
            let tint = if biome_tint {
                self.biome_water_tint(biome_id)
            } else {
                None
            };
            let tinted = multiply_with_biome_tint(color, tint, self.default_water_color);
            return blend_toward_color(tinted, tint.unwrap_or(self.default_water_color), 190);
        }
        with_alpha(color, 255)
    }

    /// Blends a transparent water color over the block below it.
    #[must_use]
    pub fn transparent_water_color(
        &self,
        water_name: &str,
        under_name: Option<&str>,
        biome_id: Option<u32>,
        depth: u8,
        biome_tint: bool,
    ) -> RgbaColor {
        let water = self.surface_block_color(water_name, biome_id, biome_tint);
        let under = under_name.map_or(self.missing_chunk_color, |name| {
            self.surface_block_color(name, biome_id, biome_tint)
        });
        let water_alpha = shallow_water_alpha(depth);
        alpha_blend(with_alpha(water, water_alpha), under)
    }

    /// Applies simple relative height shading to a color.
    #[must_use]
    pub fn height_shaded_color(&self, color: RgbaColor, current: i16, reference: i16) -> RgbaColor {
        if color.alpha == 0 {
            return color;
        }
        let delta = i32::from(current) - i32::from(reference);
        if delta == 0 {
            return color;
        }
        let factor = (delta.clamp(-8, 8) * 5).clamp(-40, 40);
        shade_color(color, factor)
    }

    /// Applies normal-style shading from west/east/north/south height samples.
    #[allow(clippy::cast_possible_truncation)]
    #[must_use]
    pub fn height_normal_shaded_color(
        &self,
        color: RgbaColor,
        west: i16,
        east: i16,
        north: i16,
        south: i16,
    ) -> RgbaColor {
        if color.alpha == 0 {
            return color;
        }
        let dx = (f32::from(east) - f32::from(west)) * 1.2;
        let dz = (f32::from(south) - f32::from(north)) * 1.2;
        let normal_length = (dx.mul_add(dx, dz.mul_add(dz, 4.0)))
            .sqrt()
            .max(f32::EPSILON);
        let normal_x = -dx / normal_length;
        let normal_y = 2.0 / normal_length;
        let normal_z = -dz / normal_length;
        let azimuth = 315.0_f32.to_radians();
        let elevation = 45.0_f32.to_radians();
        let light_horizontal = elevation.cos();
        let light_x = azimuth.sin() * light_horizontal;
        let light_y = elevation.sin();
        let light_z = -azimuth.cos() * light_horizontal;
        let dot = normal_x.mul_add(light_x, normal_y.mul_add(light_y, normal_z * light_z));
        let relief = (dx.abs() + dz.abs()).min(24.0) / 24.0;
        let relative_light = dot - light_y;
        let mut factor = if relative_light >= 0.0 {
            relative_light * 35.0
        } else {
            relative_light * 55.0
        };
        factor -= relief * 8.0;
        let factor = factor.round().clamp(-70.0, 55.0) as i32;
        if factor == 0 {
            color
        } else {
            shade_color(color, factor)
        }
    }

    /// Returns whether a block name is recognized by exact or category color lookup.
    #[must_use]
    pub fn has_block_color(&self, name: &str) -> bool {
        is_air_block(name) || self.find_block_color(name).is_some()
    }

    /// Returns whether the name is treated as air.
    #[must_use]
    pub fn is_air_block(&self, name: &str) -> bool {
        is_air_block(name)
    }

    /// Returns whether the name is treated as water.
    #[must_use]
    pub fn is_water_block(&self, name: &str) -> bool {
        is_water_block(name)
    }

    #[must_use]
    fn biome_grass_tint(&self, biome_id: Option<u32>) -> Option<RgbaColor> {
        biome_id.and_then(|id| self.biome_grass_colors.get(&id).copied())
    }

    #[must_use]
    fn biome_foliage_tint(&self, biome_id: Option<u32>) -> Option<RgbaColor> {
        biome_id.and_then(|id| self.biome_foliage_colors.get(&id).copied())
    }

    #[must_use]
    fn biome_water_tint(&self, biome_id: Option<u32>) -> Option<RgbaColor> {
        biome_id.and_then(|id| self.biome_water_colors.get(&id).copied())
    }

    #[must_use]
    pub(crate) fn is_biome_surface_color(&self, color: RgbaColor) -> bool {
        let color = with_alpha(color, 255);
        self.biome_colors
            .values()
            .any(|candidate| with_alpha(*candidate, 255) == color)
    }

    fn surface_grass_block_color(
        &self,
        mask_color: RgbaColor,
        biome_id: Option<u32>,
        biome_tint: bool,
    ) -> RgbaColor {
        let tint = if biome_tint {
            self.biome_grass_tint(biome_id)
        } else {
            None
        };
        self.surface_grass_block_color_from_tint(mask_color, tint)
    }

    fn surface_grass_block_color_from_tint(
        &self,
        mask_color: RgbaColor,
        tint: Option<RgbaColor>,
    ) -> RgbaColor {
        let tint = tint.unwrap_or(self.default_grass_color);
        let tinted = multiply_with_biome_tint(mask_color, Some(tint), self.default_grass_color);
        blend_toward_color(tinted, tint, 96)
    }

    /// Returns the transparent color used for missing chunks.
    #[must_use]
    pub const fn missing_chunk_color(&self) -> RgbaColor {
        self.missing_chunk_color
    }

    /// Returns the transparent color used for void or empty space.
    #[must_use]
    pub const fn void_color(&self) -> RgbaColor {
        self.void_color
    }

    /// Returns the fallback color used for unknown biome IDs.
    #[must_use]
    pub const fn unknown_biome_color(&self) -> RgbaColor {
        self.unknown_biome_color
    }

    /// Returns the fallback color used for unknown block names.
    #[must_use]
    pub const fn unknown_block_color(&self) -> RgbaColor {
        self.unknown_block_color
    }

    /// Returns the height-gradient color for a height within a world range.
    #[must_use]
    pub fn height_color(&self, height: i16, min_height: i16, max_height: i16) -> RgbaColor {
        if min_height >= max_height {
            return self.max_height_color;
        }
        let numerator = i32::from(height.saturating_sub(min_height))
            .clamp(0, i32::from(max_height.saturating_sub(min_height)));
        let denominator = i32::from(max_height.saturating_sub(min_height)).max(1);
        lerp_color(
            self.min_height_color,
            self.max_height_color,
            numerator,
            denominator,
        )
    }

    /// Returns the cave diagnostic color for an optional block name.
    #[must_use]
    pub fn cave_color(&self, block_name: Option<&str>) -> RgbaColor {
        let Some(block_name) = block_name else {
            return self.cave_air_color;
        };
        if is_air_block(block_name) {
            return self.cave_air_color;
        }
        if block_name.contains("water") {
            return self.cave_water_color;
        }
        if block_name.contains("lava") {
            return self.cave_lava_color;
        }
        self.cave_solid_color
    }

    fn merge_json_value(&mut self, value: &Value, report: &mut PaletteImportReport) -> Result<()> {
        match value {
            Value::Object(map) => {
                if let Some(defaults) = map.get("defaults") {
                    self.merge_biome_defaults(defaults);
                }
                if let Some(blocks) = map.get("blocks").or_else(|| map.get("block_colors")) {
                    self.merge_block_json_value(blocks, report)?;
                }
                if let Some(biomes) = map.get("biomes").or_else(|| map.get("biome_colors")) {
                    self.merge_biome_json_value(biomes, report)?;
                }
                if !map.contains_key("blocks")
                    && !map.contains_key("block_colors")
                    && !map.contains_key("biomes")
                    && !map.contains_key("biome_colors")
                {
                    if looks_like_biome_map(map) || looks_like_named_biome_map(map) {
                        self.merge_biome_json_value(value, report)?;
                    } else {
                        self.merge_block_json_value(value, report)?;
                    }
                }
                Ok(())
            }
            Value::Array(_) => self.merge_block_json_value(value, report),
            _ => Err(BedrockRenderError::Validation(
                "palette JSON must be an object or an array".to_string(),
            )),
        }
    }

    fn merge_block_json_value(
        &mut self,
        value: &Value,
        report: &mut PaletteImportReport,
    ) -> Result<()> {
        match value {
            Value::Object(map) => {
                for (name, entry) in map {
                    let Some(color) = parse_block_color_entry(name, entry) else {
                        report.skipped_entries += 1;
                        continue;
                    };
                    let normalized_name = normalize_block_name(name);
                    self.block_colors.insert(normalized_name.clone(), color);
                    if let Some(rules) = parse_block_state_color_rules(entry) {
                        self.block_state_colors
                            .insert(normalized_name.clone(), rules);
                    } else {
                        self.block_state_colors.remove(&normalized_name);
                    }
                    if let Some(variants) = parse_block_variant_colors(entry) {
                        self.block_variant_colors
                            .insert(normalized_name.clone(), variants);
                    } else {
                        self.block_variant_colors.remove(&normalized_name);
                    }
                    report.block_colors += 1;
                }
                Ok(())
            }
            Value::Array(entries) => {
                for entry in entries {
                    let Some((name, color)) = parse_named_block_entry(entry) else {
                        report.skipped_entries += 1;
                        continue;
                    };
                    let normalized_name = normalize_block_name(&name);
                    self.block_colors.insert(normalized_name.clone(), color);
                    if let Some(rules) = parse_block_state_color_rules(entry) {
                        self.block_state_colors
                            .insert(normalized_name.clone(), rules);
                    } else {
                        self.block_state_colors.remove(&normalized_name);
                    }
                    if let Some(variants) = parse_block_variant_colors(entry) {
                        self.block_variant_colors
                            .insert(normalized_name.clone(), variants);
                    } else {
                        self.block_variant_colors.remove(&normalized_name);
                    }
                    report.block_colors += 1;
                }
                Ok(())
            }
            _ => Err(BedrockRenderError::Validation(
                "block palette JSON must be an object or array".to_string(),
            )),
        }
    }

    fn merge_biome_json_value(
        &mut self,
        value: &Value,
        report: &mut PaletteImportReport,
    ) -> Result<()> {
        match value {
            Value::Object(map) => {
                for (id, entry) in map {
                    let parsed_id = id
                        .parse::<u32>()
                        .ok()
                        .or_else(|| biome_id_from_entry(entry));
                    let Some(id) = parsed_id else {
                        report.skipped_entries += 1;
                        continue;
                    };
                    let Some(color) = parse_color(entry).or_else(|| parse_nested_color(entry))
                    else {
                        report.skipped_entries += 1;
                        continue;
                    };
                    self.biome_colors.insert(id, color);
                    self.merge_biome_tints(id, entry);
                    report.biome_colors += 1;
                }
                Ok(())
            }
            Value::Array(entries) => {
                for entry in entries {
                    let Some((id, color)) = parse_named_biome_entry(entry) else {
                        report.skipped_entries += 1;
                        continue;
                    };
                    self.biome_colors.insert(id, color);
                    self.merge_biome_tints(id, entry);
                    report.biome_colors += 1;
                }
                Ok(())
            }
            _ => Err(BedrockRenderError::Validation(
                "biome palette JSON must be an object or array".to_string(),
            )),
        }
    }

    fn merge_biome_tints(&mut self, id: u32, entry: &Value) {
        let Value::Object(map) = entry else {
            return;
        };
        if let Some(color) = map.get("grass").and_then(parse_color) {
            self.biome_grass_colors.insert(id, with_alpha(color, 255));
        }
        if let Some(color) = map.get("leaves").and_then(parse_color) {
            self.biome_foliage_colors.insert(id, with_alpha(color, 255));
        }
        if let Some(color) = map.get("water").and_then(parse_color) {
            self.biome_water_colors.insert(id, with_alpha(color, 255));
        }
        if id == 1 {
            if let Some(color) = self.biome_grass_colors.get(&id).copied() {
                self.default_grass_color = color;
            }
            if let Some(color) = self.biome_foliage_colors.get(&id).copied() {
                self.default_foliage_color = color;
            }
            if let Some(color) = self.biome_water_colors.get(&id).copied() {
                self.default_water_color = color;
            }
        }
    }

    fn merge_biome_defaults(&mut self, value: &Value) {
        let Value::Object(map) = value else {
            return;
        };
        if let Some(color) = map.get("grass").and_then(parse_color) {
            self.default_grass_color = with_alpha(color, 255);
        }
        if let Some(color) = map
            .get("leaves")
            .or_else(|| map.get("foliage"))
            .and_then(parse_color)
        {
            self.default_foliage_color = with_alpha(color, 255);
        }
        if let Some(color) = map.get("water").and_then(parse_color) {
            self.default_water_color = with_alpha(color, 255);
        }
    }

    fn insert_default_blocks(&mut self) {
        for name in [
            "air",
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:structure_void",
            "minecraft:light",
            "minecraft:light_block",
        ] {
            self.block_colors.insert(name.to_string(), self.air_color);
        }
    }
    fn find_block_color(&self, name: &str) -> Option<RgbaColor> {
        self.block_colors
            .get(name)
            .copied()
            .or_else(|| {
                name.strip_prefix("minecraft:")
                    .and_then(|short_name| self.block_colors.get(short_name).copied())
            })
            .or_else(|| category_block_color(name))
    }

    fn find_state_color(&self, state: &BlockState) -> Option<RgbaColor> {
        let normalized = normalize_block_name(&state.name);
        self.block_state_colors
            .get(&normalized)
            .and_then(|rules| rules.color_for_state(state))
            .or_else(|| {
                state
                    .name
                    .strip_prefix("minecraft:")
                    .and_then(|short_name| self.block_state_colors.get(short_name))
                    .and_then(|rules| rules.color_for_state(state))
            })
    }
}

impl BlockStateColorRules {
    fn color_for_state(&self, state: &BlockState) -> Option<RgbaColor> {
        self.rules.iter().find_map(|rule| rule.matches(state))
    }
}

impl BlockStateColorRule {
    fn matches(&self, state: &BlockState) -> Option<RgbaColor> {
        match &self.selector {
            StateColorSelector::IntRange { min, max } => {
                let value = state_int(state, &self.state_name)?;
                ((*min..=*max).contains(&value)).then_some(self.color)
            }
            StateColorSelector::StringValues(values) => {
                let value = state_string(state, &self.state_name)?;
                values
                    .iter()
                    .any(|candidate| candidate == value)
                    .then_some(self.color)
            }
        }
    }
}

fn is_air_block(name: &str) -> bool {
    matches!(
        name,
        "air"
            | "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:structure_void"
            | "minecraft:light_block"
            | "minecraft:light"
    )
}

fn state_int(state: &BlockState, key: &str) -> Option<i32> {
    let value = state
        .states
        .get(key)
        .or_else(|| state.states.get(&format!("minecraft:{key}")))?;
    match value {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn state_string<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    let value = state
        .states
        .get(key)
        .or_else(|| state.states.get(&format!("minecraft:{key}")))?;
    match value {
        NbtTag::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn looks_like_biome_map(map: &serde_json::Map<String, Value>) -> bool {
    !map.is_empty() && map.keys().all(|key| key.parse::<u32>().is_ok())
}

fn looks_like_named_biome_map(map: &serde_json::Map<String, Value>) -> bool {
    !map.is_empty()
        && map.values().any(|value| {
            let Value::Object(entry) = value else {
                return false;
            };
            entry.contains_key("id") && entry.contains_key("rgb")
        })
}

fn normalize_block_name(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

fn rgba_from_static(color: StaticColor) -> RgbaColor {
    RgbaColor::new(color.red, color.green, color.blue, color.alpha)
}

fn selector_from_static(selector: StaticStateSelector) -> StateColorSelector {
    match selector {
        StaticStateSelector::IntRange { min, max } => StateColorSelector::IntRange { min, max },
        StaticStateSelector::StringValues(values) => StateColorSelector::StringValues(
            values.iter().map(|value| (*value).to_string()).collect(),
        ),
    }
}

fn parse_named_block_entry(value: &Value) -> Option<(String, RgbaColor)> {
    let Value::Object(map) = value else {
        return None;
    };
    let name = map
        .get("name")
        .or_else(|| map.get("id"))
        .or_else(|| map.get("identifier"))
        .and_then(Value::as_str)?;
    let color = parse_block_color_entry(name, value)?;
    Some((name.to_string(), color))
}

fn parse_named_biome_entry(value: &Value) -> Option<(u32, RgbaColor)> {
    let id = biome_id_from_entry(value)?;
    let color = parse_color(value).or_else(|| parse_nested_color(value))?;
    Some((id, color))
}

fn biome_id_from_entry(value: &Value) -> Option<u32> {
    let Value::Object(map) = value else {
        return None;
    };
    map.get("id")
        .or_else(|| map.get("biome_id"))
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
}

fn parse_nested_color(value: &Value) -> Option<RgbaColor> {
    let Value::Object(map) = value else {
        return None;
    };
    for key in ["default", "color", "map_color", "rgba", "rgb"] {
        if let Some(color) = map.get(key).and_then(parse_color) {
            return Some(color);
        }
    }
    average_child_colors(map)
}

fn parse_block_color_entry(block_name: &str, value: &Value) -> Option<RgbaColor> {
    parse_color(value).or_else(|| parse_nested_block_color(block_name, value))
}

fn parse_nested_block_color(block_name: &str, value: &Value) -> Option<RgbaColor> {
    let Value::Object(map) = value else {
        return None;
    };
    for key in ["default", "color", "map_color", "rgba", "rgb"] {
        if let Some(color) = map.get(key).and_then(parse_color) {
            return Some(color);
        }
    }
    best_child_color_for_block(block_name, map).or_else(|| average_child_colors(map))
}

fn parse_block_state_color_rules(value: &Value) -> Option<BlockStateColorRules> {
    let map = value.as_object()?;
    let state_colors = map.get("state_colors")?.as_object()?;
    let mut rules = Vec::new();
    for (state_name, state_rules) in state_colors {
        for rule in state_rules.as_array()? {
            let rule_map = rule.as_object()?;
            let color = rule_map.get("color").and_then(parse_color)?;
            if let Some(values) = rule_map.get("values").and_then(Value::as_array) {
                let values = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if !values.is_empty() {
                    rules.push(BlockStateColorRule {
                        state_name: state_name.clone(),
                        selector: StateColorSelector::StringValues(values),
                        color,
                    });
                }
                continue;
            }
            let min_value = rule_map
                .get("min")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())?;
            let max_value = rule_map
                .get("max")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())?;
            rules.push(BlockStateColorRule {
                state_name: state_name.clone(),
                selector: StateColorSelector::IntRange {
                    min: min_value,
                    max: max_value,
                },
                color,
            });
        }
    }
    (!rules.is_empty()).then_some(BlockStateColorRules { rules })
}

fn parse_block_variant_colors(value: &Value) -> Option<HashMap<String, RgbaColor>> {
    let map = value.as_object()?;
    let variant_colors = map.get("variant_colors")?.as_object()?;
    let mut variants = HashMap::new();
    for (variant, color) in variant_colors {
        if let Some(color) = parse_color(color) {
            variants.insert(variant.clone(), color);
        }
    }
    (!variants.is_empty()).then_some(variants)
}

fn best_child_color_for_block(
    block_name: &str,
    map: &serde_json::Map<String, Value>,
) -> Option<RgbaColor> {
    let block_tokens = block_name_tokens(block_name);
    let mut best: Option<(i32, u8, RgbaColor)> = None;
    for (texture_name, value) in map {
        let Some(color) = parse_color(value) else {
            continue;
        };
        let score = texture_match_score(texture_name, &block_tokens);
        let candidate = (score, color.alpha, color);
        if best.is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1)) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, color)| color)
}

fn block_name_tokens(block_name: &str) -> Vec<&str> {
    let short_name = block_name
        .strip_prefix("minecraft:")
        .unwrap_or(block_name)
        .trim();
    short_name
        .split(['_', ':'])
        .filter(|token| !token.is_empty() && !matches!(*token, "block" | "item"))
        .collect()
}

fn texture_match_score(texture_name: &str, block_tokens: &[&str]) -> i32 {
    let mut score = 0_i32;
    let texture_tokens = texture_name.split(['_', ':']).collect::<Vec<_>>();
    for token in block_tokens {
        if texture_tokens
            .iter()
            .any(|texture_token| texture_token == token)
        {
            score += 16;
        } else if texture_name.contains(token) {
            score += 8;
        }
    }
    if texture_tokens
        .iter()
        .any(|token| matches!(*token, "top" | "upper" | "opaque"))
    {
        score += 2;
    }
    if texture_tokens
        .iter()
        .any(|token| matches!(*token, "side" | "lower" | "bottom"))
    {
        score -= 1;
    }
    score
}

fn average_child_colors(map: &serde_json::Map<String, Value>) -> Option<RgbaColor> {
    let mut red = 0_u32;
    let mut green = 0_u32;
    let mut blue = 0_u32;
    let mut alpha = 0_u32;
    let mut count = 0_u32;
    for value in map.values() {
        let Some(color) = parse_color(value) else {
            continue;
        };
        red += u32::from(color.red);
        green += u32::from(color.green);
        blue += u32::from(color.blue);
        alpha += u32::from(color.alpha);
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some(RgbaColor::new(
        u8_from_u32(red / count),
        u8_from_u32(green / count),
        u8_from_u32(blue / count),
        u8_from_u32(alpha / count),
    ))
}

fn parse_color(value: &Value) -> Option<RgbaColor> {
    match value {
        Value::String(color) => parse_hex_color(color),
        Value::Array(channels) => parse_color_array(channels),
        Value::Object(map) => parse_color_object(map),
        Value::Number(number) => number.as_u64().and_then(parse_integer_color),
        _ => None,
    }
}

fn parse_color_array(channels: &[Value]) -> Option<RgbaColor> {
    if !(channels.len() == 3 || channels.len() == 4) {
        return None;
    }
    let red = parse_u8_channel(channels.first()?)?;
    let green = parse_u8_channel(channels.get(1)?)?;
    let blue = parse_u8_channel(channels.get(2)?)?;
    let alpha = channels.get(3).map_or(Some(255), parse_u8_channel)?;
    Some(RgbaColor::new(red, green, blue, alpha))
}

fn parse_color_object(map: &serde_json::Map<String, Value>) -> Option<RgbaColor> {
    let red = map
        .get("red")
        .or_else(|| map.get("r"))
        .and_then(parse_u8_channel)?;
    let green = map
        .get("green")
        .or_else(|| map.get("g"))
        .and_then(parse_u8_channel)?;
    let blue = map
        .get("blue")
        .or_else(|| map.get("b"))
        .and_then(parse_u8_channel)?;
    let alpha = map
        .get("alpha")
        .or_else(|| map.get("a"))
        .map_or(Some(255), parse_u8_channel)?;
    Some(RgbaColor::new(red, green, blue, alpha))
}

fn parse_u8_channel(value: &Value) -> Option<u8> {
    value
        .as_u64()
        .and_then(|channel| u8::try_from(channel).ok())
}

fn parse_hex_color(value: &str) -> Option<RgbaColor> {
    let value = value
        .trim()
        .trim_start_matches('#')
        .trim_start_matches("0x");
    match value.len() {
        6 => {
            let color = u32::from_str_radix(value, 16).ok()?;
            Some(RgbaColor::new(
                u8_from_u32((color >> 16) & 0xff),
                u8_from_u32((color >> 8) & 0xff),
                u8_from_u32(color & 0xff),
                255,
            ))
        }
        8 => {
            let color = u32::from_str_radix(value, 16).ok()?;
            Some(RgbaColor::new(
                u8_from_u32((color >> 24) & 0xff),
                u8_from_u32((color >> 16) & 0xff),
                u8_from_u32((color >> 8) & 0xff),
                u8_from_u32(color & 0xff),
            ))
        }
        _ => None,
    }
}

fn parse_integer_color(value: u64) -> Option<RgbaColor> {
    let color = u32::try_from(value).ok()?;
    Some(RgbaColor::new(
        u8_from_u32((color >> 16) & 0xff),
        u8_from_u32((color >> 8) & 0xff),
        u8_from_u32(color & 0xff),
        255,
    ))
}

fn is_water_block(name: &str) -> bool {
    matches!(
        name,
        "water" | "flowing_water" | "minecraft:water" | "minecraft:flowing_water"
    )
}

fn is_grass_tinted_block(name: &str) -> bool {
    if is_untinted_path_block(name) {
        return false;
    }
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == "grass"
        || name == "short_grass"
        || name == "tall_grass"
        || name.contains("grass_block")
        || name.contains("fern")
        || name.contains("vine")
}

fn is_surface_grass_block(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == "grass" || name.contains("grass_block")
}

fn is_foliage_tinted_block(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    if name.contains("leaf_litter") {
        return false;
    }
    name.contains("leaves")
        || name.contains("leaf")
        || name.contains("leave")
        || name.contains("foliage")
}

fn is_untinted_path_block(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(name, "grass_path" | "dirt_path")
}

#[allow(clippy::too_many_lines)]
fn category_block_color(name: &str) -> Option<RgbaColor> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    if is_untinted_path_block(name) {
        return Some(RgbaColor::new(154, 118, 62, 255));
    }
    if matches!(name, "bamboo" | "bamboo_sapling") {
        return Some(RgbaColor::new(68, 176, 32, 255));
    }
    if name == "decorated_pot" {
        return Some(RgbaColor::new(132, 94, 54, 255));
    }
    if name.contains("bamboo")
        && (name.contains("planks")
            || name.contains("mosaic")
            || name.contains("block")
            || name.contains("door")
            || name.contains("trapdoor")
            || name.contains("fence")
            || name.contains("slab")
            || name.contains("stairs"))
    {
        return Some(RgbaColor::new(174, 150, 70, 255));
    }
    if name.contains("coral") {
        return Some(RgbaColor::new(170, 78, 100, 255));
    }
    if is_water_block(name) {
        return Some(RgbaColor::new(168, 190, 224, 210));
    }
    if is_grass_tinted_block(name) {
        return Some(RgbaColor::new(214, 214, 214, 255));
    }
    if is_foliage_tinted_block(name) {
        return Some(RgbaColor::new(206, 206, 206, 238));
    }
    if name.contains("copper") {
        return Some(RgbaColor::new(150, 84, 60, 255));
    }
    if name.contains("resin") {
        return Some(RgbaColor::new(194, 86, 24, 255));
    }
    if name.contains("amethyst") {
        return Some(RgbaColor::new(132, 88, 176, 255));
    }
    if name.contains("prismarine") {
        return Some(RgbaColor::new(62, 124, 120, 255));
    }
    if name.contains("basalt") || name.contains("blackstone") {
        return Some(RgbaColor::new(42, 38, 45, 255));
    }
    if name.contains("netherrack") || name.contains("nylium") || name.contains("wart") {
        return Some(RgbaColor::new(90, 34, 40, 255));
    }
    if name.contains("end_stone") || name.contains("end_brick") {
        return Some(RgbaColor::new(190, 190, 130, 255));
    }
    if name.contains("ore") {
        return Some(RgbaColor::new(96, 98, 98, 255));
    }
    if name.contains("stone")
        || name.contains("slate")
        || name.contains("brick")
        || name.contains("polished")
        || name.contains("smooth")
        || name.contains("chiseled")
        || name.contains("tile")
    {
        return Some(RgbaColor::new(100, 102, 100, 255));
    }
    if name.contains("log")
        || name.contains("stem")
        || name.contains("wood")
        || name.contains("hyphae")
    {
        return Some(RgbaColor::new(86, 60, 36, 255));
    }
    if name.contains("planks") {
        return Some(RgbaColor::new(126, 96, 56, 255));
    }
    if name.contains("stairs")
        || name.contains("slab")
        || name.contains("fence")
        || name.contains("door")
        || name.contains("trapdoor")
        || name.contains("button")
        || name.contains("pressure_plate")
        || name.contains("sign")
        || name.contains("hanging_sign")
        || name.contains("ladder")
        || name.contains("chest")
        || name.contains("barrel")
        || name.contains("bookshelf")
    {
        return Some(RgbaColor::new(118, 82, 48, 255));
    }
    if name.contains("flower")
        || name.contains("tulip")
        || name.contains("daisy")
        || name.contains("orchid")
        || name.contains("allium")
        || name.contains("cornflower")
        || name.contains("peony")
        || name.contains("lilac")
        || name.contains("rose")
        || name.contains("petals")
        || name.contains("spore_blossom")
    {
        return Some(RgbaColor::new(178, 104, 78, 255));
    }
    if name.contains("grass")
        || name.contains("fern")
        || name.contains("bush")
        || name.contains("sapling")
        || name.contains("crop")
        || name.contains("wheat")
        || name.contains("carrots")
        || name.contains("potatoes")
        || name.contains("beetroot")
        || name.contains("melon_stem")
        || name.contains("pumpkin_stem")
        || name.contains("seagrass")
        || name.contains("kelp")
        || name.contains("lily_pad")
        || name.contains("moss")
        || name.contains("roots")
        || name.contains("azalea")
        || name.contains("propagule")
        || name.contains("cactus")
        || name.contains("sugar_cane")
    {
        return Some(RgbaColor::new(58, 124, 46, 255));
    }
    if name.contains("hay") || name.contains("sponge") || name.contains("honey") {
        return Some(RgbaColor::new(180, 140, 48, 255));
    }
    if name.contains("mushroom") {
        return Some(RgbaColor::new(126, 78, 60, 255));
    }
    if name.contains("torch")
        || name.contains("lantern")
        || name.contains("rail")
        || name.contains("redstone")
        || name.contains("repeater")
        || name.contains("comparator")
    {
        return Some(RgbaColor::new(150, 112, 60, 255));
    }
    if name.contains("sand") || name.contains("beach") {
        return Some(RgbaColor::new(184, 168, 112, 255));
    }
    if name.contains("mud") || name.contains("dirt") || name.contains("path") {
        return Some(RgbaColor::new(104, 72, 50, 255));
    }
    if name.contains("terracotta") {
        return Some(RgbaColor::new(126, 70, 52, 255));
    }
    if name.contains("concrete") {
        return Some(RgbaColor::new(112, 113, 112, 255));
    }
    if name.contains("wool") || name.contains("carpet") {
        return Some(RgbaColor::new(164, 166, 164, 255));
    }
    if name.contains("glass") {
        return Some(RgbaColor::new(150, 194, 208, 128));
    }
    if name.contains("snow") {
        return Some(RgbaColor::new(222, 228, 224, 255));
    }
    if name.contains("ice") || name.contains("frosted") {
        return Some(RgbaColor::new(112, 174, 214, 220));
    }
    if name.contains("lava") {
        return Some(RgbaColor::new(214, 72, 18, 255));
    }
    if name.contains("obsidian") {
        return Some(RgbaColor::new(25, 20, 36, 255));
    }
    if name.contains("bedrock") {
        return Some(RgbaColor::new(82, 82, 82, 255));
    }
    None
}

fn multiply_with_biome_tint(
    base: RgbaColor,
    tint: Option<RgbaColor>,
    default_tint: RgbaColor,
) -> RgbaColor {
    let tint = tint.unwrap_or(default_tint);
    RgbaColor::new(
        multiply_channel(base.red, tint.red),
        multiply_channel(base.green, tint.green),
        multiply_channel(base.blue, tint.blue),
        255,
    )
}

fn blend_toward_color(base: RgbaColor, target: RgbaColor, target_weight: u16) -> RgbaColor {
    let target_weight = target_weight.min(255);
    let base_weight = 255_u16.saturating_sub(target_weight);
    RgbaColor::new(
        weighted_channel(base.red, target.red, base_weight, target_weight),
        weighted_channel(base.green, target.green, base_weight, target_weight),
        weighted_channel(base.blue, target.blue, base_weight, target_weight),
        255,
    )
}

fn weighted_channel(base: u8, target: u8, base_weight: u16, target_weight: u16) -> u8 {
    let value = (u16::from(base) * base_weight + u16::from(target) * target_weight + 127) / 255;
    u8_from_u16(value)
}

fn multiply_channel(base: u8, tint: u8) -> u8 {
    let value = (u16::from(base) * u16::from(tint)) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn with_alpha(color: RgbaColor, alpha: u8) -> RgbaColor {
    RgbaColor::new(color.red, color.green, color.blue, alpha)
}

fn alpha_blend(foreground: RgbaColor, background: RgbaColor) -> RgbaColor {
    let alpha = u16::from(foreground.alpha);
    let inverse = 255_u16.saturating_sub(alpha);
    RgbaColor::new(
        u8_from_u16(
            ((u16::from(foreground.red) * alpha) + (u16::from(background.red) * inverse)) / 255,
        ),
        u8_from_u16(
            ((u16::from(foreground.green) * alpha) + (u16::from(background.green) * inverse)) / 255,
        ),
        u8_from_u16(
            ((u16::from(foreground.blue) * alpha) + (u16::from(background.blue) * inverse)) / 255,
        ),
        255,
    )
}

fn shallow_water_alpha(depth: u8) -> u8 {
    match depth {
        0 | 1 => 40,
        2 => 60,
        3 => 66,
        4 => 88,
        5 => 110,
        6 => 145,
        _ => 180,
    }
}

fn shade_color(color: RgbaColor, factor: i32) -> RgbaColor {
    RgbaColor::new(
        shade_channel(color.red, factor),
        shade_channel(color.green, factor),
        shade_channel(color.blue, factor),
        color.alpha,
    )
}

fn shade_channel(channel: u8, factor: i32) -> u8 {
    if factor >= 0 {
        let value = i32::from(channel) + ((255 - i32::from(channel)) * factor / 100);
        u8_from_i32(value.clamp(0, 255))
    } else {
        let value = i32::from(channel) * (100 + factor) / 100;
        u8_from_i32(value.clamp(0, 255))
    }
}

fn lerp_color(start: RgbaColor, end: RgbaColor, numerator: i32, denominator: i32) -> RgbaColor {
    RgbaColor::new(
        lerp_channel(start.red, end.red, numerator, denominator),
        lerp_channel(start.green, end.green, numerator, denominator),
        lerp_channel(start.blue, end.blue, numerator, denominator),
        lerp_channel(start.alpha, end.alpha, numerator, denominator),
    )
}

fn lerp_channel(start: u8, end: u8, numerator: i32, denominator: i32) -> u8 {
    let value =
        i32::from(start) + (i32::from(end) - i32::from(start)) * numerator / denominator.max(1);
    u8_from_i32(value.clamp(0, 255))
}

fn biome_hash_color(id: u32) -> RgbaColor {
    let hash = id.wrapping_mul(0x045d_9f3b);
    RgbaColor::new(
        64 + u8_from_u32(hash & 0x7f),
        64 + u8_from_u32((hash >> 8) & 0x7f),
        64 + u8_from_u32((hash >> 16) & 0x7f),
        255,
    )
}

fn u8_from_u16(value: u16) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn u8_from_u32(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn u8_from_i32(value: i32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_palette_imports_blocks_and_biomes() {
        let mut palette = RenderPalette::default();
        let report = palette
            .merge_json_str(
                r##"{
                    "blocks": {
                        "minecraft:test_block": "#112233",
                        "test_alpha": [10, 20, 30, 40]
                    },
                    "biomes": {
                        "900": {"color": "#445566ff"}
                    }
                }"##,
            )
            .expect("valid palette json should import");

        assert_eq!(report.block_colors, 2);
        assert_eq!(report.biome_colors, 1);
        assert_eq!(
            palette.block_color("minecraft:test_block"),
            RgbaColor::new(17, 34, 51, 255)
        );
        assert_eq!(
            palette.block_color("minecraft:test_alpha"),
            RgbaColor::new(10, 20, 30, 40)
        );
        assert_eq!(palette.biome_color(900), RgbaColor::new(68, 85, 102, 255));
    }

    #[test]
    fn json_palette_imports_variant_colors() {
        let mut palette = RenderPalette::default();
        palette
            .merge_json_str(
                r#"{
                    "blocks": {
                        "minecraft:standing_banner": {
                            "default": [255, 255, 255, 255],
                            "variant_colors": {
                                "banner_base_red": [160, 39, 34, 255]
                            }
                        }
                    }
                }"#,
            )
            .expect("variant colors should import");

        assert_eq!(
            palette.block_variant_color("minecraft:standing_banner", "banner_base_red"),
            Some(RgbaColor::new(160, 39, 34, 255))
        );
    }

    #[test]
    fn json_palette_imports_string_and_numeric_state_colors() {
        let mut palette = RenderPalette::default();
        palette
            .merge_json_str(
                r#"{
                    "blocks": {
                        "minecraft:concrete": {
                            "default": [220, 220, 220, 255],
                            "state_colors": {
                                "color": [
                                    {"min": 14, "max": 14, "color": [160, 39, 34, 255]},
                                    {"values": ["blue"], "color": [44, 48, 138, 255]}
                                ]
                            }
                        }
                    }
                }"#,
            )
            .expect("state colors should import");

        assert_eq!(
            palette.block_state_color(&test_block_state_with_int(
                "minecraft:concrete",
                "color",
                14
            )),
            RgbaColor::new(160, 39, 34, 255)
        );
        assert_eq!(
            palette.block_state_color(&test_block_state_with_string(
                "minecraft:concrete",
                "color",
                "blue"
            )),
            RgbaColor::new(44, 48, 138, 255)
        );
    }

    #[test]
    fn legacy_object_map_palette_json_imports() {
        let mut palette = RenderPalette::default();
        let report = palette
            .merge_json_str(
                r#"{
                    "minecraft:sample_multi_texture": {
                        "texture_a": [10, 20, 30, 255],
                        "texture_b": [30, 40, 50, 255]
                    },
                    "plains": {
                        "id": 1,
                        "rgb": [141, 179, 96],
                        "grass": [142, 185, 113],
                        "leaves": [113, 167, 77],
                        "water": [63, 118, 228]
                    }
                }"#,
            )
            .expect("legacy object-map style json should import");

        assert_eq!(report.block_colors, 0);
        assert_eq!(report.biome_colors, 1);
        assert_eq!(palette.biome_color(1), RgbaColor::new(141, 179, 96, 255));

        let mut block_palette = RenderPalette::default();
        let report = block_palette
            .merge_json_str(
                r#"{
                    "minecraft:sample_multi_texture": {
                        "texture_a": [10, 20, 30, 255],
                        "texture_b": [30, 40, 50, 255]
                    }
                }"#,
            )
            .expect("legacy object-map style block json should import");
        assert_eq!(report.block_colors, 1);
        assert_eq!(
            block_palette.block_color("minecraft:sample_multi_texture"),
            RgbaColor::new(10, 20, 30, 255)
        );

        let mut biome_palette = RenderPalette::default();
        let report = biome_palette
            .merge_json_str(
                r#"{
                    "plains": {
                        "id": 901,
                        "rgb": [141, 179, 96],
                        "grass": [142, 185, 113]
                    }
                }"#,
            )
            .expect("named biome json should import");
        assert_eq!(report.biome_colors, 1);
        assert_eq!(
            biome_palette.biome_color(901),
            RgbaColor::new(141, 179, 96, 255)
        );
    }

    #[test]
    fn block_palette_prefers_texture_matching_block_name() {
        let mut palette = RenderPalette::default();
        let report = palette
            .merge_json_str(
                r#"{
                    "minecraft:acacia_door": {
                        "door_wood_lower": [138, 108, 62, 255],
                        "door_spruce_lower": [105, 80, 48, 255],
                        "door_acacia_lower": [162, 91, 57, 183],
                        "door_dark_oak_lower": [73, 49, 23, 255]
                    },
                    "minecraft:oak_leaves": {
                        "leaves_oak": [144, 143, 144, 171],
                        "leaves_oak_opaque": [131, 129, 131, 255]
                    }
                }"#,
            )
            .expect("multi-texture block palette should import");

        assert_eq!(report.block_colors, 2);
        assert_eq!(
            palette.block_color("minecraft:acacia_door"),
            RgbaColor::new(162, 91, 57, 183)
        );
        assert_eq!(
            palette.block_color("minecraft:oak_leaves"),
            RgbaColor::new(131, 129, 131, 255)
        );
    }

    #[test]
    fn biome_palette_imports_surface_tint_colors() {
        let mut palette = RenderPalette::default();
        palette
            .merge_json_str(
                r#"{
                    "custom": {
                        "id": 902,
                        "rgb": [1, 2, 3],
                        "grass": [80, 200, 40],
                        "leaves": [40, 160, 30],
                        "water": [20, 60, 180]
                    }
                }"#,
            )
            .expect("biome tint json should import");

        let grass = palette.surface_block_color("minecraft:grass_block", Some(902), true);
        let leaves = palette.surface_block_color("minecraft:oak_leaves", Some(902), true);
        let water = palette.surface_block_color("minecraft:water", Some(902), true);

        assert_eq!(grass.alpha, 255);
        assert_eq!(leaves.alpha, 255);
        assert_eq!(water.alpha, 255);
        assert_ne!(grass, RgbaColor::new(1, 2, 3, 255));
        assert!(grass.green > grass.red);
        assert!(grass.green > grass.blue);
        assert_ne!(
            grass,
            palette.surface_block_color("minecraft:grass_block", None, true)
        );
        assert_ne!(
            leaves,
            palette.surface_block_color("minecraft:oak_leaves", None, true)
        );
        assert_ne!(
            water,
            palette.surface_block_color("minecraft:water", None, true)
        );
    }

    #[test]
    fn builtin_json_sources_are_loaded_by_default() {
        let palette = RenderPalette::default();
        let json_palette = RenderPalette::from_builtin_json_sources()
            .expect("built-in JSON palette sources should import");
        assert!(palette.block_colors.len() >= 1_200);
        assert!(palette.biome_colors.len() >= 88);
        assert!(palette.has_block_color("minecraft:farmland"));
        assert!(palette.has_block_color("minecraft:glow_lichen"));
        assert!(palette.has_block_color("minecraft:weeping_vines"));
        assert_eq!(
            palette.block_color("minecraft:farmland"),
            json_palette.block_color("minecraft:farmland")
        );
        assert_eq!(palette.biome_color(0), json_palette.biome_color(0));
        assert_eq!(palette.block_color("minecraft:unknown_test").alpha, 255);
    }

    #[test]
    fn builtin_biome_schema_keeps_defaults_out_of_biome_ids() {
        let value = serde_json::from_str::<Value>(RenderPalette::builtin_biome_color_json())
            .expect("built-in biome palette JSON should parse");
        let biomes = value
            .get("biomes")
            .and_then(Value::as_object)
            .expect("built-in biome palette should use wrapper schema");
        assert!(value.get("defaults").is_some());
        assert!(!biomes.contains_key("default"));
        let mut ids = std::collections::BTreeSet::new();
        for (name, entry) in biomes {
            let id = entry
                .get("id")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| panic!("{name} should have an integer id"));
            assert!(ids.insert(id), "duplicate biome id {id} at {name}");
        }
    }

    #[test]
    fn builtin_palette_sources_are_embedded() {
        assert!(RenderPalette::builtin_block_color_json().contains("grass"));
        assert!(RenderPalette::builtin_biome_color_json().contains("ocean"));
    }

    #[test]
    fn builtin_static_tables_match_json_palette_for_hot_colors() {
        let static_palette = RenderPalette::from_builtin_static_tables();
        let json_palette = RenderPalette::from_builtin_json_sources()
            .expect("built-in JSON palette should remain auditable");
        for block in [
            "minecraft:grass_block",
            "minecraft:water",
            "minecraft:acacia_log",
            "minecraft:bed",
            "minecraft:decorated_pot",
        ] {
            assert_eq!(
                static_palette.block_color(block),
                json_palette.block_color(block),
                "{block} default color differs between static and JSON palette"
            );
        }
        for biome in [0, 1, 7, 24, 48] {
            assert_eq!(
                static_palette.biome_color(biome),
                json_palette.biome_color(biome),
                "biome {biome} color differs between static and JSON palette"
            );
        }
        let log_state = BlockState {
            name: "minecraft:acacia_log".to_string(),
            states: [("pillar_axis".to_string(), NbtTag::String("y".to_string()))]
                .into_iter()
                .collect(),
            version: None,
        };
        assert_eq!(
            static_palette.block_state_color(&log_state),
            json_palette.block_state_color(&log_state)
        );
        assert_eq!(
            static_palette.block_variant_color("minecraft:bed", "bed_blue"),
            json_palette.block_variant_color("minecraft:bed", "bed_blue")
        );
    }

    #[test]
    fn builtin_palette_sources_do_not_use_tainted_source_markers() {
        for source in [
            RenderPalette::builtin_block_color_json(),
            RenderPalette::builtin_biome_color_json(),
        ] {
            let lower = source.to_ascii_lowercase();
            for marker in [
                concat!("legacy", "-current"),
                concat!("bedrock", "-level"),
                "agpl",
                concat!("wiki", "-icon-derived"),
                concat!("wiki", "-color-values"),
            ] {
                assert!(
                    !lower.contains(marker),
                    "built-in palette source should not contain `{marker}`"
                );
            }
            assert!(lower.contains("bedrock-render-clean-room-v1"));
        }
    }

    #[test]
    fn common_plant_blocks_are_known_colors() {
        let palette = RenderPalette::default();
        for name in [
            "minecraft:short_grass",
            "minecraft:tall_grass",
            "minecraft:double_plant",
            "minecraft:pink_petals",
            "minecraft:carrots",
            "minecraft:oak_trapdoor",
            "minecraft:cyan_concrete",
            "minecraft:deepslate_diamond_ore",
            "minecraft:crafting_table",
            "minecraft:farmland",
            "minecraft:glow_lichen",
            "minecraft:bubble_column",
            "minecraft:piston",
            "minecraft:reeds",
            "minecraft:fire",
            "minecraft:scaffolding",
            "minecraft:shroomlight",
            "minecraft:weeping_vines",
            "minecraft:end_portal",
            "minecraft:iron_bars",
            "minecraft:hopper",
            "minecraft:lever",
        ] {
            assert!(
                palette.has_block_color(name),
                "{name} should not be unknown"
            );
        }
    }

    #[test]
    fn builtin_common_dyed_legacy_ids_use_state_colors() {
        let palette = RenderPalette::default();
        for (legacy_name, state_key, state_value, modern_name) in [
            (
                "minecraft:concrete",
                "color",
                "red",
                "minecraft:red_concrete",
            ),
            (
                "minecraft:concretePowder",
                "color",
                "blue",
                "minecraft:blue_concrete_powder",
            ),
            ("minecraft:wool", "color", "green", "minecraft:green_wool"),
            (
                "minecraft:carpet",
                "color",
                "yellow",
                "minecraft:yellow_carpet",
            ),
            (
                "minecraft:stained_glass",
                "color",
                "cyan",
                "minecraft:cyan_stained_glass",
            ),
            (
                "minecraft:stained_glass_pane",
                "color",
                "purple",
                "minecraft:purple_stained_glass_pane",
            ),
            (
                "minecraft:shulker_box",
                "color",
                "orange",
                "minecraft:orange_shulker_box",
            ),
            (
                "minecraft:stained_hardened_clay",
                "color",
                "brown",
                "minecraft:brown_terracotta",
            ),
        ] {
            let legacy = palette.block_state_color(&test_block_state_with_string(
                legacy_name,
                state_key,
                state_value,
            ));
            let modern = palette.block_color(modern_name);
            assert!(
                color_distance(legacy, modern) <= 12,
                "{legacy_name} {state_value} should match {modern_name}: {legacy:?} vs {modern:?}"
            );
        }

        let numeric_red = palette.block_state_color(&test_block_state_with_int(
            "minecraft:concrete",
            "color",
            14,
        ));
        let named_red = palette.block_state_color(&test_block_state_with_string(
            "minecraft:concrete",
            "color",
            "red",
        ));
        assert_eq!(numeric_red, named_red);
    }

    #[test]
    fn builtin_bed_and_candle_colors_are_available() {
        let palette = RenderPalette::default();
        let red_bed = palette
            .block_variant_color("minecraft:bed", "bed_red")
            .expect("bed should include red variant");
        let blue_bed = palette
            .block_variant_color("minecraft:bed", "bed_blue")
            .expect("bed should include blue variant");
        assert!(color_distance(red_bed, blue_bed) >= 80);

        let candle = palette.block_color("minecraft:candle");
        let red_candle = palette.block_color("minecraft:red_candle");
        let red_candle_cake = palette.block_color("minecraft:red_candle_cake");
        assert!(candle.alpha == 255);
        assert!(red_candle.red > red_candle.green);
        assert_eq!(red_candle, red_candle_cake);
    }

    #[test]
    fn builtin_red_sandstone_variants_stay_red_sand_family() {
        let palette = RenderPalette::default();
        let red_sand = palette.block_color("minecraft:red_sand");
        let red_sandstone = palette.block_color("minecraft:red_sandstone");
        let cut_red_sandstone = palette.block_color("minecraft:cut_red_sandstone");
        let red_sandstone_slab = palette.block_color("minecraft:red_sandstone_slab");

        for color in [
            red_sand,
            red_sandstone,
            cut_red_sandstone,
            red_sandstone_slab,
        ] {
            assert!(color.red > color.green);
            assert!(color.green > color.blue);
        }
        assert!(color_distance(red_sandstone, cut_red_sandstone) <= 80);
        assert!(color_distance(red_sandstone, red_sandstone_slab) <= 90);
    }

    #[test]
    fn builtin_sand_state_colors_route_red_sand_variants() {
        let palette = RenderPalette::default();
        let sand = palette.block_color("minecraft:sand");
        let red_sand = palette.block_color("minecraft:red_sand");

        assert_eq!(palette.block_color("minecraft:sand"), sand);
        assert_eq!(
            palette.block_state_color(&test_block_state_with_string(
                "minecraft:sand",
                "sand_type",
                "red",
            )),
            red_sand
        );
        assert_eq!(
            palette.block_state_color(&test_block_state_with_string(
                "minecraft:sand",
                "variant",
                "red_sand",
            )),
            red_sand
        );
        assert_eq!(
            palette.block_state_color(&test_block_state_with_int("minecraft:sand", "data", 1)),
            red_sand
        );
        assert_eq!(
            palette.block_state_color(&test_block_state_with_int("minecraft:sand", "data", 0)),
            sand
        );
    }

    #[test]
    fn resource_pack_variants_keep_material_family_colors() {
        let palette = RenderPalette::default();
        let oak_planks = palette.block_color("minecraft:oak_planks");
        let oak_stairs = palette.block_color("minecraft:oak_stairs");
        let oak_slab = palette.block_color("minecraft:oak_slab");
        assert!(color_distance(oak_planks, oak_stairs) <= 35);
        assert!(color_distance(oak_planks, oak_slab) <= 35);

        let stone = palette.block_color("minecraft:stone");
        let stone_stairs = palette.block_color("minecraft:stone_stairs");
        let stone_slab = palette.block_color("minecraft:stone_slab");
        assert!(color_distance(stone, stone_stairs) <= 80);
        assert!(color_distance(stone, stone_slab) <= 120);

        let farmland = palette.block_color("minecraft:farmland");
        let dirt = palette.block_color("minecraft:dirt");
        assert!(color_distance(farmland, dirt) >= 30);
    }

    #[test]
    fn clean_room_bamboo_and_path_colors_are_semantic() {
        let palette = RenderPalette::default();
        let bamboo = palette.surface_block_color("minecraft:bamboo", Some(21), true);
        assert!(bamboo.green > bamboo.red + 20);
        assert!(bamboo.green > bamboo.blue + 30);
        let plains_grass = palette.surface_block_color("minecraft:grass_block", Some(1), true);
        let bamboo_material = palette.surface_block_color("minecraft:bamboo_block", Some(21), true);
        assert!(color_distance(bamboo, plains_grass) >= 45);
        assert!(color_distance(bamboo, bamboo_material) >= 120);

        let path_without_tint = palette.surface_block_color("minecraft:grass_path", None, false);
        let path_with_jungle_tint =
            palette.surface_block_color("minecraft:grass_path", Some(21), true);
        assert_eq!(path_without_tint, path_with_jungle_tint);
        assert!(path_with_jungle_tint.red > path_with_jungle_tint.green);
        assert!(path_with_jungle_tint.green > path_with_jungle_tint.blue);
    }

    #[test]
    fn surface_grass_uses_grass_tint_and_stays_distinct() {
        let palette = RenderPalette::default();
        let plains = palette.surface_block_color("minecraft:grass_block", Some(1), true);
        let desert = palette.surface_block_color("minecraft:grass_block", Some(2), true);
        let jungle = palette.surface_block_color("minecraft:grass_block", Some(21), true);
        let swamp = palette.surface_block_color("minecraft:grass_block", Some(6), true);
        let cherry = palette.surface_block_color("minecraft:grass_block", Some(192), true);
        let pale = palette.surface_block_color("minecraft:grass_block", Some(193), true);

        for color in [plains, desert, jungle, swamp, cherry, pale] {
            let brightness = luminance(color);
            assert!(
                (70..=205).contains(&brightness),
                "{color:?} has unexpected brightness"
            );
        }
        assert_ne!(plains, palette.biome_color(1));
        assert_ne!(desert, palette.biome_color(2));
        assert_ne!(jungle, palette.biome_color(21));
        assert_ne!(swamp, palette.biome_color(6));
        assert!(desert.red > desert.green && desert.green > desert.blue);
        assert!(jungle.green > jungle.red && jungle.green > jungle.blue);
        assert!(color_distance(plains, desert) >= 35);
        assert!(color_distance(jungle, swamp) >= 35);
        assert!(color_distance(cherry, pale) >= 25);

        let default_tinted = palette.surface_block_color("minecraft:grass_block", None, false);
        assert!(default_tinted.green > default_tinted.red);
        assert!(default_tinted.green > default_tinted.blue);
        assert!((70..=190).contains(&luminance(default_tinted)));
    }

    #[test]
    fn grass_block_surface_uses_tint_without_recoloring_sand() {
        let palette = RenderPalette::default();
        for biome_id in [1, 16, 21] {
            let grass = palette.surface_block_color("minecraft:grass_block", Some(biome_id), true);
            let biome = palette.biome_color(biome_id);
            assert_ne!(
                grass, biome,
                "grass should use grass tint, not biome viewport color"
            );
        }

        let plains_sand = palette.surface_block_color("minecraft:sand", Some(1), true);
        let jungle_sand = palette.surface_block_color("minecraft:sand", Some(21), true);
        assert_eq!(plains_sand, jungle_sand);
    }

    #[test]
    fn beach_grass_uses_grass_tint() {
        let palette = RenderPalette::default();
        let beach = palette.surface_block_color("minecraft:grass_block", Some(16), true);

        assert_ne!(beach, palette.biome_color(16));
        assert!(beach.green >= beach.red);
        assert!(luminance(beach) >= 90);
    }

    #[test]
    fn builtin_tint_masks_are_auditable_and_not_white() {
        let value = serde_json::from_str::<Value>(RenderPalette::builtin_block_color_json())
            .expect("built-in block palette JSON should parse");
        let blocks = value
            .get("blocks")
            .and_then(Value::as_object)
            .expect("built-in block palette should use wrapper schema");
        for name in ["minecraft:grass_block", "minecraft:oak_leaves"] {
            let entry = blocks
                .get(name)
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("{name} should have a block entry"));
            assert_eq!(
                entry
                    .get("resource_pack_tint_source")
                    .and_then(Value::as_str),
                Some("texture_average")
            );
            let color = entry
                .get("resource_pack_tint_mask")
                .and_then(parse_color)
                .unwrap_or_else(|| panic!("{name} should have a tint mask"));
            let max_channel = color.red.max(color.green).max(color.blue);
            assert!(
                max_channel <= 220,
                "{name} tint mask should not be near-white: {color:?}"
            );
        }
    }

    #[test]
    fn biome_water_tint_stays_blue_and_keeps_ocean_distinct() {
        let palette = RenderPalette::default();
        let ocean = palette.surface_block_color("minecraft:water", Some(0), true);
        let deep_ocean = palette.surface_block_color("minecraft:water", Some(24), true);
        let river = palette.surface_block_color("minecraft:water", Some(7), true);

        for color in [ocean, deep_ocean, river] {
            assert!(color.blue > color.green && color.green >= color.red);
            assert!(luminance(color) <= 125, "{color:?} is too pale");
        }
        assert!(color_distance(ocean, river) >= 10);
    }

    #[test]
    fn warm_ocean_water_preserves_cyan_biome_tint() {
        let palette = RenderPalette::default();
        let warm = palette.surface_block_color("minecraft:water", Some(40), true);
        let lukewarm = palette.surface_block_color("minecraft:water", Some(42), true);

        assert!(
            warm.green >= 130 && warm.blue >= 190 && warm.red <= 25,
            "{warm:?} should keep warm ocean cyan"
        );
        assert!(
            lukewarm.green >= 105 && lukewarm.blue >= 180 && lukewarm.red <= 35,
            "{lukewarm:?} should keep lukewarm ocean blue-cyan"
        );
        assert!(color_distance(warm, lukewarm) >= 20);
    }

    #[test]
    fn shallow_transparent_water_keeps_visible_cyan_signal() {
        let palette = RenderPalette::default();
        let sand = palette.surface_block_color("minecraft:sand", Some(7), true);
        let shallow = palette.transparent_water_color(
            "minecraft:water",
            Some("minecraft:sand"),
            Some(7),
            1,
            true,
        );
        let two_deep = palette.transparent_water_color(
            "minecraft:water",
            Some("minecraft:sand"),
            Some(7),
            2,
            true,
        );

        assert!(shallow.blue > sand.blue, "{shallow:?} over {sand:?}");
        assert!(two_deep.blue >= shallow.blue);
        assert!(color_distance(shallow, sand) <= 70);
        assert!(color_distance(two_deep, sand) <= 95);
        assert!(color_distance(shallow, sand) < color_distance(two_deep, sand));
        assert!(color_distance(two_deep, shallow) >= 4);
    }

    #[test]
    fn leaf_litter_is_not_foliage_tinted() {
        let palette = RenderPalette::default();
        let base = palette.block_color("minecraft:leaf_litter");
        let tinted = palette.surface_block_color("minecraft:leaf_litter", Some(21), true);
        assert_eq!(with_alpha(base, 255), tinted);
    }

    #[test]
    fn height_shading_preserves_alpha_and_changes_brightness() {
        let palette = RenderPalette::default();
        let base = RgbaColor::new(100, 120, 140, 200);
        let brighter = palette.height_shaded_color(base, 80, 70);
        let darker = palette.height_shaded_color(base, 60, 70);

        assert_eq!(brighter.alpha, 200);
        assert_eq!(darker.alpha, 200);
        assert!(brighter.red > base.red);
        assert!(darker.red < base.red);
    }

    fn luminance(color: RgbaColor) -> u16 {
        (u16::from(color.red) * 30 + u16::from(color.green) * 59 + u16::from(color.blue) * 11) / 100
    }

    fn color_distance(left: RgbaColor, right: RgbaColor) -> u16 {
        u16::from(left.red.abs_diff(right.red))
            + u16::from(left.green.abs_diff(right.green))
            + u16::from(left.blue.abs_diff(right.blue))
    }

    fn test_block_state_with_int(name: &str, key: &str, value: i32) -> BlockState {
        let mut states = std::collections::BTreeMap::new();
        states.insert(key.to_string(), NbtTag::Int(value));
        BlockState {
            name: name.to_string(),
            states,
            version: None,
        }
    }

    fn test_block_state_with_string(name: &str, key: &str, value: &str) -> BlockState {
        let mut states = std::collections::BTreeMap::new();
        states.insert(key.to_string(), NbtTag::String(value.to_string()));
        BlockState {
            name: name.to_string(),
            states,
            version: None,
        }
    }
}
