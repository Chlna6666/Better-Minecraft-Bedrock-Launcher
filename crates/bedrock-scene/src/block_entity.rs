use super::*;

pub(super) fn block_entity_model_states(
    chunk: &ChunkData,
) -> FxHashMap<BlockKey, BlockEntityModelState> {
    let mut states = FxHashMap::default();
    for entity in &chunk.block_entities {
        let Some([x, y, z]) = entity.position else {
            continue;
        };
        let NbtTag::Compound(nbt) = &entity.nbt else {
            continue;
        };
        let mut state = BlockEntityModelState::default();
        if let (Some(pair_x), Some(pair_z)) = (
            nbt.get("pairx").and_then(NbtTag::as_i32),
            nbt.get("pairz").and_then(NbtTag::as_i32),
        ) {
            let pair_direction = match (pair_x.saturating_sub(x), pair_z.saturating_sub(z)) {
                (1, 0) => Some(CardinalDirection::East),
                (-1, 0) => Some(CardinalDirection::West),
                (0, 1) => Some(CardinalDirection::South),
                (0, -1) => Some(CardinalDirection::North),
                _ => None,
            };
            state.chest_pair_direction = pair_direction;
            state.chest_pair_lead = nbt_bool(nbt.get("pairlead")).unwrap_or(false);
        }
        // The banner's dye and pattern layers live on the block entity, not in the block state.
        if entity.id.as_deref() == Some("Banner") {
            state.banner = banner_art_from_nbt(nbt).map(Arc::new);
            if state.banner.is_some() {
                // A standing banner is two blocks; both halves share the art even when only one
                // of them carries the block entity. `is_banner_block` keeps the extra entries
                // from dyeing whatever else happens to sit above or below.
                states.insert(BlockKey { x, y: y + 1, z }, state.clone());
                states.insert(BlockKey { x, y: y - 1, z }, state.clone());
            }
        }
        // A bed's dye is a `color` byte on the `Bed` block entity; the block is always `bed`.
        if entity.id.as_deref() == Some("Bed") {
            state.bed_colour = bed_colour_from_nbt(nbt);
            if state.bed_colour.is_some() {
                // Beds are two blocks and the entity sits on one half, so both halves get the
                // colour.
                states.insert(BlockKey { x, y: y + 1, z }, state.clone());
                states.insert(BlockKey { x, y: y - 1, z }, state.clone());
            }
        }
        // The statue pose is a `Pose` int on the `CopperGolemStatue` block entity, never a
        // block state.
        if entity.id.as_deref() == Some("CopperGolemStatue") {
            state.pose = nbt
                .get("Pose")
                .and_then(NbtTag::as_i32)
                .map(i64::from)
                .filter(|pose| (0..=3).contains(pose));
        }
        // A floor/ceiling skull's horizontal facing is the `Rotation` float on the `Skull`
        // block entity; the block state only carries floor/ceiling/wall attachment.
        if entity.id.as_deref() == Some("Skull")
            && let Some(NbtTag::Float(degrees)) = nbt.get("Rotation")
        {
            state.skull_yaw = Some((*degrees).round() as i32);
        }
        if state.chest_pair_direction.is_some()
            || state.banner.is_some()
            || state.bed_colour.is_some()
            || state.pose.is_some()
            || state.skull_yaw.is_some()
        {
            states.insert(BlockKey { x, y, z }, state);
        }
    }
    states
}

/// Reads a bed block entity's `color` field as one of the 16 sprite names.
///
/// Bedrock stores it as a dye index byte, but older or edited worlds may hold the name instead,
/// so both forms are accepted.
pub(super) fn bed_colour_from_nbt(
    nbt: &indexmap::IndexMap<String, NbtTag>,
) -> Option<&'static str> {
    const COLOURS: [&str; 16] = [
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
    match nbt.get("color")? {
        NbtTag::Byte(value) => usize::try_from(*value)
            .ok()
            .filter(|index| *index < 16)
            .map(|index| COLOURS[index]),
        NbtTag::Short(value) => usize::try_from(*value)
            .ok()
            .filter(|index| *index < 16)
            .map(|index| COLOURS[index]),
        NbtTag::Int(value) => usize::try_from(*value)
            .ok()
            .filter(|index| *index < 16)
            .map(|index| COLOURS[index]),
        NbtTag::String(value) => {
            let value = value.strip_prefix("minecraft:").unwrap_or(value);
            COLOURS.iter().copied().find(|colour| *colour == value)
        }
        _ => None,
    }
}

pub(super) fn is_banner_block(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(name, "banner" | "standing_banner" | "wall_banner") || name.ends_with("_banner")
}

/// Reads `Base` and the ordered `Patterns` list of a banner block entity.
///
/// Newer worlds spell the dye as a name and older ones as an index, so both forms are accepted;
/// unknown pattern ids are skipped instead of blanking the banner.
pub(super) fn banner_art_from_nbt(nbt: &indexmap::IndexMap<String, NbtTag>) -> Option<BannerArt> {
    let base = banner_dye_index(nbt.get("Base")).unwrap_or(0);
    let mut layers = Vec::new();
    if let Some(NbtTag::List(patterns)) = nbt.get("Patterns") {
        for entry in patterns {
            let NbtTag::Compound(layer) = entry else {
                continue;
            };
            let Some(NbtTag::String(id)) = layer.get("Pattern") else {
                continue;
            };
            let Some(pattern) = BannerArt::pattern_index(id) else {
                continue;
            };
            let color = banner_dye_index(layer.get("Color")).unwrap_or(0);
            layers.push(BannerLayer { pattern, color });
        }
    }
    Some(BannerArt { base, layers })
}

pub(super) fn banner_dye_index(value: Option<&NbtTag>) -> Option<u8> {
    match value? {
        NbtTag::Byte(value) => u8::try_from(*value).ok().filter(|index| *index < 16),
        NbtTag::Short(value) => u8::try_from(*value).ok().filter(|index| *index < 16),
        NbtTag::Int(value) => u8::try_from(*value).ok().filter(|index| *index < 16),
        NbtTag::String(value) => BannerArt::dye_index(value),
        _ => None,
    }
}

pub(super) fn nbt_bool(value: Option<&NbtTag>) -> Option<bool> {
    value.and_then(NbtTag::as_i32).map(|value| value != 0)
}
