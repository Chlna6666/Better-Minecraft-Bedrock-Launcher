use super::*;

#[test]
fn preloader_placement_is_redirected_to_mod_directory() {
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
