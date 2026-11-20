use super::*;

pub(super) const WATER_ALPHA: f32 = 0.46;
pub(super) const DEFAULT_WATER_RGB: [f32; 3] = [28.0 / 255.0, 76.0 / 255.0, 158.0 / 255.0];
pub(super) const LAVA_ALPHA: f32 = 1.0;
pub(super) const GLASS_ALPHA: f32 = 0.34;
pub(super) type MaterialName = Arc<str>;
pub(super) type MaterialSlot = Arc<str>;
pub(super) fn block_color(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
) -> [f32; 4] {
    if state.name.strip_prefix("minecraft:").unwrap_or(&state.name) == "redstone_wire" {
        return redstone_wire_color(state);
    }
    if let Some(color) = stem_growth_color(state) {
        return color;
    }
    if let Some(tint) = texture_tint(palette, state, biome) {
        return color_from_rgba(tint, 1.0);
    }
    let color = surface_color(palette, state, biome).to_array();
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        1.0,
    ]
}

/// Bedrock colours pumpkin/melon stems from `age` over a fixed green-to-brown ramp.
///
/// The stem texture is a grayscale mask: `red = age * 32`, `green = 255 - age * 8`,
/// `blue = age * 4`, so a sprout is pure green and a fully grown stem is brownish.
pub(super) fn stem_growth_color(state: &BlockState) -> Option<[f32; 4]> {
    let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if !matches!(name, "pumpkin_stem" | "melon_stem") {
        return None;
    }
    let age = state_i32(state, "age")
        .or_else(|| state_i32(state, "growth"))
        .unwrap_or(0)
        .clamp(0, 7) as f32;
    Some([
        (age * 32.0) / 255.0,
        (255.0 - age * 8.0) / 255.0,
        (age * 4.0) / 255.0,
        1.0,
    ])
}

pub(super) fn redstone_wire_color(state: &BlockState) -> [f32; 4] {
    let power = state_i32(state, "redstone_signal")
        .or_else(|| state_i32(state, "power"))
        .unwrap_or(0)
        .clamp(0, 15);
    let strength = power as f32 / 15.0;
    let red = if power == 0 {
        0.30
    } else {
        strength.mul_add(0.60, 0.40)
    };
    let green = (strength * strength * 0.70 - 0.50).max(0.0);
    let blue = (strength * strength * 0.60 - 0.70).max(0.0);
    [red, green, blue, 1.0]
}

/// Minecraft-style face shading. The preview is unlit, so without these
/// factors a pale block palette reads as one flat white mass.
pub(super) const FACE_SHADE_UP: f32 = 1.0;
pub(super) const FACE_SHADE_SIDE: f32 = 0.8;
pub(super) const FACE_SHADE_DOWN: f32 = 0.55;

pub(super) fn shade_face(color: [f32; 4], factor: f32) -> [f32; 4] {
    [
        color[0] * factor,
        color[1] * factor,
        color[2] * factor,
        color[3],
    ]
}

pub(super) fn block_face_colors(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
    shade_faces: bool,
) -> BlockFaceColors {
    let up_factor = if shade_faces { FACE_SHADE_UP } else { 1.0 };
    let side_factor = if shade_faces { FACE_SHADE_SIDE } else { 1.0 };
    let down_factor = if shade_faces { FACE_SHADE_DOWN } else { 1.0 };
    let normalized = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if is_full_grass_block(normalized) {
        let up = named_block_color(palette, "minecraft:grass_block", biome, BiomeTint::Apply);
        let down = named_block_color(palette, "minecraft:dirt", biome, BiomeTint::Ignore);
        let side = named_block_color(palette, "minecraft:grass", biome, BiomeTint::Apply);
        return BlockFaceColors {
            up: shade_face(up, up_factor),
            down: shade_face(down, down_factor),
            side: shade_face(side, side_factor),
        };
    }
    let color = block_color(palette, state, biome);
    let side = palette.block_side_color(&state.name);
    let side = if side == palette.block_color(&state.name) {
        color
    } else {
        side.to_array().map(|channel| f32::from(channel) / 255.0)
    };
    BlockFaceColors {
        up: shade_face(color, up_factor),
        down: shade_face(color, down_factor),
        side: shade_face(side, side_factor),
    }
}

#[derive(Clone, Copy)]
pub(super) enum BiomeTint {
    Apply,
    Ignore,
}

pub(super) fn named_block_color(
    palette: &RenderPalette,
    name: &str,
    biome: Option<BiomeSample>,
    biome_tint: BiomeTint,
) -> [f32; 4] {
    let biome_id = biome.and_then(|biome| biome.biome_id);
    let color = match biome_tint {
        BiomeTint::Apply => {
            let blended = match palette.surface_tint_kind(name) {
                Some(SurfaceTintKind::Grass) => biome.and_then(|biome| biome.grass_tint),
                Some(SurfaceTintKind::Foliage) => biome.and_then(|biome| biome.foliage_tint),
                Some(SurfaceTintKind::DryFoliage) => biome.and_then(|biome| biome.dry_foliage_tint),
                _ => None,
            };
            match blended.or_else(|| {
                biome.and_then(|biome| {
                    biome
                        .legacy_color
                        .filter(|legacy| !color_is_washed_out(*legacy))
                })
            }) {
                Some(tint) => tint,
                None => palette
                    .surface_block_tint(name, biome_id)
                    .unwrap_or_else(|| palette.block_color(name)),
            }
        }
        BiomeTint::Ignore => palette.block_color(name),
    };
    color_from_rgba(color, 1.0)
}

pub(super) fn surface_color(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
) -> RgbaColor {
    let name = state.name.as_str();
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    // Appearance entries are keyed by the canonical block identity. Legacy saves store
    // pre-rename identifiers and state-driven plant variants, so resolve the same identity the
    // resource-pack model layer uses before looking up the biome tint.
    let palette_name = palette_block_name(state);
    let palette_name = palette_name.as_ref();
    if let Some(legacy_color) = biome.and_then(|biome| biome.legacy_color) {
        if is_full_grass_block(normalized) {
            return apply_surface_grass_tint(
                palette.block_color("minecraft:grass_block"),
                legacy_color,
            );
        }
        if is_foliage_block(normalized) {
            return multiply_with_biome_tint(palette.block_color(palette_name), legacy_color);
        }
    }
    let biome_id = biome.and_then(|biome| biome.biome_id);
    let color_name = if is_full_grass_block(normalized) {
        "minecraft:grass_block"
    } else {
        palette_name
    };
    let color = palette.surface_block_color(color_name, biome_id, true);
    correct_washed_out_biome_tint(palette, color_name, biome_id, color)
}

/// Resolves the palette identity of grass-like plant states.
///
/// Bedrock saves written before the `short_grass` rename still store `minecraft:tallgrass`,
/// and `minecraft:double_plant` stores its visual kind in block state. Both must resolve to the
/// palette entry that owns the biome tint.
pub(super) fn palette_block_name<'a>(state: &'a BlockState) -> Cow<'a, str> {
    let name = state.name.as_str();
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    if normalized == "tallgrass" {
        return Cow::Borrowed("minecraft:short_grass");
    }
    if normalized == "double_plant" {
        return Cow::Owned(model_family::canonical_block_name(&block_state_query(
            state,
        )));
    }
    Cow::Borrowed(name)
}

/// Returns the pure biome tint for blocks whose texture is a grayscale appearance mask.
///
/// The palette's surface color already folds the mask average into the result; textured faces
/// sample the mask themselves, so multiplying by that surface color would apply the mask twice.
pub(super) fn texture_tint(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
) -> Option<RgbaColor> {
    let palette_name = palette_block_name(state);
    let biome_id = biome.and_then(|biome| biome.biome_id);
    let kind = palette.surface_tint_kind(palette_name.as_ref())?;
    // Bedrock averages the biome color of the block and its eight neighbors; use the precomputed
    // column tint so borders fade instead of stepping at biome-cell edges.
    let blended = match kind {
        SurfaceTintKind::Grass => biome.and_then(|biome| biome.grass_tint),
        SurfaceTintKind::Foliage => biome.and_then(|biome| biome.foliage_tint),
        // Leaf litter and the dry grasses use the dry-foliage channel, not the green one.
        SurfaceTintKind::DryFoliage => biome.and_then(|biome| biome.dry_foliage_tint),
        SurfaceTintKind::FixedFoliage => None,
    };
    if let Some(tint) = blended {
        return Some(tint);
    }
    if let Some(legacy) = biome.and_then(|biome| biome.legacy_color)
        && !color_is_washed_out(legacy)
    {
        return Some(legacy);
    }
    // No biome data (or none of it applies): `surface_block_tint` answers with the palette's
    // built-in default for this channel, so the block is never left untinted.
    palette.surface_block_tint(palette_name.as_ref(), biome_id)
}

pub(super) fn is_full_grass_block(normalized: &str) -> bool {
    normalized == "grass" || normalized == "grass_block" || normalized.ends_with("_grass_block")
}

pub(super) fn is_grass_tinted_detail_block(normalized: &str) -> bool {
    matches!(
        normalized,
        "short_grass"
            | "tall_grass"
            | "tallgrass"
            | "fern"
            | "large_fern"
            | "vine"
            | "twisting_vines"
            | "weeping_vines"
            | "seagrass"
            | "tall_seagrass"
            | "kelp"
            | "kelp_plant"
    ) || normalized.contains("grass")
        || normalized.contains("fern")
        || normalized.contains("vine")
}

pub(super) fn correct_washed_out_biome_tint(
    palette: &RenderPalette,
    name: &str,
    biome_id: Option<u32>,
    color: RgbaColor,
) -> RgbaColor {
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    if !color_is_washed_out(color) {
        return color;
    }
    if is_full_grass_block(normalized) || is_grass_tinted_detail_block(normalized) {
        return palette.surface_block_color("minecraft:grass_block", biome_id, true);
    }
    if is_foliage_block(normalized) {
        return palette.surface_block_color("minecraft:oak_leaves", biome_id, true);
    }
    color
}

pub(super) fn color_is_washed_out(color: RgbaColor) -> bool {
    let color = color.to_array();
    let red = f32::from(color[0]) / 255.0;
    let green = f32::from(color[1]) / 255.0;
    let blue = f32::from(color[2]) / 255.0;
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let luminance = red * 0.2126 + green * 0.7152 + blue * 0.0722;
    luminance > 0.78 || (maximum > 0.72 && maximum - minimum < 0.18)
}

pub(super) fn transparent_color(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
) -> [f32; 4] {
    let mut color = block_color(palette, state, biome);
    color[3] = GLASS_ALPHA;
    color
}

pub(super) fn water_color(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
) -> [f32; 4] {
    let biome_id = biome.and_then(|biome| biome.biome_id);
    let mut color = color_from_rgba(
        palette.surface_block_color(state.name.as_str(), biome_id, true),
        WATER_ALPHA,
    );
    if water_color_is_washed_out(color) {
        color = [
            DEFAULT_WATER_RGB[0],
            DEFAULT_WATER_RGB[1],
            DEFAULT_WATER_RGB[2],
            WATER_ALPHA,
        ];
    }
    color[3] = WATER_ALPHA;
    color
}

pub(super) fn color_from_rgba(color: RgbaColor, alpha: f32) -> [f32; 4] {
    let color = color.to_array();
    [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        alpha,
    ]
}

pub(super) fn water_color_is_washed_out(color: [f32; 4]) -> bool {
    let maximum = color[0].max(color[1]).max(color[2]);
    let minimum = color[0].min(color[1]).min(color[2]);
    let luminance = luminance(color);
    luminance > 0.66 || (maximum > 0.72 && maximum - minimum < 0.24)
}

pub(super) fn luminance(color: [f32; 4]) -> f32 {
    color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
}

pub(super) fn lava_color(
    palette: &RenderPalette,
    state: &BlockState,
    biome: Option<BiomeSample>,
) -> [f32; 4] {
    let mut color = block_color(palette, state, biome);
    color[3] = LAVA_ALPHA;
    color
}

pub(super) fn material_block_name<'a>(
    state: &'a BlockState,
    block_class: BlockClass,
) -> Cow<'a, str> {
    match block_class {
        BlockClass::Water => Cow::Borrowed("minecraft:water"),
        BlockClass::Lava => Cow::Borrowed("minecraft:lava"),
        _ => model_family::detail_material_block_name(&block_state_query(state))
            .unwrap_or_else(|| Cow::Borrowed(&state.name)),
    }
}
pub(super) fn apply_surface_grass_tint(base: RgbaColor, tint: RgbaColor) -> RgbaColor {
    let multiplied = multiply_with_biome_tint(base, tint);
    blend_toward_color(multiplied, tint, 96)
}

pub(super) fn multiply_with_biome_tint(base: RgbaColor, tint: RgbaColor) -> RgbaColor {
    RgbaColor::new(
        multiply_channel(base.red, tint.red),
        multiply_channel(base.green, tint.green),
        multiply_channel(base.blue, tint.blue),
        255,
    )
}

pub(super) fn multiply_channel(base: u8, tint: u8) -> u8 {
    let value = u16::from(base) * u16::from(tint) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

pub(super) fn blend_toward_color(
    base: RgbaColor,
    target: RgbaColor,
    target_weight: u16,
) -> RgbaColor {
    let target_weight = target_weight.min(255);
    let base_weight = 255_u16.saturating_sub(target_weight);
    RgbaColor::new(
        weighted_channel(base.red, target.red, base_weight, target_weight),
        weighted_channel(base.green, target.green, base_weight, target_weight),
        weighted_channel(base.blue, target.blue, base_weight, target_weight),
        255,
    )
}

pub(super) fn weighted_channel(base: u8, target: u8, base_weight: u16, target_weight: u16) -> u8 {
    let value = (u16::from(base) * base_weight + u16::from(target) * target_weight + 127) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

pub(super) fn material_name(name: &str, block_class: BlockClass) -> MaterialName {
    let block = match block_class {
        BlockClass::Water => "minecraft:water",
        BlockClass::Lava => "minecraft:lava",
        _ => name,
    };
    Arc::from(obj::material_name(block))
}

pub(super) fn face_material_name(base: MaterialName, normal: [i32; 3]) -> MaterialName {
    Arc::from(obj::face_material_name(base.as_ref(), normal))
}

pub(super) fn plane_material_name(base: MaterialName, plane: &Plane) -> MaterialName {
    Arc::from(obj::plane_material_name(
        base.as_ref(),
        plane.normal,
        plane.material_slot.as_deref(),
    ))
}
pub(super) fn shade_preview_color(mut color: [f32; 4], factor: f32) -> [f32; 4] {
    // This custom shader presents encoded RGB directly to the non-sRGB surface.
    // Keep its packed bytes encoded, but calculate face illumination in linear light.
    let encoded = [0, 1, 2].map(|channel| (color[channel].clamp(0.0, 1.0) * 255.0).round() as u8);
    let shaded = RgbaColor::new(encoded[0], encoded[1], encoded[2], 255).shade(factor);
    color[..3].copy_from_slice(
        &[shaded.red, shaded.green, shaded.blue].map(|channel| f32::from(channel) / 255.0),
    );
    color
}
