use super::*;

#[test]
fn unsafe_destination_is_rejected() {
    assert!(validate_relative_path("../Minecraft.Windows.exe").is_err());
}

#[test]
fn wildcard_file_placement_matches_nested_dll() {
    assert!(file_pattern_matches("bin/*.dll", "bin/example.dll"));
}

#[test]
fn unrelated_directory_placement_is_ignored() {
    let placements = vec![AssetPlacement {
        kind: PlacementKind::Dir,
        src: "LeviLamina/".to_string(),
        destination: "mods/LeviLamina/".to_string(),
    }];

    assert!(
        placement_destinations("Other/file.dll", &placements)
            .expect("valid placements")
            .is_empty()
    );
}

#[test]
fn uncompressed_asset_uses_lip_empty_source_key() {
    let placements = vec![AssetPlacement {
        kind: PlacementKind::File,
        src: String::new(),
        destination: "mods/example.dll".to_string(),
    }];

    assert_eq!(
        placement_destinations("", &placements).expect("valid placement"),
        vec![PathBuf::from("mods/example.dll")]
    );
}
