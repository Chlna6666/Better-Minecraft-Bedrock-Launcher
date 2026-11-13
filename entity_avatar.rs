use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn png_file_names(entity_dir: &Path) -> BTreeSet<String> {
    fs::read_dir(entity_dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", entity_dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!("failed to read entry in {}: {error}", entity_dir.display())
                })
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| {
                    panic!("entity avatar filename is not UTF-8: {}", path.display())
                })
                .to_string()
        })
        .collect()
}

pub(crate) fn generate() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let entity_dir = manifest_dir
        .join("assets")
        .join("images")
        .join("map")
        .join("entity");
    let manifest_path = entity_dir.join("manifest.json");
    let manifest_text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest_path.display()));
    let manifest: BTreeMap<String, String> = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest_path.display()));

    let manifest_files = manifest.values().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        manifest.len(),
        manifest_files.len(),
        "entity avatar manifest maps one PNG more than once"
    );
    let disk_files = png_file_names(&entity_dir);
    assert_eq!(
        manifest_files, disk_files,
        "entity avatar manifest must list every PNG exactly once"
    );

    let mut items = String::new();
    for (identifier, file_name) in manifest {
        assert!(
            !identifier.trim().is_empty(),
            "entity avatar identifier is empty"
        );
        let asset_path = format!("images/map/entity/{file_name}");
        items.push_str(&format!("    ({identifier:?}, {asset_path:?}),\n"));
    }
    let code = format!(
        "// Auto-generated from assets/images/map/entity/manifest.json. Do not edit.\n\
         const ENTITY_AVATAR_ASSETS: &[(&str, &str)] = &[\n{items}];\n"
    );
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("entity_avatar_assets.rs"), code).expect("write entity avatar catalog");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
}
