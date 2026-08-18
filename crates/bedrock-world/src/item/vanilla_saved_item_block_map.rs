//! Version-bound vanilla saved-item <-> block identifier mapping for one Bedrock release.
//!
//! BedrockData's `block_id_to_item_id_map.json` is generated from a real game release. It records the
//! block string ID and the item string ID used when that block is saved in inventories. The two names
//! are not assumed equal: doors, beds and other blockitems may intentionally use different IDs.

use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use std::collections::BTreeMap;

/// Result of asking which target block identifier belongs to one target saved-item identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VanillaSavedItemBlockMatch<'a> {
    /// The item identifier is not a blockitem in this target release.
    None,
    /// Exactly one block identifier is associated with the item.
    Unique(&'a str),
    /// More than one block identifier maps to the same item identifier.
    ///
    /// Callers performing exact reverse writes must not pick one implicitly.
    Ambiguous {
        /// First block ID in deterministic lexical order.
        first: &'a str,
        /// Second block ID proving ambiguity.
        second: &'a str,
        /// Total block IDs associated with this item.
        matches: usize,
    },
}

/// Complete vanilla block-ID -> item-ID relation for one concrete Minecraft Bedrock release.
#[derive(Debug, Clone)]
pub struct VanillaSavedItemBlockMap {
    game_version: GameVersion,
    block_to_item: BTreeMap<String, String>,
    item_to_blocks: BTreeMap<String, Vec<String>>,
}

impl VanillaSavedItemBlockMap {
    /// Parses one release's `block_id_to_item_id_map.json`.
    pub fn from_block_id_to_item_id_map_json(
        game_version: GameVersion,
        json: &str,
    ) -> Result<Self> {
        let block_to_item: BTreeMap<String, String> = serde_json::from_str(json).map_err(|error| {
            BedrockWorldError::Validation(format!(
                "invalid Bedrock block_id_to_item_id_map.json for {game_version}: {error}"
            ))
        })?;
        if block_to_item.is_empty() {
            return Err(BedrockWorldError::Validation(format!(
                "Bedrock block/item map for {game_version} is empty"
            )));
        }

        let mut item_to_blocks = BTreeMap::<String, Vec<String>>::new();
        for (block, item) in &block_to_item {
            validate_identifier(block, "block", &game_version)?;
            validate_identifier(item, "item", &game_version)?;
            item_to_blocks
                .entry(item.clone())
                .or_default()
                .push(block.clone());
        }
        for blocks in item_to_blocks.values_mut() {
            blocks.sort();
            blocks.dedup();
        }

        Ok(Self {
            game_version,
            block_to_item,
            item_to_blocks,
        })
    }

    /// Returns the exact Minecraft Bedrock release represented by this mapping.
    #[must_use]
    pub fn game_version(&self) -> &GameVersion {
        &self.game_version
    }

    /// Returns the number of block identifiers present in the source mapping.
    #[must_use]
    pub fn len(&self) -> usize {
        self.block_to_item.len()
    }

    /// Returns whether this target mapping contains no blockitems.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.block_to_item.is_empty()
    }

    /// Returns the target saved-item identifier associated with one target block identifier.
    #[must_use]
    pub fn item_id_for_block(&self, block_id: &str) -> Option<&str> {
        self.block_to_item.get(block_id).map(String::as_str)
    }

    /// Resolves one target saved-item identifier to its target block identifier without guessing.
    #[must_use]
    pub fn block_id_for_item(&self, item_id: &str) -> VanillaSavedItemBlockMatch<'_> {
        let Some(blocks) = self.item_to_blocks.get(item_id) else {
            return VanillaSavedItemBlockMatch::None;
        };
        match blocks.as_slice() {
            [] => VanillaSavedItemBlockMatch::None,
            [only] => VanillaSavedItemBlockMatch::Unique(only.as_str()),
            [first, second, ..] => VanillaSavedItemBlockMatch::Ambiguous {
                first: first.as_str(),
                second: second.as_str(),
                matches: blocks.len(),
            },
        }
    }
}

fn validate_identifier(value: &str, kind: &str, version: &GameVersion) -> Result<()> {
    if value.is_empty() || !value.contains(':') {
        Err(BedrockWorldError::Validation(format!(
            "Bedrock {kind} identifier {value:?} is invalid in block/item map for {version}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_block_and_item_ids_are_preserved() {
        let map = VanillaSavedItemBlockMap::from_block_id_to_item_id_map_json(
            GameVersion::new(vec![1, 21, 100]).unwrap(),
            r#"{
                "minecraft:bed":"minecraft:item.bed",
                "minecraft:stone":"minecraft:stone"
            }"#,
        )
        .unwrap();
        assert_eq!(map.item_id_for_block("minecraft:bed"), Some("minecraft:item.bed"));
        assert_eq!(
            map.block_id_for_item("minecraft:item.bed"),
            VanillaSavedItemBlockMatch::Unique("minecraft:bed")
        );
        assert!(matches!(
            map.block_id_for_item("minecraft:apple"),
            VanillaSavedItemBlockMatch::None
        ));
    }

    #[test]
    fn reverse_aliases_are_reported_as_ambiguous() {
        let map = VanillaSavedItemBlockMap::from_block_id_to_item_id_map_json(
            GameVersion::new(vec![1, 21, 100]).unwrap(),
            r#"{
                "minecraft:first_block":"minecraft:shared_item",
                "minecraft:second_block":"minecraft:shared_item"
            }"#,
        )
        .unwrap();
        assert_eq!(
            map.block_id_for_item("minecraft:shared_item"),
            VanillaSavedItemBlockMatch::Ambiguous {
                first: "minecraft:first_block",
                second: "minecraft:second_block",
                matches: 2,
            }
        );
    }

    #[test]
    fn invalid_non_namespaced_ids_are_rejected() {
        assert!(
            VanillaSavedItemBlockMap::from_block_id_to_item_id_map_json(
                GameVersion::new(vec![1, 21, 100]).unwrap(),
                r#"{"bed":"minecraft:item.bed"}"#,
            )
            .is_err()
        );
    }
}
