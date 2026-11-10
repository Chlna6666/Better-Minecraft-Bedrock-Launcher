use super::*;

#[test]
fn preloader_placement_preserves_original_mod_directory() {
    let mut variant = super::super::lip::PackageVariant {
        assets: vec![super::super::lip::PackageAsset {
            kind: AssetKind::Zip,
            urls: vec![],
            placements: vec![super::super::lip::AssetPlacement {
                kind: super::super::lip::PlacementKind::File,
                src: "bin/PreLoader.dll".to_string(),
                destination: "PreLoader.dll".to_string(),
            }],
        }],
        ..super::super::lip::PackageVariant::default()
    };

    super::super::planner::redirect_preloader(&mut variant);

    assert_eq!(
        variant.assets[0].placements[0].destination,
        "mods/PreLoader/PreLoader.dll"
    );
}

#[test]
fn preloader_validity_requires_bootstrap_manifest_and_dll() {
    let root = std::env::temp_dir().join(format!(
        "bmcbl-preloader-test-{}", std::process::id()
    ));
    let dir = root.join("mods").join("PreLoader");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("PreLoader.dll"), b"test").unwrap();
    assert_eq!(
        super::super::installation_state::validated_preloader_version(&root).unwrap(),
        None
    );
    std::fs::write(dir.join("manifest.json"), br#"{"name":"PreLoader","entry":"PreLoader.dll","type":"preloader-bootstrap","version":"1.16.8"}"#).unwrap();
    assert_eq!(
        super::super::installation_state::validated_preloader_version(&root).unwrap(),
        Some("1.16.8".to_string())
    );
    std::fs::remove_file(dir.join("PreLoader.dll")).unwrap();
    assert_eq!(
        super::super::installation_state::validated_preloader_version(&root).unwrap(),
        None
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn locked_path_cannot_escape_game_directory() {
    let game_directory = Path::new("C:/games/test");

    assert!(
        super::super::installation_state::safe_locked_path(game_directory, "../outside.dll")
            .is_err()
    );
}

#[test]
fn peeditor_dependency_is_ignored() {
    assert!(super::super::planner::is_ignored_dependency(
        "github.com/LiteLDev/PeEditor"
    ));
}
