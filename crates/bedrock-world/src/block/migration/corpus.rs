//! Pinned authoritative Bedrock block migration corpus metadata.
//!
//! The corpus is supplied as immutable application resources rather than fetched at runtime. This
//! manifest pins the exact PocketMine revision and validates that destructive migration never runs
//! against a partial, duplicated, or accidentally mixed schema set.

use super::{AuthoritativeBlockStateCatalog, BlockStateSchemaSource};
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

/// Validates and loads a complete pinned schema set.
///
/// Input ordering does not matter, but the filename set must match
/// [`PINNED_BLOCK_STATE_SCHEMA_FILES`] exactly. This prevents a server from performing destructive
/// migration with a partially updated resource bundle.
pub fn load_pinned_block_state_catalog(
    sources: &[BlockStateSchemaSource<'_>],
) -> Result<AuthoritativeBlockStateCatalog> {
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
    AuthoritativeBlockStateCatalog::from_sources(sources)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
