mod building;
mod decorations;
mod direction;
mod flora;
mod redstone;
pub(crate) mod shape;
mod utility;

use std::borrow::Cow;

use crate::state::{BlockStateQuery, BlockStateValue};

pub use shape::{ModelCuboid, ModelPlane, ModelShape};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFamily {
    FullBlock,
    OrientedFullBlock,
    Slab,
    Stairs,
    Ladder,
    Fence,
    FenceGate,
    Wall,
    Pane,
    Trapdoor,
    Door,
    Button,
    PressurePlate,
    Carpet,
    Rail,
    RedstoneWire,
    CrossPlant,
    Cactus,
    Bamboo,
    Cocoa,
    Azalea,
    GroundCover,
    MultiFace,
    Farmland,
    Fire,
    Liquid,
    BubbleColumn,
    Tripwire,
    ChorusPlant,
    PointedDripstone,
    MangrovePropagule,
    MangroveRoots,
    Dripleaf,
    SporeBlossom,
    SculkSensor,
    SculkShrieker,
    Vine,
    Torch,
    Lantern,
    Candle,
    Cake,
    Chain,
    Sign,
    Portal,
    Scaffolding,
    Shelf,
    Rod,
    SeaPickle,
    Egg,
    Beacon,
    InsetBlock,
    DragonEgg,
    ItemFrame,
    Head,
    Container,
    ChiseledBookshelf,
    Anvil,
    Cauldron,
    Composter,
    EndPortalFrame,
    FlowerPot,
    DecoratedPot,
    ShulkerBox,
    Stonecutter,
    Hopper,
    Bed,
    Banner,
    Campfire,
    Grindstone,
    Lectern,
    BrewingStand,
    EnchantingTable,
    Bell,
    HeavyCore,
    Conduit,
    DriedGhast,
    CopperGolemStatue,
    RedstoneDevice,
}

#[must_use]
pub fn model_family_for_block_name(name: &str) -> ModelFamily {
    let name = normalized_block_name(name);
    if name == "redstone_wire" {
        return ModelFamily::RedstoneWire;
    }
    if is_oriented_full_block_name(name) {
        return ModelFamily::OrientedFullBlock;
    }

    for resolver in [
        building::family_for,
        redstone::family_for,
        utility::family_for,
        decorations::family_for,
        flora::family_for,
    ] {
        if let Some(family) = resolver(name) {
            return family;
        }
    }

    ModelFamily::FullBlock
}

#[must_use]
pub fn model_shape_for_block_state(state: &BlockStateQuery) -> Option<ModelShape> {
    let name = normalized_block_name(&state.name);
    match model_family_for_block_name(name) {
        ModelFamily::OrientedFullBlock => Some(full_block_shape(name, state)),
        ModelFamily::Fence => Some(building::fences::fence_shape(name, state)),
        ModelFamily::FenceGate => Some(building::fences::fence_gate_shape(name, state)),
        ModelFamily::Wall => Some(building::walls::shape(state)),
        ModelFamily::Pane => Some(building::panes::shape(name, state)),
        ModelFamily::Chain => Some(building::fences::chain_shape(state)),
        ModelFamily::Slab => building::slabs::shape(name, state),
        ModelFamily::Stairs => Some(building::stairs::shape(state)),
        ModelFamily::Ladder => Some(building::stairs::ladder_shape(state)),
        ModelFamily::Trapdoor => Some(building::trapdoors::shape(state)),
        ModelFamily::Door => Some(building::doors::shape(state)),
        ModelFamily::Button => Some(redstone::interactive::button_shape(state)),
        ModelFamily::PressurePlate => Some(redstone::interactive::pressure_plate_shape()),
        ModelFamily::Carpet => Some(building::stairs::carpet_shape(name)),
        ModelFamily::Rail => Some(building::stairs::rail_shape(state)),
        ModelFamily::Sign => Some(decorations::furniture::sign_shape(name, state)),
        ModelFamily::Cactus => Some(flora::special::cactus_shape()),
        ModelFamily::Bamboo => Some(flora::special::bamboo_shape(state)),
        ModelFamily::Cocoa => Some(flora::special::cocoa_shape(state)),
        ModelFamily::Azalea => Some(flora::special::azalea_shape()),
        ModelFamily::GroundCover => Some(flora::special::ground_cover_shape(name, state)),
        ModelFamily::MultiFace => Some(flora::special::multi_face_shape(state)),
        ModelFamily::Farmland => Some(flora::natural::farmland_shape()),
        ModelFamily::Fire => Some(flora::natural::fire_shape()),
        ModelFamily::Liquid => Some(flora::natural::liquid_shape(state)),
        ModelFamily::BubbleColumn => Some(flora::natural::bubble_column_shape()),
        ModelFamily::Tripwire => Some(flora::natural::tripwire_shape()),
        ModelFamily::ChorusPlant => Some(flora::natural::chorus_plant_shape()),
        ModelFamily::PointedDripstone => Some(flora::natural::pointed_dripstone_shape(state)),
        ModelFamily::MangrovePropagule => Some(flora::natural::mangrove_propagule_shape(state)),
        ModelFamily::MangroveRoots => Some(flora::special::mangrove_roots_shape()),
        ModelFamily::Dripleaf => Some(flora::natural::dripleaf_shape(name, state)),
        ModelFamily::SporeBlossom => Some(flora::natural::spore_blossom_shape()),
        ModelFamily::SculkSensor => Some(flora::natural::sculk_sensor_shape(name)),
        ModelFamily::SculkShrieker => Some(flora::natural::sculk_shrieker_shape()),
        ModelFamily::Vine => flora::vines::shape_for_vine(name, state),
        ModelFamily::Torch => Some(decorations::lighting::torch_shape(state)),
        ModelFamily::Lantern => Some(decorations::lighting::lantern_shape(state)),
        ModelFamily::Candle => Some(decorations::lighting::candle_shape(state)),
        ModelFamily::Cake => Some(decorations::objects::cake_shape(name, state)),
        ModelFamily::Portal => Some(decorations::lighting::portal_shape(state)),
        ModelFamily::Scaffolding => Some(decorations::objects::scaffolding_shape(state)),
        ModelFamily::Shelf => Some(decorations::objects::shelf_shape(state)),
        ModelFamily::Rod => Some(decorations::objects::rod_shape(name, state)),
        ModelFamily::SeaPickle => Some(decorations::objects::sea_pickle_shape(state)),
        ModelFamily::Egg => Some(decorations::objects::egg_shape(name, state)),
        ModelFamily::Beacon => Some(decorations::objects::beacon_shape()),
        ModelFamily::InsetBlock => Some(decorations::objects::inset_block_shape(name)),
        ModelFamily::DragonEgg => Some(decorations::objects::dragon_egg_shape()),
        ModelFamily::ItemFrame => Some(decorations::objects::item_frame_shape(state)),
        ModelFamily::Head => Some(decorations::objects::head_shape(name, state)),
        ModelFamily::Container => utility::containers::container_shape(name, state),
        ModelFamily::ChiseledBookshelf => {
            Some(utility::containers::chiseled_bookshelf_shape(state))
        }
        ModelFamily::Anvil => Some(utility::stations::anvil_shape(state)),
        ModelFamily::Cauldron => Some(utility::crafting::cauldron_shape()),
        ModelFamily::Composter => Some(utility::crafting::composter_shape(state)),
        ModelFamily::EndPortalFrame => Some(utility::crafting::end_portal_frame_shape(state)),
        ModelFamily::FlowerPot => Some(utility::crafting::flower_pot_shape(name)),
        ModelFamily::DecoratedPot => Some(utility::crafting::decorated_pot_shape()),
        ModelFamily::ShulkerBox => Some(utility::containers::shulker_box_shape()),
        ModelFamily::Stonecutter => Some(utility::stations::stonecutter_shape(state)),
        ModelFamily::Hopper => Some(utility::stations::hopper_shape(state)),
        ModelFamily::Bed => Some(decorations::furniture::bed_shape(state)),
        ModelFamily::Banner => Some(decorations::furniture::banner_shape(name, state)),
        ModelFamily::Campfire => Some(decorations::campfires::shape(state)),
        ModelFamily::Grindstone => Some(utility::stations::grindstone_shape(state)),
        ModelFamily::Lectern => Some(utility::stations::lectern_shape(state)),
        ModelFamily::BrewingStand => Some(utility::crafting::brewing_stand_shape()),
        ModelFamily::EnchantingTable => Some(utility::crafting::enchanting_table_shape()),
        ModelFamily::Bell => Some(decorations::furniture::bell_shape(state)),
        ModelFamily::HeavyCore => Some(decorations::objects::heavy_core_shape()),
        ModelFamily::Conduit => Some(decorations::objects::conduit_shape()),
        ModelFamily::DriedGhast => Some(decorations::objects::dried_ghast_shape(state)),
        ModelFamily::CopperGolemStatue => Some(decorations::copper_golem::shape(state)),
        ModelFamily::RedstoneDevice => Some(redstone::redstone_device_shape(name, state)),
        ModelFamily::RedstoneWire => Some(redstone::wire::shape_for(state)),
        ModelFamily::CrossPlant => flora::cross_plants::shape_for_cross_plant(name, state),
        ModelFamily::FullBlock => Some(full_block_shape(name, state)),
    }
}

#[must_use]
pub fn full_block_shape(_name: &str, state: &BlockStateQuery) -> ModelShape {
    let mut cuboid = ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
        .with_face_material_slot(crate::material::BlockFace::Up, "up")
        .with_face_material_slot(crate::material::BlockFace::Down, "down")
        .with_face_material_slot(crate::material::BlockFace::North, "north")
        .with_face_material_slot(crate::material::BlockFace::South, "south")
        .with_face_material_slot(crate::material::BlockFace::East, "east")
        .with_face_material_slot(crate::material::BlockFace::West, "west");

    let axis = direction::state_string(state, "pillar_axis")
        .or_else(|| direction::state_string(state, "axis"))
        .unwrap_or("y");

    if axis == "x" {
        cuboid = cuboid
            .with_face_material_slot(crate::material::BlockFace::East, "top")
            .with_face_material_slot(crate::material::BlockFace::West, "top")
            .with_face_material_slot(crate::material::BlockFace::North, "side")
            .with_face_material_slot(crate::material::BlockFace::South, "side")
            .with_face_material_slot(crate::material::BlockFace::Up, "side")
            .with_face_material_slot(crate::material::BlockFace::Down, "side");
    } else if axis == "z" {
        cuboid = cuboid
            .with_face_material_slot(crate::material::BlockFace::North, "top")
            .with_face_material_slot(crate::material::BlockFace::South, "top")
            .with_face_material_slot(crate::material::BlockFace::East, "side")
            .with_face_material_slot(crate::material::BlockFace::West, "side")
            .with_face_material_slot(crate::material::BlockFace::Up, "side")
            .with_face_material_slot(crate::material::BlockFace::Down, "side");
    }

    if let Some(direction) = direction::cardinal_direction(state) {
        let front_face = match direction {
            direction::CardinalDirection::North => crate::material::BlockFace::North,
            direction::CardinalDirection::South => crate::material::BlockFace::South,
            direction::CardinalDirection::East => crate::material::BlockFace::East,
            direction::CardinalDirection::West => crate::material::BlockFace::West,
        };
        cuboid = cuboid.with_face_material_slot(front_face, "front");
    }

    ModelShape::from_cuboids([shape::detail_cuboid_with_local_uv(cuboid)])
}

#[must_use]
pub fn is_full_opaque_block(name: &str) -> bool {
    let name = normalized_block_name(name);
    if is_double_slab_block_name(name) {
        return true;
    }
    matches!(
        model_family_for_block_name(name),
        ModelFamily::FullBlock | ModelFamily::OrientedFullBlock | ModelFamily::ChiseledBookshelf
    ) && !name.contains("glass")
        && !name.contains("ice")
        && !name.contains("water")
        && !name.contains("lava")
        && !name.contains("portal")
        && !name.contains("leaves")
        && !name.contains("leaf")
        && !name.contains("foliage")
        && !name.contains("grate")
        && !name.contains("air")
}

#[must_use]
pub fn model_family_has_detail_shape(name: &str) -> bool {
    let name = normalized_block_name(name);
    if is_double_slab_block_name(name) {
        return false;
    }
    matches!(
        model_family_for_block_name(name),
        ModelFamily::OrientedFullBlock
            | ModelFamily::Slab
            | ModelFamily::Stairs
            | ModelFamily::Ladder
            | ModelFamily::Fence
            | ModelFamily::FenceGate
            | ModelFamily::Wall
            | ModelFamily::Pane
            | ModelFamily::Trapdoor
            | ModelFamily::Door
            | ModelFamily::Button
            | ModelFamily::PressurePlate
            | ModelFamily::Carpet
            | ModelFamily::Rail
            | ModelFamily::RedstoneWire
            | ModelFamily::CrossPlant
            | ModelFamily::Cactus
            | ModelFamily::Bamboo
            | ModelFamily::Cocoa
            | ModelFamily::Azalea
            | ModelFamily::GroundCover
            | ModelFamily::MultiFace
            | ModelFamily::Farmland
            | ModelFamily::Fire
            | ModelFamily::Liquid
            | ModelFamily::BubbleColumn
            | ModelFamily::Tripwire
            | ModelFamily::ChorusPlant
            | ModelFamily::PointedDripstone
            | ModelFamily::MangrovePropagule
            | ModelFamily::MangroveRoots
            | ModelFamily::Dripleaf
            | ModelFamily::SporeBlossom
            | ModelFamily::SculkSensor
            | ModelFamily::SculkShrieker
            | ModelFamily::Vine
            | ModelFamily::Torch
            | ModelFamily::Lantern
            | ModelFamily::Candle
            | ModelFamily::Cake
            | ModelFamily::Chain
            | ModelFamily::Sign
            | ModelFamily::Portal
            | ModelFamily::Scaffolding
            | ModelFamily::Shelf
            | ModelFamily::Rod
            | ModelFamily::SeaPickle
            | ModelFamily::Egg
            | ModelFamily::Beacon
            | ModelFamily::InsetBlock
            | ModelFamily::DragonEgg
            | ModelFamily::ItemFrame
            | ModelFamily::Head
            | ModelFamily::Container
            | ModelFamily::ChiseledBookshelf
            | ModelFamily::Anvil
            | ModelFamily::Cauldron
            | ModelFamily::Composter
            | ModelFamily::EndPortalFrame
            | ModelFamily::FlowerPot
            | ModelFamily::DecoratedPot
            | ModelFamily::ShulkerBox
            | ModelFamily::Stonecutter
            | ModelFamily::Hopper
            | ModelFamily::Bed
            | ModelFamily::Banner
            | ModelFamily::Campfire
            | ModelFamily::Grindstone
            | ModelFamily::Lectern
            | ModelFamily::BrewingStand
            | ModelFamily::EnchantingTable
            | ModelFamily::Bell
            | ModelFamily::HeavyCore
            | ModelFamily::Conduit
            | ModelFamily::DriedGhast
            | ModelFamily::CopperGolemStatue
            | ModelFamily::RedstoneDevice
    )
}

#[must_use]
pub fn detail_material_block_name_for_state(state: &BlockStateQuery) -> Option<Cow<'static, str>> {
    let canonical_name = canonical_block_name_for_state(state);
    let name = canonical_name
        .strip_prefix("minecraft:")
        .unwrap_or(canonical_name.as_str());
    if name == "portal" || name == "nether_portal" {
        return Some(Cow::Borrowed("minecraft:portal"));
    }
    if name == "end_portal" {
        return Some(Cow::Borrowed("minecraft:end_portal"));
    }
    if name == "redstone_wire" {
        return Some(Cow::Borrowed("minecraft:redstone_wire"));
    }
    if name == "cobweb" || name == "web" {
        return Some(Cow::Borrowed("minecraft:web"));
    }
    if name == "snow_layer" {
        return Some(Cow::Borrowed("minecraft:snow"));
    }
    if name == "carpet" {
        return Some(Cow::Borrowed("minecraft:wool"));
    }
    if name == "iron_bars" {
        return Some(Cow::Borrowed("minecraft:iron_bars"));
    }
    if name == "glass_pane" {
        return Some(Cow::Borrowed("minecraft:glass_pane"));
    }
    if name == "chain" {
        return Some(Cow::Borrowed("minecraft:chain"));
    }
    if name == "lantern" || name == "soul_lantern" {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if name == "candle" || name.ends_with("_candle") || name.contains("_candle_") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if matches!(
        name,
        "anvil" | "chipped_anvil" | "damaged_anvil" | "decorated_pot"
    ) {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if name == "stonecutter" || name == "stonecutter_block" {
        return Some(Cow::Borrowed("minecraft:stonecutter_block"));
    }
    if name == "flower_pot" || name.starts_with("potted_") {
        return Some(Cow::Borrowed("minecraft:flower_pot"));
    }
    if name == "shulker_box" || name.ends_with("_shulker_box") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if name == "hopper" {
        return Some(Cow::Borrowed("minecraft:hopper"));
    }
    if name == "chest" || name.ends_with("_chest") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if let Some(color) = name.strip_suffix("_carpet") {
        return Some(Cow::Owned(format!("minecraft:{color}_wool")));
    }
    if let Some(color) = name.strip_suffix("_stained_glass") {
        return Some(Cow::Owned(format!("minecraft:{color}_stained_glass")));
    }
    if let Some(color) = name.strip_suffix("_stained_glass_pane") {
        return Some(Cow::Owned(format!("minecraft:{color}_stained_glass_pane")));
    }
    if detail_material_uses_own_block_name(name) {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if let Some(base) = name.strip_suffix("_stairs")
        && let Some(material) = canonical_stairs_material_name(name, base)
    {
        return Some(material);
    }
    if name == "fence_gate" || name.ends_with("_fence_gate") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if name == "bamboo_fence" {
        return Some(Cow::Borrowed("minecraft:bamboo_fence"));
    }
    if let Some(base) = name.strip_suffix("_fence") {
        if name == "nether_brick_fence" {
            return Some(Cow::Borrowed("minecraft:nether_bricks"));
        }
        return wood_detail_material_name(base);
    }
    if let Some(base) = name.strip_suffix("_trapdoor") {
        return Some(Cow::Owned(format!("minecraft:{base}_trapdoor")));
    }
    if let Some(base) = name.strip_suffix("_wall") {
        return Some(canonical_wall_material_name(base));
    }
    slab_material_block_name_for_state(state).map(Cow::Borrowed)
}

fn detail_material_uses_own_block_name(name: &str) -> bool {
    matches!(
        model_family_for_block_name(name),
        ModelFamily::OrientedFullBlock
            | ModelFamily::CrossPlant
            | ModelFamily::Cactus
            | ModelFamily::Bamboo
            | ModelFamily::Cocoa
            | ModelFamily::Azalea
            | ModelFamily::GroundCover
            | ModelFamily::MultiFace
            | ModelFamily::Farmland
            | ModelFamily::Fire
            | ModelFamily::Liquid
            | ModelFamily::BubbleColumn
            | ModelFamily::Tripwire
            | ModelFamily::ChorusPlant
            | ModelFamily::PointedDripstone
            | ModelFamily::MangrovePropagule
            | ModelFamily::MangroveRoots
            | ModelFamily::Dripleaf
            | ModelFamily::SporeBlossom
            | ModelFamily::SculkSensor
            | ModelFamily::SculkShrieker
            | ModelFamily::Vine
            | ModelFamily::Cake
            | ModelFamily::Sign
            | ModelFamily::Scaffolding
            | ModelFamily::Shelf
            | ModelFamily::Rod
            | ModelFamily::SeaPickle
            | ModelFamily::Egg
            | ModelFamily::Beacon
            | ModelFamily::InsetBlock
            | ModelFamily::DragonEgg
            | ModelFamily::ItemFrame
            | ModelFamily::Head
            | ModelFamily::ChiseledBookshelf
            | ModelFamily::Composter
            | ModelFamily::EndPortalFrame
            | ModelFamily::HeavyCore
            | ModelFamily::Conduit
            | ModelFamily::DriedGhast
            | ModelFamily::CopperGolemStatue
    )
}

#[must_use]
pub fn canonical_block_name_for_state(state: &BlockStateQuery) -> String {
    let name = normalized_block_name(&state.name);
    if name == "wool" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_wool");
    }
    if name == "carpet" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_carpet");
    }
    if name == "stained_glass" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_stained_glass");
    }
    if name == "stained_glass_pane" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_stained_glass_pane");
    }
    if name == "shulker_box"
        && let Some(color) = state_color(state)
    {
        return format!("minecraft:{color}_shulker_box");
    }
    if name == "fence" || name == "fence_gate" {
        if let Some(wood) = state_wood_type(state) {
            return format!("minecraft:{wood}_{name}");
        }
    }
    if name == "cobblestone_wall" || name == "wall" {
        if let Some(wall) = state_wall_variant(state) {
            return format!("minecraft:{wall}_wall");
        }
    }
    if name == "yellow_flower" {
        return "minecraft:dandelion".to_owned();
    }
    if name == "red_flower" {
        if let Some(flower) = state_string(state, "flower_type")
            .or_else(|| state_string(state, "red_flower_type"))
            .and_then(canonical_red_flower_name)
        {
            return format!("minecraft:{flower}");
        }
        return "minecraft:poppy".to_owned();
    }
    if name == "double_plant" {
        if let Some(plant) =
            state_string(state, "double_plant_type").and_then(canonical_double_plant_name)
        {
            return format!("minecraft:{plant}");
        }
        return "minecraft:sunflower".to_owned();
    }
    state.name.clone()
}

fn state_color(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "color")
        .or_else(|| state_string(state, "color_bit"))
        .and_then(canonical_color_name)
        .or_else(|| {
            state_i64(state, "color")
                .or_else(|| state_i64(state, "color_bit"))
                .and_then(color_name_from_int)
        })
}

fn state_string<'a>(state: &'a BlockStateQuery, key: &str) -> Option<&'a str> {
    state.state(key).and_then(block_state_value_as_string)
}

fn state_i64(state: &BlockStateQuery, key: &str) -> Option<i64> {
    match state.state(key) {
        Some(BlockStateValue::Int(value)) => Some(*value),
        Some(BlockStateValue::String(value)) => value.trim().parse::<i64>().ok(),
        Some(BlockStateValue::Bool(value)) => Some(i64::from(*value)),
        None => None,
    }
}

fn normalize_state_literal(value: &str) -> &str {
    value
        .trim()
        .strip_prefix("minecraft:")
        .unwrap_or(value.trim())
}

fn block_state_value_as_string(value: &BlockStateValue) -> Option<&str> {
    match value {
        BlockStateValue::String(value) => Some(value),
        BlockStateValue::Bool(_) | BlockStateValue::Int(_) => None,
    }
}

fn canonical_color_name(value: &str) -> Option<&'static str> {
    let normalized = normalize_state_literal(value);
    if let Ok(index) = normalized.parse::<i64>() {
        return color_name_from_int(index);
    }
    Some(match normalized {
        "white" => "white",
        "orange" => "orange",
        "magenta" => "magenta",
        "light_blue" | "lightblue" | "silver_blue" => "light_blue",
        "yellow" => "yellow",
        "lime" => "lime",
        "pink" => "pink",
        "gray" | "grey" => "gray",
        "silver" | "light_gray" | "light_grey" => "silver",
        "cyan" => "cyan",
        "purple" => "purple",
        "blue" => "blue",
        "brown" => "brown",
        "green" => "green",
        "red" => "red",
        "black" => "black",
        _ => return None,
    })
}

fn color_name_from_int(value: i64) -> Option<&'static str> {
    Some(match value.rem_euclid(16) {
        0 => "white",
        1 => "orange",
        2 => "magenta",
        3 => "light_blue",
        4 => "yellow",
        5 => "lime",
        6 => "pink",
        7 => "gray",
        8 => "silver",
        9 => "cyan",
        10 => "purple",
        11 => "blue",
        12 => "brown",
        13 => "green",
        14 => "red",
        15 => "black",
        _ => return None,
    })
}

fn state_wood_type(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "wood_type")
        .or_else(|| state_string(state, "new_leaf_type"))
        .or_else(|| state_string(state, "old_leaf_type"))
        .and_then(canonical_wood_name)
        .or_else(|| state_i64(state, "wood_type").and_then(wood_name_from_int))
}

fn canonical_wood_name(value: &str) -> Option<&'static str> {
    Some(match normalize_state_literal(value) {
        "oak" => "oak",
        "spruce" => "spruce",
        "birch" => "birch",
        "jungle" => "jungle",
        "acacia" => "acacia",
        "dark_oak" | "big_oak" => "dark_oak",
        "mangrove" => "mangrove",
        "cherry" => "cherry",
        "bamboo" => "bamboo",
        "crimson" => "crimson",
        "warped" => "warped",
        "pale_oak" => "pale_oak",
        _ => return None,
    })
}

fn wood_name_from_int(value: i64) -> Option<&'static str> {
    Some(match value.rem_euclid(6) {
        0 => "oak",
        1 => "spruce",
        2 => "birch",
        3 => "jungle",
        4 => "acacia",
        5 => "dark_oak",
        _ => return None,
    })
}

fn state_wall_variant(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "wall_block_type")
        .and_then(canonical_wall_variant_name)
        .or_else(|| state_i64(state, "wall_block_type").and_then(wall_variant_name_from_int))
}

fn canonical_wall_variant_name(value: &str) -> Option<&'static str> {
    Some(match normalize_state_literal(value) {
        "cobblestone" | "normal" => "cobblestone",
        "mossy_cobblestone" | "cobblestone_mossy" | "mossy" => "mossy_cobblestone",
        "granite" => "granite",
        "diorite" => "diorite",
        "andesite" => "andesite",
        "sandstone" => "sandstone",
        "brick" | "bricks" => "brick",
        "stone_brick" | "stone_bricks" => "stone_brick",
        "mossy_stone_brick" | "mossy_stone_bricks" => "mossy_stone_brick",
        "nether_brick" | "nether_bricks" => "nether_brick",
        "end_brick" | "end_bricks" | "end_stone_brick" | "end_stone_bricks" => "end_brick",
        "prismarine" => "prismarine",
        "red_sandstone" => "red_sandstone",
        "red_nether_brick" | "red_nether_bricks" => "red_nether_brick",
        "deepslate" | "cobbled_deepslate" => "cobbled_deepslate",
        "polished_deepslate" => "polished_deepslate",
        "deepslate_brick" | "deepslate_bricks" => "deepslate_brick",
        "deepslate_tile" | "deepslate_tiles" => "deepslate_tile",
        "blackstone" => "blackstone",
        "polished_blackstone" => "polished_blackstone",
        "polished_blackstone_brick" | "polished_blackstone_bricks" => "polished_blackstone_brick",
        "mud_brick" | "mud_bricks" => "mud_brick",
        "tuff" => "tuff",
        "polished_tuff" => "polished_tuff",
        "tuff_brick" | "tuff_bricks" => "tuff_brick",
        _ => return None,
    })
}

fn wall_variant_name_from_int(value: i64) -> Option<&'static str> {
    Some(match value {
        0 => "cobblestone",
        1 => "mossy_cobblestone",
        2 => "granite",
        3 => "diorite",
        4 => "andesite",
        5 => "sandstone",
        6 => "brick",
        7 => "stone_brick",
        8 => "mossy_stone_brick",
        9 => "nether_brick",
        10 => "end_brick",
        11 => "prismarine",
        12 => "red_sandstone",
        13 => "red_nether_brick",
        _ => return None,
    })
}

fn canonical_red_flower_name(value: &str) -> Option<&'static str> {
    Some(match normalize_state_literal(value) {
        "poppy" => "poppy",
        "blue_orchid" => "blue_orchid",
        "allium" => "allium",
        "azure_bluet" => "azure_bluet",
        "red_tulip" => "red_tulip",
        "orange_tulip" => "orange_tulip",
        "white_tulip" => "white_tulip",
        "pink_tulip" => "pink_tulip",
        "oxeye_daisy" => "oxeye_daisy",
        "cornflower" => "cornflower",
        "lily_of_the_valley" => "lily_of_the_valley",
        _ => return None,
    })
}

fn canonical_double_plant_name(value: &str) -> Option<&'static str> {
    Some(match normalize_state_literal(value) {
        "sunflower" => "sunflower",
        "syringa" | "lilac" => "lilac",
        "grass" | "tall_grass" | "tallgrass" => "tall_grass",
        "fern" | "large_fern" => "large_fern",
        "rose" | "rose_bush" => "rose_bush",
        "paeonia" | "peony" => "peony",
        _ => return None,
    })
}

fn canonical_wall_material_name(base: &str) -> Cow<'static, str> {
    match base {
        "mossy_cobblestone" => Cow::Borrowed("minecraft:mossy_cobblestone"),
        "brick" => Cow::Borrowed("minecraft:bricks"),
        "stone_brick" => Cow::Borrowed("minecraft:stone_bricks"),
        "mossy_stone_brick" => Cow::Borrowed("minecraft:mossy_stone_bricks"),
        "end_brick" | "end_stone_brick" => Cow::Borrowed("minecraft:end_bricks"),
        "mud_brick" => Cow::Borrowed("minecraft:mud_bricks"),
        "nether_brick" => Cow::Borrowed("minecraft:nether_bricks"),
        "red_nether_brick" => Cow::Borrowed("minecraft:red_nether_bricks"),
        "deepslate_brick" => Cow::Borrowed("minecraft:deepslate_bricks"),
        "deepslate_tile" => Cow::Borrowed("minecraft:deepslate_tiles"),
        "polished_blackstone_brick" => Cow::Borrowed("minecraft:polished_blackstone_bricks"),
        "prismarine_brick" => Cow::Borrowed("minecraft:prismarine_bricks"),
        "tuff_brick" => Cow::Borrowed("minecraft:tuff_bricks"),
        _ => Cow::Owned(format!("minecraft:{base}")),
    }
}

fn canonical_stairs_material_name(name: &str, base: &str) -> Option<Cow<'static, str>> {
    if let Some(material) = wood_detail_material_name(base) {
        return Some(material);
    }
    if let Some(material) = canonical_dedicated_slab_material_name(name) {
        return Some(Cow::Borrowed(material));
    }
    let slab_name = format!("{base}_slab");
    if let Some(material) = canonical_dedicated_slab_material_name(&slab_name) {
        return Some(Cow::Borrowed(material));
    }
    let material = match base {
        "brick" => "minecraft:bricks",
        "stone_brick" => "minecraft:stone_bricks",
        "mossy_stone_brick" => "minecraft:mossy_stone_bricks",
        "nether_brick" => "minecraft:nether_bricks",
        "red_nether_brick" => "minecraft:red_nether_bricks",
        "end_brick" | "end_stone_brick" => "minecraft:end_bricks",
        "purpur" => "minecraft:purpur_block",
        "quartz" => "minecraft:quartz_block",
        "smooth_quartz" => "minecraft:smooth_quartz",
        "sandstone" => "minecraft:sandstone",
        "red_sandstone" => "minecraft:red_sandstone",
        "smooth_sandstone" => "minecraft:smooth_sandstone",
        "smooth_red_sandstone" => "minecraft:smooth_red_sandstone",
        "cut_sandstone" => "minecraft:cut_sandstone",
        "cut_red_sandstone" => "minecraft:cut_red_sandstone",
        "cobblestone" | "stone" | "normal_stone" => "minecraft:cobblestone",
        "mossy_cobblestone" => "minecraft:mossy_cobblestone",
        "granite" => "minecraft:granite",
        "polished_granite" => "minecraft:polished_granite",
        "diorite" => "minecraft:diorite",
        "polished_diorite" => "minecraft:polished_diorite",
        "andesite" => "minecraft:andesite",
        "polished_andesite" => "minecraft:polished_andesite",
        "prismarine" | "prismarine_rough" => "minecraft:prismarine",
        "dark_prismarine" | "prismarine_dark" => "minecraft:dark_prismarine",
        "prismarine_brick" | "prismarine_bricks" => "minecraft:prismarine_bricks",
        "polished_blackstone" => "minecraft:polished_blackstone",
        "polished_blackstone_brick" | "polished_blackstone_bricks" => {
            "minecraft:polished_blackstone_bricks"
        }
        "deepslate" => "minecraft:deepslate",
        "cobbled_deepslate" => "minecraft:cobbled_deepslate",
        "polished_deepslate" => "minecraft:polished_deepslate",
        "deepslate_brick" | "deepslate_bricks" => "minecraft:deepslate_bricks",
        "deepslate_tile" | "deepslate_tiles" => "minecraft:deepslate_tiles",
        "mud_brick" | "mud_bricks" => "minecraft:mud_bricks",
        _ => return Some(Cow::Owned(format!("minecraft:{base}"))),
    };
    Some(Cow::Borrowed(material))
}

fn wood_detail_material_name(base: &str) -> Option<Cow<'static, str>> {
    let material = match base {
        "oak" | "spruce" | "birch" | "jungle" | "acacia" | "dark_oak" | "mangrove" | "cherry"
        | "bamboo" | "crimson" | "warped" | "pale_oak" => {
            format!("minecraft:{base}_planks")
        }
        "darkoak" | "big_oak" => "minecraft:dark_oak_planks".to_owned(),
        _ => return None,
    };
    Some(Cow::Owned(material))
}

fn slab_material_block_name_for_state(state: &BlockStateQuery) -> Option<&'static str> {
    let raw_name = normalized_block_name(&state.name);
    let name = slab_material_lookup_name(raw_name);
    if let Some(material) = canonical_dedicated_slab_material_name(&name) {
        return Some(material);
    }
    let (state_key, value_map): (&str, &[(&str, &str)]) = match name.as_str() {
        "stone_slab" => (
            "stone_slab_type",
            &[
                ("smooth_stone", "minecraft:smooth_stone"),
                ("sandstone", "minecraft:sandstone"),
                ("wood", "minecraft:oak_planks"),
                ("cobblestone", "minecraft:cobblestone"),
                ("brick", "minecraft:bricks"),
                ("stone_brick", "minecraft:stone_bricks"),
                ("quartz", "minecraft:quartz_block"),
                ("nether_brick", "minecraft:nether_bricks"),
            ],
        ),
        "stone_slab2" => (
            "stone_slab_type_2",
            &[
                ("red_sandstone", "minecraft:red_sandstone"),
                ("purpur", "minecraft:purpur_block"),
                ("prismarine_rough", "minecraft:prismarine"),
                ("prismarine_dark", "minecraft:dark_prismarine"),
                ("prismarine_brick", "minecraft:prismarine_bricks"),
                ("mossy_cobblestone", "minecraft:mossy_cobblestone"),
                ("smooth_sandstone", "minecraft:smooth_sandstone"),
                ("red_nether_brick", "minecraft:red_nether_bricks"),
            ],
        ),
        "stone_slab3" => (
            "stone_slab_type_3",
            &[
                ("end_stone_brick", "minecraft:end_bricks"),
                ("smooth_red_sandstone", "minecraft:smooth_red_sandstone"),
                ("polished_andesite", "minecraft:polished_andesite"),
                ("andesite", "minecraft:andesite"),
                ("diorite", "minecraft:diorite"),
                ("polished_diorite", "minecraft:polished_diorite"),
                ("granite", "minecraft:granite"),
                ("polished_granite", "minecraft:polished_granite"),
            ],
        ),
        "stone_slab4" => (
            "stone_slab_type_4",
            &[
                ("mossy_stone_brick", "minecraft:mossy_stone_bricks"),
                ("smooth_quartz", "minecraft:smooth_quartz"),
                ("stone", "minecraft:stone"),
                ("cut_sandstone", "minecraft:cut_sandstone"),
                ("cut_red_sandstone", "minecraft:cut_red_sandstone"),
            ],
        ),
        _ => return None,
    };
    let value = state_string(state, state_key)?;
    value_map
        .iter()
        .find_map(|(candidate, material)| (*candidate == value).then_some(*material))
}

fn slab_material_lookup_name(name: &str) -> String {
    if let Some(stripped) = name.strip_prefix("double_") {
        return stripped.to_owned();
    }
    if let Some(base) = name.strip_suffix("_double_slab") {
        return format!("{base}_slab");
    }
    if let Some(base) = name.strip_suffix("_double_stone_slab") {
        return format!("{base}_stone_slab");
    }
    name.to_owned()
}

fn canonical_dedicated_slab_material_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "acacia_slab" => "minecraft:acacia_planks",
        "andesite_slab" => "minecraft:andesite",
        "bamboo_mosaic_slab" => "minecraft:bamboo_mosaic",
        "bamboo_slab" => "minecraft:bamboo_planks",
        "birch_slab" => "minecraft:birch_planks",
        "blackstone_slab" => "minecraft:blackstone",
        "brick_slab" => "minecraft:bricks",
        "cherry_slab" => "minecraft:cherry_planks",
        "cobbled_deepslate_slab" => "minecraft:cobbled_deepslate",
        "cobblestone_slab" => "minecraft:cobblestone",
        "crimson_slab" => "minecraft:crimson_planks",
        "cut_sandstone_slab" => "minecraft:cut_sandstone",
        "cut_red_sandstone_slab" => "minecraft:cut_red_sandstone",
        "dark_oak_slab" => "minecraft:dark_oak_planks",
        "dark_prismarine_slab" => "minecraft:dark_prismarine",
        "deepslate_brick_slab" => "minecraft:deepslate_bricks",
        "deepslate_tile_slab" => "minecraft:deepslate_tiles",
        "diorite_slab" => "minecraft:diorite",
        "end_brick_slab" | "end_stone_brick_slab" => "minecraft:end_bricks",
        "granite_slab" => "minecraft:granite",
        "jungle_slab" => "minecraft:jungle_planks",
        "mangrove_slab" => "minecraft:mangrove_planks",
        "mossy_cobblestone_slab" => "minecraft:mossy_cobblestone",
        "mossy_stone_brick_slab" => "minecraft:mossy_stone_bricks",
        "mud_brick_slab" => "minecraft:mud_bricks",
        "nether_brick_slab" => "minecraft:nether_bricks",
        "normal_stone_slab" => "minecraft:stone",
        "oak_slab" => "minecraft:oak_planks",
        "pale_oak_slab" => "minecraft:pale_oak_planks",
        "polished_andesite_slab" => "minecraft:polished_andesite",
        "polished_blackstone_slab" => "minecraft:polished_blackstone",
        "polished_blackstone_brick_slab" => "minecraft:polished_blackstone_bricks",
        "polished_deepslate_slab" => "minecraft:polished_deepslate",
        "polished_diorite_slab" => "minecraft:polished_diorite",
        "polished_granite_slab" => "minecraft:polished_granite",
        "polished_tuff_slab" => "minecraft:polished_tuff",
        "prismarine_slab" => "minecraft:prismarine",
        "prismarine_brick_slab" => "minecraft:prismarine_bricks",
        "purpur_slab" => "minecraft:purpur_block",
        "quartz_slab" => "minecraft:quartz_block",
        "red_nether_brick_slab" => "minecraft:red_nether_bricks",
        "red_sandstone_slab" => "minecraft:red_sandstone",
        "sandstone_slab" => "minecraft:sandstone",
        "smooth_quartz_slab" => "minecraft:smooth_quartz",
        "smooth_red_sandstone_slab" => "minecraft:smooth_red_sandstone",
        "smooth_sandstone_slab" => "minecraft:smooth_sandstone",
        "smooth_stone_slab" => "minecraft:smooth_stone",
        "spruce_slab" => "minecraft:spruce_planks",
        "stone_brick_slab" => "minecraft:stone_bricks",
        "tuff_slab" => "minecraft:tuff",
        "tuff_brick_slab" => "minecraft:tuff_bricks",
        "warped_slab" => "minecraft:warped_planks",
        _ => return None,
    })
}

fn normalized_block_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn is_oriented_full_block_name(name: &str) -> bool {
    matches!(
        name,
        "blast_furnace"
            | "furnace"
            | "smoker"
            | "lit_blast_furnace"
            | "lit_furnace"
            | "lit_smoker"
            | "crafter"
            | "end_gateway"
            | "jigsaw"
            | "loom"
            | "command_block"
            | "chain_command_block"
            | "repeating_command_block"
    ) || name.ends_with("_glazed_terracotta")
}

fn is_double_slab_block_name(name: &str) -> bool {
    name.starts_with("double_") && name.contains("slab")
        || name.ends_with("_double_slab")
        || name.ends_with("_double_stone_slab")
}

#[cfg(test)]
mod tests {
    use super::shape;
    use super::{
        ModelFamily, canonical_block_name_for_state, detail_material_block_name_for_state,
        is_full_opaque_block, model_family_for_block_name, model_shape_for_block_state,
    };

    use crate::material::BlockFace;
    use crate::state::BlockStateQuery;

    #[test]
    fn current_bedrock_26_block_variants_should_map_to_common_families() {
        for (block, family) in [
            ("minecraft:pale_oak_fence", ModelFamily::Fence),
            ("minecraft:furnace", ModelFamily::OrientedFullBlock),
            (
                "minecraft:white_glazed_terracotta",
                ModelFamily::OrientedFullBlock,
            ),
            ("minecraft:command_block", ModelFamily::OrientedFullBlock),
            ("minecraft:resin_brick_wall", ModelFamily::Wall),
            ("minecraft:copper_bars", ModelFamily::Pane),
            ("minecraft:oxidized_copper_chain", ModelFamily::Chain),
            ("minecraft:waxed_copper_chest", ModelFamily::Container),
            ("minecraft:pale_oak_shelf", ModelFamily::Shelf),
            ("minecraft:wildflowers", ModelFamily::GroundCover),
            ("minecraft:waterlily", ModelFamily::GroundCover),
            ("minecraft:short_dry_grass", ModelFamily::CrossPlant),
            ("minecraft:torchflower_crop", ModelFamily::CrossPlant),
            ("minecraft:amethyst_cluster", ModelFamily::CrossPlant),
            ("minecraft:azalea", ModelFamily::Azalea),
            ("minecraft:bamboo", ModelFamily::Bamboo),
            ("minecraft:cake", ModelFamily::Cake),
            ("minecraft:candle_cake", ModelFamily::Cake),
            ("minecraft:composter", ModelFamily::Composter),
            ("minecraft:cocoa", ModelFamily::Cocoa),
            ("minecraft:conduit", ModelFamily::Conduit),
            ("minecraft:dried_ghast", ModelFamily::DriedGhast),
            ("minecraft:end_portal_frame", ModelFamily::EndPortalFrame),
            ("minecraft:end_rod", ModelFamily::Rod),
            ("minecraft:farmland", ModelFamily::Farmland),
            ("minecraft:fire", ModelFamily::Fire),
            ("minecraft:heavy_core", ModelFamily::HeavyCore),
            ("minecraft:lightning_rod", ModelFamily::Rod),
            ("minecraft:leaf_litter", ModelFamily::GroundCover),
            ("minecraft:mangrove_roots", ModelFamily::MangroveRoots),
            ("minecraft:muddy_mangrove_roots", ModelFamily::MangroveRoots),
            ("minecraft:pointed_dripstone", ModelFamily::PointedDripstone),
            ("minecraft:resin_clump", ModelFamily::MultiFace),
            ("minecraft:scaffolding", ModelFamily::Scaffolding),
            ("minecraft:sea_pickle", ModelFamily::SeaPickle),
            ("minecraft:sculk_sensor", ModelFamily::SculkSensor),
            ("minecraft:blue_shulker_box", ModelFamily::ShulkerBox),
            (
                "minecraft:chiseled_bookshelf",
                ModelFamily::ChiseledBookshelf,
            ),
            ("minecraft:sniffer_egg", ModelFamily::Egg),
            ("minecraft:decorated_pot", ModelFamily::DecoratedPot),
            ("minecraft:stonecutter_block", ModelFamily::Stonecutter),
            ("minecraft:red_bed", ModelFamily::Bed),
            ("minecraft:standing_banner", ModelFamily::Banner),
            ("minecraft:soul_campfire", ModelFamily::Campfire),
            ("minecraft:grindstone", ModelFamily::Grindstone),
            ("minecraft:lectern", ModelFamily::Lectern),
            ("minecraft:brewing_stand", ModelFamily::BrewingStand),
            ("minecraft:enchanting_table", ModelFamily::EnchantingTable),
            ("minecraft:bell", ModelFamily::Bell),
            ("minecraft:powered_repeater", ModelFamily::RedstoneDevice),
            (
                "minecraft:copper_golem_statue",
                ModelFamily::CopperGolemStatue,
            ),
            (
                "minecraft:waxed_weathered_copper_golem_statue",
                ModelFamily::CopperGolemStatue,
            ),
        ] {
            assert_eq!(model_family_for_block_name(block), family, "{block}");
        }
    }

    #[test]
    fn legacy_bedrock_names_should_map_to_common_families() {
        for (block, family) in [
            ("minecraft:fence", ModelFamily::Fence),
            ("minecraft:cobblestone_wall", ModelFamily::Wall),
            ("minecraft:iron_bars", ModelFamily::Pane),
            ("minecraft:trapdoor", ModelFamily::Trapdoor),
            ("minecraft:wooden_door", ModelFamily::Door),
            ("minecraft:stone_slab", ModelFamily::Slab),
            ("minecraft:double_stone_slab", ModelFamily::Slab),
            ("minecraft:redstone_wire", ModelFamily::RedstoneWire),
            ("minecraft:tallgrass", ModelFamily::CrossPlant),
            ("minecraft:coral_fan_hang", ModelFamily::Vine),
            ("minecraft:coral_fan_hang2", ModelFamily::Vine),
            ("minecraft:coral_fan_hang3", ModelFamily::Vine),
            ("minecraft:chest", ModelFamily::Container),
            ("minecraft:hopper", ModelFamily::Hopper),
        ] {
            assert_eq!(model_family_for_block_name(block), family, "{block}");
        }
    }

    #[test]
    fn legacy_bedrock_variant_states_should_resolve_canonical_block_names() {
        for (state, expected) in [
            (
                BlockStateQuery::new("minecraft:red_flower")
                    .with_state("flower_type", "blue_orchid"),
                "minecraft:blue_orchid",
            ),
            (
                BlockStateQuery::new("minecraft:double_plant")
                    .with_state("double_plant_type", "sunflower"),
                "minecraft:sunflower",
            ),
            (
                BlockStateQuery::new("minecraft:fence").with_state("wood_type", "spruce"),
                "minecraft:spruce_fence",
            ),
            (
                BlockStateQuery::new("minecraft:cobblestone_wall")
                    .with_state("wall_block_type", "end_brick"),
                "minecraft:end_brick_wall",
            ),
            (
                BlockStateQuery::new("minecraft:shulker_box").with_state("color", 11),
                "minecraft:blue_shulker_box",
            ),
            (
                BlockStateQuery::new("minecraft:carpet").with_state("color", "red"),
                "minecraft:red_carpet",
            ),
        ] {
            assert_eq!(canonical_block_name_for_state(&state), expected);
        }
    }

    #[test]
    fn detail_material_should_resolve_legacy_variant_texture_sources() {
        for (state, expected) in [
            (
                BlockStateQuery::new("minecraft:carpet").with_state("color", "red"),
                "minecraft:red_wool",
            ),
            (
                BlockStateQuery::new("minecraft:fence").with_state("wood_type", "spruce"),
                "minecraft:spruce_planks",
            ),
            (
                BlockStateQuery::new("minecraft:spruce_fence_gate"),
                "minecraft:spruce_fence_gate",
            ),
            (
                BlockStateQuery::new("minecraft:fence_gate").with_state("wood_type", "spruce"),
                "minecraft:spruce_fence_gate",
            ),
            (
                BlockStateQuery::new("minecraft:glass_pane"),
                "minecraft:glass_pane",
            ),
            (
                BlockStateQuery::new("minecraft:oak_wall_sign"),
                "minecraft:oak_wall_sign",
            ),
            (
                BlockStateQuery::new("minecraft:hanging_sign"),
                "minecraft:hanging_sign",
            ),
            (
                BlockStateQuery::new("minecraft:mangrove_roots"),
                "minecraft:mangrove_roots",
            ),
            (
                BlockStateQuery::new("minecraft:cobblestone_wall")
                    .with_state("wall_block_type", "brick"),
                "minecraft:bricks",
            ),
            (
                BlockStateQuery::new("minecraft:cobblestone_wall")
                    .with_state("wall_block_type", "end_brick"),
                "minecraft:end_bricks",
            ),
            (
                BlockStateQuery::new("minecraft:stone_slab")
                    .with_state("stone_slab_type", "nether_brick"),
                "minecraft:nether_bricks",
            ),
            (
                BlockStateQuery::new("minecraft:stone_slab3")
                    .with_state("stone_slab_type_3", "end_stone_brick"),
                "minecraft:end_bricks",
            ),
            (
                BlockStateQuery::new("minecraft:pale_oak_shelf"),
                "minecraft:pale_oak_shelf",
            ),
            (BlockStateQuery::new("minecraft:cake"), "minecraft:cake"),
            (
                BlockStateQuery::new("minecraft:blue_candle_cake"),
                "minecraft:blue_candle_cake",
            ),
            (
                BlockStateQuery::new("minecraft:end_rod"),
                "minecraft:end_rod",
            ),
            (
                BlockStateQuery::new("minecraft:composter"),
                "minecraft:composter",
            ),
            (
                BlockStateQuery::new("minecraft:waxed_oxidized_copper_golem_statue"),
                "minecraft:waxed_oxidized_copper_golem_statue",
            ),
        ] {
            assert_eq!(
                detail_material_block_name_for_state(&state).as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn transparent_full_blocks_should_not_be_treated_as_opaque() {
        assert!(!is_full_opaque_block("minecraft:copper_grate"));
        assert!(!is_full_opaque_block(
            "minecraft:waxed_weathered_copper_grate"
        ));
        assert!(!is_full_opaque_block("minecraft:oak_leaves"));
        assert!(!is_full_opaque_block("minecraft:glass"));
        assert!(is_full_opaque_block("minecraft:stone"));
    }

    #[test]
    fn holoprint_reference_shapes_should_not_fallback_to_full_blocks() {
        let cake = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:cake").with_state("bite_counter", 3),
        )
        .unwrap_or_else(|| panic!("missing cake"));
        assert_eq!(cake.cuboids.len(), 1);
        assert!(cake.cuboids[0].max[0] < 0.75);
        assert!((cake.cuboids[0].max[1] - 0.5).abs() < 0.001);

        let candle_cake =
            model_shape_for_block_state(&BlockStateQuery::new("minecraft:blue_candle_cake"))
                .unwrap_or_else(|| panic!("missing candle cake"));
        assert_eq!(candle_cake.cuboids.len(), 2);
        assert_eq!(
            candle_cake.cuboids[1].material_slot.as_deref(),
            Some("candle")
        );

        let plain_candle_cake =
            model_shape_for_block_state(&BlockStateQuery::new("minecraft:candle_cake"))
                .unwrap_or_else(|| panic!("missing plain candle cake"));
        assert_eq!(plain_candle_cake.cuboids.len(), 2);

        let bamboo = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:bamboo")
                .with_state("bamboo_stalk_thickness", "thick")
                .with_state("bamboo_leaf_size", "large_leaves"),
        )
        .unwrap_or_else(|| panic!("missing bamboo"));
        assert_eq!(bamboo.cuboids.len(), 1);
        assert_eq!(bamboo.planes.len(), 2);

        let cocoa = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:cocoa")
                .with_state("age", 2)
                .with_state("direction", 3),
        )
        .unwrap_or_else(|| panic!("missing cocoa"));
        assert_eq!(cocoa.cuboids.len(), 1);
        assert!(cocoa.cuboids[0].min[0] > 0.4);

        let azalea = model_shape_for_block_state(&BlockStateQuery::new("minecraft:azalea"))
            .unwrap_or_else(|| panic!("missing azalea"));
        assert_eq!(azalea.cuboids.len(), 1);
        assert_eq!(azalea.planes.len(), 2);

        let leaf_litter = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:leaf_litter").with_state("growth", 3),
        )
        .unwrap_or_else(|| panic!("missing leaf litter"));
        assert!(leaf_litter.cuboids.is_empty());
        assert_eq!(leaf_litter.planes.len(), 4);

        let waterlily = model_shape_for_block_state(&BlockStateQuery::new("minecraft:waterlily"))
            .unwrap_or_else(|| panic!("missing waterlily"));
        assert!(waterlily.cuboids.is_empty());
        assert_eq!(waterlily.planes.len(), 1);
        assert!(
            waterlily.planes[0]
                .corners
                .iter()
                .all(|corner| (corner[1] - 0.0078125).abs() < 0.001)
        );

        let resin = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:resin_clump")
                .with_state("multi_face_direction_bits", 12),
        )
        .unwrap_or_else(|| panic!("missing resin clump"));
        assert_eq!(resin.planes.len(), 2);

        let mangrove_roots =
            model_shape_for_block_state(&BlockStateQuery::new("minecraft:mangrove_roots"))
                .unwrap_or_else(|| panic!("missing mangrove roots"));
        assert_eq!(mangrove_roots.cuboids.len(), 1);
        assert_eq!(mangrove_roots.planes.len(), 2);
        assert_eq!(
            mangrove_roots.cuboids[0]
                .face_material_slots
                .get(&BlockFace::Side)
                .map(String::as_str),
            Some("side")
        );

        let scaffolding = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:scaffolding").with_state("stability", 1),
        )
        .unwrap_or_else(|| panic!("missing scaffolding"));
        assert!(scaffolding.cuboids.len() >= 9);

        let shelf = model_shape_for_block_state(&BlockStateQuery::new("minecraft:pale_oak_shelf"))
            .unwrap_or_else(|| panic!("missing shelf"));
        assert_eq!(shelf.cuboids.len(), 3);

        let heavy_core = model_shape_for_block_state(&BlockStateQuery::new("minecraft:heavy_core"))
            .unwrap_or_else(|| panic!("missing heavy core"));
        assert!((heavy_core.cuboids[0].max[1] - 0.5).abs() < 0.001);

        let conduit = model_shape_for_block_state(&BlockStateQuery::new("minecraft:conduit"))
            .unwrap_or_else(|| panic!("missing conduit"));
        assert!((conduit.cuboids[0].max[1] - 0.375).abs() < 0.001);

        let dried_ghast =
            model_shape_for_block_state(&BlockStateQuery::new("minecraft:dried_ghast"))
                .unwrap_or_else(|| panic!("missing dried ghast"));
        assert!(dried_ghast.cuboids.len() > 1);

        let copper_golem_standing = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:copper_golem_statue").with_state("entity.Pose", 0),
        )
        .unwrap_or_else(|| panic!("missing copper golem statue"));
        assert_eq!(copper_golem_standing.cuboids.len(), 9);
        assert!(
            copper_golem_standing
                .cuboids
                .iter()
                .all(|cuboid| cuboid.material_slot.as_deref() == Some("body"))
        );
        assert!(
            copper_golem_standing
                .cuboids
                .iter()
                .any(|cuboid| (cuboid.max[1] - 1.5).abs() < 0.001)
        );
        let body_north_uv = copper_golem_standing.cuboids[0]
            .face_uvs
            .get(&BlockFace::North)
            .unwrap_or_else(|| panic!("missing copper golem body north UV"));
        assert!((body_north_uv[0][0] - 6.0 / 64.0).abs() < 0.001);
        assert!((body_north_uv[0][1] - 21.0 / 64.0).abs() < 0.001);
        assert!((body_north_uv[2][0] - 14.0 / 64.0).abs() < 0.001);
        assert!((body_north_uv[2][1] - 27.0 / 64.0).abs() < 0.001);

        let copper_golem_sitting = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:exposed_copper_golem_statue")
                .with_state("entity.Pose", 1),
        )
        .unwrap_or_else(|| panic!("missing sitting copper golem statue"));
        assert_eq!(copper_golem_sitting.cuboids.len(), 11);
        assert!(
            copper_golem_sitting
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[1] < 0.0)
        );

        let copper_golem_running_north = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:weathered_copper_golem_statue")
                .with_state("entity.Pose", 2)
                .with_state("minecraft:cardinal_direction", "north"),
        )
        .unwrap_or_else(|| panic!("missing running copper golem statue"));
        let copper_golem_running_east = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:weathered_copper_golem_statue")
                .with_state("entity.Pose", 2)
                .with_state("minecraft:cardinal_direction", "east"),
        )
        .unwrap_or_else(|| panic!("missing rotated copper golem statue"));
        assert_eq!(copper_golem_running_north.cuboids.len(), 9);
        assert!(copper_golem_running_east.cuboids[1].min[0] > 0.3);
        assert!(
            copper_golem_running_east.cuboids[1].min[2]
                > copper_golem_running_north.cuboids[1].min[2]
        );

        let composter = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:composter").with_state("composter_fill_level", 6),
        )
        .unwrap_or_else(|| panic!("missing composter"));
        assert_eq!(composter.cuboids.len(), 6);

        let portal_frame = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:end_portal_frame")
                .with_state("end_portal_eye_bit", true),
        )
        .unwrap_or_else(|| panic!("missing end portal frame"));
        assert_eq!(portal_frame.cuboids.len(), 2);
        assert!((portal_frame.cuboids[0].max[1] - 0.8125).abs() < 0.001);

        let end_rod = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:end_rod").with_state("facing_direction", 5),
        )
        .unwrap_or_else(|| panic!("missing end rod"));
        assert_eq!(end_rod.cuboids.len(), 2);
        assert!(end_rod.cuboids.iter().any(|cuboid| cuboid.max[0] == 1.0));

        let sea_pickle = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:sea_pickle").with_state("cluster_count", 3),
        )
        .unwrap_or_else(|| panic!("missing sea pickle"));
        assert_eq!(sea_pickle.cuboids.len(), 4);

        let turtle_egg = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:turtle_egg")
                .with_state("turtle_egg_count", "four_egg"),
        )
        .unwrap_or_else(|| panic!("missing turtle egg"));
        assert_eq!(turtle_egg.cuboids.len(), 4);

        let liquid = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:water").with_state("liquid_depth", 7),
        )
        .unwrap_or_else(|| panic!("missing liquid"));
        assert!(liquid.cuboids[0].max[1] < 0.1);

        let fire = model_shape_for_block_state(&BlockStateQuery::new("minecraft:fire"))
            .unwrap_or_else(|| panic!("missing fire"));
        assert_eq!(fire.planes.len(), 6);

        let coral_wall_fan = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:coral_fan_hang").with_state("coral_direction", 0),
        )
        .unwrap_or_else(|| panic!("missing coral wall fan"));
        assert!(coral_wall_fan.cuboids.is_empty());
        assert_eq!(coral_wall_fan.planes.len(), 1);
        assert!(
            coral_wall_fan.planes[0]
                .corners
                .iter()
                .all(|corner| (corner[0] - 0.99).abs() < 0.001)
        );

        let sculk = model_shape_for_block_state(&BlockStateQuery::new("minecraft:sculk_sensor"))
            .unwrap_or_else(|| panic!("missing sculk sensor"));
        assert_eq!(sculk.cuboids.len(), 1);
        assert_eq!(sculk.planes.len(), 4);

        let bookshelf = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:chiseled_bookshelf").with_state("books_stored", 63),
        )
        .unwrap_or_else(|| panic!("missing chiseled bookshelf"));
        assert_eq!(bookshelf.cuboids.len(), 7);
    }

    #[test]
    fn door_shape_should_honor_open_and_hinge_state() {
        let closed = model_shape_for_block_state(&BlockStateQuery::new("minecraft:wooden_door"))
            .expect("missing door");
        let open_left = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:wooden_door").with_state("open_bit", true),
        )
        .expect("missing door");
        let open_right = model_shape_for_block_state(
            &BlockStateQuery::new("minecraft:wooden_door")
                .with_state("open_bit", true)
                .with_state("door_hinge_bit", true),
        )
        .expect("missing door");

        assert_eq!(closed.cuboids.len(), 1);
        assert_ne!(closed.cuboids[0], open_left.cuboids[0]);
        assert_ne!(open_left.cuboids[0], open_right.cuboids[0]);
    }

    #[test]
    fn expanded_block_family_shapes_should_generate_valid_cuboids() {
        for block in [
            "minecraft:red_bed",
            "minecraft:standing_banner",
            "minecraft:campfire",
            "minecraft:grindstone",
            "minecraft:lectern",
            "minecraft:brewing_stand",
            "minecraft:enchanting_table",
            "minecraft:bell",
            "minecraft:unpowered_repeater",
        ] {
            let shape = model_shape_for_block_state(&BlockStateQuery::new(block))
                .unwrap_or_else(|| panic!("missing shape for {block}"));
            assert!(!shape.cuboids.is_empty(), "cuboids empty for {block}");
        }
    }

    #[test]
    fn cross_plant_shape_should_emit_alpha_cutout_planes() {
        let state = BlockStateQuery::new("minecraft:poppy");

        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing plant shape"));

        assert!(shape.cuboids.is_empty());
        assert_eq!(shape.planes.len(), 2);
        assert_eq!(shape.planes[0].material_slot, None);
        assert_eq!(shape.planes[0].uv, None);
    }

    #[test]
    fn row_plants_should_match_holoprint_rows_shape() {
        let wheat = model_shape_for_block_state(&BlockStateQuery::new("minecraft:wheat"))
            .unwrap_or_else(|| panic!("missing wheat shape"));

        assert!(wheat.cuboids.is_empty());
        assert_eq!(wheat.planes.len(), 4);

        let seagrass_top =
            BlockStateQuery::new("minecraft:seagrass").with_state("sea_grass_type", "double_top");
        let seagrass = model_shape_for_block_state(&seagrass_top)
            .unwrap_or_else(|| panic!("missing seagrass shape"));

        assert_eq!(seagrass.planes.len(), 4);
        assert!(
            seagrass
                .planes
                .iter()
                .all(|plane| plane.material_slot.as_deref() == Some("east"))
        );
    }

    #[test]
    fn redstone_wire_shape_should_preserve_line_material_slots() {
        let state = BlockStateQuery::new("minecraft:redstone_wire")
            .with_state("north", "side")
            .with_state("south", "side");

        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing redstone shape"));

        assert!(shape.cuboids.is_empty());
        assert_eq!(shape.planes.len(), 1);
        assert_eq!(shape.planes[0].material_slot.as_deref(), Some("down"));
        assert_eq!(shape.planes[0].uv, Some(shape::full_texture_uv()));
    }

    #[test]
    fn redstone_wire_shape_should_include_wall_planes_for_up_connections() {
        let state = BlockStateQuery::new("minecraft:redstone_wire").with_state("east", "up");

        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing redstone shape"));

        assert_eq!(shape.planes.len(), 3);
        assert!(shape.planes.iter().any(|plane| plane.normal == [1, 0, 0]));
    }

    #[test]
    fn fence_shape_should_follow_connection_state() {
        let state = BlockStateQuery::new("minecraft:oak_fence").with_state("minecraft:north", true);

        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing fence shape"));

        assert_eq!(shape.cuboids.len(), 3);
        assert!(
            shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[2] == 0.0 && cuboid.max[2] == 0.5)
        );
    }

    #[test]
    fn fence_arms_should_project_uvs_onto_the_full_block_texture() {
        let state = BlockStateQuery::new("minecraft:oak_fence").with_state("minecraft:north", true);
        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing fence shape"));
        let lower_arm = shape
            .cuboids
            .iter()
            .find(|cuboid| (cuboid.min[1] - 0.375).abs() < f32::EPSILON)
            .unwrap_or_else(|| panic!("missing lower fence arm"));

        assert_eq!(
            lower_arm.face_uvs.get(&BlockFace::Up),
            Some(&[
                [7.0 / 16.0, 0.0],
                [9.0 / 16.0, 0.0],
                [9.0 / 16.0, 8.0 / 16.0],
                [7.0 / 16.0, 8.0 / 16.0],
            ])
        );
    }

    #[test]
    fn bamboo_fence_should_use_own_material_instead_of_planks() {
        let state = BlockStateQuery::new("minecraft:bamboo_fence");

        assert_eq!(
            detail_material_block_name_for_state(&state).as_deref(),
            Some("minecraft:bamboo_fence")
        );
    }

    #[test]
    fn wall_shape_should_follow_connection_state() {
        let state = BlockStateQuery::new("minecraft:brick_wall")
            .with_state("wall_connection_type_north", "tall")
            .with_state("minecraft:wall_connection_type_south", "none")
            .with_state("wall_post_bit", true);

        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing wall shape"));

        assert_eq!(shape.cuboids.len(), 2);
        assert!(shape.cuboids.iter().any(|cuboid| cuboid.min[2] == 0.0
            && (cuboid.max[1] - 1.0).abs() < 0.001
            && (cuboid.max[2] - 0.5).abs() < 0.001));
    }

    #[test]
    fn wall_shape_should_emit_short_arms_with_bedrock_dimensions() {
        let state = BlockStateQuery::new("minecraft:cobblestone_wall")
            .with_state("minecraft:wall_connection_type_east", "short")
            .with_state("wall_post_bit", false);

        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing wall shape"));

        assert_eq!(shape.cuboids.len(), 1);
        assert!(shape.cuboids.iter().any(|cuboid| {
            cuboid.min[0].abs() < 0.001
                && (cuboid.max[0] - 0.5).abs() < 0.001
                && (cuboid.min[2] - 5.0 / 16.0).abs() < 0.001
                && (cuboid.max[2] - 11.0 / 16.0).abs() < 0.001
                && (cuboid.max[1] - 13.0 / 16.0).abs() < 0.001
        }));

        let tall = BlockStateQuery::new("minecraft:cobblestone_wall")
            .with_state("minecraft:wall_connection_type_west", 2)
            .with_state("wall_post_bit", false);

        let tall_shape =
            model_shape_for_block_state(&tall).unwrap_or_else(|| panic!("missing wall shape"));

        assert_eq!(tall_shape.cuboids.len(), 1);
        assert!(tall_shape.cuboids.iter().any(|cuboid| {
            (cuboid.min[0] - 0.5).abs() < 0.001
                && (cuboid.max[0] - 1.0).abs() < 0.001
                && (cuboid.max[1] - 1.0).abs() < 0.001
        }));
    }

    #[test]
    fn pane_shape_should_emit_isolated_cross_or_connected_arm() {
        let connected =
            BlockStateQuery::new("minecraft:iron_bars").with_state("minecraft:north", true);

        let connected_shape = model_shape_for_block_state(&connected)
            .unwrap_or_else(|| panic!("missing connected pane shape"));

        assert_eq!(connected_shape.cuboids.len(), 2);
        assert_eq!(
            connected_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::Up)
                .map(String::as_str),
            Some("up")
        );
        assert!(
            connected_shape
                .cuboids
                .iter()
                .all(|cuboid| cuboid.max[0] - cuboid.min[0] <= 0.25)
        );

        let string_connected = BlockStateQuery::new("minecraft:glass_pane")
            .with_state("east_connection_type", "short");
        let string_connected_shape = model_shape_for_block_state(&string_connected)
            .unwrap_or_else(|| panic!("missing connected pane shape"));
        assert_eq!(
            string_connected_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::Up)
                .map(String::as_str),
            Some("east")
        );
        assert!(
            string_connected_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[0] >= 0.5625 && cuboid.max[0] == 1.0)
        );

        let isolated = BlockStateQuery::new("minecraft:iron_bars");
        let isolated_shape = model_shape_for_block_state(&isolated)
            .unwrap_or_else(|| panic!("missing isolated pane shape"));

        assert_eq!(isolated_shape.cuboids.len(), 3);
        assert!(
            isolated_shape
                .cuboids
                .iter()
                .filter(|cuboid| {
                    cuboid.max[0] - cuboid.min[0] >= 1.0 || cuboid.max[2] - cuboid.min[2] >= 1.0
                })
                .count()
                == 2
        );
    }

    #[test]
    fn pane_arm_terminal_should_use_the_edge_material_slot() {
        let state = BlockStateQuery::new("minecraft:iron_bars").with_state("minecraft:east", true);
        let shape =
            model_shape_for_block_state(&state).unwrap_or_else(|| panic!("missing pane shape"));
        let east_arm = shape
            .cuboids
            .iter()
            .find(|cuboid| (cuboid.max[0] - 1.0).abs() < f32::EPSILON)
            .unwrap_or_else(|| panic!("missing east pane arm"));

        assert_eq!(
            east_arm
                .face_material_slots
                .get(&BlockFace::East)
                .map(String::as_str),
            Some("east")
        );
    }

    #[test]
    fn trapdoor_shape_should_honor_open_direction_and_half() {
        let open = BlockStateQuery::new("minecraft:oak_trapdoor")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("open_bit", true);

        let open_shape =
            model_shape_for_block_state(&open).unwrap_or_else(|| panic!("missing trapdoor shape"));

        assert_eq!(open_shape.cuboids.len(), 1);
        assert!(
            open_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[2] >= 0.8125 && cuboid.max[2] <= 1.0)
        );
        assert_eq!(
            open_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::North)
                .map(String::as_str),
            Some("up")
        );
        assert_eq!(
            open_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::South)
                .map(String::as_str),
            Some("down")
        );
        assert_eq!(
            open_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::East)
                .map(String::as_str),
            Some("side")
        );
        let north_uv = open_shape.cuboids[0]
            .face_uvs
            .get(&BlockFace::North)
            .expect("north trapdoor face should have uv");
        assert!((north_uv[2][1] - north_uv[0][1]).abs() > 0.9);

        let open_west_hinge = BlockStateQuery::new("minecraft:oak_trapdoor")
            .with_state("direction", 0)
            .with_state("open_bit", true);
        let west_hinge_shape = model_shape_for_block_state(&open_west_hinge)
            .unwrap_or_else(|| panic!("missing trapdoor shape"));
        assert!(
            west_hinge_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[0] >= 0.8125 && cuboid.max[0] <= 1.0)
        );

        let top = BlockStateQuery::new("minecraft:oak_trapdoor").with_state("half", "top");
        let top_shape =
            model_shape_for_block_state(&top).unwrap_or_else(|| panic!("missing trapdoor shape"));

        assert!(
            top_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[1] >= 0.8125 && (cuboid.max[1] - 1.0).abs() < 0.001)
        );
    }

    #[test]
    fn open_trapdoor_top_half_should_flip_the_board_uvs() {
        let bottom = BlockStateQuery::new("minecraft:oak_trapdoor")
            .with_state("direction", 2)
            .with_state("open_bit", true)
            .with_state("upside_down_bit", false);
        let top = BlockStateQuery::new("minecraft:oak_trapdoor")
            .with_state("direction", 2)
            .with_state("open_bit", true)
            .with_state("upside_down_bit", true);

        let bottom_shape =
            model_shape_for_block_state(&bottom).unwrap_or_else(|| panic!("missing trapdoor"));
        let top_shape =
            model_shape_for_block_state(&top).unwrap_or_else(|| panic!("missing trapdoor"));

        assert_ne!(
            bottom_shape.cuboids[0].face_uvs,
            top_shape.cuboids[0].face_uvs
        );
    }

    #[test]
    fn simple_partial_block_shapes_should_follow_state() {
        let top_slab =
            BlockStateQuery::new("minecraft:stone_slab").with_state("top_slot_bit", true);
        let top_slab_shape =
            model_shape_for_block_state(&top_slab).unwrap_or_else(|| panic!("missing slab shape"));
        assert!(
            top_slab_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[1] == 0.5 && cuboid.max[1] == 1.0)
        );

        let inner_stairs = BlockStateQuery::new("minecraft:oak_stairs")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("shape", "inner_left");
        let inner_stairs_shape = model_shape_for_block_state(&inner_stairs)
            .unwrap_or_else(|| panic!("missing stairs shape"));
        assert_eq!(inner_stairs_shape.cuboids.len(), 3);

        let ladder = BlockStateQuery::new("minecraft:ladder")
            .with_state("minecraft:cardinal_direction", "east");
        let ladder_shape =
            model_shape_for_block_state(&ladder).unwrap_or_else(|| panic!("missing ladder shape"));
        assert_eq!(ladder_shape.planes.len(), 1);
        assert_eq!(ladder_shape.planes[0].normal, [-1, 0, 0]);
    }

    #[test]
    fn utility_partial_block_shapes_should_emit_expected_meshes() {
        let open_gate = BlockStateQuery::new("minecraft:oak_fence_gate")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("open_bit", true);
        let gate_shape = model_shape_for_block_state(&open_gate)
            .unwrap_or_else(|| panic!("missing fence gate shape"));
        assert_eq!(gate_shape.cuboids.len(), 8);
        assert!(gate_shape.cuboids.iter().any(|cuboid| cuboid.max[2] == 1.0));

        let wall_gate = BlockStateQuery::new("minecraft:bamboo_fence_gate")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("in_wall_bit", true);
        let wall_gate_shape = model_shape_for_block_state(&wall_gate)
            .unwrap_or_else(|| panic!("missing bamboo fence gate shape"));
        assert_eq!(wall_gate_shape.cuboids.len(), 8);
        assert!(
            wall_gate_shape
                .cuboids
                .iter()
                .any(|cuboid| (cuboid.min[1] - 2.0 / 16.0).abs() < 0.001)
        );
        assert_eq!(
            detail_material_block_name_for_state(&wall_gate).as_deref(),
            Some("minecraft:bamboo_fence_gate")
        );

        let chain = BlockStateQuery::new("minecraft:chain").with_state("pillar_axis", "x");
        let chain_shape =
            model_shape_for_block_state(&chain).unwrap_or_else(|| panic!("missing chain shape"));
        assert_eq!(chain_shape.cuboids.len(), 2);
        assert!(
            chain_shape
                .cuboids
                .iter()
                .all(|cuboid| cuboid.max[0] - cuboid.min[0] == 1.0)
        );

        let button = BlockStateQuery::new("minecraft:stone_button")
            .with_state("minecraft:cardinal_direction", "west");
        let button_shape =
            model_shape_for_block_state(&button).unwrap_or_else(|| panic!("missing button shape"));
        assert!(
            button_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[0] <= 0.125 && cuboid.min[1] >= 0.3125)
        );

        let pressure_plate = BlockStateQuery::new("minecraft:stone_pressure_plate");
        let plate_shape = model_shape_for_block_state(&pressure_plate)
            .unwrap_or_else(|| panic!("missing pressure plate shape"));
        assert!(
            plate_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 0.0625)
        );

        let snow = BlockStateQuery::new("minecraft:snow_layer");
        let snow_shape =
            model_shape_for_block_state(&snow).unwrap_or_else(|| panic!("missing snow shape"));
        assert!(
            snow_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 0.125)
        );
    }

    #[test]
    fn light_and_portal_shapes_should_follow_state() {
        let wall_torch = BlockStateQuery::new("minecraft:torch")
            .with_state("minecraft:block_face", "wall")
            .with_state("minecraft:cardinal_direction", "south");
        let torch_shape = model_shape_for_block_state(&wall_torch)
            .unwrap_or_else(|| panic!("missing torch shape"));
        assert!(
            torch_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[2] >= 0.5 && cuboid.max[1] <= 0.875)
        );

        let lantern = BlockStateQuery::new("minecraft:lantern").with_state("hanging", true);
        let lantern_shape = model_shape_for_block_state(&lantern)
            .unwrap_or_else(|| panic!("missing lantern shape"));
        assert_eq!(lantern_shape.cuboids.len(), 3);
        assert!(
            lantern_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[1] == 0.75 && cuboid.max[1] == 1.0)
        );

        let candles = BlockStateQuery::new("minecraft:candle").with_state("candles", 4);
        let candle_shape =
            model_shape_for_block_state(&candles).unwrap_or_else(|| panic!("missing candle shape"));
        assert_eq!(candle_shape.cuboids.len(), 4);

        let portal = BlockStateQuery::new("minecraft:portal").with_state("portal_axis", "x");
        let portal_shape =
            model_shape_for_block_state(&portal).unwrap_or_else(|| panic!("missing portal shape"));
        assert!(
            portal_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[2] - cuboid.min[2] < 0.08)
        );
    }

    #[test]
    fn block_shell_shapes_should_follow_expected_partial_block_geometry() {
        let chest = BlockStateQuery::new("minecraft:chest")
            .with_state("minecraft:cardinal_direction", "north");
        let chest_shape =
            model_shape_for_block_state(&chest).unwrap_or_else(|| panic!("missing chest shape"));
        assert_eq!(chest_shape.cuboids.len(), 3);
        assert!(
            chest_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 0.875)
        );

        let shulker = BlockStateQuery::new("minecraft:blue_shulker_box");
        let shulker_shape = model_shape_for_block_state(&shulker)
            .unwrap_or_else(|| panic!("missing shulker box shape"));
        assert_eq!(shulker_shape.cuboids.len(), 2);
        assert!(
            shulker_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 0.5)
        );

        let anvil = BlockStateQuery::new("minecraft:anvil")
            .with_state("minecraft:cardinal_direction", "east");
        let anvil_shape =
            model_shape_for_block_state(&anvil).unwrap_or_else(|| panic!("missing anvil shape"));
        assert_eq!(anvil_shape.cuboids.len(), 4);
        assert_eq!(
            anvil_shape.cuboids[0].material_slot.as_deref(),
            Some("side")
        );
        assert!(
            anvil_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 1.0)
        );

        let stonecutter = BlockStateQuery::new("minecraft:stonecutter")
            .with_state("minecraft:cardinal_direction", "south");
        let stonecutter_shape = model_shape_for_block_state(&stonecutter)
            .unwrap_or_else(|| panic!("missing stonecutter shape"));
        assert_eq!(stonecutter_shape.cuboids.len(), 2);
        assert_eq!(stonecutter_shape.planes.len(), 1);
        assert_eq!(
            stonecutter_shape.planes[0].material_slot.as_deref(),
            Some("saw")
        );

        let hopper = BlockStateQuery::new("minecraft:hopper").with_state("facing_direction", 5);
        let hopper_shape =
            model_shape_for_block_state(&hopper).unwrap_or_else(|| panic!("missing hopper shape"));
        assert_eq!(hopper_shape.cuboids.len(), 7);
        assert!(
            hopper_shape
                .cuboids
                .iter()
                .all(|cuboid| !cuboid.face_uvs.is_empty())
        );
        assert!(
            hopper_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min == [0.125, 0.625, 0.125]
                    && cuboid.max == [0.875, 0.6875, 0.875])
        );
        assert!(
            hopper_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[0] == 1.0 && cuboid.min[2] == 0.375)
        );

        let potted = BlockStateQuery::new("minecraft:potted_poppy");
        let potted_shape = model_shape_for_block_state(&potted)
            .unwrap_or_else(|| panic!("missing flower pot shape"));
        assert_eq!(potted_shape.cuboids.len(), 2);
        assert_eq!(potted_shape.planes.len(), 2);
    }

    #[test]
    fn vine_and_rail_shapes_should_emit_detail_planes() {
        let vine = BlockStateQuery::new("minecraft:vine").with_state("north", true);
        let vine_shape =
            model_shape_for_block_state(&vine).unwrap_or_else(|| panic!("missing vine shape"));
        assert_eq!(vine_shape.planes.len(), 1);
        assert_eq!(vine_shape.planes[0].normal, [0, 0, 1]);

        let rail = BlockStateQuery::new("minecraft:rail").with_state("rail_direction", "east_west");
        let rail_shape =
            model_shape_for_block_state(&rail).unwrap_or_else(|| panic!("missing rail shape"));
        assert!(rail_shape.cuboids.is_empty());
        assert_eq!(rail_shape.planes.len(), 1);
        assert_eq!(rail_shape.planes[0].material_slot.as_deref(), Some("up"));
    }

    #[test]
    fn sign_and_cauldron_shapes_should_emit_non_full_geometry() {
        let standing_sign = BlockStateQuery::new("minecraft:standing_sign");
        let standing_sign_shape = model_shape_for_block_state(&standing_sign)
            .unwrap_or_else(|| panic!("missing standing sign shape"));
        assert_eq!(standing_sign_shape.cuboids.len(), 2);

        let wall_sign = BlockStateQuery::new("minecraft:wall_sign")
            .with_state("minecraft:cardinal_direction", "west");
        let wall_sign_shape =
            model_shape_for_block_state(&wall_sign).unwrap_or_else(|| panic!("missing wall sign"));
        assert_eq!(wall_sign_shape.cuboids.len(), 1);

        let hanging_sign = BlockStateQuery::new("minecraft:hanging_sign");
        let hanging_sign_shape = model_shape_for_block_state(&hanging_sign)
            .unwrap_or_else(|| panic!("missing hanging sign"));
        assert!(hanging_sign_shape.cuboids.len() >= 3);

        let wall_hanging_sign = BlockStateQuery::new("minecraft:oak_wall_hanging_sign")
            .with_state("minecraft:cardinal_direction", "east");
        let wall_hanging_sign_shape = model_shape_for_block_state(&wall_hanging_sign)
            .unwrap_or_else(|| panic!("missing wall hanging sign"));
        assert!(
            wall_hanging_sign_shape
                .cuboids
                .iter()
                .any(|cuboid| { cuboid.min[0] >= 14.5 / 16.0 && cuboid.max[0] == 1.0 })
        );

        let cauldron = BlockStateQuery::new("minecraft:cauldron");
        let cauldron_shape =
            model_shape_for_block_state(&cauldron).unwrap_or_else(|| panic!("missing cauldron"));
        assert_eq!(cauldron_shape.cuboids.len(), 5);
    }

    #[test]
    fn advanced_block_states_should_generate_custom_geometry() {
        let repeater_d0 =
            BlockStateQuery::new("minecraft:unpowered_repeater").with_state("repeater_delay", 0);
        let repeater_d3 =
            BlockStateQuery::new("minecraft:unpowered_repeater").with_state("repeater_delay", 3);
        let shape_d0 = model_shape_for_block_state(&repeater_d0).unwrap();
        let shape_d3 = model_shape_for_block_state(&repeater_d3).unwrap();
        assert_ne!(shape_d0.cuboids[2].min[2], shape_d3.cuboids[2].min[2]);

        let piston_normal = BlockStateQuery::new("minecraft:piston");
        let piston_extended =
            BlockStateQuery::new("minecraft:piston").with_state("extended_bit", true);
        assert_eq!(
            model_shape_for_block_state(&piston_normal)
                .unwrap()
                .cuboids
                .len(),
            1
        );
        assert_eq!(
            model_shape_for_block_state(&piston_extended)
                .unwrap()
                .cuboids
                .len(),
            3
        );

        let lectern_empty = BlockStateQuery::new("minecraft:lectern");
        let lectern_book =
            BlockStateQuery::new("minecraft:lectern").with_state("has_book_bit", true);
        assert_eq!(
            model_shape_for_block_state(&lectern_empty)
                .unwrap()
                .cuboids
                .len(),
            3
        );
        assert_eq!(
            model_shape_for_block_state(&lectern_book)
                .unwrap()
                .cuboids
                .len(),
            4
        );
    }

    #[test]
    fn full_block_shapes_and_orientations_should_generate_valid_cuboids() {
        let stone = BlockStateQuery::new("minecraft:stone");
        let stone_shape = model_shape_for_block_state(&stone).unwrap();
        assert_eq!(stone_shape.cuboids.len(), 1);
        assert_eq!(stone_shape.cuboids[0].min, [0.0, 0.0, 0.0]);
        assert_eq!(stone_shape.cuboids[0].max, [1.0, 1.0, 1.0]);

        let log_x = BlockStateQuery::new("minecraft:oak_log").with_state("pillar_axis", "x");
        let log_shape = model_shape_for_block_state(&log_x).unwrap();
        assert_eq!(
            log_shape.cuboids[0]
                .face_material_slots
                .get(&crate::material::BlockFace::East)
                .map(|s| s.as_str()),
            Some("top")
        );
    }
}
