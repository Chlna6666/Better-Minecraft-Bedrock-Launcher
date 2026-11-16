//! Banner dye colours and pattern-sheet layout shared by the block-entity reader and the
//! texture compositor.
//!
//! Bedrock stores a banner's art on the block entity: `Base` names the dye colour of the cloth
//! and `Patterns` is an ordered list of `{ Pattern, Color }` layers. The client composites them
//! over `textures/entity/banner/banner_base` with the masks in
//! `textures/entity/banner/banner.tga`; the rects below come from the game's
//! `data/texture_backwards_compatibility_mapping.json`, which lists every pattern's `uv` in that
//! 512 x 512 sheet.

/// The 16 Bedrock dye colours in `Base` / `Color` index order.
pub const DYE_COLORS: [[u8; 3]; 16] = [
    [249, 255, 254], // white
    [249, 128, 29],  // orange
    [199, 78, 189],  // magenta
    [58, 179, 218],  // light blue
    [254, 216, 61],  // yellow
    [128, 199, 31],  // lime
    [243, 139, 170], // pink
    [71, 79, 82],    // gray
    [157, 157, 151], // light gray
    [22, 156, 156],  // cyan
    [137, 50, 184],  // purple
    [60, 68, 170],   // blue
    [131, 84, 50],   // brown
    [94, 124, 22],   // green
    [176, 46, 38],   // red
    [29, 29, 33],    // black
];

/// Dye colour names as newer worlds spell them in `Color` / `Base`.
pub const DYE_NAMES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// The two cloth panels inside `banner_base` and every `banner.tga` cell, as `[x0, y0, x1, y1]`.
///
/// The sheet holds the same 20 x 40 px cloth twice: the front panel at `(1, 1)..(21, 41)` and a
/// **mirrored** copy for the back at `(22, 1)..(42, 41)`. Row 0 / column 0 are the sheet's edge
/// bleed, and the 2 px gutter at x 42..43 separates the cloth from the pole. Measured across all
/// 38 pattern cells: mirroring front onto back matches 99.9% of the alpha mask, while comparing
/// the two panels unmirrored matches only ~65%.
pub const CLOTH_PANELS: [[u32; 4]; 2] = [[1, 1, 21, 41], [22, 1, 42, 41]];

/// Whether the back panel is the mirror image of the front one.
pub const CLOTH_BACK_MIRRORED: bool = true;

/// Side of one pattern cell in `banner.tga`.
pub const PATTERN_CELL: u32 = 64;

/// One dye/pattern layer of a banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BannerLayer {
    /// Index into [`PATTERNS`], which names the mask texture and its `banner.tga` cell.
    pub pattern: u8,
    /// Dye colour index, 0..16.
    pub color: u8,
}

/// Everything that turns a white banner_base sheet into one banner's art.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BannerArt {
    /// `Base` dye colour index; also the colour behind every pattern layer.
    pub base: u8,
    /// `Patterns` in application order (first layer is the bottom one).
    pub layers: Vec<BannerLayer>,
}

impl BannerArt {
    /// Resolves an NBT pattern id (`"bs"`, `"bl"`, ...) into its [`PATTERNS`] entry.
    #[must_use]
    pub fn pattern_index(id: &str) -> Option<u8> {
        let id = id.trim();
        PATTERNS
            .iter()
            .position(|pattern| pattern.ids.contains(&id))
            .map(|index| index as u8)
    }

    /// `banner.tga` cell of one pattern, when the legacy sheet carries it.
    #[must_use]
    pub fn pattern_cell(pattern: u8) -> Option<u8> {
        PATTERNS.get(usize::from(pattern))?.cell
    }

    /// Loose entity texture key of one pattern's own 64 x 64 art.
    #[must_use]
    pub fn pattern_texture_key(pattern: u8) -> Option<&'static str> {
        Some(PATTERNS.get(usize::from(pattern))?.texture)
    }

    /// Whether one pattern's texture is a finished cloth instead of a tinted mask.
    #[must_use]
    pub fn pattern_is_full_sheet(pattern: u8) -> bool {
        PATTERNS
            .get(usize::from(pattern))
            .is_some_and(|pattern| pattern.full_sheet)
    }

    /// Resolves a dye colour written as an index or as a name.
    #[must_use]
    pub fn dye_index(value: &str) -> Option<u8> {
        let value = value.trim();
        if let Ok(index) = value.parse::<u8>() {
            return (index < 16).then_some(index);
        }
        DYE_NAMES
            .iter()
            .position(|name| *name == value)
            .map(|index| index as u8)
    }

    /// Top-left corner of one pattern cell in `banner.tga`.
    #[must_use]
    pub fn cell_origin(cell: u8) -> [u32; 2] {
        let cell = u32::from(cell);
        [(cell % 8) * PATTERN_CELL, (cell / 8) * PATTERN_CELL]
    }

    /// Material-name suffix that carries this art through the face pipeline.
    ///
    /// Material names are sanitised to `[A-Za-z0-9_.-]`, so the payload is hex pairs:
    /// `bannerart-<base>-<cell><color><cell><color>...`.
    #[must_use]
    pub fn material_name(&self) -> String {
        let mut name = format!("{BANNER_MATERIAL_PREFIX}{:02x}", self.base);
        for layer in &self.layers {
            name.push('-');
            name.push_str(&format!("{:02x}{:02x}", layer.pattern, layer.color));
        }
        name
    }

    /// Reads the art back out of a material name produced by [`BannerArt::material_name`].
    #[must_use]
    pub fn from_material_name(material: &str) -> Option<Self> {
        let payload = material.strip_prefix(BANNER_MATERIAL_PREFIX)?;
        // The face pipeline appends the model slot (`__mat_front`) after the base name.
        let payload = payload
            .split(crate::obj::MATERIAL_SLOT_SEPARATOR)
            .next()
            .unwrap_or(payload);
        let mut parts = payload.split('-');
        let base = u8::from_str_radix(parts.next()?, 16).ok()?;
        let mut layers = Vec::new();
        for part in parts {
            if part.len() != 4 {
                return None;
            }
            layers.push(BannerLayer {
                pattern: u8::from_str_radix(&part[0..2], 16).ok()?,
                color: u8::from_str_radix(&part[2..4], 16).ok()?,
            });
        }
        Some(Self { base, layers })
    }

    /// Whether the art would change the plain sheet at all.
    #[must_use]
    pub fn is_vanilla_white(&self) -> bool {
        self.base == 0 && self.layers.is_empty()
    }
}

/// Prefix of the synthetic material name that carries banner art.
pub const BANNER_MATERIAL_PREFIX: &str = "bannerart-";

/// One banner pattern: the block-entity id, its own mask texture and its `banner.tga` cell.
pub struct BannerPattern {
    /// Values accepted for `Patterns[].Pattern`; the first one is canonical.
    pub ids: &'static [&'static str],
    /// Loose entity texture holding this pattern's 64 x 64 art.
    ///
    /// 26.60 ships one file per pattern (`banner_border`, `banner_flow`, ...), which also covers
    /// the patterns the legacy 512 x 512 sheet never had.
    pub texture: &'static str,
    /// Cell in the legacy `banner.tga` sheet, when that sheet has one.
    pub cell: Option<u8>,
    /// Whether the texture is a whole finished cloth rather than a tinted mask.
    ///
    /// The ominous banner ships its grey cloth and dark illager face already painted (and its own
    /// pole), so it replaces `banner_base` instead of being stamped over it.
    pub full_sheet: bool,
}

const fn pattern(
    ids: &'static [&'static str],
    texture: &'static str,
    cell: Option<u8>,
) -> BannerPattern {
    BannerPattern {
        ids,
        texture,
        cell,
        full_sheet: false,
    }
}

/// A pattern whose texture is a finished cloth, not a mask.
const fn full_sheet_pattern(
    ids: &'static [&'static str],
    texture: &'static str,
) -> BannerPattern {
    BannerPattern {
        ids,
        texture,
        cell: None,
        full_sheet: true,
    }
}

/// Every banner pattern, in `banner.tga` cell order.
///
/// Ids are the Bedrock short codes from the wiki's pattern table; cells come from the UV rects in
/// `data/texture_backwards_compatibility_mapping.json` (`banner_border` → `[64,0]`,
/// `banner_triangles_top` → `[384,256]`, ...). The last four have no sheet cell in 26.60 and are
/// only available through their own textures.
pub const PATTERNS: [BannerPattern; 43] = [
    pattern(&["bo"], "entity/banner/banner_border", Some(1)),
    pattern(&["bri"], "entity/banner/banner_bricks", Some(2)),
    pattern(&["mc"], "entity/banner/banner_circle", Some(3)),
    pattern(&["cre"], "entity/banner/banner_creeper", Some(4)),
    pattern(&["cr"], "entity/banner/banner_cross", Some(5)),
    pattern(&["cbo"], "entity/banner/banner_curly_border", Some(6)),
    pattern(&["lud"], "entity/banner/banner_diagonal_up_left", Some(7)),
    pattern(&["rud"], "entity/banner/banner_diagonal_up_right", Some(8)),
    pattern(&["ld"], "entity/banner/banner_diagonal_left", Some(9)),
    pattern(&["rd"], "entity/banner/banner_diagonal_right", Some(10)),
    pattern(&["flo"], "entity/banner/banner_flower", Some(11)),
    pattern(&["gra"], "entity/banner/banner_gradient", Some(12)),
    pattern(&["gru"], "entity/banner/banner_gradient_up", Some(13)),
    pattern(&["hh"], "entity/banner/banner_half_horizontal", Some(14)),
    pattern(&["hhb"], "entity/banner/banner_half_horizontal_bottom", Some(15)),
    pattern(&["vh"], "entity/banner/banner_half_vertical", Some(16)),
    pattern(&["vhr"], "entity/banner/banner_half_vertical_right", Some(17)),
    pattern(&["moj"], "entity/banner/banner_mojang", Some(18)),
    pattern(&["mr"], "entity/banner/banner_rhombus", Some(19)),
    pattern(&["sku"], "entity/banner/banner_skull", Some(20)),
    pattern(&["ss"], "entity/banner/banner_small_stripes", Some(21)),
    pattern(&["bl"], "entity/banner/banner_square_bottom_left", Some(22)),
    pattern(&["br"], "entity/banner/banner_square_bottom_right", Some(23)),
    pattern(&["tl"], "entity/banner/banner_square_top_left", Some(24)),
    pattern(&["tr"], "entity/banner/banner_square_top_right", Some(25)),
    pattern(&["sc"], "entity/banner/banner_straight_cross", Some(26)),
    pattern(&["bs"], "entity/banner/banner_stripe_bottom", Some(27)),
    pattern(&["cs"], "entity/banner/banner_stripe_center", Some(28)),
    pattern(&["dls"], "entity/banner/banner_stripe_downleft", Some(29)),
    pattern(&["drs"], "entity/banner/banner_stripe_downright", Some(30)),
    pattern(&["ls"], "entity/banner/banner_stripe_left", Some(31)),
    pattern(&["ms"], "entity/banner/banner_stripe_middle", Some(32)),
    pattern(&["rs"], "entity/banner/banner_stripe_right", Some(33)),
    pattern(&["ts"], "entity/banner/banner_stripe_top", Some(34)),
    pattern(&["bt"], "entity/banner/banner_triangle_bottom", Some(35)),
    pattern(&["tt"], "entity/banner/banner_triangle_top", Some(36)),
    pattern(&["bts"], "entity/banner/banner_triangles_bottom", Some(37)),
    pattern(&["tts"], "entity/banner/banner_triangles_top", Some(38)),
    // Added after the legacy sheet; only the per-pattern textures exist.
    pattern(&["flw"], "entity/banner/banner_flow", None),
    pattern(&["gus"], "entity/banner/banner_guster", None),
    pattern(&["glb"], "entity/banner/banner_globe", None),
    pattern(&["pig"], "entity/banner/banner_piglin", None),
    // The ominous banner: grey cloth with the illager face already painted on.
    full_sheet_pattern(
        &["ill", "illager", "banner_illager", "minecraft:illager"],
        "entity/banner/banner_illager",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_ids_map_to_their_own_texture_and_legacy_cell() {
        // Ids come from the wiki's Bedrock pattern table; cells from the UV rects in
        // `texture_backwards_compatibility_mapping.json` (`banner_border` at (64, 0),
        // `banner_square_bottom_left` at (384, 128), `banner_triangles_top` at (384, 256)).
        let border = BannerArt::pattern_index("bo").expect("border");
        assert_eq!(BannerArt::pattern_texture_key(border), Some("entity/banner/banner_border"));
        assert_eq!(BannerArt::pattern_cell(border), Some(1));
        assert_eq!(BannerArt::cell_origin(1), [64, 0]);

        let triangles_top = BannerArt::pattern_index("tts").expect("triangles_top");
        assert_eq!(BannerArt::pattern_cell(triangles_top), Some(38));
        assert_eq!(BannerArt::cell_origin(38), [384, 256]);

        let square_bl = BannerArt::pattern_index("bl").expect("square_bottom_left");
        assert_eq!(BannerArt::pattern_cell(square_bl), Some(22));
        assert_eq!(BannerArt::cell_origin(22), [384, 128]);

        // Patterns added after the legacy sheet only have their own texture.
        let flow = BannerArt::pattern_index("flw").expect("flow");
        assert_eq!(BannerArt::pattern_texture_key(flow), Some("entity/banner/banner_flow"));
        assert_eq!(BannerArt::pattern_cell(flow), None);
        for id in ["gus", "glb", "pig"] {
            let index = BannerArt::pattern_index(id).unwrap_or_else(|| panic!("{id}"));
            assert!(BannerArt::pattern_cell(index).is_none(), "{id} has no sheet cell");
        }

        // The ominous banner's own aliases resolve to one entry, and it is a finished cloth.
        let ominous = BannerArt::pattern_index("ill").expect("ill");
        for alias in ["illager", "banner_illager", "minecraft:illager"] {
            assert_eq!(BannerArt::pattern_index(alias), Some(ominous));
        }
        assert!(BannerArt::pattern_is_full_sheet(ominous));
        assert!(!BannerArt::pattern_is_full_sheet(border));
        assert_eq!(
            BannerArt::pattern_texture_key(ominous),
            Some("entity/banner/banner_illager")
        );

        assert_eq!(BannerArt::pattern_index("skull"), None);
        // Every id is unique and every cell is unique.
        // No id may belong to two patterns (aliases make the total larger than the entry count).
        let mut ids: Vec<&str> = PATTERNS
            .iter()
            .flat_map(|pattern| pattern.ids.iter().copied())
            .collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total);
        let mut cells: Vec<u8> = PATTERNS.iter().filter_map(|pattern| pattern.cell).collect();
        cells.sort_unstable();
        cells.dedup();
        assert_eq!(cells.len(), 38);
    }

    #[test]
    fn material_names_round_trip_the_dye_and_layers() {
        let art = BannerArt {
            base: 14,
            layers: vec![
                BannerLayer {
                    pattern: 4,
                    color: 0,
                },
                BannerLayer {
                    pattern: 26,
                    color: 15,
                },
            ],
        };
        let name = art.material_name();
        assert_eq!(name, "bannerart-0e-0400-1a0f");
        // The face pipeline appends the model slot; parsing must ignore it.
        assert_eq!(
            BannerArt::from_material_name("bannerart-0e-0400-1a0f__mat_front"),
            Some(art)
        );
        assert_eq!(
            BannerArt::from_material_name("minecraft_standing_banner"),
            None
        );
    }

    #[test]
    fn dye_colours_resolve_from_names_and_indices() {
        assert_eq!(BannerArt::dye_index("14"), Some(14));
        assert_eq!(BannerArt::dye_index("red"), Some(14));
        assert_eq!(BannerArt::dye_index("light_blue"), Some(3));
        assert_eq!(BannerArt::dye_index("16"), None);
        assert_eq!(BannerArt::dye_index("chartreuse"), None);
    }
}
