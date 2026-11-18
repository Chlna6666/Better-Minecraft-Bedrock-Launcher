//! Probe: report which texture paths resolve in the installed pack stack.
use std::path::Path;

use bedrock_block_model::ObjTextureResolver;

fn main() {
    let root = std::env::args().nth(1).expect("usage: probe_paths <version-root> <path>...");
    let resolver = ObjTextureResolver::with_package_roots([Path::new(&root)], "textures");
    for material in std::env::args().skip(2) {
        // `material` is a full logical path like textures/entity/shulker/shulker_silver.
        let probe = format!("minecraft_shulker_box__mat_{material}");
        match resolver.texture_for(&probe, [0, 1, 0]) {
            Some(t) => println!("{material} -> {}", t.relative_path),
            None => println!("{material} -> <MISSING>"),
        }
    }
}
