//! Minecraft Bedrock vanilla BlockState palette for one concrete game release.
//!
//! A target palette is used by exact downgrades to prove that a semantic BlockState existed in the
//! requested older game and to recover the persisted BlockState `version` used by that target.

use super::BlockStateStorageVersion;
use crate::chunk::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use std::collections::BTreeMap;

/// Complete vanilla BlockState set for one concrete Minecraft Bedrock game version.
///
/// Entries are grouped by block identifier for lookup by `&str`. Semantic states inside each group are
/// retained in canonical-byte order, giving deterministic iteration and ambiguity reporting across
/// processes and platforms. Storage `version` is ignored by semantic identity but retained in values.
#[derive(Debug, Clone)]
pub struct VanillaBlockStatePalette {
    game_version: GameVersion,
    storage_version: BlockStateStorageVersion,
    states: BTreeMap<String, Vec<BlockState>>,
    len: usize,
}

impl VanillaBlockStatePalette {
    /// Builds a target-game vanilla palette from exact BlockState entries.
    ///
    /// Every entry must carry the same persisted BlockState version. Duplicate semantic states are
    /// rejected because an exact downgrade must have one unambiguous target representation.
    pub fn new(game_version: GameVersion, states: Vec<BlockState>) -> Result<Self> {
        if states.is_empty() {
            return Err(BedrockWorldError::Validation(
                "vanilla BlockState palette cannot be empty".to_string(),
            ));
        }

        let first_version = states[0].version.ok_or_else(|| {
            BedrockWorldError::Validation(format!(
                "vanilla BlockState palette entry {} has no storage version",
                states[0].name
            ))
        })?;
        let storage_version = BlockStateStorageVersion::from_raw(first_version);
        let mut canonical = BTreeMap::<String, BTreeMap<Vec<u8>, BlockState>>::new();
        let mut len = 0usize;

        for state in states {
            let version = state.version.ok_or_else(|| {
                BedrockWorldError::Validation(format!(
                    "vanilla BlockState palette entry {} has no storage version",
                    state.name
                ))
            })?;
            if version != first_version {
                return Err(BedrockWorldError::Validation(format!(
                    "vanilla BlockState palette mixes storage versions {first_version} and {version}"
                )));
            }

            let key = state.canonical_bytes()?;
            let block_name = state.name.clone();
            if canonical
                .entry(block_name)
                .or_default()
                .insert(key, state)
                .is_some()
            {
                return Err(BedrockWorldError::Validation(
                    "vanilla BlockState palette contains duplicate semantic state".to_string(),
                ));
            }
            len = len.saturating_add(1);
        }

        let states = canonical
            .into_iter()
            .map(|(name, states)| (name, states.into_values().collect()))
            .collect();
        Ok(Self {
            game_version,
            storage_version,
            states,
            len,
        })
    }

    /// Returns the exact Minecraft Bedrock game version represented by this palette.
    #[must_use]
    pub fn game_version(&self) -> &GameVersion {
        &self.game_version
    }

    /// Returns the persisted BlockState version used by every target palette entry.
    #[must_use]
    pub const fn storage_version(&self) -> BlockStateStorageVersion {
        self.storage_version
    }

    /// Returns the number of semantic BlockState entries in this target palette.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the target palette contains no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterates every exact target-palette state in deterministic semantic order without allocating.
    pub fn states(&self) -> impl Iterator<Item = &BlockState> {
        self.states.values().flat_map(|states| states.iter())
    }

    /// Finds the exact target-game representation of a semantic BlockState.
    ///
    /// The source state's persisted `version` is ignored for matching. A successful result therefore
    /// provides both proof that the state existed in the target game and the exact target storage
    /// version that should be written.
    #[must_use]
    pub fn target_state(&self, source: &BlockState) -> Option<&BlockState> {
        self.states
            .get(source.name.as_str())?
            .iter()
            .find(|target| target.states == source.states)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtTag;
    use std::collections::BTreeMap;

    fn state(name: &str, facing: i32, version: i32) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::from([("facing_direction".to_string(), NbtTag::Int(facing))]),
            version: Some(version),
        }
    }

    #[test]
    fn target_lookup_ignores_source_storage_version() {
        let target = state("minecraft:test", 2, 17_000_001);
        let palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 17, 40]).unwrap(),
            vec![target.clone()],
        )
        .unwrap();
        let source = state("minecraft:test", 2, 18_168_865);
        assert_eq!(palette.target_state(&source), Some(&target));
        assert_eq!(palette.storage_version().raw(), 17_000_001);
    }

    #[test]
    fn target_lookup_refuses_unavailable_permutation() {
        let palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 17, 40]).unwrap(),
            vec![state("minecraft:test", 2, 17_000_001)],
        )
        .unwrap();
        assert!(
            palette
                .target_state(&state("minecraft:test", 3, 18_168_865))
                .is_none()
        );
    }

    #[test]
    fn state_iterator_is_canonical_and_deterministic() {
        let palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 17, 40]).unwrap(),
            vec![
                state("minecraft:second", 0, 17_000_001),
                state("minecraft:first", 1, 17_000_001),
                state("minecraft:first", 0, 17_000_001),
            ],
        )
        .unwrap();
        let order = palette
            .states()
            .map(|state| {
                (
                    state.name.as_str(),
                    state.states["facing_direction"].clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(order[0].0, "minecraft:first");
        assert_eq!(order[1].0, "minecraft:first");
        assert_eq!(order[2].0, "minecraft:second");
    }

    #[test]
    fn mixed_target_storage_versions_are_rejected() {
        assert!(
            VanillaBlockStatePalette::new(
                GameVersion::new(vec![1, 17, 40]).unwrap(),
                vec![
                    state("minecraft:first", 0, 17_000_001),
                    state("minecraft:second", 0, 17_000_002),
                ],
            )
            .is_err()
        );
    }
}
