use std::collections::HashMap;

use super::*;

#[test]
fn loader_versions_match_zero_padded_game_version() {
    let versions = HashMap::from([("1.26.20.04".to_string(), vec!["26.20.7".to_string()])]);

    assert_eq!(
        loader_versions_for_game(&versions, "1.26.20.4"),
        vec!["26.20.7"]
    );
}

#[test]
fn loader_versions_match_short_download_version_prefix() {
    let versions = HashMap::from([("1.26.20.04".to_string(), vec!["26.20.7".to_string()])]);

    assert_eq!(
        loader_versions_for_game(&versions, "1.26.20"),
        vec!["26.20.7"]
    );
}

#[test]
fn loader_versions_match_download_version_without_major_prefix() {
    let versions = HashMap::from([("1.26.20.04".to_string(), vec!["26.20.7".to_string()])]);

    assert_eq!(
        loader_versions_for_game(&versions, "26.20"),
        vec!["26.20.7"]
    );
}

#[test]
fn loader_versions_keep_supported_legacy_download_versions() {
    let versions = HashMap::from([
        ("1.21.124.02".to_string(), vec!["1.8.0-rc.2".to_string()]),
        ("1.21.132.01".to_string(), vec!["1.9.9".to_string()]),
    ]);

    assert_eq!(
        loader_versions_for_game(&versions, "1.21.124"),
        vec!["1.8.0-rc.2"]
    );
    assert_eq!(
        loader_versions_for_game(&versions, "1.21.132"),
        vec!["1.9.9"]
    );
}

#[test]
fn loader_versions_reject_different_game_version() {
    let versions = HashMap::from([("1.26.20.04".to_string(), vec!["26.20.7".to_string()])]);

    assert!(loader_versions_for_game(&versions, "1.26.10.04").is_empty());
}
