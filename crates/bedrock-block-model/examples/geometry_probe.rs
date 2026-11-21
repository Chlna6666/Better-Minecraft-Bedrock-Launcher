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
            for cube in &bone.cubes {
                println!(
                    " origin {:?} size {:?} uv {:?} material_instance {:?} face_material_instances {:?}",
                    cube.origin,
                    cube.size,
                    cube.uv.as_ref().map(|uv| &uv.raw),
                    cube.material_instance,
                    cube.face_material_instances,
                );
            }
        }
    }
    Ok(())
}
