//! Complete vanilla saved-item identifier set for one concrete Minecraft Bedrock release.
//!
//! `required_item_list.json` is generated from a real Bedrock release and contains the required item
//! registry for that game. The palette is deliberately version-bound: callers supply the exact game
//! version that produced the file instead of asking this library to infer a release from current data.

use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Registry metadata retained for one vanilla item identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VanillaSavedItemEntry {
    /// Runtime ID assigned by the target Bedrock release.
    pub runtime_id: i32,
    /// Whether the target release reports this item as component-based, when the source contains it.
    pub component_based: Option<bool>,
    /// Item registry version reported by the target release, when present.
    pub version: Option<i32>,
}

/// Complete vanilla item identifier palette for one concrete Minecraft Bedrock game version.
///
/// This proves identifier existence only. Runtime IDs are retained as evidence but are not required to
/// be globally unique here because world saved-item compatibility is keyed by the persisted namespaced
/// identifier. Item upgrade rules remain responsible for historical ID/meta rewrites, while block
/// palettes remain responsible for persisted BlockState existence.
#[derive(Debug, Clone)]
pub struct VanillaSavedItemPalette {
    game_version: GameVersion,
    entries: BTreeMap<String, VanillaSavedItemEntry>,
}

impl VanillaSavedItemPalette {
    /// Parses one release's `required_item_list.json`.
    ///
    /// Unknown per-entry fields are intentionally ignored so the reader remains usable across real
    /// BedrockData generations that add registry metadata unrelated to saved-world item identity.
    pub fn from_required_item_list_json(game_version: GameVersion, json: &str) -> Result<Self> {
        let source: BTreeMap<String, RequiredItemEntry> =
            serde_json::from_str(json).map_err(|error| {
                BedrockWorldError::Validation(format!(
                    "invalid Bedrock required_item_list.json for {game_version}: {error}"
                ))
            })?;
        if source.is_empty() {
            return Err(BedrockWorldError::Validation(format!(
                "Bedrock required item list for {game_version} is empty"
            )));
        }

        let mut entries = BTreeMap::new();
        for (name, entry) in source {
            if name.is_empty() || !name.contains(':') {
                return Err(BedrockWorldError::Validation(format!(
                    "Bedrock required item list for {game_version} contains invalid identifier {name:?}"
                )));
            }
            entries.insert(
                name,
                VanillaSavedItemEntry {
                    runtime_id: entry.runtime_id,
                    component_based: entry.component_based,
                    version: entry.version,
                },
            );
        }
        Ok(Self {
            game_version,
            entries,
        })
    }

    /// Returns the exact Minecraft Bedrock release represented by this item palette.
    #[must_use]
    pub fn game_version(&self) -> &GameVersion {
        &self.game_version
    }

    /// Returns the number of required vanilla item identifiers in the target release.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the target palette contains no identifiers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns whether one exact namespaced item identifier exists in the target release.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Returns target-release registry metadata for one exact item identifier.
    #[must_use]
    pub fn entry(&self, name: &str) -> Option<VanillaSavedItemEntry> {
        self.entries.get(name).copied()
    }

    /// Iterates required item identifiers in deterministic lexical order without allocating.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

#[derive(Debug, Deserialize)]
struct RequiredItemEntry {
    runtime_id: i32,
    #[serde(default)]
    component_based: Option<bool>,
    #[serde(default)]
    version: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_item_list_is_bound_to_the_supplied_game_release() {
        let palette = VanillaSavedItemPalette::from_required_item_list_json(
            GameVersion::new(vec![1, 21, 100]).unwrap(),
            r#"{
                "minecraft:stone":{"runtime_id":1,"component_based":false,"version":2},
                "minecraft:test_component":{"runtime_id":-7,"component_based":true,"version":3,"future_field":"ignored"}
            }"#,
        )
        .unwrap();
        assert_eq!(palette.game_version().components(), &[1, 21, 100]);
        assert!(palette.contains("minecraft:stone"));
        assert_eq!(
            palette.entry("minecraft:test_component"),
            Some(VanillaSavedItemEntry {
                runtime_id: -7,
                component_based: Some(true),
                version: Some(3),
            })
        );
        assert_eq!(
            palette.names().collect::<Vec<_>>(),
            vec!["minecraft:stone", "minecraft:test_component"]
        );
    }

    #[test]
    fn older_minimal_registry_entries_are_accepted() {
        let palette = VanillaSavedItemPalette::from_required_item_list_json(
            GameVersion::new(vec![1, 16, 0]).unwrap(),
            r#"{"minecraft:stone":{"runtime_id":1}}"#,
        )
        .unwrap();
        assert_eq!(
            palette.entry("minecraft:stone"),
            Some(VanillaSavedItemEntry {
                runtime_id: 1,
                component_based: None,
                version: None,
            })
        );
    }

    #[test]
    fn empty_or_non_namespaced_identifiers_are_rejected() {
        assert!(
            VanillaSavedItemPalette::from_required_item_list_json(
                GameVersion::new(vec![1, 20, 0]).unwrap(),
                r#"{"stone":{"runtime_id":1}}"#,
            )
            .is_err()
        );
    }
}
