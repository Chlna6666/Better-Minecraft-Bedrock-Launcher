//! Exact older-Modern saved-item target built from item, block and item<->block version evidence.

use super::{
    NamedSavedItemId, SavedItemVersionMatch, SavedItemVersionTarget, VanillaSavedItemBlockMap,
    VanillaSavedItemBlockMatch,
};
use crate::block::{BlockStateVersionMatch, BlockStateVersionTarget};
use crate::chunk::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;

/// Exact result of matching one source Modern saved item against a concrete older Modern release.
#[derive(Debug, Clone, PartialEq)]
pub enum ModernSavedItemTargetMatch {
    /// No target-release item identity forward-resolves to the source item.
    MissingItem,
    /// Multiple target-release item identities converge to the source item.
    AmbiguousItem {
        first: NamedSavedItemId,
        second: NamedSavedItemId,
        matches: usize,
    },
    /// The target item is a normal non-block item and the source correctly has no persisted `Block`.
    Item { item: NamedSavedItemId },
    /// The target item is not a blockitem, but the source carries a persisted BlockState.
    UnexpectedSourceBlock { item: NamedSavedItemId },
    /// Target item->block data has multiple block IDs for one target item ID.
    AmbiguousTargetBlockItem {
        item: NamedSavedItemId,
        first_block: String,
        second_block: String,
        matches: usize,
    },
    /// The target item is a blockitem, but the source has no persisted Modern `Block` payload.
    SourceBlockRequired {
        item: NamedSavedItemId,
        target_block_id: String,
    },
    /// No target-palette BlockState forward-upgrades to the source BlockState.
    MissingBlockState {
        item: NamedSavedItemId,
        target_block_id: String,
    },
    /// Multiple target-palette BlockStates converge to the source BlockState.
    AmbiguousBlockState {
        item: NamedSavedItemId,
        target_block_id: String,
        first: BlockState,
        second: BlockState,
        matches: usize,
    },
    /// Item->block mapping and BlockState reverse lookup disagree on the target block identifier.
    BlockIdentityMismatch {
        item: NamedSavedItemId,
        expected_block_id: String,
        actual_block_id: String,
        block: BlockState,
    },
    /// Both target item and exact target BlockState are uniquely proven and mutually consistent.
    BlockItem {
        item: NamedSavedItemId,
        block: BlockState,
    },
}

impl ModernSavedItemTargetMatch {
    /// Returns whether this match is safe to write without choosing an alias or inventing data.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        matches!(Self::Item { .. } | Self::BlockItem { .. }, self)
    }
}

/// Reusable exact target for Modern (MCPE 1.9+) saved items in one concrete Bedrock release.
///
/// The three constituent data sets must describe the same target `GameVersion`:
/// - complete target item registry reverse index,
/// - complete target vanilla BlockState reverse index,
/// - generated target block-ID -> item-ID relation.
///
/// No string-name equality fallback is used for blockitems.
#[derive(Debug, Clone)]
pub struct ModernSavedItemTarget {
    items: SavedItemVersionTarget,
    blocks: BlockStateVersionTarget,
    item_blocks: VanillaSavedItemBlockMap,
}

impl ModernSavedItemTarget {
    /// Combines target-version item, block and item<->block evidence.
    pub fn new(
        items: SavedItemVersionTarget,
        blocks: BlockStateVersionTarget,
        item_blocks: VanillaSavedItemBlockMap,
    ) -> Result<Self> {
        let target = items.target_game_version();
        if blocks.target_game_version() != target {
            return Err(BedrockWorldError::Validation(format!(
                "Modern saved-item target version mismatch: item target {target}, BlockState target {}",
                blocks.target_game_version()
            )));
        }
        if item_blocks.game_version() != target {
            return Err(BedrockWorldError::Validation(format!(
                "Modern saved-item target version mismatch: item target {target}, item/block map {}",
                item_blocks.game_version()
            )));
        }
        if !is_modern_game_version(target) {
            return Err(BedrockWorldError::Validation(format!(
                "Modern saved-item target requires Bedrock 1.9.0 or newer, got {target}"
            )));
        }
        Ok(Self {
            items,
            blocks,
            item_blocks,
        })
    }

    /// Source game version used by the item-version reverse index.
    #[must_use]
    pub fn source_game_version(&self) -> &GameVersion {
        self.items.source_game_version()
    }

    /// Concrete target game version shared by all target evidence.
    #[must_use]
    pub fn target_game_version(&self) -> &GameVersion {
        self.items.target_game_version()
    }

    /// Source BlockState storage endpoint used by the block reverse index.
    #[must_use]
    pub const fn source_block_state_version(&self) -> crate::block::BlockStateStorageVersion {
        self.blocks.source_storage_version()
    }

    /// Target BlockState storage endpoint retained by target block states.
    #[must_use]
    pub const fn target_block_state_version(&self) -> crate::block::BlockStateStorageVersion {
        self.blocks.target_storage_version()
    }

    /// Matches one source Modern saved item to this concrete older target release.
    ///
    /// `source_block` is the parsed persisted `Block` BlockState when present. A target blockitem
    /// requires it; a target non-block item rejects it. Block identity is validated against the
    /// generated target block->item relation, never by assuming equal item/block names.
    pub fn match_item(
        &self,
        source_item: &NamedSavedItemId,
        source_block: Option<&BlockState>,
    ) -> Result<ModernSavedItemTargetMatch> {
        let item = match self.items.match_item(source_item) {
            SavedItemVersionMatch::Missing => return Ok(ModernSavedItemTargetMatch::MissingItem),
            SavedItemVersionMatch::Ambiguous {
                first,
                second,
                matches,
            } => {
                return Ok(ModernSavedItemTargetMatch::AmbiguousItem {
                    first,
                    second,
                    matches,
                });
            }
            SavedItemVersionMatch::Unique(item) => item,
        };

        match self.item_blocks.block_id_for_item(&item.name) {
            VanillaSavedItemBlockMatch::None => {
                if source_block.is_some() {
                    Ok(ModernSavedItemTargetMatch::UnexpectedSourceBlock { item })
                } else {
                    Ok(ModernSavedItemTargetMatch::Item { item })
                }
            }
            VanillaSavedItemBlockMatch::Ambiguous {
                first,
                second,
                matches,
            } => Ok(ModernSavedItemTargetMatch::AmbiguousTargetBlockItem {
                item,
                first_block: first.to_string(),
                second_block: second.to_string(),
                matches,
            }),
            VanillaSavedItemBlockMatch::Unique(target_block_id) => {
                let target_block_id = target_block_id.to_string();
                let Some(source_block) = source_block else {
                    return Ok(ModernSavedItemTargetMatch::SourceBlockRequired {
                        item,
                        target_block_id,
                    });
                };
                match self.blocks.match_state(source_block)? {
                    BlockStateVersionMatch::Missing => {
                        Ok(ModernSavedItemTargetMatch::MissingBlockState {
                            item,
                            target_block_id,
                        })
                    }
                    BlockStateVersionMatch::Ambiguous {
                        first,
                        second,
                        matches,
                    } => Ok(ModernSavedItemTargetMatch::AmbiguousBlockState {
                        item,
                        target_block_id,
                        first,
                        second,
                        matches,
                    }),
                    BlockStateVersionMatch::Unique(block) => {
                        if block.name != target_block_id {
                            Ok(ModernSavedItemTargetMatch::BlockIdentityMismatch {
                                item,
                                expected_block_id: target_block_id,
                                actual_block_id: block.name.clone(),
                                block,
                            })
                        } else {
                            Ok(ModernSavedItemTargetMatch::BlockItem { item, block })
                        }
                    }
                }
            }
        }
    }
}

fn is_modern_game_version(version: &GameVersion) -> bool {
    let components = version.components();
    match components.first().copied().unwrap_or(0).cmp(&1) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => components.get(1).copied().unwrap_or(0) >= 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{
        AuthoritativeBlockStateCatalog, BlockStateSchemaSource, VanillaBlockStatePalette,
    };
    use crate::item::{SavedItemUpgradeSource, SavedItemVersionTable, VanillaSavedItemPalette};
    use crate::nbt::NbtTag;
    use std::collections::BTreeMap;

    fn block(name: &str, version: i32) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::from([("variant".to_string(), NbtTag::Int(0))]),
            version: Some(version),
        }
    }

    fn target() -> ModernSavedItemTarget {
        let item_rules = SavedItemVersionTable::from_sources(&[
            SavedItemUpgradeSource {
                name: "0001_1.6_beta_to_1.6.0.json",
                json: "{}",
            },
            SavedItemUpgradeSource {
                name: "0011_1.11.4_to_1.12.0.json",
                json: r#"{"renamedIds":{"minecraft:old_door":"minecraft:new_door"}}"#,
            },
        ])
        .unwrap();
        let item_palette = VanillaSavedItemPalette::from_required_item_list_json(
            GameVersion::new(vec![1, 9, 0]).unwrap(),
            r#"{
                "minecraft:item.door":{"runtime_id":1},
                "minecraft:apple":{"runtime_id":2}
            }"#,
        )
        .unwrap();
        // Source item identity is already the same for the door in this compact test; the mapping
        // intentionally demonstrates that target block and item names need not match.
        let items = item_rules
            .older_target(&GameVersion::new(vec![1, 12, 0]).unwrap(), &item_palette)
            .unwrap();

        let target_block_version = crate::block::BlockStateStorageVersion::from_components(1, 9, 0, 1).raw();
        let block_catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_test.json",
            json: r#"{"maxVersionMajor":1,"maxVersionMinor":12,"maxVersionPatch":0,"maxVersionRevision":1}"#,
        }])
        .unwrap();
        let block_palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 9, 0]).unwrap(),
            vec![block("minecraft:door", target_block_version)],
        )
        .unwrap();
        let blocks = BlockStateVersionTarget::build(&block_catalog, &block_palette).unwrap();
        let item_blocks = VanillaSavedItemBlockMap::from_block_id_to_item_id_map_json(
            GameVersion::new(vec![1, 9, 0]).unwrap(),
            r#"{"minecraft:door":"minecraft:item.door"}"#,
        )
        .unwrap();
        ModernSavedItemTarget::new(items, blocks, item_blocks).unwrap()
    }

    #[test]
    fn non_block_item_is_exact_without_source_block() {
        assert!(matches!(
            target()
                .match_item(
                    &NamedSavedItemId {
                        name: "minecraft:apple".to_string(),
                        meta: 0,
                    },
                    None,
                )
                .unwrap(),
            ModernSavedItemTargetMatch::Item { .. }
        ));
    }

    #[test]
    fn target_blockitem_requires_source_block_payload() {
        assert!(matches!(
            target()
                .match_item(
                    &NamedSavedItemId {
                        name: "minecraft:item.door".to_string(),
                        meta: 0,
                    },
                    None,
                )
                .unwrap(),
            ModernSavedItemTargetMatch::SourceBlockRequired { .. }
        ));
    }

    #[test]
    fn different_target_item_and_block_names_are_proven_by_mapping() {
        let target = target();
        let source_block_version = target.source_block_state_version().raw();
        assert!(matches!(
            target
                .match_item(
                    &NamedSavedItemId {
                        name: "minecraft:item.door".to_string(),
                        meta: 0,
                    },
                    Some(&block("minecraft:door", source_block_version)),
                )
                .unwrap(),
            ModernSavedItemTargetMatch::BlockItem { item, block }
                if item.name == "minecraft:item.door" && block.name == "minecraft:door"
        ));
    }

    #[test]
    fn target_evidence_versions_must_match() {
        let target = target();
        assert_eq!(target.target_game_version().components(), &[1, 9, 0]);
    }
}
