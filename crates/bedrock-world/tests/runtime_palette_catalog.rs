//! Public API tests for the read-only runtime block-palette catalog.
//!
//! Source: pmmp/bds-mod-mapping runtime dumps. Network runtime ID is the
//! state's index. These are runtime registry observations, not migration data.

use bedrock_world::block::palettes::{
    RuntimePaletteValue, runtime_palette_for, runtime_palette_state, runtime_palette_versions,
};

#[test]
fn covers_all_bundled_runtime_dumps() {
    let versions = runtime_palette_versions();
    assert_eq!(versions.len(), 50);
    assert!(versions.contains(&"1.12.0"));
    assert!(versions.contains(&"1.20.80.24_beta"));
    assert!(versions.contains(&"1.26.60"));
    for label in &versions {
        let palette = runtime_palette_for(label)
            .unwrap_or_else(|| panic!("missing runtime palette for {label}"));
        assert!(!palette.is_empty());
        assert_eq!(palette.len(), palette.states().count());
        // Every state has a coherent NBT storage version within one palette.
        assert_eq!(palette.storage_versions().len(), 1);
    }
}

#[test]
fn network_runtime_id_is_exact_index() {
    let palette = runtime_palette_for("1.20.80.24_beta").expect("palette");
    assert_eq!(palette.len(), 14140);
    assert_eq!(palette.storage_versions(), vec![18108419]);
    for (index, state) in palette.states().enumerate() {
        assert_eq!(state.runtime_id(), index as u32);
    }
    assert_eq!(palette.state(0).expect("state 0").name(), "minecraft:cyan_terracotta");
    assert!(palette.state(14140).is_none());
}

#[test]
fn typed_states_are_preserved_exactly() {
    let state = runtime_palette_state("1.20.80.24_beta", 2).expect("blue_candle state");
    assert_eq!(state.name(), "minecraft:blue_candle");
    assert_eq!(state.storage_version(), 18108419);
    assert_eq!(
        state.states().get("lit"),
        Some(&RuntimePaletteValue::Byte(0))
    );
    assert_eq!(
        state.states().get("candles"),
        Some(&RuntimePaletteValue::Int(0))
    );
    assert_eq!(
        state.states().get("lit").map(RuntimePaletteValue::type_name),
        Some("Byte")
    );
}

#[test]
fn helper_lookup_matches_palette_lookup() {
    for runtime_id in [0u32, 1, 100, 14000] {
        let direct = runtime_palette_for("1.26.60")
            .expect("palette")
            .state(runtime_id);
        let via_helper = runtime_palette_state("1.26.60", runtime_id);
        assert_eq!(direct.map(|s| s.name()), via_helper.map(|s| s.name()));
    }
}
