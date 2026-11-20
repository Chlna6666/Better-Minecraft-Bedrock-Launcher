//! Diagnostic: dump official engine geometries matching a substring.
use std::path::Path;

use bedrock_block_model::BlockModelRepository;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os().nth(1).ok_or("usage: geometry_probe PACK_ROOT substring")?;
    let needle = std::env::args_os()
        .nth(2)
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let models = BlockModelRepository::load_packs([Path::new(&root)])?;
    for (identifier, geometry) in &models.geometries.geometries {
        if !identifier.to_lowercase().contains(&needle.to_lowercase()) {
            continue;
        }
        println!("===== {identifier}");
        for bone in &geometry.bones {
            if let Some(raw) = bone.raw.as_object() {
                for cube in raw.get("cubes").and_then(|c| c.as_array()).into_iter().flatten() {
                    println!(
                        " origin {:?} size {:?} uv {:?} texture {:?} material_instances {:?}",
                        cube.get("origin"),
                        cube.get("size"),
                        cube.get("uv"),
                        cube.get("texture"),
                        cube.get("material_instances"),
                    );
                }
            }
        }
    }
    Ok(())
}
