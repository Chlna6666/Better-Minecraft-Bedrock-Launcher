//! Version-bounded Bedrock saved-item ID/meta rules for concrete game releases.
//!
//! The item upgrade corpus is a change log, not a complete item registry. Exact older-target lookup
//! therefore combines its ordered ID/meta rules with a complete [`VanillaSavedItemPalette`] for the
//! requested target game. The target palette proves item existence; the rules prove how that target
//! identity reaches the source game version.

use super::{NamedSavedItemId, SavedItemUpgradeSource, VanillaSavedItemPalette};
use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// Result of reversing one source string-ID/meta item to a concrete older Bedrock release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedItemVersionMatch {
    /// No item in the target release forward-resolves to the source item.
    Missing,
    /// Exactly one target-release identity is proven.
    Unique(NamedSavedItemId),
    /// Multiple target-release identities converge to the same source item.
    Ambiguous {
        /// First deterministic target candidate.
        first: NamedSavedItemId,
        /// Second deterministic target candidate proving ambiguity.
        second: NamedSavedItemId,
        /// Total unique target candidates.
        matches: usize,
    },
}

impl SavedItemVersionMatch {
    /// Returns the target identity only when it is unique.
    #[must_use]
    pub fn unique(self) -> Option<NamedSavedItemId> {
        match self {
            Self::Unique(value) => Some(value),
            Self::Missing | Self::Ambiguous { .. } => None,
        }
    }
}

/// Ordered item ID/meta changes with real Bedrock game-version endpoints parsed from schema filenames.
#[derive(Debug, Clone)]
pub struct SavedItemVersionTable {
    rules: Vec<VersionRule>,
    earliest_endpoint: GameVersion,
    latest_endpoint: GameVersion,
}

impl SavedItemVersionTable {
    /// Parses ordered item schema sources such as `0011_1.11.4_to_1.12.0.json`.
    pub fn from_sources(sources: &[SavedItemUpgradeSource<'_>]) -> Result<Self> {
        if sources.is_empty() {
            return Err(validation("saved-item version rule list is empty"));
        }
        let mut rules = Vec::with_capacity(sources.len());
        let mut ids = BTreeSet::new();
        for source in sources {
            let id = schema_id(source.name)?;
            if !ids.insert(id) {
                return Err(validation(format!(
                    "duplicate saved-item schema id {id}"
                )));
            }
            let result_version = schema_result_version(source.name)?;
            let document: VersionRuleDocument = serde_json::from_str(source.json).map_err(|error| {
                validation(format!(
                    "invalid saved-item version source {}: {error}",
                    source.name
                ))
            })?;
            let mut remapped_metas = BTreeMap::new();
            for (name, values) in document.remapped_metas {
                let mut parsed = BTreeMap::new();
                for (raw_meta, target) in values {
                    let meta = raw_meta.parse::<i32>().map_err(|error| {
                        validation(format!(
                            "saved-item source {} has invalid metadata key {raw_meta:?}: {error}",
                            source.name
                        ))
                    })?;
                    parsed.insert(meta, target);
                }
                remapped_metas.insert(name, parsed);
            }
            rules.push(VersionRule {
                id,
                result_version,
                renamed_ids: document.renamed_ids,
                remapped_metas,
            });
        }
        rules.sort_by_key(|rule| rule.id);
        for window in rules.windows(2) {
            if compare_release(&window[0].result_version, &window[1].result_version)
                == Ordering::Greater
            {
                return Err(validation(format!(
                    "saved-item schema {} endpoint {} is newer than later schema {} endpoint {}",
                    window[0].id,
                    window[0].result_version,
                    window[1].id,
                    window[1].result_version
                )));
            }
        }
        let earliest_endpoint = rules
            .first()
            .expect("non-empty checked above")
            .result_version
            .clone();
        let latest_endpoint = rules
            .last()
            .expect("non-empty checked above")
            .result_version
            .clone();
        Ok(Self {
            rules,
            earliest_endpoint,
            latest_endpoint,
        })
    }

    /// Earliest concrete game-version endpoint represented by the supplied change rules.
    #[must_use]
    pub fn earliest_endpoint(&self) -> &GameVersion {
        &self.earliest_endpoint
    }

    /// Latest concrete game-version endpoint represented by the supplied change rules.
    #[must_use]
    pub fn latest_endpoint(&self) -> &GameVersion {
        &self.latest_endpoint
    }

    /// Builds a reusable reverse target from one known source game version to an older target palette.
    ///
    /// The source must not be newer than the rule corpus endpoint, because changes after the corpus
    /// would be unknown. The target must be at or after the earliest rule endpoint and not newer than
    /// the source. Building the target precomputes name/meta reverse indices once; per-item lookup does
    /// not scan the target palette.
    pub fn older_target(
        &self,
        source_game_version: &GameVersion,
        target_palette: &VanillaSavedItemPalette,
    ) -> Result<SavedItemVersionTarget> {
        let target_game_version = target_palette.game_version();
        if compare_release(source_game_version, &self.latest_endpoint) == Ordering::Greater {
            return Err(validation(format!(
                "source Bedrock version {source_game_version} is newer than saved-item rule endpoint {}",
                self.latest_endpoint
            )));
        }
        if compare_release(target_game_version, &self.earliest_endpoint) == Ordering::Less {
            return Err(validation(format!(
                "target Bedrock version {target_game_version} predates saved-item rule endpoint {}",
                self.earliest_endpoint
            )));
        }
        if compare_release(target_game_version, source_game_version) == Ordering::Greater {
            return Err(validation(format!(
                "saved-item older target {target_game_version} is newer than source {source_game_version}"
            )));
        }

        let rules = self
            .rules
            .iter()
            .filter(|rule| {
                compare_release(&rule.result_version, target_game_version) == Ordering::Greater
                    && compare_release(&rule.result_version, source_game_version) != Ordering::Greater
            })
            .cloned()
            .collect::<Vec<_>>();
        SavedItemVersionTarget::build(
            source_game_version.clone(),
            target_game_version.clone(),
            target_palette,
            rules,
        )
    }
}

/// Precomputed exact item-ID/meta reverse target for one source and one older Bedrock release.
#[derive(Debug, Clone)]
pub struct SavedItemVersionTarget {
    source_game_version: GameVersion,
    target_game_version: GameVersion,
    rules: Vec<VersionRule>,
    passthrough_names: BTreeMap<String, Vec<String>>,
    remapped: BTreeMap<NamedSavedItemId, Vec<NamedSavedItemId>>,
}

impl SavedItemVersionTarget {
    fn build(
        source_game_version: GameVersion,
        target_game_version: GameVersion,
        target_palette: &VanillaSavedItemPalette,
        rules: Vec<VersionRule>,
    ) -> Result<Self> {
        let mut remap_metas = BTreeSet::<i32>::new();
        for rule in &rules {
            for values in rule.remapped_metas.values() {
                remap_metas.extend(values.keys().copied());
            }
        }
        let sentinel = (i32::MIN..)
            .find(|value| !remap_metas.contains(value))
            .ok_or_else(|| validation("could not select saved-item metadata sentinel"))?;

        let mut passthrough_names = BTreeMap::<String, Vec<String>>::new();
        let mut remapped = BTreeMap::<NamedSavedItemId, BTreeSet<NamedSavedItemId>>::new();
        for target_name in target_palette.names() {
            let passthrough = apply_rules(&rules, target_name, sentinel);
            if passthrough.meta != sentinel {
                return Err(validation(format!(
                    "saved-item metadata sentinel unexpectedly remapped for {target_name}"
                )));
            }
            passthrough_names
                .entry(passthrough.name)
                .or_default()
                .push(target_name.to_string());

            for &meta in &remap_metas {
                let source = apply_rules(&rules, target_name, meta);
                remapped
                    .entry(source)
                    .or_default()
                    .insert(NamedSavedItemId {
                        name: target_name.to_string(),
                        meta,
                    });
            }
        }
        for names in passthrough_names.values_mut() {
            names.sort();
            names.dedup();
        }
        let remapped = remapped
            .into_iter()
            .map(|(source, targets)| (source, targets.into_iter().collect()))
            .collect();

        Ok(Self {
            source_game_version,
            target_game_version,
            rules,
            passthrough_names,
            remapped,
        })
    }

    /// Source Bedrock release whose saved item is being reversed.
    #[must_use]
    pub fn source_game_version(&self) -> &GameVersion {
        &self.source_game_version
    }

    /// Concrete older Bedrock release represented by the target item palette.
    #[must_use]
    pub fn target_game_version(&self) -> &GameVersion {
        &self.target_game_version
    }

    /// Finds exact target-release ID/meta candidates for one source string-ID/meta item.
    ///
    /// Every returned candidate originated in the target release's complete vanilla item palette and
    /// is re-run through the selected forward rules before acceptance. Ambiguous aliases are reported
    /// rather than selected implicitly.
    #[must_use]
    pub fn match_item(&self, source: &NamedSavedItemId) -> SavedItemVersionMatch {
        let mut candidates = BTreeSet::<NamedSavedItemId>::new();
        if let Some(exact) = self.remapped.get(source) {
            candidates.extend(exact.iter().cloned());
        }
        if let Some(names) = self.passthrough_names.get(&source.name) {
            for name in names {
                let candidate = NamedSavedItemId {
                    name: name.clone(),
                    meta: source.meta,
                };
                if apply_rules(&self.rules, &candidate.name, candidate.meta) == *source {
                    candidates.insert(candidate);
                }
            }
        }

        let mut iter = candidates.into_iter();
        let Some(first) = iter.next() else {
            return SavedItemVersionMatch::Missing;
        };
        let Some(second) = iter.next() else {
            return SavedItemVersionMatch::Unique(first);
        };
        let matches = 2usize.saturating_add(iter.count());
        SavedItemVersionMatch::Ambiguous {
            first,
            second,
            matches,
        }
    }
}

#[derive(Debug, Clone)]
struct VersionRule {
    id: u32,
    result_version: GameVersion,
    renamed_ids: BTreeMap<String, String>,
    remapped_metas: BTreeMap<String, BTreeMap<i32, String>>,
}

#[derive(Debug, Deserialize)]
struct VersionRuleDocument {
    #[serde(default, rename = "renamedIds")]
    renamed_ids: BTreeMap<String, String>,
    #[serde(default, rename = "remappedMetas")]
    remapped_metas: BTreeMap<String, BTreeMap<String, String>>,
}

fn apply_rules(rules: &[VersionRule], item_id: &str, meta: i32) -> NamedSavedItemId {
    let mut name = item_id.to_string();
    let mut meta = meta;
    for rule in rules {
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
    NamedSavedItemId { name, meta }
}

fn schema_id(name: &str) -> Result<u32> {
    let prefix = name
        .split_once('_')
        .map(|(prefix, _)| prefix)
        .ok_or_else(|| validation(format!("saved-item schema filename has no numeric prefix: {name}")))?;
    prefix
        .parse::<u32>()
        .map_err(|error| validation(format!("invalid saved-item schema id in {name}: {error}")))
}

fn schema_result_version(name: &str) -> Result<GameVersion> {
    let without_json = name
        .strip_suffix(".json")
        .ok_or_else(|| validation(format!("saved-item schema filename is not JSON: {name}")))?;
    let target = without_json
        .rsplit_once("_to_")
        .map(|(_, target)| target)
        .ok_or_else(|| validation(format!("saved-item schema filename has no _to_ endpoint: {name}")))?;
    let target = target.strip_suffix("_beta").unwrap_or(target);
    let components = target
        .split('.')
        .map(|component| {
            component.parse::<i32>().map_err(|error| {
                validation(format!(
                    "saved-item schema {name} has invalid endpoint component {component:?}: {error}"
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    GameVersion::new(components)
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

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source<'a>(name: &'a str, json: &'a str) -> SavedItemUpgradeSource<'a> {
        SavedItemUpgradeSource { name, json }
    }

    fn palette(version: &[i32], names: &[&str]) -> VanillaSavedItemPalette {
        let entries = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                format!(
                    r#""{name}":{{"runtime_id":{},"component_based":false,"version":2}}"#,
                    index + 1
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        VanillaSavedItemPalette::from_required_item_list_json(
            GameVersion::new(version.to_vec()).unwrap(),
            &format!("{{{entries}}}"),
        )
        .unwrap()
    }

    #[test]
    fn target_between_schema_endpoints_uses_only_later_changes() {
        let table = SavedItemVersionTable::from_sources(&[
            source(
                "0001_1.6_beta_to_1.6.0.json",
                r#"{"renamedIds":{"minecraft:pre":"minecraft:one_six"}}"#,
            ),
            source(
                "0011_1.11.4_to_1.12.0.json",
                r#"{"renamedIds":{"minecraft:one_six":"minecraft:one_twelve"}}"#,
            ),
            source(
                "0021_1.16.0_to_1.16.100.json",
                r#"{"renamedIds":{"minecraft:one_twelve":"minecraft:modern"}}"#,
            ),
        ])
        .unwrap();
        let target_palette = palette(&[1, 9, 0], &["minecraft:one_six"]);
        let target = table
            .older_target(
                &GameVersion::new(vec![1, 16, 100]).unwrap(),
                &target_palette,
            )
            .unwrap();
        assert_eq!(target.target_game_version().components(), &[1, 9, 0]);
        assert_eq!(
            target.match_item(&NamedSavedItemId {
                name: "minecraft:modern".to_string(),
                meta: 4,
            }),
            SavedItemVersionMatch::Unique(NamedSavedItemId {
                name: "minecraft:one_six".to_string(),
                meta: 4,
            })
        );
    }

    #[test]
    fn target_palette_proves_later_added_unchanged_item_exists() {
        let table = SavedItemVersionTable::from_sources(&[
            source("0001_1.6_beta_to_1.6.0.json", "{}"),
            source("0011_1.11.4_to_1.12.0.json", "{}"),
        ])
        .unwrap();
        let target_palette = palette(&[1, 12, 0], &["minecraft:later_item"]);
        let target = table
            .older_target(
                &GameVersion::new(vec![1, 12, 0]).unwrap(),
                &target_palette,
            )
            .unwrap();
        assert_eq!(
            target.match_item(&NamedSavedItemId {
                name: "minecraft:later_item".to_string(),
                meta: 17,
            }),
            SavedItemVersionMatch::Unique(NamedSavedItemId {
                name: "minecraft:later_item".to_string(),
                meta: 17,
            })
        );
        assert_eq!(
            target.match_item(&NamedSavedItemId {
                name: "minecraft:not_in_target".to_string(),
                meta: 0,
            }),
            SavedItemVersionMatch::Missing
        );
    }

    #[test]
    fn metadata_remap_is_reversed_without_scanning_palette_per_lookup() {
        let table = SavedItemVersionTable::from_sources(&[
            source("0001_1.6_beta_to_1.6.0.json", "{}"),
            source(
                "0011_1.11.4_to_1.12.0.json",
                r#"{"remappedMetas":{"minecraft:old":{"3":"minecraft:new"}}}"#,
            ),
        ])
        .unwrap();
        let target_palette = palette(&[1, 9, 0], &["minecraft:old"]);
        let target = table
            .older_target(
                &GameVersion::new(vec![1, 12, 0]).unwrap(),
                &target_palette,
            )
            .unwrap();
        assert_eq!(
            target.match_item(&NamedSavedItemId {
                name: "minecraft:new".to_string(),
                meta: 0,
            }),
            SavedItemVersionMatch::Unique(NamedSavedItemId {
                name: "minecraft:old".to_string(),
                meta: 3,
            })
        );
    }

    #[test]
    fn aliases_are_reported_as_ambiguous() {
        let table = SavedItemVersionTable::from_sources(&[
            source("0001_1.6_beta_to_1.6.0.json", "{}"),
            source(
                "0011_1.11.4_to_1.12.0.json",
                r#"{"renamedIds":{"minecraft:first":"minecraft:new","minecraft:second":"minecraft:new"}}"#,
            ),
        ])
        .unwrap();
        let target_palette = palette(&[1, 9, 0], &["minecraft:first", "minecraft:second"]);
        let target = table
            .older_target(
                &GameVersion::new(vec![1, 12, 0]).unwrap(),
                &target_palette,
            )
            .unwrap();
        assert!(matches!(
            target.match_item(&NamedSavedItemId {
                name: "minecraft:new".to_string(),
                meta: 0,
            }),
            SavedItemVersionMatch::Ambiguous { matches: 2, .. }
        ));
    }

    #[test]
    fn source_newer_than_rule_corpus_is_refused() {
        let table = SavedItemVersionTable::from_sources(&[
            source("0001_1.6_beta_to_1.6.0.json", "{}"),
            source("0011_1.11.4_to_1.12.0.json", "{}"),
        ])
        .unwrap();
        assert!(
            table
                .older_target(
                    &GameVersion::new(vec![1, 13, 0]).unwrap(),
                    &palette(&[1, 12, 0], &["minecraft:stone"]),
                )
                .is_err()
        );
    }
}
