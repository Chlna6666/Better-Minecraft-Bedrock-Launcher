//! Guards the embedded BedrockSceneCapture model library.
//!
//! The archive is produced by `scripts/pack_bsc_model_library.py` from a real capture export
//! and embedded through `build.rs`. A build without it silently falls back to procedural
//! shapes, which is why these assertions exist: an empty library must be a build failure, not
//! a quiet downgrade.

use bedrock_block_model::RuntimeCaptureCatalog;

#[test]
fn embedded_capture_library_is_present_and_readable() {
    let catalog = RuntimeCaptureCatalog::open_embedded().expect("embedded BSC model library");
    assert!(
        catalog.state_count() > 1000,
        "embedded BSC library has {} states; expected the full registry export",
        catalog.state_count()
    );
    assert!(
        !catalog.game_version().is_empty(),
        "embedded BSC manifest must record its target game version"
    );
}
