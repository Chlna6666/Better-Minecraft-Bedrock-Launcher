//! Probe: resolve a material against an explicit ordered pack-root list.
use std::path::{Path, PathBuf};

use bedrock_block_model::ObjTextureResolver;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(material)) = (args.next(), args.next()) else {
        eprintln!("usage: probe_roots <version-root> <material>");
        std::process::exit(2);
    };
    let mut roots: Vec<PathBuf> = vec![PathBuf::from(&root)];
    let packs = Path::new(&root).join("data/resource_packs");
    let mut names: Vec<String> = std::fs::read_dir(&packs)
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for name in &names {
        let client = packs.join(name).join("client");
        if client.exists() {
            roots.push(client);
        }
        roots.push(packs.join(name));
    }
    println!("roots={}", roots.len());
    let resolver = ObjTextureResolver::with_pack_roots(&roots, "textures");
    match resolver.texture_for(&material, [1, 0, 0]) {
        Some(t) => println!("{material} -> {}", t.relative_path),
        None => println!("{material} -> <MISSING>"),
    }
}
