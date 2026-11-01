//! Read-only probe: resolve real material names against an installed Bedrock version.
//!
//! cargo run -p bedrock-block-model --example probe_materials -- "<version dir>"
use std::path::Path;

use bedrock_block_model::ObjTextureResolver;

fn main() {
    let Some(raw) = std::env::args().nth(1) else {
        eprintln!("usage: probe_materials <version dir>");
        return;
    };
    let root = Path::new(&raw);
    let resolver = ObjTextureResolver::with_package_roots([root], "textures");
    let samples = [
        ("minecraft_stone_side", [1, 0, 0]),
        ("minecraft_grass_top", [0, 1, 0]),
        ("minecraft_grass_side", [0, 0, 1]),
        ("minecraft_water_side", [1, 0, 0]),
        ("minecraft_water_up", [0, 1, 0]),
        ("minecraft_chest_front", [0, 0, 1]),
        ("minecraft_itemframe_background", [0, 0, 1]),
        ("minecraft_ladder", [0, 0, 1]),
        ("minecraft_rail", [0, 1, 0]),
        ("minecraft_rail_normal", [0, 1, 0]),
        ("minecraft_rail_normal_turned", [0, 1, 0]),
        ("minecraft_anvil", [1, 0, 0]),
        ("minecraft_anvil_side", [1, 0, 0]),
        ("minecraft_anvil_top_damaged_2", [0, 1, 0]),
        ("minecraft_door_wood_upper", [0, 0, 1]),
        ("minecraft_trapdoor", [0, 0, 1]),
        ("minecraft_tallgrass", [0, 0, 1]),
        ("minecraft_tall_grass_top", [0, 1, 0]),
        ("minecraft_tall_grass_bottom", [0, -1, 0]),
        ("minecraft_flower_rose", [0, 0, 1]),
        ("minecraft_flower_dandelion", [0, 0, 1]),
        ("minecraft_bubble_column", [1, 0, 0]),
        ("minecraft_wheat", [0, 0, 1]),
        ("minecraft_double_plant_grass_top", [0, 1, 0]),
    ];
    let mut missing = 0;
    for (material, normal) in samples {
        match resolver.texture_for(material, normal) {
            Some(texture) => println!("{material} -> {}", texture.relative_path),
            None => {
                missing += 1;
                println!("{material} -> MISSING");
            }
        }
    }
    println!("missing: {missing}");
    println!(
        "flipbook water_side: {}",
        resolver.is_flipbook_texture("textures/blocks/water_flow_grey.png")
    );
    for item in [
        "minecraft:apple",
        "minecraft:ink_sac",
        "minecraft:prismarine_crystals",
        "minecraft:wooden_sword",
        "minecraft:filled_map",
    ] {
        match resolver.item_texture_for(item) {
            Some(texture) => println!("item {item} -> {}", texture.relative_path),
            None => println!("item {item} -> MISSING"),
        }
    }
}
