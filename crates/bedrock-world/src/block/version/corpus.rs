//! Pinned authoritative Bedrock block migration corpus metadata.
//!
//! The corpus is supplied as immutable application resources rather than fetched at runtime. This
//! manifest pins the exact PocketMine revision and validates that destructive migration never runs
//! against a partial, duplicated, or accidentally mixed schema set.

use super::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
};
use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeSet;

/// PocketMine `bedrock-block-upgrade-schema` package version pinned by this library revision.
pub const PINNED_BLOCK_UPGRADE_SCHEMA_VERSION: &str = "5.2.0";
/// Git commit backing [`PINNED_BLOCK_UPGRADE_SCHEMA_VERSION`].
pub const PINNED_BLOCK_UPGRADE_SCHEMA_COMMIT: &str =
    "5d7889c9a1cdf9e3cd814d2a104ad69b75116ec7";
/// Expected legacy numeric-ID map file in the pinned corpus.
pub const PINNED_LEGACY_BLOCK_ID_MAP_FILE: &str = "block_legacy_id_map.json";
/// 1.9 numeric ID/meta table, needed when a caller intentionally targets pre-1.12 BlockState data.
pub const PINNED_LEGACY_ID_META_1_9_TABLE_FILE: &str = "id_meta_to_nbt/1.9.0.bin";
/// 1.12 numeric ID/meta table used to lift classic terrain into the versioned BlockState pipeline.
pub const PINNED_LEGACY_ID_META_1_12_TABLE_FILE: &str = "id_meta_to_nbt/1.12.0.bin";

/// Complete ordered BlockState schema list in PocketMine `5.2.0`.
pub const PINNED_BLOCK_STATE_SCHEMA_FILES: &[&str] = &[
    "0001_1.9.0_to_1.10.0.json",
    "0011_1.10.0_to_1.12.0.json",
    "0021_1.12.0_to_1.13.0.json",
    "0031_1.13.0_to_1.14.0.json",
    "0041_1.14.0_to_1.16.0.57_beta.json",
    "0051_1.16.0.57_beta_to_1.16.0.59_beta.json",
    "0061_1.16.0.59_beta_to_1.16.0.68_beta.json",
    "0071_1.16.0_to_1.16.100.json",
    "0081_1.16.200_to_1.16.210.json",
    "0091_1.17.10_to_1.17.30.json",
    "0101_1.17.30_to_1.17.40.json",
    "0111_1.18.0_to_1.18.10.json",
    "0121_1.18.10_to_1.18.20.27_beta.json",
    "0131_1.18.20.27_beta_to_1.18.30.json",
    "0141_1.18.30_to_1.19.0.34_beta.json",
    "0151_1.19.0.34_beta_to_1.19.20.json",
    "0161_1.19.50_to_1.19.60.26_beta.json",
    "0171_1.19.60_to_1.19.70.26_beta.json",
    "0181_1.19.70_to_1.19.80.24_beta.json",
    "0191_1.19.80.24_beta_to_1.20.0.23_beta.json",
    "0201_1.20.0.23_beta_to_1.20.10.24_beta.json",
    "0211_1.20.10.24_beta_to_1.20.20.23_beta.json",
    "0221_1.20.20.23_beta_to_1.20.30.22_beta.json",
    "0231_1.20.30.22_beta_to_1.20.40.24_beta.json",
    "0241_1.20.40.24_beta_to_1.20.50.23_beta.json",
    "0251_1.20.50.23_beta_to_1.20.60.26_beta.json",
    "0261_1.20.60.26_beta_to_1.20.70.24_beta.json",
    "0271_1.20.70.24_beta_to_1.20.80.24_beta.json",
    "0281_1.20.80.24_beta_to_1.21.0.25_beta.json",
    "0291_1.21.0.25_beta_to_1.21.20.24_beta.json",
    "0301_1.21.20.24_beta_to_1.21.30.24_beta.json",
    "0311_1.21.30.24_beta_to_1.21.40.25_beta.json",
    "0321_1.21.50.29_beta_to_1.21.60.28_beta.json",
    "0331_1.21.100.23_beta_to_1.21.110.26_beta.json",
];

/// Validates and loads the complete pinned schema set, targeting the newest represented version.
///
/// Input ordering does not matter, but the filename set must match
/// [`PINNED_BLOCK_STATE_SCHEMA_FILES`] exactly. This prevents a server from performing destructive
/// migration with a partially updated resource bundle.
pub fn load_pinned_block_state_catalog(
    sources: &[BlockStateSchemaSource<'_>],
) -> Result<AuthoritativeBlockStateCatalog> {
    validate_pinned_sources(sources)?;
    AuthoritativeBlockStateCatalog::from_sources(sources)
}

/// Loads the complete pinned corpus as a catalogue bound to one exact historical target version.
///
/// All 34 pinned sources must still be supplied and validated. The loader then compiles only schema
/// groups whose result version is at or below `target_version`, so the per-block migration hot path
/// remains identical to a newest-version catalogue. The target must be an actual authoritative schema
/// endpoint; arbitrary version stamping and downgrades are refused.
pub fn load_pinned_block_state_catalog_for_target(
    sources: &[BlockStateSchemaSource<'_>],
    target_version: BlockStateStorageVersion,
) -> Result<AuthoritativeBlockStateCatalog> {
    validate_pinned_sources(sources)?;
    load_catalog_for_target(sources, target_version)
}

fn load_catalog_for_target(
    sources: &[BlockStateSchemaSource<'_>],
    target_version: BlockStateStorageVersion,
) -> Result<AuthoritativeBlockStateCatalog> {
    let mut selected = Vec::with_capacity(sources.len());
    for source in sources {
        if schema_result_version(source)? <= target_version {
            selected.push(*source);
        }
    }
    if selected.is_empty() {
        return Err(BedrockWorldError::Validation(format!(
            "target BlockState version {} predates every schema endpoint in the supplied corpus",
            target_version.raw()
        )));
    }

    let catalog = AuthoritativeBlockStateCatalog::from_sources(&selected)?;
    if catalog.output_version() != target_version {
        return Err(BedrockWorldError::Validation(format!(
            "target BlockState version {} is not an authoritative schema endpoint; nearest compiled endpoint is {}",
            target_version.raw(),
            catalog.output_version().raw()
        )));
    }
    Ok(catalog)
}

fn schema_result_version(source: &BlockStateSchemaSource<'_>) -> Result<BlockStateStorageVersion> {
    let value: serde_json::Value = serde_json::from_str(source.json).map_err(|error| {
        BedrockWorldError::Validation(format!(
            "invalid BlockState schema {} while selecting target: {error}",
            source.name
        ))
    })?;
    let root = value.as_object().ok_or_else(|| {
        BedrockWorldError::Validation(format!(
            "BlockState schema {} root must be an object",
            source.name
        ))
    })?;
    let component = |name: &str| -> Result<u8> {
        let raw = root
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                BedrockWorldError::Validation(format!(
                    "BlockState schema {} field {name} must be an unsigned integer",
                    source.name
                ))
            })?;
        u8::try_from(raw).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "BlockState schema {} field {name} exceeds u8",
                source.name
            ))
        })
    };

    Ok(BlockStateStorageVersion::from_components(
        component("maxVersionMajor")?,
        component("maxVersionMinor")?,
        component("maxVersionPatch")?,
        component("maxVersionRevision")?,
    ))
}

fn validate_pinned_sources(sources: &[BlockStateSchemaSource<'_>]) -> Result<()> {
    if sources.len() != PINNED_BLOCK_STATE_SCHEMA_FILES.len() {
        return Err(BedrockWorldError::Validation(format!(
            "pinned BlockState corpus requires {} schemas, got {}",
            PINNED_BLOCK_STATE_SCHEMA_FILES.len(),
            sources.len()
        )));
    }
    let expected = PINNED_BLOCK_STATE_SCHEMA_FILES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let supplied = sources.iter().map(|source| source.name).collect::<BTreeSet<_>>();
    if supplied.len() != sources.len() {
        return Err(BedrockWorldError::Validation(
            "pinned BlockState corpus contains duplicate filenames".to_string(),
        ));
    }
    if supplied != expected {
        let missing = expected.difference(&supplied).copied().collect::<Vec<_>>();
        let unexpected = supplied.difference(&expected).copied().collect::<Vec<_>>();
        return Err(BedrockWorldError::Validation(format!(
            "pinned BlockState corpus mismatch; missing={missing:?}, unexpected={unexpected:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockState;
    use std::collections::BTreeMap;

    #[test]
    fn manifest_is_strictly_schema_id_sorted() {
        let ids = PINNED_BLOCK_STATE_SCHEMA_FILES
            .iter()
            .map(|name| {
                name.split_once('_')
                    .expect("schema filename prefix")
                    .0
                    .parse::<u32>()
                    .expect("numeric schema prefix")
            })
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 34);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn target_bound_catalog_stops_at_requested_endpoint() {
        let first = r#"{
            "maxVersionMajor":1,"maxVersionMinor":12,"maxVersionPatch":0,"maxVersionRevision":1,
            "renamedIds":{"minecraft:old":"minecraft:middle"}
        }"#;
        let second = r#"{
            "maxVersionMajor":1,"maxVersionMinor":13,"maxVersionPatch":0,"maxVersionRevision":1,
            "renamedIds":{"minecraft:middle":"minecraft:new"}
        }"#;
        let sources = [
            BlockStateSchemaSource {
                name: "0011_test.json",
                json: first,
            },
            BlockStateSchemaSource {
                name: "0021_test.json",
                json: second,
            },
        ];
        let target = BlockStateStorageVersion::from_components(1, 12, 0, 1);
        let catalog = load_catalog_for_target(&sources, target).expect("target-bound catalog");
        let output = catalog
            .upgrade(&BlockState {
                name: "minecraft:old".to_string(),
                states: BTreeMap::new(),
                version: Some(BlockStateStorageVersion::from_components(1, 10, 0, 0).raw()),
            })
            .expect("upgrade to historical endpoint");
        assert_eq!(catalog.output_version(), target);
        assert_eq!(output.version, Some(target.raw()));
        assert_eq!(output.name, "minecraft:middle");
    }
}
