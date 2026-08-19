//! Exact older-Modern saved-item target built from item, block and item<->block version evidence.

use super::{
    NamedSavedItemId, SavedItemVersionMatch, SavedItemVersionTarget, VanillaSavedItemBlockMap,
    VanillaSavedItemBlockMatch,
};
use crate::block::version::{BlockStateVersionMatch, BlockStateVersionTarget};
use crate::chunk::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;

/// Exact result of matching one source Modern saved item against a concrete older Modern release.
#[derive(Debug, Clone, PartialEq)]
pub enum ModernSavedItemTargetMatch {
    /// The source item has no authoritative item identity in the target version.
    MissingItem,
    /// More than one target item identity maps back to the source item.
    AmbiguousItem {
        /// First candidate target item identity.
        first: NamedSavedItemId,
        /// Second candidate target item identity.
        second: NamedSavedItemId,
        /// Number of matching target candidates discovered by the reverse index.
        matches: usize,
    },
    /// The source item is a normal non-block item and has one exact target item identity.
    Item {
        /// Exact target item identity including name and metadata.
        item: NamedSavedItemId,
    },
    /// The source item carried a `Block` payload but the target item is not a block item.
    UnexpectedSourceBlock {
        /// Target item identity that does not expect a block payload.
        item: NamedSavedItemId,
    },
    /// The target item maps to more than one possible historical block identifier.
    AmbiguousTargetBlockItem {
        /// Target item identity that requires a block payload.
        item: NamedSavedItemId,
        /// First candidate target block identifier.
        first_block: String,
        /// Second candidate target block identifier.
        second_block: String,
        /// Number of block candidates discovered by the item/block map.
        matches: usize,
    },
    /// The target item is a block item but the source item has no `Block` payload to reverse.
    SourceBlockRequired {
        /// Target item identity that requires a block payload.
        item: NamedSavedItemId,
        /// Target block identifier required by the target item.
        target_block_id: String,
    },
    /// The source `Block` payload has no proven BlockState representation in the target version.
    MissingBlockState {
        /// Target item identity associated with the missing block state.
        item: NamedSavedItemId,
        /// Target block identifier required by the target item.
        target_block_id: String,
    },
    /// More than one target BlockState can represent the source `Block` payload.
    AmbiguousBlockState {
        /// Target item identity associated with the ambiguous block state.
        item: NamedSavedItemId,
        /// Target block identifier required by the target item.
        target_block_id: String,
        /// First candidate target BlockState.
        first: BlockState,
        /// Second candidate target BlockState.
        second: BlockState,
        /// Number of BlockState candidates discovered by the reverse index.
        matches: usize,
    },
    /// The proven BlockState resolves to a different block identifier than the target item expects.
    BlockIdentityMismatch {
        /// Target item identity associated with the mismatched block state.
        item: NamedSavedItemId,
        /// Block identifier required by the target item/block map.
        expected_block_id: String,
        /// Block identifier produced by the BlockState reverse mapping.
        actual_block_id: String,
        /// Proven target BlockState whose name did not match the item/block map.
        block: BlockState,
    },
    /// The source block item has one exact target item identity and target BlockState.
    BlockItem {
        /// Exact target item identity including name and metadata.
        item: NamedSavedItemId,
        /// Exact target BlockState to persist in the item `Block` payload.
        block: BlockState,
    },
}

impl ModernSavedItemTargetMatch {
    /// Returns true when the match is an exact writable item or block-item target.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Item { .. } | Self::BlockItem { .. })
    }
}

/// Reusable exact target for Modern (MCPE 1.9+) saved items in one concrete Bedrock release.
///
/// Item and BlockState reverse indices must share the same concrete source and target game versions.
/// The target block-ID -> item-ID relation must share the same target version. Blockitems never fall
/// back to item/block string-name equality.
#[derive(Debug, Clone)]
pub struct ModernSavedItemTarget {
    items: SavedItemVersionTarget,
    blocks: BlockStateVersionTarget,
    item_blocks: VanillaSavedItemBlockMap,
}

impl ModernSavedItemTarget {
    /// Builds a target from version-aligned item, BlockState and item/block evidence.
    ///
    /// All three inputs must describe the same target game version. The item and BlockState targets
    /// must also share the same source game version so source saved-item NBT and source BlockState
    /// payloads are reversed from one coherent release.
    pub fn new(
        items: SavedItemVersionTarget,
        blocks: BlockStateVersionTarget,
        item_blocks: VanillaSavedItemBlockMap,
    ) -> Result<Self> {
        let source = items.source_game_version();
        if blocks.source_game_version() != source {
            return Err(BedrockWorldError::Validation(format!(
                "Modern saved-item source version mismatch: item source {source}, BlockState source {}",
                blocks.source_game_version()
            )));
        }

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

    /// Returns the source game version accepted by this target.
    #[must_use]
    pub fn source_game_version(&self) -> &GameVersion {
        self.items.source_game_version()
    }

    /// Returns the concrete target game version this target writes.
    #[must_use]
    pub fn target_game_version(&self) -> &GameVersion {
        self.items.target_game_version()
    }

    /// Returns the source BlockState storage version accepted by the BlockState reverse index.
    #[must_use]
    pub const fn source_block_state_version(&self) -> crate::block::BlockStateStorageVersion {
        self.blocks.source_storage_version()
    }

    /// Returns the target BlockState storage version emitted by the BlockState reverse index.
    #[must_use]
    pub const fn target_block_state_version(&self) -> crate::block::BlockStateStorageVersion {
        self.blocks.target_storage_version()
    }

    /// Matches one source Modern saved item to this concrete older target release.
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
