//! Preflight of named saved items against historical numeric Bedrock item representation.

use super::legacy_saved_item::{
    LegacySavedItemId, LegacySavedItemIdTable, LegacySavedItemMatch, NamedSavedItemId,
};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

/// One concrete reason a named saved item is not yet proven writable as historical numeric data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySavedItemIssue {
    /// Stable NBT path from the checked root, such as `$.Inventory[3]`.
    pub path: String,
    /// Named ID/meta observed in the item.
    pub item: NamedSavedItemId,
    /// Compatibility problem at this path.
    pub kind: LegacySavedItemIssueKind,
}

/// Historical numeric compatibility problem for one saved item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySavedItemIssueKind {
    /// No numeric ID/meta pair forward-upgrades to this named item.
    MissingNumericId,
    /// More than one numeric ID/meta pair forward-upgrades to this named item.
    AmbiguousNumericId {
        /// First historical candidate.
        first: LegacySavedItemId,
        /// Second historical candidate proving ambiguity.
        second: LegacySavedItemId,
    },
    /// The item carries a `Block` BlockState that needs an independent reverse BlockState proof.
    BlockStateRequired {
        /// Unique numeric item ID/meta candidate already proven for the item identity.
        item: LegacySavedItemId,
    },
}

/// Aggregate historical numeric saved-item preflight for an NBT tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacySavedItemCheckReport {
    /// Recognised saved-item compounds visited.
    pub items_seen: usize,
    /// Items already stored with a numeric `id` and therefore not rewritten by this check.
    pub already_numeric: usize,
    /// Named items with one unique numeric ID/meta representation.
    pub named_unique: usize,
    /// Named items with no numeric representation.
    pub named_missing: usize,
    /// Named items with multiple numeric representations.
    pub named_ambiguous: usize,
    /// Unique items whose `Block` payload still needs BlockState reverse validation.
    pub block_states_required: usize,
    /// Detailed issues; successful ordinary items do not allocate path entries here.
    pub issues: Vec<LegacySavedItemIssue>,
}

impl LegacySavedItemCheckReport {
    /// Returns whether every named item has exactly one numeric ID/meta representation.
    #[must_use]
    pub fn item_ids_are_exact(&self) -> bool {
        self.named_missing == 0 && self.named_ambiguous == 0
    }

    /// Returns whether item IDs are exact and no block item still needs BlockState reverse proof.
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.item_ids_are_exact() && self.block_states_required == 0
    }
}

/// Recursively checks saved-item ID/meta representability without modifying the NBT tree.
pub fn check_legacy_numeric_saved_items(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
) -> Result<LegacySavedItemCheckReport> {
    let mut report = LegacySavedItemCheckReport::default();
    check_tag(nbt, table, "$", &mut report)?;
    Ok(report)
}

fn check_tag(
    tag: &NbtTag,
    table: &LegacySavedItemIdTable,
    path: &str,
    report: &mut LegacySavedItemCheckReport,
) -> Result<()> {
    match tag {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            check_item(root, table, path, report)?;
            for (name, child) in root {
                if name == "Block" {
                    continue;
                }
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = child_path(path, name);
                    check_tag(child, table, &child_path, report)?;
                }
            }
        }
        NbtTag::Compound(root) => {
            for (name, child) in root {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = child_path(path, name);
                    check_tag(child, table, &child_path, report)?;
                }
            }
        }
        NbtTag::List(values) => {
            for (index, child) in values.iter().enumerate() {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = format!("{path}[{index}]");
                    check_tag(child, table, &child_path, report)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_item(
    root: &IndexMap<String, NbtTag>,
    table: &LegacySavedItemIdTable,
    path: &str,
    report: &mut LegacySavedItemCheckReport,
) -> Result<()> {
    report.items_seen = report.items_seen.saturating_add(1);
    match read_item_id(root)? {
        SavedItemId::Numeric(_) => {
            report.already_numeric = report.already_numeric.saturating_add(1);
            Ok(())
        }
        SavedItemId::Named(name) => {
            let item = NamedSavedItemId {
                name,
                meta: read_item_meta(root)?,
            };
            match table.match_numeric(&item) {
                LegacySavedItemMatch::Missing => {
                    report.named_missing = report.named_missing.saturating_add(1);
                    report.issues.push(LegacySavedItemIssue {
                        path: path.to_string(),
                        item,
                        kind: LegacySavedItemIssueKind::MissingNumericId,
                    });
                }
                LegacySavedItemMatch::Ambiguous { first, second } => {
                    report.named_ambiguous = report.named_ambiguous.saturating_add(1);
                    report.issues.push(LegacySavedItemIssue {
                        path: path.to_string(),
                        item,
                        kind: LegacySavedItemIssueKind::AmbiguousNumericId { first, second },
                    });
                }
                LegacySavedItemMatch::Unique(legacy) => {
                    report.named_unique = report.named_unique.saturating_add(1);
                    if matches!(root.get("Block"), Some(NbtTag::Compound(_))) {
                        report.block_states_required =
                            report.block_states_required.saturating_add(1);
                        report.issues.push(LegacySavedItemIssue {
                            path: path.to_string(),
                            item,
                            kind: LegacySavedItemIssueKind::BlockStateRequired { item: legacy },
                        });
                    } else if let Some(other) = root.get("Block") {
                        return Err(BedrockWorldError::CorruptWorld(format!(
                            "saved item at {path} has non-compound Block payload: {other:?}"
                        )));
                    }
                }
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SavedItemId {
    Numeric(i32),
    Named(String),
}

fn read_item_id(root: &IndexMap<String, NbtTag>) -> Result<SavedItemId> {
    if let Some(value) = root.get("Name") {
        return match value {
            NbtTag::String(name) if !name.is_empty() => Ok(SavedItemId::Named(name.clone())),
            NbtTag::String(_) => Err(validation("saved item Name is empty")),
            other => Err(validation(format!(
                "saved item Name has invalid NBT type: {other:?}"
            ))),
        };
    }
    let value = root
        .get("id")
        .ok_or_else(|| validation("recognised saved item has neither Name nor id"))?;
    match value {
        NbtTag::String(name) if !name.is_empty() => Ok(SavedItemId::Named(name.clone())),
        NbtTag::String(_) => Err(validation("saved item id string is empty")),
        NbtTag::Byte(value) => Ok(SavedItemId::Numeric(i32::from(*value))),
        NbtTag::Short(value) => Ok(SavedItemId::Numeric(i32::from(*value))),
        NbtTag::Int(value) => Ok(SavedItemId::Numeric(*value)),
        NbtTag::Long(value) => i32::try_from(*value)
            .map(SavedItemId::Numeric)
            .map_err(|_| validation("saved item numeric id exceeds i32")),
        other => Err(validation(format!(
            "saved item id has invalid NBT type: {other:?}"
        ))),
    }
}

fn read_item_meta(root: &IndexMap<String, NbtTag>) -> Result<i32> {
    for key in ["Damage", "Aux"] {
        let Some(value) = root.get(key) else {
            continue;
        };
        return integer_i32(value).ok_or_else(|| {
            validation(format!("saved item {key} is not an i32-compatible integer"))
        });
    }
    Ok(0)
}

fn integer_i32(value: &NbtTag) -> Option<i32> {
    match value {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn looks_like_item_stack(root: &IndexMap<String, NbtTag>) -> bool {
    let has_id = root.contains_key("Name") || root.contains_key("id");
    let has_count = matches!(
        root.get("Count"),
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
    );
    has_id && has_count
}

fn child_path(parent: &str, field: &str) -> String {
    if field
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        format!("{parent}.{field}")
    } else {
        format!("{parent}[{field:?}]")
    }
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::SavedItemUpgradeSource;

    fn item(name: &str, damage: i16, with_block: bool) -> NbtTag {
        let mut root = IndexMap::from([
            ("Name".to_string(), NbtTag::String(name.to_string())),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("Damage".to_string(), NbtTag::Short(damage)),
        ]);
        if with_block {
            root.insert("Block".to_string(), NbtTag::Compound(IndexMap::new()));
        }
        NbtTag::Compound(root)
    }

    #[test]
    fn player_tree_reports_unique_missing_and_block_state_requirements() {
        let sources = [SavedItemUpgradeSource {
            name: "0001_test.json",
            json: r#"{"renamedIds":{"minecraft:old":"minecraft:new","minecraft:block_old":"minecraft:block_new"}}"#,
        }];
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1,"minecraft:block_old":2}"#,
            "{}",
            &sources,
        )
        .unwrap();
        let player = NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![
                item("minecraft:new", 5, false),
                item("minecraft:missing", 0, false),
                item("minecraft:block_new", 0, true),
            ]),
        )]));
        let report = check_legacy_numeric_saved_items(&player, &table).unwrap();
        assert_eq!(report.items_seen, 3);
        assert_eq!(report.named_unique, 2);
        assert_eq!(report.named_missing, 1);
        assert_eq!(report.block_states_required, 1);
        assert_eq!(report.issues.len(), 2);
        assert_eq!(report.issues[0].path, "$.Inventory[1]");
        assert_eq!(report.issues[1].path, "$.Inventory[2]");
        assert!(!report.is_fully_proven());
    }

    #[test]
    fn nested_saved_items_are_checked_but_block_state_payload_is_not_walked() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:outer":1,"minecraft:nested":2}"#,
            "{}",
            &[],
        )
        .unwrap();
        let mut outer = match item("minecraft:outer", 0, true) {
            NbtTag::Compound(root) => root,
            _ => unreachable!(),
        };
        outer.insert(
            "tag".to_string(),
            NbtTag::Compound(IndexMap::from([(
                "Nested".to_string(),
                item("minecraft:nested", 0, false),
            )])),
        );
        let report = check_legacy_numeric_saved_items(&NbtTag::Compound(outer), &table).unwrap();
        assert_eq!(report.items_seen, 2);
        assert_eq!(report.named_unique, 2);
        assert_eq!(report.block_states_required, 1);
    }

    #[test]
    fn unusual_compound_name_uses_bracket_path_not_dot_bracket_path() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:test":1}"#,
            "{}",
            &[],
        )
        .unwrap();
        let root = NbtTag::Compound(IndexMap::from([(
            "custom.field".to_string(),
            item("minecraft:missing", 0, false),
        )]));
        let report = check_legacy_numeric_saved_items(&root, &table).unwrap();
        assert_eq!(report.issues[0].path, "$[\"custom.field\"]");
    }
}
