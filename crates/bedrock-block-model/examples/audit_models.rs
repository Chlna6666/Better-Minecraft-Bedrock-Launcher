//! Diagnostic audit over every block model family.
//!
//! Usage: `cargo run -p bedrock-block-model --example audit_models -- <version-or-pack-root>`
//!
//! Prints, per representative block: missing cuboid UVs, unresolved face materials, and
//! suspicious texture identities (a face whose art is the same placeholder for every block).

use std::collections::{BTreeMap, BTreeSet};

use bedrock_block_model::{
    BlockModelRepository, BlockStateQuery, ObjTextureResolver,
    block_export_material_name_for_block, block_export_material_name_for_face,
    block_export_material_name_for_plane, block_export_material_name_for_slot,
    block_face_for_normal, model_family_for_block_name, model_shape_for_block_state,
};

/// One representative state per model family, named the way the world stores blocks.
const SAMPLES: &[&str] = &[
    // building
    "minecraft:stone_slab",
    "minecraft:oak_stairs",
    "minecraft:ladder",
    "minecraft:oak_fence",
    "minecraft:oak_fence_gate",
    "minecraft:cobblestone_wall",
    "minecraft:glass_pane",
    "minecraft:iron_bars",
    "minecraft:oak_trapdoor",
    "minecraft:acacia_trapdoor",
    "minecraft:oak_door",
    "minecraft:white_carpet",
    "minecraft:rail",
    "minecraft:redstone_wire",
    // flora
    "minecraft:poppy",
    "minecraft:cactus",
    "minecraft:bamboo",
    "minecraft:cocoa",
    "minecraft:azalea",
    "minecraft:short_grass",
    "minecraft:vine",
    "minecraft:fire",
    "minecraft:soul_fire",
    "minecraft:water",
    "minecraft:chorus_plant",
    "minecraft:pointed_dripstone",
    "minecraft:bubble_column",
    "minecraft:trip_wire",
    "minecraft:sculk_sensor",
    "minecraft:sculk_shrieker",
    "minecraft:spore_blossom",
    "minecraft:big_dripleaf",
    "minecraft:small_dripleaf_block",
    "minecraft:mangrove_propagule",
    "minecraft:farmland",
    // lighting / decoration
    "minecraft:torch",
    "minecraft:soul_torch",
    "minecraft:redstone_torch",
    "minecraft:lantern",
    "minecraft:soul_lantern",
    "minecraft:candle",
    "minecraft:black_candle",
    "minecraft:end_rod",
    "minecraft:lightning_rod",
    "minecraft:chain",
    "minecraft:iron_chain",
    "minecraft:campfire",
    "minecraft:soul_campfire",
    "minecraft:bell",
    "minecraft:enchanting_table",
    "minecraft:flower_pot",
    "minecraft:decorated_pot",
    "minecraft:standing_banner",
    "minecraft:wall_banner",
    "minecraft:white_banner",
    // redstone
    "minecraft:piston",
    "minecraft:sticky_piston",
    "minecraft:piston_arm_collision",
    "minecraft:sticky_piston_arm_collision",
    "minecraft:observer",
    "minecraft:dispenser",
    "minecraft:dropper",
    "minecraft:unpowered_repeater",
    "minecraft:powered_repeater",
    "minecraft:unpowered_comparator",
    "minecraft:powered_comparator",
    "minecraft:daylight_detector",
    "minecraft:stone_button",
    "minecraft:stone_pressure_plate",
    "minecraft:lever",
    "minecraft:tripwire_hook",
    // utility
    "minecraft:chest",
    "minecraft:trapped_chest",
    "minecraft:ender_chest",
    "minecraft:barrel",
    "minecraft:shulker_box",
    "minecraft:blue_shulker_box",
    "minecraft:furnace",
    "minecraft:hopper",
    "minecraft:cauldron",
    "minecraft:composter",
    "minecraft:anvil",
    "minecraft:bookshelf",
    "minecraft:chiseled_bookshelf",
    "minecraft:crafting_table",
    "minecraft:stonecutter_block",
    "minecraft:grindstone",
    "minecraft:lectern",
    "minecraft:brewing_stand",
    "minecraft:enchanting_table",
    "minecraft:beacon",
    "minecraft:conduit",
    "minecraft:heavy_core",
    "minecraft:bed",
    "minecraft:oak_shelf",
    "minecraft:cake",
    "minecraft:scaffolding",
    "minecraft:sea_pickle",
    "minecraft:turtle_egg",
    "minecraft:dragon_egg",
    "minecraft:end_portal_frame",
    "minecraft:oak_standing_sign",
    "minecraft:oak_wall_sign",
    "minecraft:oak_hanging_sign",
    // remaining families
    "minecraft:dried_ghast",
    "minecraft:copper_golem_statue",
    "minecraft:big_dripleaf",
    "minecraft:mangrove_roots",
    "minecraft:pale_moss_carpet",
    "minecraft:sandstone_slab",
    "minecraft:end_portal",
    "minecraft:nether_portal",
    // heads
    "minecraft:creeper_head",
    "minecraft:skeleton_skull",
    "minecraft:wither_skeleton_skull",
    "minecraft:zombie_head",
    "minecraft:player_head",
    "minecraft:dragon_head",
    "minecraft:piglin_head",
];

fn main() {
    let root = std::env::args().nth(1).expect("version or pack root");
    let pack = std::path::PathBuf::from(format!("{root}/data/resource_packs/vanilla"));
    let repo = BlockModelRepository::load_packs([pack]).expect("packs");
    let resolver = ObjTextureResolver::with_package_roots([root.as_str()], "textures");

    let mut by_family: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut placeholder_textures: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut problems = 0usize;

    for name in SAMPLES {
        let family = format!("{:?}", model_family_for_block_name(name));
        let query = BlockStateQuery::new(*name);
        let Some(shape) = model_shape_for_block_state(&query) else {
            by_family
                .entry(family)
                .or_default()
                .push(format!("{name}: NO SHAPE"));
            problems += 1;
            continue;
        };

        let mut notes = Vec::new();
        if shape.cuboids.is_empty() && shape.planes.is_empty() {
            notes.push("EMPTY".to_string());
            problems += 1;
        }
        let missing_uv = shape
            .cuboids
            .iter()
            .filter(|c| c.face_uvs.is_empty())
            .count();
        let thin_missing_uv = shape
            .cuboids
            .iter()
            .filter(|c| {
                let w = c.max[0] - c.min[0];
                let d = c.max[2] - c.min[2];
                (w < 0.3 || d < 0.3) && c.face_uvs.is_empty()
            })
            .count();
        if missing_uv > 0 {
            notes.push(format!("noUV={missing_uv} thinNoUV={thin_missing_uv}"));
        }

        // Resolve the texture every face would use.
        let base = block_export_material_name_for_block(name);
        let mut textures = BTreeSet::new();
        let mut unresolved = 0usize;
        for cuboid in &shape.cuboids {
            for normal in [
                [0, 1, 0],
                [0, -1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [1, 0, 0],
                [-1, 0, 0],
            ] {
                let face = block_face_for_normal(normal);
                let material = match cuboid
                    .face_material_slots
                    .get(&face)
                    .or(cuboid.material_slot.as_ref())
                {
                    Some(slot) => block_export_material_name_for_slot(&base, slot),
                    None => block_export_material_name_for_face(&base, normal),
                };
                match resolver.texture_for(&material, normal) {
                    Some(texture) => {
                        textures.insert(texture.relative_path);
                    }
                    None => unresolved += 1,
                }
            }
        }
        for plane in &shape.planes {
            let material = block_export_material_name_for_plane(
                &base,
                plane.normal,
                plane.material_slot.as_deref(),
            );
            match resolver.texture_for(&material, plane.normal) {
                Some(texture) => {
                    textures.insert(texture.relative_path);
                }
                None => unresolved += 1,
            }
        }
        if unresolved > 0 {
            notes.push(format!("unresolvedMaterials={unresolved}"));
            problems += 1;
        }
        if textures.is_empty() {
            notes.push("NO TEXTURE".to_string());
            problems += 1;
        }
        // A single shared placeholder is the signature of the `skull`->soul_sand and
        // `bed_bottom`->planks_oak style bugs.
        if textures.len() == 1
            && matches!(
                textures.iter().next().map(String::as_str),
                Some("textures/blocks/soul_sand.png") | Some("textures/blocks/planks_oak.png")
            )
        {
            notes.push(format!("PLACEHOLDER {:?}", textures));
            problems += 1;
        }
        placeholder_textures
            .entry(family.clone())
            .or_default()
            .extend(textures.iter().cloned());

        by_family.entry(family).or_default().push(format!(
            "{name} [cub {} plane {}] {}{}",
            shape.cuboids.len(),
            shape.planes.len(),
            notes.join(", "),
            if notes.is_empty() {
                String::new()
            } else {
                " <-- CHECK".to_string()
            }
        ));
    }

    for (family, rows) in &by_family {
        println!("== {family}");
        for row in rows {
            println!("   {row}");
        }
    }
    println!("\n== distinct textures per family");
    for (family, textures) in &placeholder_textures {
        println!(
            "{family}: {:?}",
            textures.iter().take(6).collect::<Vec<_>>()
        );
    }
    println!("\nproblems: {problems}");
}
