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
    /// Exact ordinary item with no persisted target BlockState.
    Item { item: NamedSavedItemId },
    /// Source carries `Block`, but the target item is not a blockitem.
    UnexpectedSourceBlock { item: NamedSavedItemId },
    /// Target item->block data maps one item to multiple block identifiers.
    AmbiguousTargetBlockItem {
        item: NamedSavedItemId,
        first_block: String,
        second_block: String,
        matches: usize,
    },
    /// Target is a blockitem but source Modern data has no persisted `Block` payload.
    SourceBlockRequired {
        item: NamedSavedItemId,
        target_block_id: String,
    },
    /// Source BlockState has no representation in the target vanilla block palette.
    MissingBlockState {
        item: NamedSavedItemId,
        target_block_id: String,
    },
    /// Multiple target BlockStates converge to the source BlockState.
    AmbiguousBlockState {
        item: NamedSavedItemId,
        target_block_id: String,
        first: BlockState,
        second: BlockState,
        matches: usize,
    },
    /// Item->block mapping and reversed target BlockState disagree on block identity.
    BlockIdentityMismatch {
        item: NamedSavedItemId,
        expected_block_id: String,
        actual_block_id: String,
        block: BlockState,
    },
    /// Target item and target BlockState are both unique and mutually consistent.
    BlockItem {
        item: NamedSavedItemId,
        block: BlockState,
    },
}

impl ModernSavedItemTargetMatch {
    /// Returns whether this result can be written without choosing aliases or inventing data.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Item { .. } | Self::BlockItem { .. })
    }
}

/// Reusable exact target for Modern (MCPE 1.9+) saved items in one concrete Bedrock release.
///
/// The three target data sets must describe the same `GameVersion`: complete item registry reverse
/// index, complete vanilla BlockState reverse index, and generated block-ID -> item-ID relation.
/// Blockitems never fall back to comparing item and block names for equality.
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

    /// Target BlockState storage endpoint retained by returned target states.
    #[must_use]
    pub const fn target_block_state_version(&self) -> crate::block::BlockStateStorageVersion {
        self.blocks.target_storage_version()
    }

    /// Matches one source Modern saved item to this concrete older target release.
    ///
    /// `source_block` is the parsed persisted `Block` BlockState when present. A target blockitem
    /// requires it; a target ordinary item rejects it. Block identity is validated through the
    /// generated target block->item relation rather than inferred from string-name equality.
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
