//! Historical Bedrock saved-item identities and exact reverse representation checks.
//!
//! Reverse lookup never inverts rename/remap rules heuristically. Candidate historical ID/meta pairs
//! are run through the same authoritative forward item rules and accepted only when the resulting
//! named ID/meta exactly matches the requested saved item.

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

/// One exact Classic (MCPE <= 1.5) numeric saved-item representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LegacySavedItemId {
    /// Historical numeric item ID.
    pub numeric_id: i32,
    /// Historical auxiliary metadata value.
    pub meta: i32,
}

/// One exact Medieval (MCPE 1.6-1.8) string-ID saved-item representation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MedievalSavedItemId {
    /// Historical string item identifier persisted by the 1.6-1.8 format.
    pub name: String,
    /// Historical auxiliary metadata value.
    pub meta: i32,
}

/// Result of asking whether one named saved item has an exact Classic numeric representation.
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
        /// Second historical representation proving ambiguity.
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

/// Result of asking whether one named saved item has an exact Medieval string-ID representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MedievalSavedItemMatch {
    /// No historical string ID/meta pair upgrades to the requested named ID/meta.
    Missing,
    /// Exactly one historical string ID/meta pair represents the requested item.
    Unique(MedievalSavedItemId),
    /// Multiple distinct historical string ID/meta pairs converge to the same requested item.
    Ambiguous {
        /// First matching historical representation in string-ID/meta order.
        first: MedievalSavedItemId,
        /// Second matching historical representation proving ambiguity.
        second: MedievalSavedItemId,
    },
}

impl MedievalSavedItemMatch {
    /// Returns the historical pair only when the representation is unique.
    #[must_use]
    pub fn unique(self) -> Option<MedievalSavedItemId> {
        match self {
            Self::Unique(value) => Some(value),
            Self::Missing | Self::Ambiguous { .. } => None,
        }
    }
}

/// Authoritative historical saved-item table with forward-verified reverse lookup.
#[derive(Debug, Clone)]
pub struct LegacySavedItemIdTable {
    catalog: AuthoritativeItemMigrationCatalog,
    legacy_ids: Vec<(i32, String)>,
    legacy_names: Vec<String>,
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
        let mut legacy_names = source_ids
            .iter()
            .filter_map(|(name, numeric_id)| (*numeric_id != 0).then_some(name.clone()))
            .collect::<Vec<_>>();
        legacy_names.sort();
        legacy_names.dedup();

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
            legacy_names,
            remapped_source_metas: metas.into_iter().collect(),
        })
    }

    /// Returns the historical string item identifier behind one Classic numeric candidate.
    #[must_use]
    pub fn legacy_item_name(&self, legacy: LegacySavedItemId) -> Option<&str> {
        self.catalog.legacy_numeric_name(legacy.numeric_id)
    }

    /// Returns the 1.12-era block identifier associated with one Classic blockitem candidate.
    #[must_use]
    pub fn legacy_block_id(&self, legacy: LegacySavedItemId) -> Option<&str> {
        let item_name = self.legacy_item_name(legacy)?;
        self.catalog.legacy_block_id(item_name)
    }

    /// Returns the block identifier associated with one Medieval string-ID blockitem candidate.
    #[must_use]
    pub fn medieval_block_id(&self, medieval: &MedievalSavedItemId) -> Option<&str> {
        self.catalog.legacy_block_id(&medieval.name)
    }

    /// Applies authoritative forward rules to one Classic numeric pair.
    #[must_use]
    pub fn named_id(&self, legacy: LegacySavedItemId) -> Option<NamedSavedItemId> {
        let name = self.catalog.legacy_numeric_name(legacy.numeric_id)?;
        let upgraded = self.catalog.upgrade_id_meta(name, legacy.meta);
        Some(NamedSavedItemId {
            name: upgraded.name,
            meta: upgraded.meta,
        })
    }

    /// Applies authoritative forward rules to one Medieval string-ID/meta pair.
    #[must_use]
    pub fn named_id_from_medieval(&self, medieval: &MedievalSavedItemId) -> NamedSavedItemId {
        let upgraded = self.catalog.upgrade_id_meta(&medieval.name, medieval.meta);
        NamedSavedItemId {
            name: upgraded.name,
            meta: upgraded.meta,
        }
    }

    /// Finds an exact Classic numeric representation for one named saved item.
    ///
    /// Candidate metadata consists of the requested target metadata plus every source metadata value
    /// explicitly referenced by authoritative `remappedMetas` rules. Every candidate is verified by
    /// running the full ordered forward rule chain. Numeric aliases remain distinct because Classic
    /// saves persist the numeric ID itself.
    #[must_use]
    pub fn match_numeric(&self, target: &NamedSavedItemId) -> LegacySavedItemMatch {
        let target_already_present = self.remapped_source_metas.binary_search(&target.meta).is_ok();
        let mut first = None::<LegacySavedItemId>;

        for (numeric_id, historical_name) in &self.legacy_ids {
            let mut index = 0usize;
            let mut target_emitted = target_already_present;
            while index < self.remapped_source_metas.len() || !target_emitted {
                let meta = next_candidate_meta(
                    &self.remapped_source_metas,
                    target.meta,
                    &mut index,
                    &mut target_emitted,
                );
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

    /// Finds an exact Medieval (MCPE 1.6-1.8) string-ID/meta representation.
    ///
    /// Numeric aliases are deliberately collapsed before this search because Medieval saves persist
    /// the string item identifier, not the old numeric ID. Distinct historical strings that converge
    /// to the same modern item remain ambiguous and are never selected implicitly.
    #[must_use]
    pub fn match_medieval(&self, target: &NamedSavedItemId) -> MedievalSavedItemMatch {
        let target_already_present = self.remapped_source_metas.binary_search(&target.meta).is_ok();
        let mut first = None::<MedievalSavedItemId>;

        for historical_name in &self.legacy_names {
            let mut index = 0usize;
            let mut target_emitted = target_already_present;
            while index < self.remapped_source_metas.len() || !target_emitted {
                let meta = next_candidate_meta(
                    &self.remapped_source_metas,
                    target.meta,
                    &mut index,
                    &mut target_emitted,
                );
                let upgraded = self.catalog.upgrade_id_meta(historical_name, meta);
                if upgraded.name != target.name || upgraded.meta != target.meta {
                    continue;
                }
                let candidate = MedievalSavedItemId {
                    name: historical_name.clone(),
                    meta,
                };
                match &first {
                    None => first = Some(candidate),
                    Some(previous) if previous == &candidate => {}
                    Some(previous) => {
                        return MedievalSavedItemMatch::Ambiguous {
                            first: previous.clone(),
                            second: candidate,
                        };
                    }
                }
            }
        }
        first.map_or(MedievalSavedItemMatch::Missing, MedievalSavedItemMatch::Unique)
    }

    /// Returns the number of historical non-zero numeric IDs considered by Classic reverse lookup.
    #[must_use]
    pub fn legacy_id_count(&self) -> usize {
        self.legacy_ids.len()
    }

    /// Returns the number of distinct historical string IDs considered by Medieval reverse lookup.
    #[must_use]
    pub fn medieval_name_count(&self) -> usize {
        self.legacy_names.len()
    }

    /// Returns how many distinct metadata values appear as explicit remap sources.
    #[must_use]
    pub fn remapped_source_meta_count(&self) -> usize {
        self.remapped_source_metas.len()
    }
}

fn next_candidate_meta(
    remapped: &[i32],
    target: i32,
    index: &mut usize,
    target_emitted: &mut bool,
) -> i32 {
    if !*target_emitted && (*index == remapped.len() || target < remapped[*index]) {
        *target_emitted = true;
        target
    } else {
        let meta = remapped[*index];
        *index += 1;
        meta
    }
}

/// Loads the pinned, Git-blob-verified item corpus and builds its historical reverse table.
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
            table.match_medieval(&target),
            MedievalSavedItemMatch::Unique(MedievalSavedItemId {
                name: "minecraft:old".to_string(),
                meta: 3,
            })
        );
        assert_eq!(
            table.named_id_from_medieval(&MedievalSavedItemId {
                name: "minecraft:old".to_string(),
                meta: 3,
            }),
            target
        );
    }

    #[test]
    fn numeric_aliases_do_not_make_medieval_string_identity_ambiguous() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            "{}",
            &[],
        )
        .unwrap();
        let target = NamedSavedItemId {
            name: "minecraft:old".to_string(),
            meta: 2,
        };
        assert_eq!(
            table.match_medieval(&target),
            MedievalSavedItemMatch::Unique(MedievalSavedItemId {
                name: "minecraft:old".to_string(),
                meta: 2,
            })
        );
    }

    #[test]
    fn distinct_historical_strings_remain_ambiguous_for_medieval_target() {
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
        let result = table.match_medieval(&NamedSavedItemId {
            name: "minecraft:new".to_string(),
            meta: 0,
        });
        assert!(matches!(result, MedievalSavedItemMatch::Ambiguous { .. }));
    }

    #[test]
    fn reverse_match_reports_alias_ambiguity_instead_of_picking_one_numeric_id() {
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
        assert!(matches!(
            table.match_numeric(&target),
            LegacySavedItemMatch::Ambiguous { .. }
        ));
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
        assert_eq!(
            table.match_medieval(&NamedSavedItemId {
                name: "minecraft:air".to_string(),
                meta: 0,
            }),
            MedievalSavedItemMatch::Missing
        );
    }

    #[test]
    fn historical_blockitem_mapping_is_exposed_without_modern_name_guessing() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old_item":5,"minecraft:plain":6}"#,
            r#"{"minecraft:old_item":"minecraft:old_block"}"#,
            &[],
        )
        .unwrap();
        let blockitem = LegacySavedItemId {
            numeric_id: 5,
            meta: 7,
        };
        assert_eq!(table.legacy_item_name(blockitem), Some("minecraft:old_item"));
        assert_eq!(table.legacy_block_id(blockitem), Some("minecraft:old_block"));
        assert_eq!(
            table.medieval_block_id(&MedievalSavedItemId {
                name: "minecraft:old_item".to_string(),
                meta: 7,
            }),
            Some("minecraft:old_block")
        );
    }
}
