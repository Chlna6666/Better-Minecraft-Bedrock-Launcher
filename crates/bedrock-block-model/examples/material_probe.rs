//! Resolve material names against an installed version's resource packs.
//!
//! Material names follow the exporter's `block__mat_slot` spelling (`__mat_` separates the
//! block from its model slot), so a family slot can be checked without rendering.
//!
//! Usage: `material_probe <version-root> <material> [<material> ...]`

use std::path::Path;

use bedrock_block_model::ObjTextureResolver;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: material_probe <version-root> <material> ...");
        std::process::exit(2);
    };
    let resolver = ObjTextureResolver::with_package_roots([Path::new(&root)], "textures");
    for material in args {
        // A leading `entity:` names a loose entity texture instead of a terrain material.
        let resolved = if let Some(key) = material.strip_prefix("entity:") {
            resolver.entity_texture(key)
        } else {
            resolver.texture_for(&material, [0, 1, 0])
        };
        match resolved {
            Some(texture) => println!("{material} -> {}", texture.relative_path),
            None => println!("{material} -> <MISSING>"),
        }
    }
}
