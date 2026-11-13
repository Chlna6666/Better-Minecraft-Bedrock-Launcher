//! Forward-verified reverse BlockState target for one older Minecraft Bedrock vanilla palette.

use super::{AuthoritativeBlockStateCatalog, BlockStateStorageVersion, VanillaBlockStatePalette};
use crate::chunk::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// Result of reversing one semantic source BlockState to a concrete older vanilla palette.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockStateVersionMatch {
    /// No target-palette state forward-upgrades to the source state.
    Missing,
    /// Exactly one target-palette state represents the source state.
    Unique(BlockState),
    /// Multiple target-palette states converge to the same source semantic state.
    Ambiguous {
        /// First deterministic target state.
        first: BlockState,
        /// Second deterministic target state proving ambiguity.
        second: BlockState,
        /// Total matching target states.
        matches: usize,
    },
}

impl BlockStateVersionMatch {
    /// Returns the target state only when the reverse representation is unique.
    #[must_use]
    pub fn unique(self) -> Option<BlockState> {
        match self {
            Self::Unique(value) => Some(value),
            Self::Missing | Self::Ambiguous { .. } => None,
        }
    }
}

/// Reusable reverse index from one concrete source Bedrock release to one older vanilla palette.
///
/// Every target state is upgraded once through the source-bound authoritative catalog. The upgraded
/// semantic state becomes the reverse key, while the retained value is the exact target-palette state
/// including its persisted target storage version. `source_game_version` is explicit because a packed
/// BlockState storage version does not uniquely identify a Minecraft game release.
#[derive(Debug, Clone)]
pub struct BlockStateVersionTarget {
    source_game_version: GameVersion,
    source_storage_version: BlockStateStorageVersion,
    target_storage_version: BlockStateStorageVersion,
    target_game_version: GameVersion,
    by_source: BTreeMap<Vec<u8>, ReverseEntry>,
    target_states: usize,
}

impl BlockStateVersionTarget {
    /// Builds a proof-by-forward-execution reverse target.
    ///
    /// The caller binds the authoritative source catalog to the concrete source game release. The
    /// target palette must not represent a newer game or a newer BlockState storage version.
    pub fn build(
        source_game_version: GameVersion,
        source_catalog: &AuthoritativeBlockStateCatalog,
        target_palette: &VanillaBlockStatePalette,
    ) -> Result<Self> {
        if compare_release(target_palette.game_version(), &source_game_version) == Ordering::Greater
        {
            return Err(BedrockWorldError::Validation(format!(
                "BlockState target game version {} is newer than source {}",
                target_palette.game_version(),
                source_game_version
            )));
        }

        let source_storage_version = source_catalog.output_version();
        let target_storage_version = target_palette.storage_version();
        if target_storage_version > source_storage_version {
            return Err(BedrockWorldError::Validation(format!(
                "BlockState target storage version {} is newer than source endpoint {}",
                target_storage_version.raw(),
                source_storage_version.raw()
            )));
        }

        let mut by_source = BTreeMap::<Vec<u8>, ReverseEntry>::new();
        let mut target_states = 0usize;
        for target in target_palette.states() {
            let source = source_catalog.upgrade(target)?;
            if source.version != Some(source_storage_version.raw()) {
                return Err(BedrockWorldError::Validation(format!(
                    "BlockState catalog returned version {:?}, expected {} for target {}",
                    source.version,
                    source_storage_version.raw(),
                    target.name
                )));
            }
            by_source
                .entry(source.canonical_bytes()?)
                .and_modify(|entry| entry.push(target.clone()))
                .or_insert_with(|| ReverseEntry::new(target.clone()));
            target_states = target_states.saturating_add(1);
        }

        Ok(Self {
            source_game_version,
            source_storage_version,
            target_storage_version,
            target_game_version: target_palette.game_version().clone(),
            by_source,
            target_states,
        })
    }

    /// Concrete source Minecraft Bedrock release used to interpret source BlockStates.
    #[must_use]
    pub fn source_game_version(&self) -> &GameVersion {
        &self.source_game_version
    }

    /// Source BlockState storage endpoint used for forward verification.
    #[must_use]
    pub const fn source_storage_version(&self) -> BlockStateStorageVersion {
        self.source_storage_version
    }

    /// Target BlockState storage version retained by returned target states.
    #[must_use]
    pub const fn target_storage_version(&self) -> BlockStateStorageVersion {
        self.target_storage_version
    }

    /// Concrete Minecraft Bedrock release represented by the target palette.
    #[must_use]
    pub fn target_game_version(&self) -> &GameVersion {
        &self.target_game_version
    }

    /// Number of exact target-palette states indexed.
    #[must_use]
    pub const fn target_state_count(&self) -> usize {
        self.target_states
    }

    /// Finds exact target-palette candidates for one source semantic BlockState.
    ///
    /// Source persisted `version` is ignored by canonical identity; only `name+states` are matched.
    pub fn match_state(&self, source: &BlockState) -> Result<BlockStateVersionMatch> {
        let key = source.canonical_bytes()?;
        Ok(self
            .by_source
            .get(&key)
            .map_or(BlockStateVersionMatch::Missing, ReverseEntry::as_match))
    }
}

#[derive(Debug, Clone)]
struct ReverseEntry {
    first: BlockState,
    second: Option<BlockState>,
    matches: usize,
}

impl ReverseEntry {
    fn new(first: BlockState) -> Self {
        Self {
            first,
            second: None,
            matches: 1,
        }
    }

    fn push(&mut self, state: BlockState) {
        self.matches = self.matches.saturating_add(1);
        if self.second.is_none() {
            self.second = Some(state);
        }
    }

    fn as_match(&self) -> BlockStateVersionMatch {
        match &self.second {
            None => BlockStateVersionMatch::Unique(self.first.clone()),
            Some(second) => BlockStateVersionMatch::Ambiguous {
                first: self.first.clone(),
                second: second.clone(),
                matches: self.matches,
            },
        }
    }
}

fn compare_release(left: &GameVersion, right: &GameVersion) -> Ordering {
    let len = left.components().len().max(right.components().len());
    for index in 0..len {
        let left = left.components().get(index).copied().unwrap_or(0);
        let right = right.components().get(index).copied().unwrap_or(0);
        match left.cmp(&right) {
            Ordering::Equal => {}
            order => return order,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockStateSchemaSource, BlockStateStorageVersion};
    use crate::nbt::NbtTag;
    use std::collections::BTreeMap;

    fn state(name: &str, version: i32) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::from([("variant".to_string(), NbtTag::Int(0))]),
            version: Some(version),
        }
    }

    fn catalog(json: &str) -> AuthoritativeBlockStateCatalog {
        AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_test.json",
            json,
        }])
        .unwrap()
    }

    #[test]
    fn forward_rename_recovers_exact_target_state() {
        let target_version = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        let source_version = BlockStateStorageVersion::from_components(1, 13, 0, 1).raw();
        let source_catalog = catalog(
            r#"{"maxVersionMajor":1,"maxVersionMinor":13,"maxVersionPatch":0,"maxVersionRevision":1,"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
        );
        let target_palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 12, 0]).unwrap(),
            vec![state("minecraft:old", target_version)],
        )
        .unwrap();
        let reverse = BlockStateVersionTarget::build(
            GameVersion::new(vec![1, 13, 0]).unwrap(),
            &source_catalog,
            &target_palette,
        )
        .unwrap();
        assert_eq!(reverse.source_game_version().components(), &[1, 13, 0]);
        assert_eq!(
            reverse
                .match_state(&state("minecraft:new", source_version))
                .unwrap()
                .unique(),
            Some(state("minecraft:old", target_version))
        );
    }

    #[test]
    fn converging_target_aliases_are_ambiguous() {
        let target_version = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        let source_version = BlockStateStorageVersion::from_components(1, 13, 0, 1).raw();
        let source_catalog = catalog(
            r#"{"maxVersionMajor":1,"maxVersionMinor":13,"maxVersionPatch":0,"maxVersionRevision":1,"renamedIds":{"minecraft:first":"minecraft:new","minecraft:second":"minecraft:new"}}"#,
        );
        let target_palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 12, 0]).unwrap(),
            vec![
                state("minecraft:second", target_version),
                state("minecraft:first", target_version),
            ],
        )
        .unwrap();
        assert!(matches!(
            BlockStateVersionTarget::build(
                GameVersion::new(vec![1, 13, 0]).unwrap(),
                &source_catalog,
                &target_palette,
            )
            .unwrap()
            .match_state(&state("minecraft:new", source_version))
            .unwrap(),
            BlockStateVersionMatch::Ambiguous { matches: 2, .. }
        ));
    }

    #[test]
    fn unavailable_target_state_is_missing() {
        let target_version = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        let source_version = BlockStateStorageVersion::from_components(1, 13, 0, 1).raw();
        let source_catalog = catalog(
            r#"{"maxVersionMajor":1,"maxVersionMinor":13,"maxVersionPatch":0,"maxVersionRevision":1}"#,
        );
        let target_palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 12, 0]).unwrap(),
            vec![state("minecraft:stone", target_version)],
        )
        .unwrap();
        assert_eq!(
            BlockStateVersionTarget::build(
                GameVersion::new(vec![1, 13, 0]).unwrap(),
                &source_catalog,
                &target_palette,
            )
            .unwrap()
            .match_state(&state("minecraft:future", source_version))
            .unwrap(),
            BlockStateVersionMatch::Missing
        );
    }

    #[test]
    fn newer_target_game_version_is_rejected_even_if_storage_version_is_older() {
        let target_version = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        let source_catalog = catalog(
            r#"{"maxVersionMajor":1,"maxVersionMinor":13,"maxVersionPatch":0,"maxVersionRevision":1}"#,
        );
        let target_palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 14, 0]).unwrap(),
            vec![state("minecraft:stone", target_version)],
        )
        .unwrap();
        assert!(
            BlockStateVersionTarget::build(
                GameVersion::new(vec![1, 13, 0]).unwrap(),
                &source_catalog,
                &target_palette,
            )
            .is_err()
        );
    }
}
