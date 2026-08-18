//! Historical numeric Bedrock saved-item IDs and exact reverse representation checks.
//!
//! A reverse lookup never inverts rename/remap rules heuristically. Candidate historical numeric
//! ID/meta pairs are run through the same authoritative forward item rules and accepted only when the
//! resulting named ID/meta exactly matches the requested saved item.

use super::saved_item::{
    AuthoritativeItemMigrationCatalog, ItemSchemaSource, PINNED_ITEM_SCHEMA_FILES,
    load_pinned_item_migration_catalog_from_dir,
};
use crate::error::{BedrockWorldError, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const LEGACY_ITEM_ID_MAP_FILE: &str = "item_legacy_id_map.json";
const ITEM_SCHEMA_DIR: &str = "id_meta_upgrade_schema";

/// One ordered authoritative saved-item upgrade document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedItemUpgradeSource<'a> {
    /// Source filename including its numeric priority prefix.
    pub name: &'a str,
    /// UTF-8 JSON source.
    pub json: &'a str,
}

/// Named saved-item ID plus the persisted auxiliary metadata value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NamedSavedItemId {
    /// Namespaced item identifier.
    pub name: String,
    /// Item metadata remaining after authoritative ID/meta upgrades.
    pub meta: i32,
}

/// One exact historical numeric saved-item representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LegacySavedItemId {
    /// Historical numeric item ID.
    pub numeric_id: i32,
    /// Historical auxiliary metadata value.
    pub meta: i32,
}

/// Result of asking whether one named saved item has an exact historical numeric representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySavedItemMatch {
    /// No historical numeric pair upgrades to the requested named ID/meta.
    Missing,
    /// Exactly one historical numeric pair represents the requested named ID/meta.
    Unique(LegacySavedItemId),
    /// Multiple historical numeric pairs converge to the same requested named ID/meta.
    ///
    /// The first two candidates are retained in deterministic numeric-ID/meta order. Callers must not
    /// pick one implicitly because doing so would invent information absent from the modern item.
    Ambiguous {
        /// First matching historical representation.
        first: LegacySavedItemId,
        /// Second matching historical representation proving ambiguity.
        second: LegacySavedItemId,
    },
}

impl LegacySavedItemMatch {
    /// Returns the historical pair only when the representation is unique.
    #[must_use]
    pub const fn unique(self) -> Option<LegacySavedItemId> {
        match self {
            Self::Unique(value) => Some(value),
            Self::Missing | Self::Ambiguous { .. } => None,
        }
    }
}

/// Authoritative historical numeric saved-item table with forward-verified reverse lookup.
#[derive(Debug, Clone)]
pub struct LegacySavedItemIdTable {
    catalog: AuthoritativeItemMigrationCatalog,
    legacy_ids: Vec<(i32, String)>,
    remapped_source_metas: Vec<i32>,
}

impl LegacySavedItemIdTable {
    /// Builds a table from the same immutable item resources used by the forward item rules.
    pub fn from_sources(
        legacy_item_id_map_json: &str,
        item_to_block_1_12_json: &str,
        sources: &[SavedItemUpgradeSource<'_>],
    ) -> Result<Self> {
        let internal_sources = sources
            .iter()
            .map(|source| ItemSchemaSource {
                name: source.name,
                json: source.json,
            })
            .collect::<Vec<_>>();
        let catalog = AuthoritativeItemMigrationCatalog::from_sources(
            legacy_item_id_map_json,
            item_to_block_1_12_json,
            &internal_sources,
        )?;
        Self::from_catalog_and_sources(catalog, legacy_item_id_map_json, sources)
    }

    fn from_catalog_and_sources(
        catalog: AuthoritativeItemMigrationCatalog,
        legacy_item_id_map_json: &str,
        sources: &[SavedItemUpgradeSource<'_>],
    ) -> Result<Self> {
        let source_ids: BTreeMap<String, i32> = serde_json::from_str(legacy_item_id_map_json)
            .map_err(|error| validation(format!("invalid legacy item id map: {error}")))?;
        let mut legacy_ids = source_ids
            .into_iter()
            .filter_map(|(name, numeric_id)| (numeric_id != 0).then_some((numeric_id, name)))
            .collect::<Vec<_>>();
        legacy_ids.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

        let mut metas = BTreeSet::from([0_i32]);
        for source in sources {
            let document: ReverseMetaDocument = serde_json::from_str(source.json).map_err(|error| {
                validation(format!(
                    "invalid item upgrade source {} while collecting historical metadata: {error}",
                    source.name
                ))
            })?;
            for values in document.remapped_metas.values() {
                for raw_meta in values.keys() {
                    let meta = raw_meta.parse::<i32>().map_err(|error| {
                        validation(format!(
                            "item upgrade source {} has invalid metadata key {raw_meta:?}: {error}",
                            source.name
                        ))
                    })?;
                    metas.insert(meta);
                }
            }
        }

        Ok(Self {
            catalog,
            legacy_ids,
            remapped_source_metas: metas.into_iter().collect(),
        })
    }

    /// Applies authoritative forward rules to one historical numeric pair.
    #[must_use]
    pub fn named_id(&self, legacy: LegacySavedItemId) -> Option<NamedSavedItemId> {
        let name = self.catalog.legacy_numeric_name(legacy.numeric_id)?;
        let upgraded = self.catalog.upgrade_id_meta(name, legacy.meta);
        Some(NamedSavedItemId {
            name: upgraded.name,
            meta: upgraded.meta,
        })
    }

    /// Finds an exact historical numeric representation for one named saved item.
    ///
    /// Candidate metadata consists of the requested target metadata plus every source metadata value
    /// explicitly referenced by authoritative `remappedMetas` rules. This is complete for the rule
    /// model: unchanged metadata can only originate from the same value, while changed metadata must
    /// appear as a source key in a remap rule. Every candidate is then verified by running the full
    /// ordered forward rule chain. Lookup performs no temporary metadata-vector allocation.
    #[must_use]
    pub fn match_numeric(&self, target: &NamedSavedItemId) -> LegacySavedItemMatch {
        let target_already_present = self.remapped_source_metas.binary_search(&target.meta).is_ok();
        let mut first = None::<LegacySavedItemId>;

        for (numeric_id, historical_name) in &self.legacy_ids {
            let mut index = 0usize;
            let mut target_emitted = target_already_present;
            while index < self.remapped_source_metas.len() || !target_emitted {
                let meta = if !target_emitted
                    && (index == self.remapped_source_metas.len()
                        || target.meta < self.remapped_source_metas[index])
                {
                    target_emitted = true;
                    target.meta
                } else {
                    let meta = self.remapped_source_metas[index];
                    index += 1;
                    meta
                };

                let upgraded = self.catalog.upgrade_id_meta(historical_name, meta);
                if upgraded.name != target.name || upgraded.meta != target.meta {
                    continue;
                }
                let candidate = LegacySavedItemId {
                    numeric_id: *numeric_id,
                    meta,
                };
                match first {
                    None => first = Some(candidate),
                    Some(previous) if previous == candidate => {}
                    Some(previous) => {
                        return LegacySavedItemMatch::Ambiguous {
                            first: previous,
                            second: candidate,
                        };
                    }
                }
            }
        }
        first.map_or(LegacySavedItemMatch::Missing, LegacySavedItemMatch::Unique)
    }

    /// Returns the number of historical non-zero numeric IDs considered by reverse lookup.
    #[must_use]
    pub fn legacy_id_count(&self) -> usize {
        self.legacy_ids.len()
    }

    /// Returns how many distinct metadata values appear as explicit remap sources.
    #[must_use]
    pub fn remapped_source_meta_count(&self) -> usize {
        self.remapped_source_metas.len()
    }
}

/// Loads the pinned, Git-blob-verified item corpus and builds its historical numeric reverse table.
pub fn load_pinned_legacy_saved_item_id_table_from_dir(
    root: impl AsRef<Path>,
) -> Result<LegacySavedItemIdTable> {
    let root = root.as_ref();
    let catalog = load_pinned_item_migration_catalog_from_dir(root)?;
    let legacy_json = fs::read_to_string(root.join(LEGACY_ITEM_ID_MAP_FILE)).map_err(|error| {
        validation(format!(
            "failed to read pinned {LEGACY_ITEM_ID_MAP_FILE}: {error}"
        ))
    })?;
    let mut schema_json = Vec::with_capacity(PINNED_ITEM_SCHEMA_FILES.len());
    for name in PINNED_ITEM_SCHEMA_FILES {
        let path = root.join(ITEM_SCHEMA_DIR).join(name);
        schema_json.push((
            *name,
            fs::read_to_string(&path).map_err(|error| {
                validation(format!("failed to read pinned {}: {error}", path.display()))
            })?,
        ));
    }
    let sources = schema_json
        .iter()
        .map(|(name, json)| SavedItemUpgradeSource {
            name: *name,
            json: json.as_str(),
        })
        .collect::<Vec<_>>();
    LegacySavedItemIdTable::from_catalog_and_sources(catalog, &legacy_json, &sources)
}

#[derive(Debug, Deserialize)]
struct ReverseMetaDocument {
    #[serde(default, rename = "remappedMetas")]
    remapped_metas: BTreeMap<String, BTreeMap<String, String>>,
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_match_uses_forward_remap_semantics() {
        let sources = [SavedItemUpgradeSource {
            name: "0001_test.json",
            json: r#"{"remappedMetas":{"minecraft:old":{"3":"minecraft:new"}}}"#,
        }];
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            "{}",
            &sources,
        )
        .unwrap();
        let target = NamedSavedItemId {
            name: "minecraft:new".to_string(),
            meta: 0,
        };
        assert_eq!(
            table.match_numeric(&target),
            LegacySavedItemMatch::Unique(LegacySavedItemId {
                numeric_id: 1,
                meta: 3,
            })
        );
        assert_eq!(
            table.named_id(LegacySavedItemId {
                numeric_id: 1,
                meta: 3,
            }),
            Some(target)
        );
    }

    #[test]
    fn reverse_match_reports_alias_ambiguity_instead_of_picking_one_id() {
        let sources = [SavedItemUpgradeSource {
            name: "0001_test.json",
            json: r#"{"renamedIds":{"minecraft:first":"minecraft:new","minecraft:second":"minecraft:new"}}"#,
        }];
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:first":1,"minecraft:second":2}"#,
            "{}",
            &sources,
        )
        .unwrap();
        let target = NamedSavedItemId {
            name: "minecraft:new".to_string(),
            meta: 5,
        };
        assert_eq!(
            table.match_numeric(&target),
            LegacySavedItemMatch::Ambiguous {
                first: LegacySavedItemId {
                    numeric_id: 1,
                    meta: 5,
                },
                second: LegacySavedItemId {
                    numeric_id: 2,
                    meta: 5,
                },
            }
        );
    }

    #[test]
    fn reverse_match_does_not_generate_numeric_air_id_zero() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:air":0,"minecraft:stone":1}"#,
            "{}",
            &[],
        )
        .unwrap();
        assert_eq!(
            table.match_numeric(&NamedSavedItemId {
                name: "minecraft:air".to_string(),
                meta: 0,
            }),
            LegacySavedItemMatch::Missing
        );
    }
}
