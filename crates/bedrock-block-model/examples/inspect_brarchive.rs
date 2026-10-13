//! Read-only smoke check of installed game resources, without extracting a pack.
use std::path::Path;
use std::time::Instant;

use bedrock_block_model::{
    BlockModelRepository, ObjTextureCopy, ObjTextureResolver, read_obj_texture_copy_image,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("usage: inspect_brarchive PACK_ROOT")?;
    let started = Instant::now();
    let models = BlockModelRepository::load_packs([Path::new(&root)])?;
    println!(
        "pack={} blocks={} textures={} geometries={} load_ms={:.3}",
        Path::new(&root).display(),
        models.blocks.len(),
        models.terrain_textures.textures.len(),
        models.geometries.geometries.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    let resolver = ObjTextureResolver::try_with_pack_roots([Path::new(&root)], "textures")?;
    let texture = resolver
        .texture_for("minecraft_stone", [0, 1, 0])
        .ok_or("missing stone texture")?;
    let source_bytes = texture.source_path.read()?;
    let copy = ObjTextureCopy {
        source_path: texture.source_path,
        relative_path: texture.relative_path,
        tint: None,
        alpha_mask: false,
    };
    drop(resolver);
    let image = read_obj_texture_copy_image(&copy)?;
    println!(
        "stone_source={:?} bytes={} image={}x{} elapsed_ms={:.3}",
        copy.source_path,
        source_bytes.len(),
        image.width(),
        image.height(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}
