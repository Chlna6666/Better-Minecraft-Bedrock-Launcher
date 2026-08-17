//! Terrain surface role helpers shared by chunk decoding and render sampling.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Role assigned to a block during surface sampling.
pub enum TerrainSurfaceRole {
    /// Air terrain role.
    Air,
    /// Water terrain role.
    Water,
    /// Thin overlay terrain role.
    Overlay,
    /// Primary solid terrain role.
    Primary,
}

pub(crate) fn is_air_block_name(name: &str) -> bool {
    matches!(
        name,
        "air"
            | "cave_air"
            | "void_air"
            | "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:structure_void"
            | "minecraft:light_block"
            | "minecraft:light"
    )
}

pub(crate) fn is_water_block_name(name: &str) -> bool {
    matches!(
        name,
        "water" | "flowing_water" | "minecraft:water" | "minecraft:flowing_water"
    )
}

/// Returns the surface role for a block name.
pub fn terrain_surface_role(name: &str) -> TerrainSurfaceRole {
    if is_air_block_name(name) {
        return TerrainSurfaceRole::Air;
    }
    if is_water_block_name(name) {
        return TerrainSurfaceRole::Water;
    }
    if terrain_surface_overlay_alpha(name).is_some() {
        return TerrainSurfaceRole::Overlay;
    }
    TerrainSurfaceRole::Primary
}

/// Returns the overlay alpha for a thin terrain-surface block.
pub fn terrain_surface_overlay_alpha(name: &str) -> Option<u8> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    if name.contains("carpet") {
        return None;
    }
    if matches!(
        name,
        "short_grass" | "tallgrass" | "tall_grass" | "fern" | "large_fern" | "vine"
    ) || name.contains("vine")
    {
        return Some(82);
    }
    if matches!(
        name,
        "deadbush"
            | "dead_bush"
            | "brown_mushroom"
            | "red_mushroom"
            | "poppy"
            | "dandelion"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "wither_rose"
            | "torchflower"
    ) || name.contains("flower")
        || name.contains("sapling")
        || name.contains("bush")
        || name.contains("petals")
        || name.contains("tulip")
    {
        return Some(115);
    }
    if matches!(
        name,
        "tripWire"
            | "trip_wire"
            | "tripwire_hook"
            | "redstone_wire"
            | "rail"
            | "detector_rail"
            | "activator_rail"
            | "golden_rail"
    ) {
        return Some(130);
    }
    if matches!(
        name,
        "torch"
            | "redstone_torch"
            | "unlit_redstone_torch"
            | "soul_torch"
            | "copper_torch"
            | "lever"
    ) || name.contains("button")
        || name.contains("pressure_plate")
    {
        return Some(155);
    }
    None
}
