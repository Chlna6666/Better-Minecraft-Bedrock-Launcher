//! End-to-end check: resolve modern block textures from a real game data dir.
//!
//! Usage: cargo run --example resolve_real_stack -p bedrock-block-model
use std::path::Path;

use bedrock_block_model::{ObjTextureResolver, vanilla_resource_pack_roots_from_packages};

fn main() {
    let package = Path::new(
        r"C:\Users\Administrator\Desktop\BMCBL\target\debug\BMCBL\versions\26.51",
    );
    let roots = vanilla_resource_pack_roots_from_packages([package]);
    println!("pack roots discovered: {}", roots.len());
    for root in &roots {
        println!("  {}", root.display());
    }

    let resolver = ObjTextureResolver::with_pack_roots(&roots, "textures");
    println!();
    // Face normals: up, north, south (front), side.
    let probes: [(&str, [i32; 3]); 24] = [
        ("minecraft_oak_planks", [0, 0, 1]),
        ("minecraft_poppy", [0, 1, 0]),
        ("minecraft_cherry_leaves", [0, 0, 1]),
        ("minecraft_pale_oak_planks", [0, 0, 1]),
        ("minecraft_oak_door", [0, 0, 1]),
        ("minecraft_furnace", [0, 0, -1]),
        ("minecraft_furnace_front_on", [0, 0, -1]),
        ("minecraft_glass", [0, 0, 1]),
        ("minecraft_crafting_table", [0, 0, 1]),
        ("minecraft_grass_block", [0, 1, 0]),
        ("minecraft_stone", [0, 0, 1]),
        ("minecraft_tallgrass", [0, 1, 0]),
        ("minecraft_seagrass", [0, 1, 0]),
        ("minecraft_tall_seagrass", [0, 1, 0]),
        ("minecraft_kelp", [0, 1, 0]),
        ("minecraft_kelp_plant", [0, 1, 0]),
        ("minecraft_oak_sapling", [0, 1, 0]),
        ("minecraft_dandelion", [0, 1, 0]),
        ("minecraft_sugar_cane", [0, 1, 0]),
        ("minecraft_vine", [0, 1, 0]),
        ("minecraft_lily_pad", [0, 1, 0]),
        ("minecraft_ladder", [0, 0, 1]),
        ("minecraft_rail", [0, 1, 0]),
        ("minecraft_torch", [0, 1, 0]),
    ];
    let mut hit = 0usize;
    for (material, normal) in probes {
        let resolved = resolver.texture_for(material, normal);
        match resolved {
            Some(texture) => {
                hit += 1;
                println!("{material:<32} -> {}", texture.relative_path);
            }
            None => println!("{material:<32} -> <MISSING>"),
        }
    }
    println!();
    println!("resolved {hit}/{}", probes.len());
}
