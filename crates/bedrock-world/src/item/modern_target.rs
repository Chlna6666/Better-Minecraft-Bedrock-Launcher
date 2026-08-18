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
    MissingItem,
    AmbiguousItem {
        first: NamedSavedItemId,
        second: NamedSavedItemId,
        matches: usize,
    },
    Item { item: NamedSavedItemId },
    UnexpectedSourceBlock { item: NamedSavedItemId },
    AmbiguousTargetBlockItem {
        item: NamedSavedItemId,
        first_block: String,
        second_block: String,
        matches: usize,
    },
    SourceBlockRequired {
        item: NamedSavedItemId,
        target_block_id: String,
    },
    MissingBlockState {
        item: NamedSavedItemId,
        target_block_id: String,
    },
    AmbiguousBlockState {
        item: NamedSavedItemId,
        target_block_id: String,
        first: BlockState,
        second: BlockState,
        matches: usize,
    },
    BlockIdentityMismatch {
        item: NamedSavedItemId,
        expected_block_id: String,
        actual_block_id: String,
        block: BlockState,
    },
    BlockItem {
        item: NamedSavedItemId,
        block: BlockState,
    },
}

impl ModernSavedItemTargetMatch {
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

    #[must_use]
    pub fn source_game_version(&self) -> &GameVersion {
        self.items.source_game_version()
    }

    #[must_use]
    pub fn target_game_version(&self) -> &GameVersion {
        self.items.target_game_version()
    }

    #[must_use]
    pub const fn source_block_state_version(&self) -> crate::block::BlockStateStorageVersion {
        self.blocks.source_storage_version()
    }

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
