mod building;
mod decorations;
mod direction;
mod flora;
mod redstone;
pub mod shape;
mod utility;

use std::borrow::Cow;

use crate::state::{BlockStateQuery, BlockStateValue};

pub use shape::{ModelCuboid, ModelPlane, ModelPlaneSidedness, ModelShape};

/// Bed block geometry from the active resource pack.
pub mod bed {
    pub use super::decorations::bed::{GEOMETRY_IDENTIFIER, shape_from_geometry};
}

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
pub fn classify(name: &str) -> ModelFamily {
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
fn model_shape(state: &BlockStateQuery) -> Option<ModelShape> {
    let name = normalized_block_name(&state.name);
    match classify(name) {
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
        ModelFamily::Tripwire => Some(flora::natural::tripwire_shape(state)),
        ModelFamily::ChorusPlant => {
            let name = canonical_block_name(state);
            Some(flora::natural::chorus_shape(&name, state))
        }
        ModelFamily::PointedDripstone => Some(flora::natural::pointed_dripstone_shape(state)),
        ModelFamily::MangrovePropagule => Some(flora::natural::mangrove_propagule_shape(state)),
        ModelFamily::MangroveRoots => Some(flora::special::mangrove_roots_shape()),
        ModelFamily::Dripleaf => Some(flora::natural::dripleaf_shape(name, state)),
        ModelFamily::SporeBlossom => Some(flora::natural::spore_blossom_shape()),
        ModelFamily::SculkSensor => Some(flora::natural::sculk_sensor_shape(name)),
        ModelFamily::SculkShrieker => Some(flora::natural::sculk_shrieker_shape()),
        ModelFamily::Vine => flora::vines::vine_shape(name, state),
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
        ModelFamily::Head => Some(decorations::skulls::head_shape(name, state)),
        ModelFamily::Container => utility::containers::container_shape(name, state),
        ModelFamily::ChiseledBookshelf => {
            Some(utility::containers::chiseled_bookshelf_shape(state))
        }
        ModelFamily::Anvil => Some(utility::stations::anvil_shape(state)),
        ModelFamily::Cauldron => Some(utility::crafting::cauldron_shape(state)),
        ModelFamily::Composter => Some(utility::crafting::composter_shape(state)),
        ModelFamily::EndPortalFrame => Some(utility::crafting::end_portal_frame_shape(state)),
        ModelFamily::FlowerPot => Some(utility::crafting::flower_pot_shape(name)),
        ModelFamily::DecoratedPot => Some(utility::crafting::decorated_pot_shape()),
        ModelFamily::ShulkerBox => Some(utility::containers::shulker_box_shape()),
        ModelFamily::Stonecutter => Some(utility::stations::stonecutter_shape(state)),
        ModelFamily::Hopper => Some(utility::stations::hopper_shape(state)),
        ModelFamily::Bed => Some(decorations::bed::shape(state)),
        ModelFamily::Banner => Some(decorations::furniture::banner_shape(name, state)),
        ModelFamily::Campfire => Some(decorations::campfires::shape(name, state)),
        ModelFamily::Grindstone => Some(utility::stations::grindstone_shape(state)),
        ModelFamily::Lectern => Some(utility::stations::lectern_shape(state)),
        ModelFamily::BrewingStand => Some(utility::crafting::brewing_stand_shape()),
        ModelFamily::EnchantingTable => Some(utility::crafting::enchanting_table_shape()),
        ModelFamily::Bell => Some(decorations::furniture::bell_shape(state)),
        ModelFamily::HeavyCore => Some(decorations::objects::heavy_core_shape()),
        ModelFamily::Conduit => Some(decorations::objects::conduit_shape()),
        ModelFamily::DriedGhast => Some(decorations::objects::dried_ghast_shape(state)),
        ModelFamily::CopperGolemStatue => Some(decorations::copper_golem::shape(state)),
        ModelFamily::RedstoneDevice => Some(redstone::device_shape(name, state)),
        ModelFamily::RedstoneWire => Some(redstone::wire::shape_for(state)),
        ModelFamily::CrossPlant => flora::cross_plants::cross_plant_shape(name, state),
        ModelFamily::FullBlock => Some(full_block_shape(name, state)),
    }
}

/// Resolves the built-in model-family shape for a block state, including Bedrock texture slots
/// and full-tile UV defaults for faces that do not define their own coordinates.
#[must_use]
pub fn resolve_shape(state: &BlockStateQuery) -> Option<ModelShape> {
    let mut shape = model_shape(state)?;
    apply_double_plant_material_slots(state, &mut shape);
    fill_missing_full_texture_uv(&mut shape);
    Some(shape)
}

pub(super) fn fill_missing_full_texture_uv(shape: &mut ModelShape) {
    for cuboid in &mut shape.cuboids {
        for face in [
            crate::material::BlockFace::Up,
            crate::material::BlockFace::Down,
            crate::material::BlockFace::North,
            crate::material::BlockFace::South,
            crate::material::BlockFace::East,
            crate::material::BlockFace::West,
        ] {
            if !cuboid.face_uvs.contains_key(&face) {
                *cuboid =
                    cuboid.clone().with_face_uv(face, shape::full_texture_face_uv(face));
            }
        }
    }
    for plane in &mut shape.planes {
        if plane.uv.is_none() {
            // A plane's normal names the side it faces, so a vertical card needs the upright
            // sample for the same reason a box side does.
            let uv = match plane.normal {
                [0, 1, 0] | [0, -1, 0] => shape::full_texture_uv(),
                _ => shape::upright_full_texture_uv(),
            };
            plane.uv = Some(uv);
        }
    }
}

pub(super) fn apply_double_plant_material_slots(state: &BlockStateQuery, shape: &mut ModelShape) {
    let canonical = canonical_block_name(state);
    let normalized = canonical.strip_prefix("minecraft:").unwrap_or(&canonical);
    if !matches!(
        normalized,
        "tall_grass"
            | "tallgrass"
            | "large_fern"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "pitcher_plant"
            | "double_plant"
    ) {
        return;
    }
    let upper = state
        .state("upper_block_bit")
        .is_some_and(|value| value.is_truthy());
    let slot = if upper { "up" } else { "down" };
    for plane in &mut shape.planes {
        plane.material_slot = Some(slot.to_owned());
    }
}

/// Whether a block uses the engine-specific procedural shape fallback when a captured or
/// resource-pack model is unavailable.
#[must_use]
pub fn needs_engine_fallback(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == "bed"
        || name.ends_with("_bed")
        || name == "shulker_box"
        || name.ends_with("_shulker_box")
        // The cauldron family carries the liquid surface (water/lava/powder snow by
        // `cauldron_liquid` + `fill_level`), which the registry block definition cannot
        // express — registry geometry would drop the interior contents.
        || name == "cauldron"
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

    // Only blocks whose `blocks.json` entry has a `front` slot turn their faces. A pillar's
    // `pillar_axis` already assigned `top`/`side`, and re-running the face map would clobber it.
    // Each world face takes the slot authored on the face the rotation sends there, so the
    // whole textured model turns as a unit: jigsaw's socket lands on the facing face with the
    // arrows rolled onto the remaining four, and a west-facing furnace shows plain sides on its
    // back instead of a second front. Keeping the authored roles in place (the previous
    // approach) left `front`/`back` art stuck on the north/south faces for every non-identity
    // facing.
    //
    // A pillar's `pillar_axis` already assigned `top`/`side`; re-running the face map would
    // clobber it, so axis-aligned blocks skip this step. Six-face-uniform blocks (e.g. end
    // gateway, whose `blocks.json` has a single `textures` key) must also skip it: giving one
    // face a `front` slot made the resolver fall through to the `textures/items` icon.
    if normalized_block_name(_name) == "jigsaw" {
        // The jigsaw turns its whole textured cube: the authored model (socket north, dark
        // south, arrows on the four roll faces, all arrows running along the socket axis) aims
        // its socket at `facing_direction`, then `rotation` rolls it around that axis. The
        // slot-map path below would place the textures on the right faces but leave every
        // arrow upright — the arrows must travel with the rigid rotation instead.
        let mut rotated = ModelShape::from_cuboids([shape::detail_cuboid_with_local_uv(cuboid)]);
        match direction::block_facing(state).map(direction::target_face) {
            Some(crate::material::BlockFace::South) => {
                rotated = shape::rotate_shape_y_degrees(rotated, 180.0);
            }
            Some(crate::material::BlockFace::East) => {
                rotated = shape::rotate_shape_y_degrees(rotated, 90.0);
            }
            Some(crate::material::BlockFace::West) => {
                rotated = shape::rotate_shape_y_degrees(rotated, 270.0);
            }
            Some(crate::material::BlockFace::Up) => {
                rotated = shape::rotate_shape_x(rotated, 1);
            }
            Some(crate::material::BlockFace::Down) => {
                rotated = shape::rotate_shape_x(rotated, 3);
            }
            _ => {}
        }
        let roll = direction::state_i64(state, "rotation").unwrap_or(0).rem_euclid(4) as i32;
        let roll_y = |degrees: f32, shape: ModelShape| shape::rotate_shape_y_degrees(shape, degrees);
        match direction::block_facing(state).map(direction::target_face) {
            Some(crate::material::BlockFace::North) => {
                rotated = roll_y(90.0, rotated);
                rotated = shape::rotate_shape_x(rotated, roll);
                rotated = roll_y(270.0, rotated);
            }
            Some(crate::material::BlockFace::South) => {
                rotated = roll_y(270.0, rotated);
                rotated = shape::rotate_shape_x(rotated, roll);
                rotated = roll_y(90.0, rotated);
            }
            Some(crate::material::BlockFace::East) => {
                rotated = shape::rotate_shape_x(rotated, roll);
            }
            Some(crate::material::BlockFace::West) => {
                rotated = shape::rotate_shape_x(rotated, 4 - roll);
            }
            Some(crate::material::BlockFace::Up) => {
                rotated = roll_y((roll * 90) as f32, rotated);
            }
            Some(crate::material::BlockFace::Down) => {
                rotated = roll_y(((4 - roll) * 90) as f32, rotated);
            }
            _ => {}
        }
        return rotated;
    }
    if !matches!(axis, "x" | "z") && is_oriented_full_block_name(_name) {
        let rotation = direction::face_rotation(_name, state);
        // Glazed terracotta ships ONE texture key: every face resolves the same art, so the
        // slot map below cannot express the official `minecraft:transformation` rotation —
        // the pattern must turn with the block. A rigid yaw about Y keeps the side art
        // upright (each side face shows the art exactly as authored) and only the top/bottom
        // caps rotate in-plane by the yaw amount.
        if is_single_texture_oriented_name(_name) {
            let yaw = rotation.quarter_turns();
            if yaw != 0 {
                let full = shape::full_texture_uv();
                cuboid = cuboid
                    .with_face_uv(crate::material::BlockFace::Up, shape::rotate_uv_quarter(full, yaw))
                    .with_face_uv(crate::material::BlockFace::Down, shape::rotate_uv_quarter(full, yaw));
            }
        }
        for face in [
            crate::material::BlockFace::Up,
            crate::material::BlockFace::Down,
            crate::material::BlockFace::North,
            crate::material::BlockFace::South,
            crate::material::BlockFace::East,
            crate::material::BlockFace::West,
        ] {
            let slot = match rotation.authored_face_for(face) {
                crate::material::BlockFace::Up => "up",
                crate::material::BlockFace::Down => "down",
                crate::material::BlockFace::North => "north",
                crate::material::BlockFace::South => "south",
                crate::material::BlockFace::East => "east",
                _ => "west",
            };
            cuboid = cuboid.with_face_material_slot(face, slot);
        }
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
        classify(name),
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
pub fn has_detail_shape(name: &str) -> bool {
    let name = normalized_block_name(name);
    if is_double_slab_block_name(name) {
        return false;
    }
    matches!(
        classify(name),
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
pub fn detail_material_block_name(state: &BlockStateQuery) -> Option<Cow<'static, str>> {
    let canonical_name = canonical_block_name(state);
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
    if matches!(name, "moss_carpet" | "pale_moss_carpet") {
        // These are plant materials, not dye-colored wool carpets. Preserve their
        // block definitions so resource packs can supply the appropriate textures.
        return Some(Cow::Owned(format!("minecraft:{name}")));
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
    slab_material_block_name(state).map(Cow::Borrowed)
}

fn detail_material_uses_own_block_name(name: &str) -> bool {
    matches!(
        classify(name),
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
pub fn canonical_block_name(state: &BlockStateQuery) -> String {
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
    state.state(key).and_then(as_string)
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

fn as_string(value: &BlockStateValue) -> Option<&str> {
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
        _ if base.ends_with("_brick") => Cow::Owned(format!("minecraft:{base}s")),
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
        _ if base.ends_with("_brick") => return Some(Cow::Owned(format!("minecraft:{base}s"))),
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

fn slab_material_block_name(state: &BlockStateQuery) -> Option<&'static str> {
    let raw_name = normalized_block_name(&state.name);
    let name = slab_material_lookup_name(raw_name);
    if let Some(material) = canonical_dedicated_slab_material_name(&name) {
        return Some(material);
    }
    let (state_key, value_map): (&str, &[(&str, &str)]) = match name.as_str() {
        "stone_slab" | "stone_block_slab" => (
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
        "stone_slab2" | "stone_block_slab2" => (
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
        "stone_slab3" | "stone_block_slab3" => (
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
        "stone_slab4" | "stone_block_slab4" => (
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

/// Oriented blocks whose `blocks.json` entry ships ONE texture key (glazed terracotta): the
/// slot remap has nothing to remap, so the official rigid rotation must turn the cap UVs.
fn is_single_texture_oriented_name(name: &str) -> bool {
    name.ends_with("_glazed_terracotta")
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
    use super::{
        ModelFamily, canonical_block_name, classify, detail_material_block_name,
        is_full_opaque_block, model_shape, resolve_shape, shape::ModelShape,
    };

    use crate::material::BlockFace;
    use crate::state::BlockStateQuery;

    #[test]
    fn plant_carpets_and_brick_details_keep_valid_material_identities() {
        for (block, expected) in [
            ("moss_carpet", "moss_carpet"),
            ("pale_moss_carpet", "pale_moss_carpet"),
            ("resin_brick_wall", "resin_bricks"),
            ("resin_brick_stairs", "resin_bricks"),
            ("cinnabar_brick_wall", "cinnabar_bricks"),
            ("cinnabar_brick_stairs", "cinnabar_bricks"),
            ("sulfur_brick_wall", "sulfur_bricks"),
            ("sulfur_brick_stairs", "sulfur_bricks"),
            ("red_carpet", "red_wool"),
        ] {
            let state = BlockStateQuery::new(format!("minecraft:{block}"));
            assert_eq!(
                detail_material_block_name(&state).as_deref(),
                Some(format!("minecraft:{expected}").as_str())
            );
        }
    }

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
            assert_eq!(classify(block), family, "{block}");
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
            assert_eq!(classify(block), family, "{block}");
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
            assert_eq!(canonical_block_name(&state), expected);
        }
    }

    #[test]
    fn stone_block_slabs_resolve_variants_for_both_halves_and_double_slabs() {
        for (suffix, key, value, material) in [
            (
                "",
                "stone_slab_type",
                "smooth_stone",
                "minecraft:smooth_stone",
            ),
            (
                "",
                "stone_slab_type",
                "nether_brick",
                "minecraft:nether_bricks",
            ),
            (
                "2",
                "stone_slab_type_2",
                "prismarine_brick",
                "minecraft:prismarine_bricks",
            ),
            ("3", "stone_slab_type_3", "andesite", "minecraft:andesite"),
            (
                "3",
                "stone_slab_type_3",
                "end_stone_brick",
                "minecraft:end_bricks",
            ),
            ("4", "stone_slab_type_4", "stone", "minecraft:stone"),
        ] {
            for stem in ["stone_slab", "stone_block_slab"] {
                for prefix in ["", "double_"] {
                    for top in [false, true] {
                        let name = format!("minecraft:{prefix}{stem}{suffix}");
                        let state = BlockStateQuery::new(&name)
                            .with_state(key, value)
                            .with_state("top_slot_bit", top);
                        assert_eq!(
                            detail_material_block_name(&state).as_deref(),
                            Some(material),
                            "{name} {key}={value} top={top}"
                        );
                        let shape = model_shape(&state).unwrap();
                        let slab = &shape.cuboids[0];
                        let (min_y, max_y) = if !prefix.is_empty() {
                            (0.0, 1.0)
                        } else if top {
                            (0.5, 1.0)
                        } else {
                            (0.0, 0.5)
                        };
                        assert_eq!((slab.min[1], slab.max[1]), (min_y, max_y));
                    }
                }
            }
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
                detail_material_block_name(&state).as_deref(),
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
        let cake =
            model_shape(&BlockStateQuery::new("minecraft:cake").with_state("bite_counter", 3))
                .unwrap_or_else(|| panic!("missing cake"));
        assert_eq!(cake.cuboids.len(), 1);
        // Vanilla cake slices eat from the west side, so min x rises and max x stays at 15.
        assert!(cake.cuboids[0].min[0] > 0.25);
        assert!((cake.cuboids[0].max[1] - 0.5).abs() < 0.001);

        let candle_cake = model_shape(&BlockStateQuery::new("minecraft:blue_candle_cake"))
            .unwrap_or_else(|| panic!("missing candle cake"));
        assert_eq!(candle_cake.cuboids.len(), 2);
        assert_eq!(
            candle_cake.cuboids[1].material_slot.as_deref(),
            Some("candle")
        );

        let plain_candle_cake = model_shape(&BlockStateQuery::new("minecraft:candle_cake"))
            .unwrap_or_else(|| panic!("missing plain candle cake"));
        assert_eq!(plain_candle_cake.cuboids.len(), 2);

        let bamboo = model_shape(
            &BlockStateQuery::new("minecraft:bamboo")
                .with_state("bamboo_stalk_thickness", "thick")
                .with_state("bamboo_leaf_size", "large_leaves"),
        )
        .unwrap_or_else(|| panic!("missing bamboo"));
        assert_eq!(bamboo.cuboids.len(), 1);
        assert_eq!(bamboo.planes.len(), 2);

        let cocoa = model_shape(
            &BlockStateQuery::new("minecraft:cocoa")
                .with_state("age", 2)
                .with_state("direction", 3),
        )
        .unwrap_or_else(|| panic!("missing cocoa"));
        assert_eq!(cocoa.cuboids.len(), 1);
        assert!(cocoa.cuboids[0].min[0] > 0.4);

        let azalea = model_shape(&BlockStateQuery::new("minecraft:azalea"))
            .unwrap_or_else(|| panic!("missing azalea"));
        assert_eq!(azalea.cuboids.len(), 1);
        assert_eq!(azalea.planes.len(), 2);

        let leaf_litter =
            model_shape(&BlockStateQuery::new("minecraft:leaf_litter").with_state("growth", 3))
                .unwrap_or_else(|| panic!("missing leaf litter"));
        assert!(leaf_litter.cuboids.is_empty());
        assert_eq!(leaf_litter.planes.len(), 4);

        let waterlily = model_shape(&BlockStateQuery::new("minecraft:waterlily"))
            .unwrap_or_else(|| panic!("missing waterlily"));
        assert!(waterlily.cuboids.is_empty());
        assert_eq!(waterlily.planes.len(), 1);
        assert!(
            waterlily.planes[0]
                .corners
                .iter()
                .all(|corner| (corner[1] - 0.0078125).abs() < 0.001)
        );

        let resin = model_shape(
            &BlockStateQuery::new("minecraft:resin_clump")
                .with_state("multi_face_direction_bits", 12),
        )
        .unwrap_or_else(|| panic!("missing resin clump"));
        assert_eq!(resin.planes.len(), 2);

        let mangrove_roots = model_shape(&BlockStateQuery::new("minecraft:mangrove_roots"))
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

        let scaffolding =
            model_shape(&BlockStateQuery::new("minecraft:scaffolding").with_state("stability", 1))
                .unwrap_or_else(|| panic!("missing scaffolding"));
        assert!(scaffolding.cuboids.len() >= 9);

        // The shelf uses the client's own tessellated mesh (capture geo 118 / appearance 2376),
        // with one 16-plane mesh per facing — not 3 hand-built cuboids with estimated UVs.
        let shelf = model_shape(&BlockStateQuery::new("minecraft:pale_oak_shelf"))
            .unwrap_or_else(|| panic!("missing shelf"));
        assert!(shelf.cuboids.is_empty());
        assert_eq!(shelf.planes.len(), 16);

        let heavy_core = model_shape(&BlockStateQuery::new("minecraft:heavy_core"))
            .unwrap_or_else(|| panic!("missing heavy core"));
        assert!((heavy_core.cuboids[0].max[1] - 0.5).abs() < 0.001);

        let conduit = model_shape(&BlockStateQuery::new("minecraft:conduit"))
            .unwrap_or_else(|| panic!("missing conduit"));
        assert!((conduit.cuboids[0].max[1] - 0.375).abs() < 0.001);

        let dried_ghast = model_shape(&BlockStateQuery::new("minecraft:dried_ghast"))
            .unwrap_or_else(|| panic!("missing dried ghast"));
        assert!(dried_ghast.cuboids.len() > 1);

        let copper_golem_standing = model_shape(
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
        // The golem's face is on its south (+Z) face; Bedrock's box unwrap puts that panel at
        // `u + depth` (verified against the eyes in copper_golem.png), not the north panel.
        let body_south_uv = copper_golem_standing.cuboids[0]
            .face_uvs
            .get(&BlockFace::South)
            .unwrap_or_else(|| panic!("missing copper golem body south UV"));
        assert!((body_south_uv[0][0] - 6.0 / 64.0).abs() < 0.001);
        assert!((body_south_uv[0][1] - 21.0 / 64.0).abs() < 0.001);
        assert!((body_south_uv[2][0] - 14.0 / 64.0).abs() < 0.001);
        assert!((body_south_uv[2][1] - 27.0 / 64.0).abs() < 0.001);

        let copper_golem_sitting = model_shape(
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

        let copper_golem_running_north = model_shape(
            &BlockStateQuery::new("minecraft:weathered_copper_golem_statue")
                .with_state("entity.Pose", 2)
                .with_state("minecraft:cardinal_direction", "north"),
        )
        .unwrap_or_else(|| panic!("missing running copper golem statue"));
        let copper_golem_running_east = model_shape(
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

        let composter = model_shape(
            &BlockStateQuery::new("minecraft:composter").with_state("composter_fill_level", 6),
        )
        .unwrap_or_else(|| panic!("missing composter"));
        assert_eq!(composter.cuboids.len(), 6);

        let portal_frame = model_shape(
            &BlockStateQuery::new("minecraft:end_portal_frame")
                .with_state("end_portal_eye_bit", true),
        )
        .unwrap_or_else(|| panic!("missing end portal frame"));
        assert_eq!(portal_frame.cuboids.len(), 2);
        assert!((portal_frame.cuboids[0].max[1] - 0.8125).abs() < 0.001);

        let end_rod = model_shape(
            &BlockStateQuery::new("minecraft:end_rod").with_state("facing_direction", 5),
        )
        .unwrap_or_else(|| panic!("missing end rod"));
        assert_eq!(end_rod.cuboids.len(), 2);
        assert!(end_rod.cuboids.iter().any(|cuboid| cuboid.max[0] == 1.0));

        let sea_pickle = model_shape(
            &BlockStateQuery::new("minecraft:sea_pickle").with_state("cluster_count", 2),
        )
        .unwrap_or_else(|| panic!("missing sea pickle"));
        // Official model: three body cuboids (cluster_count 2 = three pickles),
        // each with a flat cap quad and two sprout sheets as planes — no
        // glowing crown boxes.
        assert_eq!(sea_pickle.cuboids.len(), 3);
        assert_eq!(sea_pickle.planes.len(), 9);
        let dead_pickle = model_shape(
            &BlockStateQuery::new("minecraft:sea_pickle")
                .with_state("cluster_count", 2)
                .with_state("dead_bit", 1),
        )
        .unwrap_or_else(|| panic!("missing dead sea pickle"));
        assert_eq!(dead_pickle.cuboids.len(), 3);
        assert_eq!(dead_pickle.planes.len(), 0);

        let turtle_egg = model_shape(
            &BlockStateQuery::new("minecraft:turtle_egg")
                .with_state("turtle_egg_count", "four_egg"),
        )
        .unwrap_or_else(|| panic!("missing turtle egg"));
        assert_eq!(turtle_egg.cuboids.len(), 4);

        let liquid =
            model_shape(&BlockStateQuery::new("minecraft:water").with_state("liquid_depth", 7))
                .unwrap_or_else(|| panic!("missing liquid"));
        assert!(liquid.cuboids[0].max[1] < 0.1);

        let fire = model_shape(&BlockStateQuery::new("minecraft:fire"))
            .unwrap_or_else(|| panic!("missing fire"));
        // Vanilla `template_fire_floor` is four 22.5-degree sheets standing on the block's
        // centre lines, not a 45-degree cross.
        // The capture stores eight planes: two edge sheets plus two tilted sheets per horizontal
        // pair of directions.
        assert_eq!(fire.planes.len(), 8);
        assert!(fire.planes.iter().all(|plane| {
            // Every sheet faces along a single horizontal axis.
            (plane.normal[0] == 0) != (plane.normal[2] == 0)
                && plane.material_slot.as_deref() == Some("side")
        }));
        // Captured BE vertices reach y = 1.4 (22.4 px) and spread to 0.1..0.9 at the top, so the
        // flames spill above their block and the pair opens upward.
        assert!(
            fire.planes
                .iter()
                .any(|plane| plane.corners.iter().any(|corner| corner[1] > 1.3)),
            "fire stands above its block"
        );
        let offsets: Vec<(f32, f32)> = fire
            .planes
            .iter()
            .map(|plane| {
                let axis = if plane.normal[2] != 0 { 2 } else { 0 };
                (plane.corners[0][axis], plane.corners[2][axis])
            })
            .collect();
        assert!(
            offsets.iter().any(|(bottom, _)| *bottom > 0.6)
                && offsets.iter().any(|(bottom, _)| *bottom < 0.4),
            "the sheets stand either side of the centre line: {offsets:?}"
        );
        // Two kinds of sheet, both measured from the capture: the **tilted** ones cross in the
        // middle and open upward (their tip is further from the centre than their base), and the
        // **edge** ones hug a block boundary and lean slightly inward.
        let tilted = |offset: (f32, f32)| offset.0 > 0.15 && offset.0 < 0.85;
        assert!(
            offsets
                .iter()
                .filter(|offset| tilted(**offset))
                .all(|(bottom, top)| (top - 0.5).abs() > (bottom - 0.5).abs()),
            "the tilted sheets open upward: {offsets:?}"
        );
        assert!(
            offsets
                .iter()
                .filter(|offset| !tilted(**offset))
                .all(|(bottom, _)| *bottom < 0.15 || *bottom > 0.85),
            "the edge sheets hug a block boundary: {offsets:?}"
        );

        let end_portal = model_shape(&BlockStateQuery::new("minecraft:end_portal"))
            .unwrap_or_else(|| panic!("missing end portal"));
        assert_eq!(end_portal.cuboids.len(), 1);
        assert!((end_portal.cuboids[0].max[1] - 0.75).abs() < 0.001);
        assert_eq!(end_portal.cuboids[0].min[0], 0.0);
        assert_eq!(end_portal.cuboids[0].max[0], 1.0);

        let coral_wall_fan = model_shape(
            &BlockStateQuery::new("minecraft:coral_fan_hang").with_state("coral_direction", 0),
        )
        .unwrap_or_else(|| panic!("missing coral wall fan"));
        assert!(coral_wall_fan.cuboids.is_empty());
        assert_eq!(coral_wall_fan.planes.len(), 1);
        // The wall fan is a horizontal plate hugging the east wall (capture ref 233).
        assert!(
            coral_wall_fan.planes[0]
                .corners
                .iter()
                .all(|corner| (corner[0] - 0.99).abs() < 0.001 || (corner[0] - 0.19).abs() < 0.001)
        );

        let sculk = model_shape(&BlockStateQuery::new("minecraft:sculk_sensor"))
            .unwrap_or_else(|| panic!("missing sculk sensor"));
        assert_eq!(sculk.cuboids.len(), 1);
        assert_eq!(sculk.planes.len(), 4);

        let bookshelf = model_shape(
            &BlockStateQuery::new("minecraft:chiseled_bookshelf").with_state("books_stored", 63),
        )
        .unwrap_or_else(|| panic!("missing chiseled bookshelf"));
        assert_eq!(bookshelf.cuboids.len(), 7);
        assert_eq!(
            bookshelf.cuboids[1]
                .face_material_slots
                .get(&crate::material::BlockFace::Up)
                .map(String::as_str),
            Some("up"),
            "book tops must use the wooden shelf top, not the book spine"
        );
    }

    #[test]
    fn door_shape_should_honor_open_and_hinge_state() {
        let closed =
            model_shape(&BlockStateQuery::new("minecraft:wooden_door")).expect("missing door");
        let open_left = model_shape(
            &BlockStateQuery::new("minecraft:wooden_door").with_state("open_bit", true),
        )
        .expect("missing door");
        let open_right = model_shape(
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
    fn door_slab_matches_the_captured_placement() {
        // The client capture (refs 22-36) places the door slab on a specific cell edge for each
        // (direction, hinge, open) triple, and both halves always agree. This table is that
        // capture, so a direction-mapping regression shows up here instead of only on screen.
        fn placement(name: &str, direction: &str, hinge: bool, open: bool) -> String {
            let state = BlockStateQuery::new(name)
                .with_state("minecraft:cardinal_direction", direction)
                .with_state("door_hinge_bit", hinge)
                .with_state("open_bit", open);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing {name}"));
            let cuboid = &shape.cuboids[0];
            let span_x = cuboid.max[0] - cuboid.min[0];
            let span_z = cuboid.max[2] - cuboid.min[2];
            if span_x < span_z {
                if cuboid.min[0] < 0.1 { "X low" } else { "X high" }.to_owned()
            } else if cuboid.min[2] < 0.1 {
                "Z low".to_owned()
            } else {
                "Z high".to_owned()
            }
        }

        let cases = [
            ("south", false, false, "X low"),
            ("south", true, false, "X low"),
            ("south", false, true, "Z low"),
            ("south", true, true, "Z high"),
            ("west", false, false, "Z low"),
            ("west", true, false, "Z low"),
            ("west", false, true, "X high"),
            ("west", true, true, "X low"),
            ("north", false, false, "X high"),
            ("north", true, false, "X high"),
            ("north", false, true, "Z high"),
            ("north", true, true, "Z low"),
            ("east", false, false, "Z high"),
            ("east", true, false, "Z high"),
            ("east", false, true, "X low"),
            ("east", true, true, "X high"),
        ];
        for (direction, hinge, open, expected) in cases {
            let actual = placement("minecraft:wooden_door", direction, hinge, open);
            assert_eq!(
                actual, expected,
                "door {direction} hinge={hinge} open={open} should sit at {expected}"
            );
        }

        // Both halves of one door agree, so the upper half never disagrees with the lower.
        for (direction, hinge, open, _) in cases {
            let lower = BlockStateQuery::new("minecraft:wooden_door")
                .with_state("minecraft:cardinal_direction", direction)
                .with_state("door_hinge_bit", hinge)
                .with_state("open_bit", open)
                .with_state("upper_block_bit", false);
            let upper = lower.clone().with_state("upper_block_bit", true);
            let lower_shape = model_shape(&lower).expect("lower half");
            let upper_shape = model_shape(&upper).expect("upper half");
            assert_eq!(
                lower_shape.cuboids[0].min, upper_shape.cuboids[0].min,
                "door halves must agree on {direction} hinge={hinge} open={open}"
            );
            // The halves differ only in which art tile they bind.
            assert_ne!(
                lower_shape.cuboids[0].material_slot, upper_shape.cuboids[0].material_slot,
                "each half samples its own tile"
            );
        }
    }

    #[test]
    fn bed_faces_use_unit_axis_normals() {
        // The bed is the only family whose geometry is authored standing on end and then rotated
        // into block space, so its normals are derived from the winding. Taking the sign of each
        // cross-product component independently produced normals like `[1, 1, -1]`, which are not
        // unit vectors: the renderer's face-shading lookup missed them.
        for direction in 0..4 {
            for head in [false, true] {
                let state = BlockStateQuery::new("minecraft:bed")
                    .with_state("direction", direction)
                    .with_state("head_piece_bit", head);
                let shape = model_shape(&state).unwrap_or_else(|| panic!("missing bed"));
                assert!(!shape.planes.is_empty(), "bed should emit planes");
                for plane in &shape.planes {
                    let non_zero = plane.normal.iter().filter(|value| **value != 0).count();
                    assert_eq!(
                        non_zero, 1,
                        "bed direction={direction} head={head} emitted {:?}, which is not a unit axis normal",
                        plane.normal
                    );
                }
            }
        }
    }

    #[test]
    fn standing_signs_and_banners_turn_by_ground_direction() {
        fn bounds(name: &str, direction: i64) -> ([f32; 3], [f32; 3], usize) {
            let state = BlockStateQuery::new(name).with_state("ground_sign_direction", direction);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing {name}"));
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for plane in &shape.planes {
                for corner in plane.corners {
                    for axis in 0..3 {
                        min[axis] = min[axis].min(corner[axis]);
                        max[axis] = max[axis].max(corner[axis]);
                    }
                }
            }
            for cuboid in &shape.cuboids {
                for axis in 0..3 {
                    min[axis] = min[axis].min(cuboid.min[axis]);
                    max[axis] = max[axis].max(cuboid.max[axis]);
                }
            }
            (min, max, shape.planes.len())
        }

        // 0 faces south (the authored board), 4 faces west: 90 degrees later the board is thin
        // in x and wide in z.
        for name in ["minecraft:standing_sign", "minecraft:standing_banner"] {
            let (_, south_max, _) = bounds(name, 0);
            assert!(south_max[0] > 0.9, "{name} south board spans x");
            let (west_min, west_max, planes) = bounds(name, 4);
            assert!(planes > 0, "{name} must rotate into quads");
            assert!(
                (west_max[0] - west_min[0]) < 0.2,
                "{name} west board is thin in x"
            );
            assert!(
                (west_max[2] - west_min[2]) > 0.8,
                "{name} west board spans z"
            );
        }
    }

    #[test]
    fn top_trapdoors_mirror_the_whole_panel_not_just_the_thin_edge() {
        use crate::material::BlockFace;

        fn open(name: &str, half: &str) -> crate::ModelShape {
            let state = BlockStateQuery::new(name)
                .with_state("open", 1)
                .with_state("facing_direction", 4)
                .with_state("minecraft:vertical_half", half);
            model_shape(&state).unwrap_or_else(|| panic!("missing {name}"))
        }

        let bottom = open("minecraft:oak_trapdoor", "bottom");
        let top = open("minecraft:oak_trapdoor", "top");
        let bottom_cuboid = &bottom.cuboids[0];
        let top_cuboid = &top.cuboids[0];
        // Same footprint: the top panel is the floor one mirrored through the block's middle.
        assert!((bottom_cuboid.min[0] - top_cuboid.min[0]).abs() < 1e-4);
        assert!((bottom_cuboid.max[0] - top_cuboid.max[0]).abs() < 1e-4);
        assert!((bottom_cuboid.min[1] - (1.0 - top_cuboid.max[1])).abs() < 1e-4);
        assert!((bottom_cuboid.max[1] - (1.0 - top_cuboid.min[1])).abs() < 1e-4);
        // The wide faces have to carry the art, not just the 3 px edge: `up` and `down` swap.
        assert_eq!(
            bottom_cuboid.face_material_slots.get(&BlockFace::Up),
            top_cuboid.face_material_slots.get(&BlockFace::Down)
        );
        assert_eq!(
            bottom_cuboid.face_material_slots.get(&BlockFace::Down),
            top_cuboid.face_material_slots.get(&BlockFace::Up)
        );
        // The big side faces still exist on both halves and keep the `side` slot.
        assert_eq!(
            top_cuboid.face_material_slots.get(&BlockFace::North),
            Some(&"side".to_owned())
        );
        assert_eq!(
            top_cuboid.face_material_slots.get(&BlockFace::South),
            Some(&"side".to_owned())
        );
    }

    #[test]
    fn standing_banner_stands_up_with_the_official_assembly() {
        use crate::material::BlockFace;

        // The official entity model (2/3 block scale) stands UP from the ground: a 2 x 42 px
        // pole, the 20 x 40 px cloth hanging from the 2 px bar capping the top. The banner
        // reaches 1.83 blocks with a one-block hitbox and nothing dips below the floor.
        fn banner(name: &str) -> crate::ModelShape {
            model_shape(&BlockStateQuery::new(name))
                .unwrap_or_else(|| panic!("missing {name}"))
        }

        let shape = banner("minecraft:standing_banner");
        assert_eq!(shape.cuboids.len(), 3, "pole, cloth and top bar");
        for cuboid in &shape.cuboids {
            assert!(cuboid.min[1] > -1e-4, "nothing hangs below the floor");
        }
        // The cloth spans x 10 px, y 4..44 px and hangs one model px in front of the pole;
        // every number is the official box divided by 24.
        let flag = shape
            .cuboids
            .iter()
            .find(|cuboid| cuboid.max[1] > 1.8 && cuboid.max[0] - cuboid.min[0] > 0.8)
            .expect("cloth");
        assert!((flag.min[0] - (0.5 - 10.0 / 24.0)).abs() < 1e-4);
        assert!((flag.min[1] - 4.0 / 24.0).abs() < 1e-4);
        assert!((flag.max[1] - 44.0 / 24.0).abs() < 1e-4);
        assert!((flag.min[2] - (0.5 + 1.0 / 24.0)).abs() < 1e-4);
        // The south face carries the front panel, the north face the shaded back panel.
        let front = flag.face_uvs.get(&BlockFace::South).expect("front uv");
        let u = front.iter().map(|corner| corner[0]).collect::<Vec<_>>();
        let v = front.iter().map(|corner| corner[1]).collect::<Vec<_>>();
        assert!(u.contains(&(1.0 / 64.0)) && u.contains(&(21.0 / 64.0)));
        assert!(v.contains(&(1.0 / 64.0)) && v.contains(&(41.0 / 64.0)));
        let back = flag.face_uvs.get(&BlockFace::North).expect("back uv");
        assert!(back
            .iter()
            .any(|corner| (corner[0] - 22.0 / 64.0).abs() < 1e-4));
        // The pole rises the full 42 px and the bar caps the top at 42..44 px.
        let pole = shape
            .cuboids
            .iter()
            .find(|cuboid| cuboid.max[0] - cuboid.min[0] < 0.1)
            .expect("pole");
        assert!((pole.max[1] - 42.0 / 24.0).abs() < 1e-4 && pole.min[1].abs() < 1e-4);
        let bar = shape
            .cuboids
            .iter()
            .find(|cuboid| cuboid.min[1] >= 42.0 / 24.0 - 1e-4)
            .expect("top bar");
        assert!((bar.min[1] - 42.0 / 24.0).abs() < 1e-4);
        assert!((bar.max[1] - 44.0 / 24.0).abs() < 1e-4);
        // The bar reads the official wood windows: south at (2, 44)..(22, 46).
        let bar_front = bar.face_uvs.get(&BlockFace::South).expect("bar uv");
        assert!(bar_front
            .iter()
            .any(|corner| (corner[1] - 46.0 / 64.0).abs() < 1e-4));
        // The legacy `banner` spelling renders the same assembly.
        let legacy = banner("minecraft:banner");
        assert_eq!(legacy.cuboids.len(), shape.cuboids.len());
        // The wall banner mounts the bar on the wall edge and dips below the block line.
        let wall = banner("minecraft:wall_banner");
        assert_eq!(wall.cuboids.len(), 2, "cloth and mount bar");
        let wall_flag = wall
            .cuboids
            .iter()
            .find(|cuboid| cuboid.min[1] < 0.0)
            .expect("hanging cloth");
        assert!((wall_flag.min[1] + 19.5 / 24.0).abs() < 1e-4);
    }

    #[test]
    fn bell_samples_the_corner_of_its_sheets() {
        // `bell_side` is an 8 x 9 piece and the top/bottom sheets are 8 x 8 squares, all in the
        // tile's top-left corner. Whole-tile UVs showed a quarter-size bell inside transparent
        // gutters.
        let shape = model_shape(&BlockStateQuery::new("minecraft:bell")).expect("bell");
        let mut sampled = 0;
        for cuboid in &shape.cuboids {
            for (face, uv) in &cuboid.face_uvs {
                for corner in uv {
                    assert!(
                        corner[0] <= 0.5 + 1e-4 && corner[1] <= 9.0 / 16.0 + 1e-4,
                        "{face:?} leaves the bell art window: {corner:?}"
                    );
                    sampled += 1;
                }
            }
        }
        assert!(sampled >= 24, "the bell body and skirt should carry UVs");
    }

    #[test]
    fn bell_and_grindstone_follow_their_attachment() {
        use std::collections::BTreeSet;

        fn signature(name: &str, key: &str, value: &str) -> String {
            let state = BlockStateQuery::new(name).with_state(key, value);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing {name}"));
            format!(
                "{:?}{:?}",
                shape
                    .cuboids
                    .iter()
                    .map(|c| (c.min, c.max))
                    .collect::<Vec<_>>(),
                shape.planes.len()
            )
        }

        let mut bell = BTreeSet::new();
        for attachment in ["standing", "hanging", "side", "multiple"] {
            assert!(
                bell.insert(signature("minecraft:bell", "attachment", attachment)),
                "bell attachment {attachment} renders like another"
            );
        }
        let mut grindstone = BTreeSet::new();
        for attachment in ["standing", "hanging", "side"] {
            assert!(
                grindstone.insert(signature("minecraft:grindstone", "attachment", attachment)),
                "grindstone attachment {attachment} renders like another"
            );
        }
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
            let shape = model_shape(&BlockStateQuery::new(block))
                .unwrap_or_else(|| panic!("missing shape for {block}"));
            // Beds come from the official entity model, so they emit textured quads only.
            assert!(
                !shape.cuboids.is_empty() || !shape.planes.is_empty(),
                "empty shape for {block}"
            );
        }
    }

    #[test]
    fn cross_plant_shape_should_emit_alpha_cutout_planes() {
        let state = BlockStateQuery::new("minecraft:poppy");

        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing plant shape"));

        assert!(shape.cuboids.is_empty());
        assert_eq!(shape.planes.len(), 2);
        assert_eq!(shape.planes[0].material_slot, None);
        assert_eq!(
            shape.planes[0].uv,
            Some([[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]])
        );
    }

    #[test]
    fn double_plant_halves_use_up_down_textures() {
        let lower = BlockStateQuery::new("minecraft:tall_grass");
        let lower_shape =
            resolve_shape(&lower).unwrap_or_else(|| panic!("missing lower tall grass shape"));
        assert!(
            lower_shape
                .planes
                .iter()
                .any(|plane| plane.material_slot.as_deref() == Some("down")),
            "lower half should use the bottom texture slot"
        );

        let upper =
            BlockStateQuery::new("minecraft:tall_grass").with_state("upper_block_bit", true);
        let upper_shape =
            resolve_shape(&upper).unwrap_or_else(|| panic!("missing upper tall grass shape"));
        assert!(
            upper_shape
                .planes
                .iter()
                .any(|plane| plane.material_slot.as_deref() == Some("up")),
            "upper half should use the top texture slot"
        );
    }

    #[test]
    fn row_plants_should_match_holoprint_rows_shape() {
        let wheat = model_shape(&BlockStateQuery::new("minecraft:wheat"))
            .unwrap_or_else(|| panic!("missing wheat shape"));

        assert!(wheat.cuboids.is_empty());
        assert_eq!(wheat.planes.len(), 4);

        let seagrass_top =
            BlockStateQuery::new("minecraft:seagrass").with_state("sea_grass_type", "double_top");
        let seagrass =
            model_shape(&seagrass_top).unwrap_or_else(|| panic!("missing seagrass shape"));

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

        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing redstone shape"));

        assert!(shape.cuboids.is_empty());
        assert_eq!(shape.planes.len(), 1);
        assert_eq!(shape.planes[0].material_slot.as_deref(), Some("down"));
        // North-south dust rotates the horizontal stripe texture a quarter turn.
        assert_eq!(
            shape.planes[0].uv,
            Some([[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]])
        );
    }

    #[test]
    fn redstone_wire_shape_should_include_wall_planes_for_up_connections() {
        let state = BlockStateQuery::new("minecraft:redstone_wire").with_state("east", "up");

        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing redstone shape"));

        assert_eq!(shape.planes.len(), 3);
        let wall = shape
            .planes
            .iter()
            .find(|plane| plane.normal == [1, 0, 0])
            .expect("east wall-climb plane");
        assert_eq!(
            wall.uv,
            Some([[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]])
        );
    }

    #[test]
    fn fence_shape_should_follow_connection_state() {
        let state = BlockStateQuery::new("minecraft:oak_fence").with_state("minecraft:north", true);

        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing fence shape"));

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
        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing fence shape"));
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
            detail_material_block_name(&state).as_deref(),
            Some("minecraft:bamboo_fence")
        );
    }

    #[test]
    fn wall_shape_should_follow_connection_state() {
        let state = BlockStateQuery::new("minecraft:brick_wall")
            .with_state("wall_connection_type_north", "tall")
            .with_state("minecraft:wall_connection_type_south", "none")
            .with_state("wall_post_bit", true);

        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing wall shape"));

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

        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing wall shape"));

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

        let tall_shape = model_shape(&tall).unwrap_or_else(|| panic!("missing wall shape"));

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

        let connected_shape =
            model_shape(&connected).unwrap_or_else(|| panic!("missing connected pane shape"));

        // Vanilla multipart: the post is its two caps (planes); one arm cuboid per
        // connected side + one noside quad per open side join them.
        assert_eq!(connected_shape.cuboids.len(), 1);
        assert_eq!(connected_shape.planes.len(), 5);
        assert_eq!(
            connected_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::Up)
                .map(String::as_str),
            Some("east")
        );
        // The post caps stay inside the 2 px column; the connected arm is the only part
        // that spans the full cell.
        assert!(connected_shape.planes.iter().all(|plane| {
            plane.corners.iter().all(|corner| {
                (0.4375..=0.5625).contains(&corner[0]) && (0.4375..=0.5625).contains(&corner[2])
            })
        }));
        assert_eq!(
            connected_shape
                .cuboids
                .iter()
                .filter(|cuboid| {
                    cuboid.max[0] - cuboid.min[0] > 0.25 || cuboid.max[2] - cuboid.min[2] > 0.25
                })
                .count(),
            1,
            "only the connected arm leaves the post"
        );

        let string_connected = BlockStateQuery::new("minecraft:glass_pane")
            .with_state("east_connection_type", "short");
        let string_connected_shape = model_shape(&string_connected)
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
        let isolated_shape =
            model_shape(&isolated).unwrap_or_else(|| panic!("missing isolated pane shape"));

        // An isolated pane is the post caps plus four noside quads, none leaves the column.
        assert_eq!(isolated_shape.cuboids.len(), 0, "the post is caps only");
        assert_eq!(isolated_shape.planes.len(), 6, "two caps + one card per direction");
        assert!(
            isolated_shape.planes.iter().all(|plane| {
                plane.corners.iter().all(|corner| {
                    (0.4375..=0.5625).contains(&corner[0]) && (0.4375..=0.5625).contains(&corner[2])
                })
            }),
            "an isolated pane stays a 2 px card"
        );
    }

    #[test]
    fn pane_arm_terminal_should_use_the_edge_material_slot() {
        let state = BlockStateQuery::new("minecraft:iron_bars").with_state("minecraft:east", true);
        let shape = model_shape(&state).unwrap_or_else(|| panic!("missing pane shape"));
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

        let open_shape = model_shape(&open).unwrap_or_else(|| panic!("missing trapdoor shape"));

        assert_eq!(open_shape.cuboids.len(), 1);
        // `direction` is the side the trapdoor sits on, so a north trapdoor hangs on the
        // north boundary of its block.
        assert!(
            open_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[2].abs() < 0.001 && cuboid.max[2] <= 0.1875)
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
        let west_hinge_shape =
            model_shape(&open_west_hinge).unwrap_or_else(|| panic!("missing trapdoor shape"));
        assert!(
            west_hinge_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[0].abs() < 0.001 && cuboid.max[0] <= 0.1875)
        );

        let top = BlockStateQuery::new("minecraft:oak_trapdoor").with_state("half", "top");
        let top_shape = model_shape(&top).unwrap_or_else(|| panic!("missing trapdoor shape"));

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

        let bottom_shape = model_shape(&bottom).unwrap_or_else(|| panic!("missing trapdoor"));
        let top_shape = model_shape(&top).unwrap_or_else(|| panic!("missing trapdoor"));

        assert_ne!(
            bottom_shape.cuboids[0].face_uvs,
            top_shape.cuboids[0].face_uvs
        );
    }

    #[test]
    fn simple_partial_block_shapes_should_follow_state() {
        let top_slab =
            BlockStateQuery::new("minecraft:stone_slab").with_state("top_slot_bit", true);
        let top_slab_shape = model_shape(&top_slab).unwrap_or_else(|| panic!("missing slab shape"));
        assert!(
            top_slab_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.min[1] == 0.5 && cuboid.max[1] == 1.0)
        );

        let inner_stairs = BlockStateQuery::new("minecraft:oak_stairs")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("shape", "inner_left");
        let inner_stairs_shape =
            model_shape(&inner_stairs).unwrap_or_else(|| panic!("missing stairs shape"));
        assert_eq!(inner_stairs_shape.cuboids.len(), 3);

        let ladder = BlockStateQuery::new("minecraft:ladder")
            .with_state("minecraft:cardinal_direction", "east");
        let ladder_shape = model_shape(&ladder).unwrap_or_else(|| panic!("missing ladder shape"));
        assert_eq!(ladder_shape.planes.len(), 1);
        assert_eq!(ladder_shape.planes[0].normal, [1, 0, 0]);
    }

    #[test]
    fn open_trapdoor_leaf_hangs_on_its_direction_side() {
        fn panel(direction: i64) -> ([f32; 3], [f32; 3]) {
            let state = BlockStateQuery::new("minecraft:birch_trapdoor")
                .with_state("direction", direction)
                .with_state("open_bit", true)
                .with_state("upside_down_bit", true);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing trapdoor shape"));
            assert_eq!(shape.cuboids.len(), 1, "direction={direction}");
            (shape.cuboids[0].min, shape.cuboids[0].max)
        }

        // Bedrock `direction` names the side the trapdoor sits on (0=west, 1=east, 2=north,
        // 3=south), so the open leaf hangs on that same block boundary.
        let (west_min, west_max) = panel(0);
        assert!(west_min[0].abs() < 0.001 && (west_max[0] - 0.1875).abs() < 0.001);
        let (east_min, east_max) = panel(1);
        assert!((east_min[0] - 0.8125).abs() < 0.001 && (east_max[0] - 1.0).abs() < 0.001);
        let (north_min, north_max) = panel(2);
        assert!(north_min[2].abs() < 0.001 && (north_max[2] - 0.1875).abs() < 0.001);
        let (south_min, south_max) = panel(3);
        assert!((south_min[2] - 0.8125).abs() < 0.001 && (south_max[2] - 1.0).abs() < 0.001);
    }

    #[test]
    fn open_fence_gate_leaves_turn_with_bedrock_cardinal_rotation() {
        fn shape_bounds(direction: &str) -> ([f32; 3], [f32; 3]) {
            let state = BlockStateQuery::new("minecraft:birch_fence_gate")
                .with_state("minecraft:cardinal_direction", direction)
                .with_state("open_bit", true);
            let shape = model_shape(&state)
                .unwrap_or_else(|| panic!("missing fence gate shape {direction}"));
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for cuboid in &shape.cuboids {
                for axis in 0..3 {
                    min[axis] = min[axis].min(cuboid.min[axis]);
                    max[axis] = max[axis].max(cuboid.max[axis]);
                }
            }
            (min, max)
        }

        // A north-authored open gate keeps its posts in the z=7..9 band and swings both leaves
        // towards +z. Bedrock turns that model with [0, 90, 0] for west, [0, 180, 0] for south
        // and [0, -90, 0] for east, so each facing moves the leaves onto its own side.
        let north = shape_bounds("north");
        assert!(
            (north.0[2] - 7.0 / 16.0).abs() < 0.001 && (north.1[2] - 15.0 / 16.0).abs() < 0.001
        );
        let south = shape_bounds("south");
        assert!((south.0[2] - 1.0 / 16.0).abs() < 0.001 && (south.1[2] - 9.0 / 16.0).abs() < 0.001);
        let west = shape_bounds("west");
        assert!((west.0[0] - 7.0 / 16.0).abs() < 0.001 && (west.1[0] - 15.0 / 16.0).abs() < 0.001);
        let east = shape_bounds("east");
        assert!((east.0[0] - 1.0 / 16.0).abs() < 0.001 && (east.1[0] - 9.0 / 16.0).abs() < 0.001);
    }

    #[test]
    fn wall_torches_mount_on_the_named_side() {
        fn bounds(facing: &str) -> ([f32; 3], [f32; 3]) {
            let state = BlockStateQuery::new("minecraft:torch")
                .with_state("torch_facing_direction", facing);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing torch shape"));
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for cuboid in &shape.cuboids {
                for axis in 0..3 {
                    min[axis] = min[axis].min(cuboid.min[axis]);
                    max[axis] = max[axis].max(cuboid.max[axis]);
                }
            }
            for plane in &shape.planes {
                for corner in plane.corners {
                    for axis in 0..3 {
                        min[axis] = min[axis].min(corner[axis]);
                        max[axis] = max[axis].max(corner[axis]);
                    }
                }
            }
            (min, max)
        }

        // Values are the post the Bedrock client draws for each mount.
        let north = bounds("north");
        assert!((north.0[2] + 0.0631).abs() < 0.001 && (north.1[2] - 0.31).abs() < 0.001);
        assert!((north.0[1] - 0.2).abs() < 0.001 && (north.1[1] - 0.81875).abs() < 0.001);
        let east = bounds("east");
        assert!((east.0[0] - 0.69).abs() < 0.001 && (east.1[0] - 1.0631).abs() < 0.001);
        let south = bounds("south");
        assert!((south.0[2] - 0.69).abs() < 0.001 && (south.1[2] - 1.0631).abs() < 0.001);
        let west = bounds("west");
        assert!((west.0[0] + 0.0631).abs() < 0.001 && (west.1[0] - 0.31).abs() < 0.001);

        // A floor torch stands in the middle of its cell.
        let top = bounds("top");
        assert!((top.0[0] - 0.4375).abs() < 0.001 && (top.1[0] - 0.5625).abs() < 0.001);
        assert!(top.0[1].abs() < 0.001 && (top.1[1] - 0.61875).abs() < 0.001);
    }

    #[test]
    fn repeater_output_torch_follows_the_opposite_cardinal() {
        fn output_post(direction: &str) -> ([f32; 2], [f32; 2]) {
            let state = BlockStateQuery::new("minecraft:unpowered_repeater")
                .with_state("minecraft:cardinal_direction", direction)
                .with_state("repeater_delay", 0);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing repeater shape"));
            // The output post hugs the output edge (official repeater model:
            // authored at z 2..4 px, 2 px wide, standing 2..7 px on the slab);
            // the delay post stays nearer the centre.
            let torch = shape
                .cuboids
                .iter()
                .filter(|cuboid| (cuboid.max[1] - 0.4375).abs() < 0.001)
                .min_by(|a, b| {
                    let distance = |c: &crate::model_family::ModelCuboid| {
                        let centre_x = (c.min[0] + c.max[0]) / 2.0 - 0.5;
                        let centre_z = (c.min[2] + c.max[2]) / 2.0 - 0.5;
                        centre_x.abs() + centre_z.abs()
                    };
                    distance(b).total_cmp(&distance(a))
                })
                .unwrap_or_else(|| panic!("missing repeater torches"));
            ([torch.min[0], torch.max[0]], [torch.min[2], torch.max[2]])
        }

        // `minecraft:cardinal_direction` points from the output towards the input (minecraft
        // wiki), so the output post lands on the opposite edge. The official post is a 2 x 5 x 2
        // column 2 px inside the output edge (JE repeater model z 2..4).
        let south = output_post("south");
        assert!((south.1[0] - 0.125).abs() < 0.001 && (south.1[1] - 0.25).abs() < 0.001);
        let north = output_post("north");
        assert!((north.1[0] - 0.75).abs() < 0.001 && (north.1[1] - 0.875).abs() < 0.001);
        let east = output_post("east");
        assert!((east.0[0] - 0.125).abs() < 0.001 && (east.0[1] - 0.25).abs() < 0.001);
        let west = output_post("west");
        assert!((west.0[0] - 0.75).abs() < 0.001 && (west.0[1] - 0.875).abs() < 0.001);
    }

    #[test]
    fn repeater_delay_post_slides_two_px_per_delay_step() {
        fn delay_post_z(tick: i64) -> ([f32; 2], [f32; 2]) {
            let state = BlockStateQuery::new("minecraft:unpowered_repeater")
                .with_state("minecraft:cardinal_direction", "south")
                .with_state("repeater_delay", tick);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing repeater shape"));
            // The delay post is the one nearer the cell centre (output south ->
            // rotation north leaves the authored frame: posts at z 2..4 and
            // z 6..8 for delay 0).
            let torch = shape
                .cuboids
                .iter()
                .filter(|cuboid| (cuboid.max[1] - 0.4375).abs() < 0.001)
                .max_by(|a, b| {
                    let centre_a = (a.min[2] + a.max[2]) / 2.0;
                    let centre_b = (b.min[2] + b.max[2]) / 2.0;
                    centre_a.total_cmp(&centre_b)
                })
                .unwrap_or_else(|| panic!("missing repeater torches"));
            ([torch.min[2], torch.max[2]], [torch.min[0], torch.max[0]])
        }

        // Official models: 1tick z 6..8, 2tick z 8..10, 3tick z 10..12, 4tick z 12..14.
        assert_eq!(delay_post_z(0).0, [0.375, 0.5]);
        assert_eq!(delay_post_z(1).0, [0.5, 0.625]);
        assert_eq!(delay_post_z(2).0, [0.625, 0.75]);
        assert_eq!(delay_post_z(3).0, [0.75, 0.875]);
    }

    #[test]
    fn comparator_front_post_lights_only_in_subtract_mode() {
        fn posts(mode: Option<&str>, lit_bit: bool) -> (usize, Vec<String>) {
            let mut state =
                BlockStateQuery::new("minecraft:unpowered_comparator").with_state("output_subtract_bit", lit_bit);
            if let Some(mode) = mode {
                state = state.with_state("comparator_mode", mode);
            }
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing comparator shape"));
            let lit = shape
                .cuboids
                .iter()
                .filter(|cuboid| {
                    cuboid
                        .material_slot
                        .as_deref()
                        .is_some_and(|slot| slot == "torch_on")
                })
                .count();
            let slots: Vec<String> = shape
                .cuboids
                .iter()
                .filter_map(|cuboid| cuboid.material_slot.clone())
                .collect();
            (lit, slots)
        }

        // Official comparator models: the short front post lights in subtract
        // mode; the two 5 px input posts light with the powered output.
        assert_eq!(posts(Some("compare"), false).0, 0);
        assert_eq!(posts(Some("subtract"), true).0, 1);
    }

    #[test]
    fn vertical_pistons_push_along_their_axis() {
        let up = BlockStateQuery::new("minecraft:piston").with_state("facing_direction", 1);
        let up_shape = model_shape(&up).unwrap_or_else(|| panic!("missing piston shape"));
        assert_eq!(
            up_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::Up)
                .map(String::as_str),
            Some("top_normal")
        );

        let down = BlockStateQuery::new("minecraft:piston").with_state("facing_direction", 0);
        let down_shape = model_shape(&down).unwrap_or_else(|| panic!("missing piston shape"));
        assert_eq!(
            down_shape.cuboids[0]
                .face_material_slots
                .get(&BlockFace::Down)
                .map(String::as_str),
            Some("top_normal")
        );

        // The head block is the official plate + shaft assembly (two cuboids) spanning its
        // whole cell; the shaft's last 4 px reach into the base block.
        let arm = BlockStateQuery::new("minecraft:piston_arm_collision")
            .with_state("facing_direction", 1);
        let arm_shape = model_shape(&arm).unwrap_or_else(|| panic!("missing piston arm shape"));
        assert_eq!(arm_shape.cuboids.len(), 2, "plate and shaft");
        let top = arm_shape
            .cuboids
            .iter()
            .max_by(|a, b| a.max[1].total_cmp(&b.max[1]))
            .expect("plate");
        assert!((top.max[1] - 1.0).abs() < 0.001, "the plate caps the cell top");
        assert!((top.max[1] - top.min[1] - 0.25).abs() < 0.001, "plate is 4 px");
        let bottom = arm_shape
            .cuboids
            .iter()
            .min_by(|a, b| a.min[1].total_cmp(&b.min[1]))
            .expect("shaft");
        assert!(
            bottom.min[1] < -0.2,
            "the shaft reaches 4 px into the base block"
        );
    }

    #[test]
    fn campfire_lit_log_faces_turn_with_the_block() {
        fn lit_faces(direction: &str) -> Vec<BlockFace> {
            let state = BlockStateQuery::new("minecraft:campfire")
                .with_state("minecraft:cardinal_direction", direction);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing campfire shape"));
            shape
                .cuboids
                .iter()
                .flat_map(|cuboid| cuboid.face_material_slots.iter())
                .filter(|(_, slot)| slot.as_str() == "lit_log")
                .map(|(face, _)| *face)
                .collect()
        }

        // The north-authored table lights one log end towards the east; the block turn decides
        // which world face shows it.
        assert!(lit_faces("north").contains(&BlockFace::East));
        assert!(lit_faces("east").contains(&BlockFace::South));
        assert!(lit_faces("south").contains(&BlockFace::West));
        assert!(lit_faces("west").contains(&BlockFace::North));
    }

    #[test]
    fn cardinal_blocks_turn_all_four_ways() {
        use std::collections::BTreeSet;

        fn signature(shape: &crate::model_family::ModelShape) -> String {
            let mut parts = Vec::new();
            for cuboid in &shape.cuboids {
                let mut slots: Vec<String> = cuboid
                    .face_material_slots
                    .iter()
                    .map(|(face, slot)| format!("{face:?}={slot}"))
                    .collect();
                slots.sort();
                let mut uvs: Vec<String> = cuboid
                    .face_uvs
                    .iter()
                    .map(|(face, uv)| format!("{face:?}{uv:?}"))
                    .collect();
                uvs.sort();
                parts.push(format!("{:?}{:?}{slots:?}{uvs:?}", cuboid.min, cuboid.max));
            }
            for plane in &shape.planes {
                parts.push(format!(
                    "{:?}{:?}{:?}{:?}",
                    plane.normal, plane.corners, plane.material_slot, plane.uv
                ));
            }
            parts.join("|")
        }

        // The anvil's boxes are symmetric under 180 degrees, so its facing only shows through
        // the in-face UV rotation.
        for name in [
            "minecraft:stonecutter_block",
            "minecraft:lectern",
            "minecraft:anvil",
        ] {
            let mut seen = BTreeSet::new();
            for direction in ["north", "east", "south", "west"] {
                let state = BlockStateQuery::new(name)
                    .with_state("minecraft:cardinal_direction", direction);
                let shape = model_shape(&state).unwrap_or_else(|| panic!("missing {name}"));
                assert!(
                    seen.insert(signature(&shape)),
                    "{name} renders {direction} like another facing"
                );
            }
        }

        let mut observer = BTreeSet::new();
        for facing in ["north", "east", "south", "west", "up", "down"] {
            let state = BlockStateQuery::new("minecraft:observer")
                .with_state("minecraft:facing_direction", facing);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing observer"));
            // The front art must follow the state: the model's north face is `observer_front`.
            let front = shape.cuboids[0]
                .face_material_slots
                .iter()
                .find(|(_, slot)| slot.as_str() == "north")
                .map(|(face, _)| *face)
                .unwrap_or_else(|| panic!("observer lost its front face"));
            let expected = match facing {
                "north" => BlockFace::North,
                "east" => BlockFace::East,
                "south" => BlockFace::South,
                "west" => BlockFace::West,
                "up" => BlockFace::Up,
                _ => BlockFace::Down,
            };
            assert_eq!(front, expected, "observer facing {facing}");
            assert!(observer.insert(signature(&shape)));
        }
    }

    #[test]
    fn device_faces_take_their_art_by_role_not_by_rotation() {
        use crate::material::BlockFace;

        fn slots(name: &str, facing: &str) -> Vec<(BlockFace, String)> {
            let state = BlockStateQuery::new(name).with_state("minecraft:facing_direction", facing);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing {name}"));
            shape.cuboids[0]
                .face_material_slots
                .iter()
                .map(|(face, slot)| (*face, slot.clone()))
                .collect()
        }
        let slot_of = |name: &str, facing: &str, face: BlockFace| {
            slots(name, facing)
                .into_iter()
                .find(|(candidate, _)| *candidate == face)
                .map(|(_, slot)| slot)
                .unwrap_or_else(|| panic!("{name}/{facing} lost {face:?}"))
        };

        // A horizontal dispenser shows `dispenser_front_horizontal` (the `south` slot) on the
        // face it points at, and `dispenser_side` (the `north` slot) everywhere else.
        assert_eq!(slot_of("minecraft:dispenser", "north", BlockFace::North), "south");
        assert_eq!(slot_of("minecraft:dispenser", "north", BlockFace::South), "north");
        assert_eq!(slot_of("minecraft:dispenser", "east", BlockFace::East), "south");
        // Pointing up/down uses the vertical front art, which the pack stores in the `east` slot.
        assert_eq!(slot_of("minecraft:dispenser", "up", BlockFace::Up), "east");
        assert_eq!(slot_of("minecraft:dispenser", "down", BlockFace::Down), "east");
        // The lids stay `dispenser_top`.
        assert_eq!(slot_of("minecraft:dispenser", "north", BlockFace::Up), "up");
        assert_eq!(slot_of("minecraft:dispenser", "north", BlockFace::Down), "up");
        // Droppers behave the same way with their own art.
        assert_eq!(slot_of("minecraft:dropper", "north", BlockFace::North), "south");
        assert_eq!(slot_of("minecraft:dropper", "up", BlockFace::Up), "east");

        // The observer's front is the `north` slot, its back the `south` slot.
        assert_eq!(slot_of("minecraft:observer", "north", BlockFace::North), "north");
        assert_eq!(slot_of("minecraft:observer", "north", BlockFace::South), "south");
        assert_eq!(slot_of("minecraft:observer", "east", BlockFace::East), "north");
        assert_eq!(slot_of("minecraft:observer", "east", BlockFace::West), "south");
        assert_eq!(slot_of("minecraft:observer", "up", BlockFace::Up), "north");
        assert_eq!(slot_of("minecraft:observer", "up", BlockFace::Down), "south");
        // The observer keeps distinct top and bottom art.
        assert_eq!(slot_of("minecraft:observer", "north", BlockFace::Up), "up");
        assert_eq!(slot_of("minecraft:observer", "north", BlockFace::Down), "down");
    }

    #[test]
    fn buttons_mount_on_walls_ceilings_and_floors() {
        fn plate(facing: i64) -> ([f32; 3], [f32; 3]) {
            let state = BlockStateQuery::new("minecraft:acacia_button")
                .with_state("facing_direction", facing)
                .with_state("button_pressed_bit", false);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing button shape"));
            (shape.cuboids[0].min, shape.cuboids[0].max)
        }

        // Vanilla button: 6 x 4 px footprint, 2 px deep, so a floor button is 6 x 2 x 4.
        // facing_direction orders 0=down, 1=up, 2..5 horizontal.
        let floor = plate(1);
        assert!((floor.0[0] - 0.3125).abs() < 0.001 && (floor.1[0] - 0.6875).abs() < 0.001);
        assert!(floor.0[1].abs() < 0.001 && (floor.1[1] - 0.125).abs() < 0.001);
        assert!((floor.0[2] - 0.375).abs() < 0.001 && (floor.1[2] - 0.625).abs() < 0.001);
        let ceiling = plate(0);
        assert!((ceiling.0[1] - 0.875).abs() < 0.001 && (ceiling.1[1] - 1.0).abs() < 0.001);
        // A wall button stands 4 px tall and 2 px deep.
        let north = plate(2);
        assert!(north.0[2].abs() < 0.001 && (north.1[2] - 0.125).abs() < 0.001);
        assert!((north.0[1] - 0.375).abs() < 0.001 && (north.1[1] - 0.625).abs() < 0.001);
    }

    #[test]
    fn spikes_taper_by_thickness_and_hanging() {
        fn spike(name: &str, thickness: &str, hanging: bool) -> crate::model_family::ModelShape {
            let state = BlockStateQuery::new(name)
                .with_state("dripstone_thickness", thickness)
                .with_state("hanging", hanging);
            model_shape(&state).unwrap_or_else(|| panic!("missing {name}"))
        }

        // Verified against the real client: the capture stores 16 vertices per segment, all on
        // the y = 0 / y = 1 layers, spanning ~0.9 on both axes — two rotated sheets, not a box.
        for (name, thickness) in [
            ("minecraft:sulfur_spike", "tip"),
            ("minecraft:sulfur_spike", "base"),
            ("minecraft:pointed_dripstone", "base"),
            ("minecraft:pointed_dripstone", "frustum"),
        ] {
            let shape = spike(name, thickness, false);
            assert!(shape.cuboids.is_empty(), "{name}/{thickness} must not be a box");
            assert_eq!(shape.planes.len(), 2, "{name}/{thickness} is a cross");
            for plane in &shape.planes {
                assert!(plane.corners.iter().any(|corner| corner[1] == 0.0));
                assert!(plane.corners.iter().any(|corner| corner[1] == 1.0));
                for corner in plane.corners {
                    assert!(
                        (0.17..=0.83).contains(&corner[0]) && (0.17..=0.83).contains(&corner[2]),
                        "{name}/{thickness} corner {corner:?} leaves the 45-degree span"
                    );
                }
            }
        }

        // The segment picks its art; `tip_merge` maps to `merge`, and the `[down, up]` frame has
        // to be explicit or a standing spike samples the hanging art.
        let slot = |thickness: &str, hanging: bool| {
            spike("minecraft:sulfur_spike", thickness, hanging).planes[0]
                .material_slot
                .clone()
        };
        assert_eq!(slot("tip", false), Some("tip_up".to_owned()));
        assert_eq!(slot("base", false), Some("base_up".to_owned()));
        assert_eq!(slot("tip_merge", false), Some("merge_up".to_owned()));
        assert_eq!(slot("merge", false), Some("merge_up".to_owned()));
        assert_eq!(slot("tip", true), Some("tip_down".to_owned()));

        // A hanging spike keeps the same planes but flips the art so the point faces down.
        let standing = spike("minecraft:sulfur_spike", "tip", false);
        let hanging = spike("minecraft:sulfur_spike", "tip", true);
        let standing_uv = standing.planes[0].uv.expect("standing uv");
        let hanging_uv = hanging.planes[0].uv.expect("hanging uv");
        assert_ne!(standing_uv, hanging_uv);
        // The `_up_tip` art is narrow at the top and only paints rows 4..15, so a standing tip
        // samples that band instead of the whole tile; a hanging tip uses the top 12 rows.
        let v_range = |uv: [[f32; 2]; 4]| {
            let lo = uv.iter().map(|c| c[1]).fold(f32::INFINITY, f32::min);
            let hi = uv.iter().map(|c| c[1]).fold(f32::NEG_INFINITY, f32::max);
            (lo, hi)
        };
        let (stand_lo, stand_hi) = v_range(standing_uv);
        let (hang_lo, hang_hi) = v_range(hanging_uv);
        assert!(
            (stand_hi - stand_lo - 0.75).abs() < 1e-3,
            "a standing tip samples 12 of 16 rows: {stand_lo}..{stand_hi}"
        );
        assert!(
            (hang_hi - hang_lo - 0.75).abs() < 1e-3,
            "a hanging tip samples 12 of 16 rows: {hang_lo}..{hang_hi}"
        );
        // The standing band sits low in the tile, the hanging band high.
        assert!(stand_lo > hang_lo);
    }

    #[test]
    fn campfire_uvs_stay_inside_one_flipbook_frame() {
        let lit = BlockStateQuery::new("minecraft:campfire")
            .with_state("minecraft:cardinal_direction", "south");
        let shape = model_shape(&lit).unwrap_or_else(|| panic!("missing campfire"));

        // `campfire_log_lit` is a 16 x 64 flipbook, so every face has to stay inside one
        // 16 x 16 frame; a per-log tile offset would leave that range.
        for cuboid in &shape.cuboids {
            for (face, uv) in &cuboid.face_uvs {
                for corner in uv {
                    assert!(
                        (0.0..=1.0).contains(&corner[0]) && (0.0..=1.0).contains(&corner[1]),
                        "{face:?} leaves the texture frame: {corner:?}"
                    );
                }
            }
        }
        assert!(
            shape.cuboids.iter().any(|cuboid| cuboid
                .face_material_slots
                .values()
                .any(|slot| slot == "lit_log")),
            "a lit campfire draws at least one lit log face"
        );

        // The flame sheets sample the whole first frame of the 16 x 128 fire flipbook.
        for plane in &shape.planes {
            let uv = plane.uv.expect("flame uv");
            let v_min = uv
                .iter()
                .map(|corner| corner[1])
                .fold(f32::INFINITY, f32::min);
            let v_max = uv.iter().map(|corner| corner[1]).fold(0.0_f32, f32::max);
            assert!(
                (v_max - v_min - 1.0).abs() < 0.001,
                "the flame sheet samples one full frame"
            );
        }
        assert_eq!(shape.planes.len(), 2, "two crossed flame sheets");

        let unlit = BlockStateQuery::new("minecraft:campfire")
            .with_state("minecraft:cardinal_direction", "south")
            .with_state("extinguished", true);
        let unlit_shape = model_shape(&unlit).unwrap_or_else(|| panic!("missing campfire"));
        assert!(
            unlit_shape.planes.is_empty(),
            "an unlit campfire has no fire"
        );
        assert!(
            unlit_shape.cuboids.iter().all(|cuboid| cuboid
                .face_material_slots
                .values()
                .all(|slot| slot == "log")),
            "an unlit campfire has no lit log faces"
        );
    }

    #[test]
    fn campfire_log_faces_avoid_the_transparent_filler() {
        // `campfire_log` paints three windows only: the bark strip (rows 0..4), the cut end at
        // (0, 4)..(4, 8) and the charred bed (rows 8..16). The rest of rows 4..8 is transparent
        // black filler; sampling it drew a black band along the bottom of every log.
        for extinguished in [false, true] {
            let state = BlockStateQuery::new("minecraft:campfire")
                .with_state("minecraft:cardinal_direction", "south")
                .with_state("extinguished", extinguished);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing campfire"));
            for cuboid in &shape.cuboids {
                for (face, slot) in &cuboid.face_material_slots {
                    if slot != "log" {
                        continue;
                    }
                    let uv = cuboid
                        .face_uvs
                        .get(face)
                        .copied()
                        .unwrap_or_else(|| panic!("missing uv for {face:?}"));
                    for corner in uv {
                        let filler = corner[0] > 0.25 + 1e-3
                            && corner[1] > 0.25 + 1e-3
                            && corner[1] < 0.5 - 1e-3;
                        assert!(!filler, "{face:?} samples the filler at {corner:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn hanging_plants_flip_their_art() {
        // A mangrove propagule is two crossed cards, each stepped from a narrow stalk to a wide
        // leaf (measured from the client). The hanging form is a different layout — a stem with a
        // collar — not the standing plant mirrored, so each is checked against its own capture.
        fn bounds(name: &str, hanging: bool, stage: i64) -> ([f32; 3], [f32; 3]) {
            let state = BlockStateQuery::new(name)
                .with_state("hanging", hanging)
                .with_state("propagule_stage", stage);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing {name}"));
            assert!(
                !shape.cuboids.is_empty() || !shape.planes.is_empty(),
                "{name} should have geometry"
            );
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for cuboid in &shape.cuboids {
                for axis in 0..3 {
                    min[axis] = min[axis].min(cuboid.min[axis]);
                    max[axis] = max[axis].max(cuboid.max[axis]);
                }
            }
            for plane in &shape.planes {
                for corner in plane.corners {
                    for axis in 0..3 {
                        min[axis] = min[axis].min(corner[axis]);
                        max[axis] = max[axis].max(corner[axis]);
                    }
                }
            }
            (min, max)
        }
        let (stand_min, stand_max) = bounds("minecraft:mangrove_propagule", false, 0);
        // Standing stage 0 matches capture ref 767: `(0.1511,0,0.1178)..(0.5489,0.9375,0.5155)`.
        assert!((stand_min[0] - 0.1511).abs() < 1e-3, "x starts at 0.1511");
        assert!((stand_max[0] - 0.5489).abs() < 1e-3, "x ends at 0.5489");
        assert!((stand_min[2] - 0.1178).abs() < 1e-3, "z starts at 0.1178");
        assert!(stand_min[1].abs() < 1e-3, "a standing propagule starts at y 0");
        assert!(
            (stand_max[1] - 0.9375).abs() < 1e-3,
            "the standing leaves stop at 0.9375"
        );
        // Stages slide the plant across the cell without changing its height (refs 767-771).
        for stage in 1..=4 {
            let (_, stage_max) = bounds("minecraft:mangrove_propagule", false, stage);
            assert!(
                (stage_max[1] - 0.9375).abs() < 1e-3,
                "stage {stage} keeps the standing height"
            );
        }
        // The hanging seedling (official JE hanging models): leaf pair at the ceiling, cap,
        // body at y 0.625..0.8125, four tilted collar plates fanning to x/z 0.13..0.87, and
        // the stage-4 hypocotyl reaching the floor.
        let (hang_min, hang_max) = bounds("minecraft:mangrove_propagule", true, 4);
        assert!(hang_min[1].abs() < 1e-3, "the hanging stem reaches the floor");
        assert!((hang_max[1] - 1.0).abs() < 1e-3, "and its cap reaches the ceiling");
        assert!(
            (hang_min[0] - 2.0908 / 16.0).abs() < 1e-3,
            "the collar spreads to 2.0908 px in x"
        );
        assert!((hang_max[2] - 13.9092 / 16.0).abs() < 1e-3);
        // The hypocotyl grows with the stage: stage 1 has none, stages 2..4 start at
        // model y 7 / 3 / 0.
        for (stage, floor) in [(2, 7.0 / 16.0), (3, 3.0 / 16.0), (4, 0.0)] {
            let (stage_min, _) = bounds("minecraft:mangrove_propagule", true, stage);
            assert!(
                (stage_min[1] - floor).abs() < 1e-3,
                "stage {stage} stem starts at {floor}"
            );
        }
        let (stage1_min, _) = bounds("minecraft:mangrove_propagule", true, 1);
        assert!(
            stage1_min[1] > 0.5,
            "stage 1 ships no hypocotyl below the body"
        );
    }

    #[test]
    fn mangrove_propagule_cards_sample_the_painted_sprite_regions() {
        // The 16x16 `mangrove_propagule` tile paints the leaf at `u 4..11, v 1..7` and the
        // hypocotyl (stem) at `u 7..9, v 7..16`; everything else is transparent. The JE model
        // uses exactly those two rects, and sampling the whole tile (the previous
        // `upright_full_texture_uv`) landed mostly on empty pixels.
        let shape = model_shape(
            &BlockStateQuery::new("minecraft:mangrove_propagule")
                .with_state("hanging", false)
                .with_state("propagule_stage", 0),
        )
        .unwrap_or_else(|| panic!("missing propagule"));
        assert_eq!(shape.planes.len(), 4, "two cards, two sections each");
        let mut seen_leaf = false;
        let mut seen_stem = false;
        for plane in &shape.planes {
            let uv = plane.uv.expect("propagule plane without uv");
            let u0 = uv.iter().map(|c| c[0] * 16.0).fold(f32::INFINITY, f32::min);
            let u1 = uv.iter().map(|c| c[0] * 16.0).fold(f32::NEG_INFINITY, f32::max);
            let v0 = uv.iter().map(|c| c[1] * 16.0).fold(f32::INFINITY, f32::min);
            let v1 = uv.iter().map(|c| c[1] * 16.0).fold(f32::NEG_INFINITY, f32::max);
            if (u0 - 4.0).abs() < 1e-3 && (u1 - 11.0).abs() < 1e-3 {
                assert!((v0 - 1.0).abs() < 1e-3 && (v1 - 7.0).abs() < 1e-3, "leaf rect {v0}..{v1}");
                seen_leaf = true;
            } else if (u0 - 7.0).abs() < 1e-3 && (u1 - 9.0).abs() < 1e-3 {
                assert!((v0 - 7.0).abs() < 1e-3 && (v1 - 16.0).abs() < 1e-3, "stem rect {v0}..{v1}");
                seen_stem = true;
            } else {
                panic!("propagule plane samples u {u0}..{u1}, outside the painted regions");
            }
        }
        assert!(seen_leaf && seen_stem, "both the leaf and stem rects must be used");
    }

    #[test]
    fn bed_side_faces_sample_the_length_band() {
        // Bedrock's box-UV unwrap of the 6 px mattress cube puts its +X/-X (world upright)
        // sides on `u 0..6 / 22..28` (the 6 px thickness) with V running along the bed's length.
        // Each half therefore spans 16 px of V (the full bed is 32 px). An earlier remap forced
        // V onto a 6 px band, collapsing the side quads to a line and sampling the leg art.
        let shape = model_shape(
            &BlockStateQuery::new("minecraft:bed")
                .with_state("direction", 0)
                .with_state("head_piece_bit", true),
        )
        .unwrap_or_else(|| panic!("missing bed"));
        let mut checked = 0;
        for plane in &shape.planes {
            if !matches!(plane.normal, [1, 0, 0] | [-1, 0, 0]) {
                continue;
            }
            // Only the mattress body is 6 px deep; the legs are separate 3 px cubes with their
            // own band, so skip anything that is not 0.375 tall.
            let low = plane.corners.iter().map(|c| c[1]).fold(f32::INFINITY, f32::min);
            let high = plane.corners.iter().map(|c| c[1]).fold(f32::NEG_INFINITY, f32::max);
            if (high - low - 0.375).abs() > 1e-3 {
                continue;
            }
            let uv = plane.uv.expect("bed side plane without uv");
            let v0 = uv.iter().map(|c| c[1] * 64.0).fold(f32::INFINITY, f32::min);
            let v1 = uv.iter().map(|c| c[1] * 64.0).fold(f32::NEG_INFINITY, f32::max);
            assert!(
                (v1 - v0 - 16.0).abs() < 1e-3,
                "mattress side spans v {v0}..{v1}, expected a 16 px length band"
            );
            checked += 1;
        }
        assert!(checked > 0, "the bed must emit upright mattress side faces");
    }

    #[test]
    fn bed_colour_travels_in_the_material_slot() {
        // Bedrock names every bed block plain `bed` and keeps the dye in the block entity, so the
        // colour has to reach the shape as a synthetic state and end up in the slot name.
        let white = model_shape(&BlockStateQuery::new("minecraft:bed"));
        let red = model_shape(
            &BlockStateQuery::new("minecraft:bed").with_state("bed_colour", "red"),
        );
        let slot_of = |shape: Option<ModelShape>| {
            shape
                .and_then(|shape| shape.planes.first().and_then(|p| p.material_slot.clone()))
        };
        assert_eq!(slot_of(white).as_deref(), Some("bed"));
        assert_eq!(slot_of(red).as_deref(), Some("bed_red"));
        // A legacy name that already carries the colour still resolves.
        let legacy = model_shape(&BlockStateQuery::new("minecraft:red_bed"));
        assert_eq!(slot_of(legacy).as_deref(), Some("bed_red"));
    }

    fn item_frame_plate_hugs_the_support_side() {
        fn frame_bounds(facing: i64) -> ([f32; 3], [f32; 3]) {
            let state =
                BlockStateQuery::new("minecraft:frame").with_state("facing_direction", facing);
            let shape = model_shape(&state).unwrap_or_else(|| panic!("missing item frame shape"));
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for cuboid in &shape.cuboids {
                for axis in 0..3 {
                    min[axis] = min[axis].min(cuboid.min[axis]);
                    max[axis] = max[axis].max(cuboid.max[axis]);
                }
            }
            (min, max)
        }

        // A north-facing frame is mounted on a wall to its south, and so on.
        assert!((frame_bounds(2).1[2] - 1.0).abs() < 0.001);
        assert!(frame_bounds(3).0[2].abs() < 0.001);
        assert!((frame_bounds(4).1[0] - 1.0).abs() < 0.001);
        assert!(frame_bounds(5).0[0].abs() < 0.001);
    }

    #[test]
    fn utility_partial_block_shapes_should_emit_expected_meshes() {
        let open_gate = BlockStateQuery::new("minecraft:oak_fence_gate")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("open_bit", true);
        let gate_shape =
            model_shape(&open_gate).unwrap_or_else(|| panic!("missing fence gate shape"));
        assert_eq!(gate_shape.cuboids.len(), 8);
        // A north-facing open gate swings its leaves towards +z, ending at z = 15/16.
        assert!(
            gate_shape
                .cuboids
                .iter()
                .any(|cuboid| (cuboid.max[2] - 0.9375).abs() < 0.001)
        );

        let wall_gate = BlockStateQuery::new("minecraft:bamboo_fence_gate")
            .with_state("minecraft:cardinal_direction", "north")
            .with_state("in_wall_bit", true);
        let wall_gate_shape =
            model_shape(&wall_gate).unwrap_or_else(|| panic!("missing bamboo fence gate shape"));
        assert_eq!(wall_gate_shape.cuboids.len(), 8);
        assert!(
            wall_gate_shape
                .cuboids
                .iter()
                .any(|cuboid| (cuboid.min[1] - 2.0 / 16.0).abs() < 0.001)
        );
        assert_eq!(
            detail_material_block_name(&wall_gate).as_deref(),
            Some("minecraft:bamboo_fence_gate")
        );

        let chain = BlockStateQuery::new("minecraft:chain").with_state("pillar_axis", "x");
        let chain_shape = model_shape(&chain).unwrap_or_else(|| panic!("missing chain shape"));
        // Vanilla chains are two 45-degree sheets spanning the whole axis, not a box.
        assert_eq!(chain_shape.planes.len(), 2);
        assert!(chain_shape.cuboids.is_empty());
        assert!(chain_shape.planes.iter().all(|plane| {
            plane
                .corners
                .iter()
                .all(|corner| corner[0] == 0.0 || corner[0] == 1.0)
                && plane.uv.is_some()
        }));

        let button = BlockStateQuery::new("minecraft:stone_button")
            .with_state("minecraft:cardinal_direction", "west");
        let button_shape = model_shape(&button).unwrap_or_else(|| panic!("missing button shape"));
        assert!(
            button_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[0] <= 0.125 && cuboid.min[1] >= 0.3125)
        );

        let pressure_plate = BlockStateQuery::new("minecraft:stone_pressure_plate");
        let plate_shape =
            model_shape(&pressure_plate).unwrap_or_else(|| panic!("missing pressure plate shape"));
        assert!(
            plate_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 0.0625)
        );

        let snow = BlockStateQuery::new("minecraft:snow_layer");
        let snow_shape = model_shape(&snow).unwrap_or_else(|| panic!("missing snow shape"));
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
        let torch_shape = model_shape(&wall_torch).unwrap_or_else(|| panic!("missing torch shape"));
        // The vanilla wall torch is one tilted 2 x 2 x 9.9 px post, so the shape is six sheets
        // and leans onto its support (south here).
        assert_eq!(torch_shape.planes.len(), 6);
        let max_z = torch_shape
            .planes
            .iter()
            .flat_map(|plane| plane.corners)
            .map(|corner| corner[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let min_z = torch_shape
            .planes
            .iter()
            .flat_map(|plane| plane.corners)
            .map(|corner| corner[2])
            .fold(f32::INFINITY, f32::min);
        assert!(
            (max_z - 1.0631).abs() < 0.001 && (min_z - 0.69).abs() < 0.001,
            "the torch post must lean onto the south support face"
        );

        let lantern = BlockStateQuery::new("minecraft:lantern").with_state("hanging", true);
        let lantern_shape =
            model_shape(&lantern).unwrap_or_else(|| panic!("missing lantern shape"));
        assert_eq!(lantern_shape.cuboids.len(), 2);
        assert!(
            lantern_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] > 0.5 && cuboid.max[1] < 0.7)
        );
        assert_eq!(lantern_shape.planes.len(), 2);
        assert!(
            lantern_shape.cuboids.iter().all(|cuboid| {
                cuboid
                    .face_uvs
                    .contains_key(&crate::material::BlockFace::Up)
                    && cuboid
                        .face_uvs
                        .contains_key(&crate::material::BlockFace::North)
            }),
            "lantern cuboids must sample their own texture rectangles"
        );

        let candles = BlockStateQuery::new("minecraft:candle").with_state("candles", 4);
        let candle_shape = model_shape(&candles).unwrap_or_else(|| panic!("missing candle shape"));
        assert_eq!(candle_shape.cuboids.len(), 4);

        let portal = BlockStateQuery::new("minecraft:portal").with_state("portal_axis", "x");
        let portal_shape = model_shape(&portal).unwrap_or_else(|| panic!("missing portal shape"));
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
        let chest_shape = model_shape(&chest).unwrap_or_else(|| panic!("missing chest shape"));
        // Bedrock chest: body + overlapping lid, latch baked into the 16x16 front tile.
        assert_eq!(chest_shape.cuboids.len(), 2);
        assert!(
            chest_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 0.875)
        );
        assert!(
            chest_shape
                .cuboids
                .iter()
                .all(|cuboid| cuboid.face_uvs.len() == 6),
            "chest faces must sample their whole 16x16 tile"
        );

        let shulker = BlockStateQuery::new("minecraft:blue_shulker_box");
        let shulker_shape =
            model_shape(&shulker).unwrap_or_else(|| panic!("missing shulker box shape"));
        assert_eq!(shulker_shape.cuboids.len(), 2);
        assert!(
            shulker_shape
                .cuboids
                .iter()
                .any(|cuboid| cuboid.max[1] == 0.5)
        );

        let anvil = BlockStateQuery::new("minecraft:anvil")
            .with_state("minecraft:cardinal_direction", "east");
        let anvil_shape = model_shape(&anvil).unwrap_or_else(|| panic!("missing anvil shape"));
        // The anvil now uses the client's decoded mesh (4 boxes = 24 planes per facing).
        assert!(anvil_shape.cuboids.is_empty());
        assert_eq!(anvil_shape.planes.len(), 24);
        assert!(
            anvil_shape
                .planes
                .iter()
                .any(|p| p.corners.iter().any(|c| (c[1] - 1.0).abs() < 0.001))
        );

        let stonecutter = BlockStateQuery::new("minecraft:stonecutter")
            .with_state("minecraft:cardinal_direction", "south");
        let stonecutter_shape =
            model_shape(&stonecutter).unwrap_or_else(|| panic!("missing stonecutter shape"));
        // The official model: the 9 px base slab plus the zero-thickness vertical saw blade.
        assert_eq!(stonecutter_shape.cuboids.len(), 1);
        assert_eq!(stonecutter_shape.planes.len(), 1);
        assert_eq!(
            stonecutter_shape.planes[0].material_slot.as_deref(),
            Some("saw")
        );
        assert!(
            stonecutter_shape.planes[0]
                .corners
                .iter()
                .all(|c| (c[0] - 1.0 / 16.0).abs() < 0.001 || (c[0] - 15.0 / 16.0).abs() < 0.001),
            "the blade spans x 1..15 px"
        );

        let hopper = BlockStateQuery::new("minecraft:hopper").with_state("facing_direction", 5);
        let hopper_shape = model_shape(&hopper).unwrap_or_else(|| panic!("missing hopper shape"));
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
        let potted_shape =
            model_shape(&potted).unwrap_or_else(|| panic!("missing flower pot shape"));
        // JE `flower_pot` model: five boxes whose per-face UVs name the painted regions of the
        // 16x16 tile (the earlier single-box + alpha-cutout approach sampled the whole tile and
        // rendered a plain brown block). The poppy adds a cross-plane pair.
        assert_eq!(potted_shape.cuboids.len(), 5);
        assert_eq!(potted_shape.planes.len(), 2);
    }

    #[test]
    fn vine_and_rail_shapes_should_emit_detail_planes() {
        let vine = BlockStateQuery::new("minecraft:vine").with_state("north", true);
        let vine_shape = model_shape(&vine).unwrap_or_else(|| panic!("missing vine shape"));
        assert_eq!(vine_shape.planes.len(), 1);
        assert_eq!(vine_shape.planes[0].normal, [0, 0, 1]);

        let rail = BlockStateQuery::new("minecraft:rail").with_state("rail_direction", 1_i64);
        let rail_shape = model_shape(&rail).unwrap_or_else(|| panic!("missing rail shape"));
        assert!(rail_shape.cuboids.is_empty());
        assert_eq!(rail_shape.planes.len(), 1);
        assert_eq!(rail_shape.planes[0].material_slot.as_deref(), Some("side"));
        assert!(
            rail_shape.planes[0]
                .corners
                .iter()
                .all(|corner| (corner[1] - 0.0625).abs() < 1e-6)
        );

        let sloped = BlockStateQuery::new("minecraft:rail").with_state("rail_direction", 2_i64);
        let sloped_shape = model_shape(&sloped).unwrap_or_else(|| panic!("missing sloped rail"));
        assert!(
            sloped_shape.planes[0]
                .corners
                .iter()
                .any(|corner| corner[1] > 1.0)
        );

        let curved = BlockStateQuery::new("minecraft:rail").with_state("rail_direction", 6_i64);
        let curved_shape = model_shape(&curved).unwrap_or_else(|| panic!("missing curved rail"));
        assert_eq!(curved_shape.planes[0].material_slot.as_deref(), Some("up"));
    }

    #[test]
    fn sign_and_cauldron_shapes_should_emit_non_full_geometry() {
        let standing_sign = BlockStateQuery::new("minecraft:standing_sign");
        let standing_sign_shape =
            model_shape(&standing_sign).unwrap_or_else(|| panic!("missing standing sign shape"));
        assert_eq!(standing_sign_shape.cuboids.len(), 2);

        let wall_sign = BlockStateQuery::new("minecraft:wall_sign")
            .with_state("minecraft:cardinal_direction", "west");
        let wall_sign_shape =
            model_shape(&wall_sign).unwrap_or_else(|| panic!("missing wall sign"));
        assert_eq!(wall_sign_shape.cuboids.len(), 1);

        let hanging_sign = BlockStateQuery::new("minecraft:hanging_sign");
        let hanging_sign_shape =
            model_shape(&hanging_sign).unwrap_or_else(|| panic!("missing hanging sign"));
        assert!(hanging_sign_shape.cuboids.len() >= 3);

        let wall_hanging_sign = BlockStateQuery::new("minecraft:oak_wall_hanging_sign")
            .with_state("minecraft:cardinal_direction", "east");
        let wall_hanging_sign_shape =
            model_shape(&wall_hanging_sign).unwrap_or_else(|| panic!("missing wall hanging sign"));
        assert!(
            wall_hanging_sign_shape
                .cuboids
                .iter()
                .any(|cuboid| { cuboid.min[0] >= 14.5 / 16.0 && cuboid.max[0] == 1.0 })
        );

        // Java `block/cauldron`: four walls, an inner floor and eight corner feet.
        let cauldron = BlockStateQuery::new("minecraft:cauldron");
        let cauldron_shape = model_shape(&cauldron).unwrap_or_else(|| panic!("missing cauldron"));
        assert_eq!(cauldron_shape.cuboids.len(), 13);
        assert!(
            cauldron_shape
                .cuboids
                .iter()
                .all(|cuboid| cuboid.max[1] <= 1.0)
        );
    }

    #[test]
    fn advanced_block_states_should_generate_custom_geometry() {
        let repeater_d0 =
            BlockStateQuery::new("minecraft:unpowered_repeater").with_state("repeater_delay", 0);
        let repeater_d3 =
            BlockStateQuery::new("minecraft:unpowered_repeater").with_state("repeater_delay", 3);
        let shape_d0 = model_shape(&repeater_d0).unwrap();
        let shape_d3 = model_shape(&repeater_d3).unwrap();
        // Cuboids: [0] base, [1] output post, [2] delay post (official model).
        assert_ne!(shape_d0.cuboids[2].min[2], shape_d3.cuboids[2].min[2]);

        // Bedrock keeps the base and the head in separate blocks: the base is a full cube,
        // the head is the plate + shaft assembly.
        let piston_normal = BlockStateQuery::new("minecraft:piston");
        let piston_extended =
            BlockStateQuery::new("minecraft:piston").with_state("extended_bit", true);
        assert_eq!(model_shape(&piston_normal).unwrap().cuboids.len(), 1);
        assert_eq!(model_shape(&piston_extended).unwrap().cuboids.len(), 1);
        // The head is its own block with its own direction: plate (0.25 thick) + shaft.
        let arm = BlockStateQuery::new("minecraft:piston_arm_collision")
            .with_state("facing_direction", 3);
        let arm_shape = model_shape(&arm).unwrap();
        assert_eq!(arm_shape.cuboids.len(), 2);
        let plate = arm_shape
            .cuboids
            .iter()
            .find(|cuboid| cuboid.max[0] - cuboid.min[0] > 0.9)
            .expect("plate");
        assert!((plate.max[0] - plate.min[0]).abs() > 0.9);
        assert!(plate.min[0].abs() < 0.001, "plate caps the west push face");

        // Capture ref 760 lectern: base + post cuboids plus the six planes of the 2 px tilted
        // board slab; the book is an extra cuboid when the lectern holds one.
        let lectern_empty = BlockStateQuery::new("minecraft:lectern");
        let lectern_book =
            BlockStateQuery::new("minecraft:lectern").with_state("has_book_bit", true);
        let empty = model_shape(&lectern_empty).unwrap();
        assert_eq!(empty.cuboids.len(), 2);
        assert_eq!(empty.planes.len(), 6);
        let with_book = model_shape(&lectern_book).unwrap();
        assert_eq!(with_book.cuboids.len(), 3);
        assert_eq!(with_book.planes.len(), 6);
    }

    #[test]
    fn full_block_shapes_and_orientations_should_generate_valid_cuboids() {
        let stone = BlockStateQuery::new("minecraft:stone");
        let stone_shape = model_shape(&stone).unwrap();
        assert_eq!(stone_shape.cuboids.len(), 1);
        assert_eq!(stone_shape.cuboids[0].min, [0.0, 0.0, 0.0]);
        assert_eq!(stone_shape.cuboids[0].max, [1.0, 1.0, 1.0]);

        let log_x = BlockStateQuery::new("minecraft:oak_log").with_state("pillar_axis", "x");
        let log_shape = model_shape(&log_x).unwrap();
        assert_eq!(
            log_shape.cuboids[0]
                .face_material_slots
                .get(&crate::material::BlockFace::East)
                .map(|s| s.as_str()),
            Some("top")
        );
    }
}
