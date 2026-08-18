//! Historical Bedrock saved-item identities and exact reverse representation checks.
//!
//! Reverse lookup never inverts rename/remap rules heuristically. Candidate historical ID/meta pairs
//! are run through the same authoritative forward item rules and accepted only when the resulting
//! named ID/meta exactly matches the requested saved item.

use super::saved_item::{
    AuthoritativeItemMigrationCatalog, ItemIdentity, ItemSchemaSource, PINNED_ITEM_SCHEMA_FILES,
    load_pinned_item_migration_catalog_from_dir,
};
use crate::error::{BedrockWorldError, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const LEGACY_ITEM_ID_MAP_FILE: &str = "item_legacy_id_map.json";
const ITEM_TO_BLOCK_1_12_FILE: &str = "1.12.0_item_id_to_block_id_map.json";
const ITEM_SCHEMA_DIR: &str = "id_meta_upgrade_schema";
const MEDIEVAL_ENDPOINT_SCHEMA_ID: u32 = 1;
const ITEM_TO_BLOCK_ENDPOINT_SCHEMA_ID: u32 = 11;

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
    /// Historical string item identifier at the 1.6.0 storage endpoint.
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
    /// No proven 1.6.0 string ID/meta pair upgrades to the requested named ID/meta.
    Missing,
    /// Exactly one 1.6.0 string ID/meta pair represents the requested item.
    Unique(MedievalSavedItemId),
    /// Multiple distinct 1.6.0 string ID/meta pairs converge to the requested item.
    Ambiguous {
        /// First matching Medieval representation.
        first: MedievalSavedItemId,
        /// Second matching Medieval representation proving ambiguity.
        second: MedievalSavedItemId,
    },
}

impl MedievalSavedItemMatch {
    /// Returns the Medieval pair only when the representation is unique.
    #[must_use]
    pub fn unique(self) -> Option<MedievalSavedItemId> {
        match self {
            Self::Unique(value) => Some(value),
            Self::Missing | Self::Ambiguous { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct HistoricalItemRules {
    rules: Vec<HistoricalItemRule>,
}

#[derive(Debug, Clone)]
struct HistoricalItemRule {
    renamed_ids: BTreeMap<String, String>,
    remapped_metas: BTreeMap<String, BTreeMap<i32, String>>,
}

impl HistoricalItemRules {
    fn apply(&self, item_id: &str, meta: i32) -> ItemIdentity {
        let mut name = item_id.to_string();
        let mut meta = meta;
        for rule in &self.rules {
            if let Some(target) = rule
                .remapped_metas
                .get(&name)
                .and_then(|values| values.get(&meta))
            {
                name = target.clone();
                meta = 0;
            } else if let Some(target) = rule.renamed_ids.get(&name) {
                name = target.clone();
            }
        }
        ItemIdentity { name, meta }
    }
}

/// Authoritative historical saved-item table with forward-verified reverse lookup.
#[derive(Debug, Clone)]
pub struct LegacySavedItemIdTable {
    catalog: AuthoritativeItemMigrationCatalog,
    legacy_ids: Vec<(i32, String)>,
    legacy_names: Vec<String>,
    remapped_source_metas: Vec<i32>,
    medieval_endpoint: HistoricalItemRules,
    through_1_12: HistoricalItemRules,
    medieval_to_1_12: HistoricalItemRules,
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
        let mut medieval_endpoint = HistoricalItemRules::default();
        let mut through_1_12 = HistoricalItemRules::default();
        let mut medieval_to_1_12 = HistoricalItemRules::default();
        for source in sources {
            let id = source_schema_id(source.name)?;
            let document: ReverseRuleDocument = serde_json::from_str(source.json).map_err(|error| {
                validation(format!(
                    "invalid item upgrade source {} while preparing historical reverse data: {error}",
                    source.name
                ))
            })?;
            let mut remapped_metas = BTreeMap::new();
            for (name, values) in document.remapped_metas {
                let mut parsed = BTreeMap::new();
                for (raw_meta, target) in values {
                    let meta = raw_meta.parse::<i32>().map_err(|error| {
                        validation(format!(
                            "item upgrade source {} has invalid metadata key {raw_meta:?}: {error}",
                            source.name
                        ))
                    })?;
                    metas.insert(meta);
                    parsed.insert(meta, target);
                }
                remapped_metas.insert(name, parsed);
            }
            let rule = HistoricalItemRule {
                renamed_ids: document.renamed_ids,
                remapped_metas,
            };
            if id <= MEDIEVAL_ENDPOINT_SCHEMA_ID {
                medieval_endpoint.rules.push(rule.clone());
            }
            if id <= ITEM_TO_BLOCK_ENDPOINT_SCHEMA_ID {
                through_1_12.rules.push(rule.clone());
            }
            if id > MEDIEVAL_ENDPOINT_SCHEMA_ID && id <= ITEM_TO_BLOCK_ENDPOINT_SCHEMA_ID {
                medieval_to_1_12.rules.push(rule);
            }
        }

        Ok(Self {
            catalog,
            legacy_ids,
            legacy_names,
            remapped_source_metas: metas.into_iter().collect(),
            medieval_endpoint,
            through_1_12,
            medieval_to_1_12,
        })
    }

    /// Returns the historical pre-1.6 string item identifier behind one Classic numeric candidate.
    #[must_use]
    pub fn legacy_item_name(&self, legacy: LegacySavedItemId) -> Option<&str> {
        self.catalog.legacy_numeric_name(legacy.numeric_id)
    }

    /// Converts one Classic numeric candidate to its exact 1.6.0 Medieval string-ID representation.
    #[must_use]
    pub fn medieval_id_from_classic(&self, legacy: LegacySavedItemId) -> Option<MedievalSavedItemId> {
        let name = self.legacy_item_name(legacy)?;
        let endpoint = self.medieval_endpoint.apply(name, legacy.meta);
        Some(MedievalSavedItemId {
            name: endpoint.name,
            meta: endpoint.meta,
        })
    }

    /// Returns the 1.12-era block identifier associated with one Classic blockitem candidate.
    ///
    /// The old item ID/meta is first advanced only through the 1.12 item endpoint before consulting
    /// the authoritative `1.12.0_item_id_to_block_id_map.json` table.
    #[must_use]
    pub fn legacy_block_id(&self, legacy: LegacySavedItemId) -> Option<&str> {
        let item_name = self.legacy_item_name(legacy)?;
        let item_1_12 = self.through_1_12.apply(item_name, legacy.meta);
        self.catalog.legacy_block_id(&item_1_12.name)
    }

    /// Returns the 1.12-era block identifier associated with one Medieval blockitem candidate.
    #[must_use]
    pub fn medieval_block_id(&self, medieval: &MedievalSavedItemId) -> Option<&str> {
        let item_1_12 = self.medieval_to_1_12.apply(&medieval.name, medieval.meta);
        self.catalog.legacy_block_id(&item_1_12.name)
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

    /// Finds an exact Classic numeric representation for one named saved item.
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
    /// Candidate origins come only from the authoritative Classic ID table, are first normalized
    /// through the 1.6.0 endpoint, and are then deduplicated by their actual persisted Medieval
    /// string-ID/meta pair. The original candidate is still run through the complete authoritative
    /// forward chain to prove that the endpoint reaches the requested modern item. This deliberately
    /// refuses later-introduced items for which the pinned corpus does not prove 1.6-era existence.
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
                let endpoint = self.medieval_endpoint.apply(historical_name, meta);
                let candidate = MedievalSavedItemId {
                    name: endpoint.name,
                    meta: endpoint.meta,
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

    /// Returns the number of distinct pre-1.6 string origins considered for Medieval proof.
    #[must_use]
    pub fn medieval_origin_count(&self) -> usize {
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

fn source_schema_id(name: &str) -> Result<u32> {
    let prefix = name
        .split_once('_')
        .map(|(prefix, _)| prefix)
        .ok_or_else(|| validation(format!("item schema filename has no numeric prefix: {name}")))?;
    prefix
        .parse::<u32>()
        .map_err(|error| validation(format!("invalid item schema id in {name}: {error}")))
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
    // Read this file here as well so pinned-load failures surface before the reverse table is exposed.
    let _item_to_block = fs::read_to_string(root.join(ITEM_TO_BLOCK_1_12_FILE)).map_err(|error| {
        validation(format!(
            "failed to read pinned {ITEM_TO_BLOCK_1_12_FILE}: {error}"
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
struct ReverseRuleDocument {
    #[serde(default, rename = "renamedIds")]
    renamed_ids: BTreeMap<String, String>,
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
    fn medieval_candidate_uses_1_6_endpoint_name() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:nametag":421}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_1.6_beta_to_1.6.0.json",
                json: r#"{"renamedIds":{"minecraft:nametag":"minecraft:name_tag"}}"#,
            }],
        )
        .unwrap();
        let target = NamedSavedItemId {
            name: "minecraft:name_tag".to_string(),
            meta: 0,
        };
        assert_eq!(
            table.match_medieval(&target),
            MedievalSavedItemMatch::Unique(MedievalSavedItemId {
                name: "minecraft:name_tag".to_string(),
                meta: 0,
            })
        );
        assert_eq!(
            table.medieval_id_from_classic(LegacySavedItemId {
                numeric_id: 421,
                meta: 0,
            }),
            Some(MedievalSavedItemId {
                name: "minecraft:name_tag".to_string(),
                meta: 0,
            })
        );
    }

    #[test]
    fn medieval_endpoint_aliases_are_deduplicated() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:first":1,"minecraft:second":2}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:first":"minecraft:same","minecraft:second":"minecraft:same"}}"#,
            }],
        )
        .unwrap();
        assert_eq!(
            table.match_medieval(&NamedSavedItemId {
                name: "minecraft:same".to_string(),
                meta: 0,
            }),
            MedievalSavedItemMatch::Unique(MedievalSavedItemId {
                name: "minecraft:same".to_string(),
                meta: 0,
            })
        );
        assert!(matches!(
            table.match_numeric(&NamedSavedItemId {
                name: "minecraft:same".to_string(),
                meta: 0,
            }),
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
    fn block_mapping_is_resolved_at_1_12_endpoint() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old_item":5}"#,
            r#"{"minecraft:item_1_12":"minecraft:block_1_12"}"#,
            &[
                SavedItemUpgradeSource {
                    name: "0001_test.json",
                    json: r#"{"renamedIds":{"minecraft:old_item":"minecraft:item_1_6"}}"#,
                },
                SavedItemUpgradeSource {
                    name: "0011_test.json",
                    json: r#"{"renamedIds":{"minecraft:item_1_6":"minecraft:item_1_12"}}"#,
                },
            ],
        )
        .unwrap();
        let classic = LegacySavedItemId {
            numeric_id: 5,
            meta: 3,
        };
        let medieval = MedievalSavedItemId {
            name: "minecraft:item_1_6".to_string(),
            meta: 3,
        };
        assert_eq!(table.legacy_block_id(classic), Some("minecraft:block_1_12"));
        assert_eq!(
            table.medieval_block_id(&medieval),
            Some("minecraft:block_1_12")
        );
    }
}
