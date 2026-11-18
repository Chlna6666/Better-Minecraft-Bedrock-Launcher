//! Probe: print the texture each jigsaw material slot resolves to.
use std::path::Path;

use bedrock_block_model::ObjTextureResolver;

fn main() {
    let root = std::env::args().nth(1).expect("usage: probe_slots <root> <material>...");
    let resolver = ObjTextureResolver::with_package_roots([Path::new(&root)], "textures");
    for material in std::env::args().skip(2) {
        match resolver.texture_for(&material, [0, 1, 0]) {
            Some(texture) => println!(
                "{material} -> {} flipbook={}",
                texture.relative_path,
                resolver.is_flipbook_texture(&texture.relative_path)
            ),
            None => println!("{material} -> <MISSING>"),
        }
    }
}
