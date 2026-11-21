use super::*;

#[derive(Clone, Debug)]
pub(super) struct BlockRecord {
    pub(super) key: BlockKey,
    pub(super) colors: BlockFaceColors,
    pub(super) material: MaterialName,
}

impl BlockRecord {
    pub(super) const fn new(
        key: BlockKey,
        colors: BlockFaceColors,
        material: MaterialName,
    ) -> Self {
        Self {
            key,
            colors,
            material,
        }
    }

    pub(super) const fn uniform(key: BlockKey, color: [f32; 4], material: MaterialName) -> Self {
        Self::new(key, BlockFaceColors::uniform(color), material)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BlockFaceColors {
    pub(super) up: [f32; 4],
    pub(super) down: [f32; 4],
    pub(super) side: [f32; 4],
}

impl BlockFaceColors {
    pub(super) const fn uniform(color: [f32; 4]) -> Self {
        Self {
            up: color,
            down: color,
            side: color,
        }
    }

    pub(super) const fn color_from_normal(self, normal: [i32; 3]) -> [f32; 4] {
        if normal[1] > 0 {
            self.up
        } else if normal[1] < 0 {
            self.down
        } else {
            self.side
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NeighborModel {
    None,
    Pane,
    Fence,
    Wall,
    RedstoneWire { power: u8 },
    Stairs,
    /// Piston bases decide their pushed-face art from the arm block in front: modern
    /// Bedrock states carry no `extended_bit`, and an extended base swaps its pushed
    /// face to the dark inner art when a `piston_arm_collision` block faces back at it.
    PistonBase(PistonFacing),
    /// A door **upper** half. Its own `door_hinge_bit`/`open_bit` are unreliable storage
    /// (the metadata cannot encode a right hinge), so the placement comes from the lower
    /// half below; only the facing direction is read from the upper block itself.
    DoorUpper(CardinalDirection),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConnectionKind {
    Pane,
    Fence,
    FenceGate,
    Wall,
    RedstoneWire,
    RedstoneAny,
    RedstoneAxisX,
    RedstoneAxisZ,
    /// The `piston_arm_collision` / `sticky_piston_arm_collision` head block. Its own
    /// connection kind so a piston base can tell the arm apart from other redstone
    /// devices when it derives its extended look from the facing neighbour.
    PistonArm,
    /// A door **lower** half carrying the functional hinge/open states. Bedrock stores the
    /// reliable `door_hinge_bit`/`open_bit` on the lower half only — the upper half cannot
    /// encode a right hinge and its `open_bit` is degenerate — so an upper half derives its
    /// placement from the kind stored on the half below it.
    DoorLower { hinge: bool, open: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StairState {
    pub(super) facing: CardinalDirection,
    pub(super) top: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct BlockEntityModelState {
    pub(super) chest_pair_direction: Option<CardinalDirection>,
    pub(super) chest_pair_lead: bool,
    /// Banner dye and pattern layers stored on the block entity.
    pub(super) banner: Option<Arc<BannerArt>>,
    /// Bed dye stored on the block entity.
    ///
    /// Bedrock names every bed block plain `bed` and keeps the colour in the `Bed` block entity's
    /// `color` field, so the block name alone cannot tell a red bed from a white one.
    pub(super) bed_colour: Option<&'static str>,
    /// Copper golem statue pose stored on the block entity (`Pose` 0..=3).
    pub(super) pose: Option<i64>,
    /// Skull yaw stored on the block entity (`Rotation`, degrees clockwise from south).
    pub(super) skull_yaw: Option<i32>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BlockClass {
    Air,
    Opaque,
    TransparentGlass,
    DetailOpaque,
    DetailGlass,
    Water,
    Lava,
    SkipTransparent,
}

pub(super) fn block_class(name: &str) -> BlockClass {
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    if matches!(normalized, "air" | "cave_air" | "void_air") {
        return BlockClass::Air;
    }
    // Technical invisible blocks: the game hides them (barrier shows only its
    // particle texture while held), so the preview must not emit faces either.
    if normalized == "barrier"
        || normalized == "structure_void"
        || normalized.starts_with("light_block")
    {
        return BlockClass::Air;
    }
    if matches!(normalized, "water" | "flowing_water") {
        return BlockClass::Water;
    }
    if matches!(normalized, "lava" | "flowing_lava") {
        return BlockClass::Lava;
    }
    if is_full_grass_block(normalized) {
        return BlockClass::Opaque;
    }
    if is_glass_block(normalized) {
        if is_detail_shape_block(normalized) {
            return BlockClass::DetailGlass;
        }
        return BlockClass::TransparentGlass;
    }
    if is_foliage_block(normalized) {
        return BlockClass::Opaque;
    }
    if is_detail_shape_block(normalized) {
        return BlockClass::DetailOpaque;
    }
    if is_transparent_detail_block(normalized) {
        return BlockClass::SkipTransparent;
    }
    BlockClass::Opaque
}

pub(super) fn block_class_is_renderable(block_class: BlockClass) -> bool {
    !matches!(block_class, BlockClass::Air | BlockClass::SkipTransparent)
}

#[cfg(test)]
pub(super) fn is_solid_block(name: &str) -> bool {
    block_class(name) == BlockClass::Opaque
}

pub(super) fn is_transparent_detail_block(normalized: &str) -> bool {
    let transparent_exact = [
        "deadbush",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "snow_layer",
        // `tripwire` renders its own flat string plane (Tripwire family) and must stay
        // renderable — skipping it erased the line between two tripwire hooks.
    ];
    if transparent_exact.contains(&normalized) {
        return true;
    }

    let transparent_suffixes = ["mushroom", "banner", "coral"];

    transparent_suffixes
        .iter()
        .any(|suffix| normalized == *suffix || normalized.ends_with(&format!("_{suffix}")))
}

pub(super) fn is_detail_shape_block(normalized: &str) -> bool {
    model_family::has_detail_shape(normalized)
}

pub(super) fn is_foliage_block(normalized: &str) -> bool {
    if normalized.contains("leaf_litter") {
        return false;
    }
    normalized == "leaves"
        || normalized.ends_with("_leaves")
        || normalized.contains("leaves")
        || normalized.ends_with("_leaf")
        || normalized.contains("foliage")
}

pub(super) fn is_glass_block(normalized: &str) -> bool {
    normalized == "glass"
        || normalized == "glass_pane"
        || normalized.ends_with("_glass")
        || normalized.ends_with("_glass_pane")
        || normalized.contains("stained_glass")
}
