//! Diagnostic: dump official engine geometries or terrain keys matching a substring.
use std::path::{Path, PathBuf};

use bedrock_block_model::BlockModelRepository;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::args_os().nth(1).ok_or("usage: texture_size PACK_ROOT [substring]")?);
    let needle = std::env::args_os()
        .nth(2)
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let models = BlockModelRepository::load_packs([root.as_path()])?;
    for (identifier, geometry) in &models.geometries.geometries {
        if !identifier.to_lowercase().contains(&needle.to_lowercase()) {
            continue;
        }
        println!("===== geometry {identifier}");
        for bone in &geometry.bones {
            for cube in &bone.cubes {
                println!(
                    " origin {:?} size {:?} uv {:?}",
                    cube.origin,
                    cube.size,
                    cube.uv.as_ref().map(|uv| &uv.raw),
                );
            }
        }
    }
    Ok(())
}
