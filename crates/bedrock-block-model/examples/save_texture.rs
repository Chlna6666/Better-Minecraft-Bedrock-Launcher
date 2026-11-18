//! Save a resolved material texture to a PNG for inspection.
use std::path::Path;

use bedrock_block_model::obj::{decode_texture_image, path_extension_eq};
use bedrock_block_model::ObjTextureResolver;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(material), Some(out)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: save_texture <version-root> <material> <out.png>");
        std::process::exit(2);
    };
    let resolver = ObjTextureResolver::with_package_roots([Path::new(&root)], "textures");
    let texture = resolver
        .texture_for(&material, [0, 1, 0])
        .ok_or("texture not resolved")?;
    let bytes = texture.source_path.read()?;
    let tga = path_extension_eq(texture.source_path.path(), "tga");
    let image = decode_texture_image(&bytes, tga)?.to_rgba8();
    image.save(&out)?;
    println!("{} -> {} ({}x{})", material, out, image.width(), image.height());
    Ok(())
}
