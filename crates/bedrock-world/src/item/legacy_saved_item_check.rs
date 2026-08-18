//! Preflight of named saved items against historical numeric Bedrock item representation.

use super::legacy_saved_item::{
    LegacySavedItemId, LegacySavedItemIdTable, LegacySavedItemMatch, NamedSavedItemId,
};
use crate::block::{
    LegacyNumericBlock, LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
    LegacyNumericBlockUpgradeTable, read_block_state_nbt,
};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

/// Historical numeric block tables used to prove a saved item's persisted `Block` payload.
///
/// `numeric` is the original historical ID/meta table. `upgraded` must be built from that table with
/// authoritative BlockState upgrade rules. Keeping both lets the check prove both the modern semantic
/// BlockState and the exact historical block identifier expected by the saved-item mapping.
#[derive(Debug, Clone, Copy)]
pub struct LegacySavedItemBlockStateTables<'a> {
    /// Historical numeric BlockState source table.
    pub numeric: &'a LegacyNumericBlockStateTable,
    /// Forward-verified reverse lookup built from `numeric`.
    pub upgraded: &'a LegacyNumericBlockUpgradeTable,
}

impl<'a> LegacySavedItemBlockStateTables<'a> {
    /// Creates one paired historical block lookup.
    #[must_use]
    pub const fn new(
        numeric: &'a LegacyNumericBlockStateTable,
        upgraded: &'a LegacyNumericBlockUpgradeTable,
    ) -> Self {
        Self { numeric, upgraded }
    }
}

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The item carries a `Block` BlockState but no block tables were supplied to this check.
    BlockStateRequired {
        /// Unique numeric item ID/meta candidate already proven for the item identity.
        item: LegacySavedItemId,
    },
    /// The unique numeric item candidate is not classified as a historical blockitem.
    BlockItemMappingMissing {
        /// Unique numeric item ID/meta candidate.
        item: LegacySavedItemId,
    },
    /// No historical numeric block candidate forward-upgrades to the persisted `Block` state.
    BlockNumericMissing {
        /// Unique numeric item ID/meta candidate.
        item: LegacySavedItemId,
    },
    /// Multiple historical numeric blocks forward-upgrade to the persisted `Block` state.
    BlockNumericAmbiguous {
        /// Unique numeric item ID/meta candidate.
        item: LegacySavedItemId,
        /// First historical block candidate.
        first: LegacyNumericBlock,
        /// Second historical block candidate.
        second: LegacyNumericBlock,
        /// Total matching historical block candidates.
        matches: usize,
    },
    /// The `Block` payload resolves to a different historical block identifier than the item mapping.
    BlockIdentityMismatch {
        /// Unique numeric item ID/meta candidate.
        item: LegacySavedItemId,
        /// Unique historical block candidate derived from the `Block` payload.
        block: LegacyNumericBlock,
        /// Historical block identifier expected by the item→block table.
        expected: String,
        /// Historical BlockState name stored by the numeric block table.
        actual: String,
    },
    /// The `Block` payload resolves to a different historical metadata value than the item metadata.
    BlockMetadataMismatch {
        /// Unique numeric item ID/meta candidate.
        item: LegacySavedItemId,
        /// Unique historical block candidate derived from the `Block` payload.
        block: LegacyNumericBlock,
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
    /// Block payloads proven to round-trip through the same historical block ID/meta as their item.
    pub block_states_proven: usize,
    /// Unique block items still requiring BlockState reverse validation because no block tables were supplied.
    pub block_states_required: usize,
    /// Block payloads checked with block tables but found incompatible with the historical item candidate.
    pub block_states_incompatible: usize,
    /// Detailed issues; successful ordinary items do not allocate path entries here.
    pub issues: Vec<LegacySavedItemIssue>,
}

impl LegacySavedItemCheckReport {
    /// Returns whether every named item has exactly one numeric ID/meta representation.
    #[must_use]
    pub fn item_ids_are_exact(&self) -> bool {
        self.named_missing == 0 && self.named_ambiguous == 0
    }

    /// Returns whether item IDs and all persisted blockitem states are exactly proven.
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.item_ids_are_exact()
            && self.block_states_required == 0
            && self.block_states_incompatible == 0
    }
}

/// Recursively checks saved-item ID/meta representability without modifying the NBT tree.
///
/// Items carrying a persisted `Block` compound remain reported as [`LegacySavedItemIssueKind::BlockStateRequired`].
/// Use [`check_legacy_numeric_saved_items_with_blocks`] when authoritative block tables are available.
pub fn check_legacy_numeric_saved_items(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
) -> Result<LegacySavedItemCheckReport> {
    check_legacy_numeric_saved_items_inner(nbt, table, None)
}

/// Recursively checks saved-item ID/meta and persisted `Block` BlockState representability.
///
/// A blockitem is proven only when its named item has one historical numeric candidate, its `Block`
/// payload has one forward-verified numeric block candidate, the raw historical block name matches the
/// item→block mapping, and both sides use the same metadata value. No NBT is modified.
pub fn check_legacy_numeric_saved_items_with_blocks(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
) -> Result<LegacySavedItemCheckReport> {
    check_legacy_numeric_saved_items_inner(nbt, table, Some(blocks))
}

fn check_legacy_numeric_saved_items_inner(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<LegacySavedItemCheckReport> {
    let mut report = LegacySavedItemCheckReport::default();
    check_tag(nbt, table, blocks, "$", &mut report)?;
    Ok(report)
}

fn check_tag(
    tag: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    path: &str,
    report: &mut LegacySavedItemCheckReport,
) -> Result<()> {
    match tag {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            check_item(root, table, blocks, path, report)?;
            for (name, child) in root {
                if name == "Block" {
                    continue;
                }
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = child_path(path, name);
                    check_tag(child, table, blocks, &child_path, report)?;
                }
            }
        }
        NbtTag::Compound(root) => {
            for (name, child) in root {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = child_path(path, name);
                    check_tag(child, table, blocks, &child_path, report)?;
                }
            }
        }
        NbtTag::List(values) => {
            for (index, child) in values.iter().enumerate() {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = format!("{path}[{index}]");
                    check_tag(child, table, blocks, &child_path, report)?;
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
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
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
                    match root.get("Block") {
                        Some(block @ NbtTag::Compound(_)) => {
                            if let Some(blocks) = blocks {
                                check_block_state(block, &item, legacy, table, blocks, path, report)?;
                            } else {
                                report.block_states_required =
                                    report.block_states_required.saturating_add(1);
                                report.issues.push(LegacySavedItemIssue {
                                    path: path.to_string(),
                                    item,
                                    kind: LegacySavedItemIssueKind::BlockStateRequired { item: legacy },
                                });
                            }
                        }
                        Some(other) => {
                            return Err(BedrockWorldError::CorruptWorld(format!(
                                "saved item at {path} has non-compound Block payload: {other:?}"
                            )));
                        }
                        None => {}
                    }
                }
            }
            Ok(())
        }
    }
}

fn check_block_state(
    block_tag: &NbtTag,
    item: &NamedSavedItemId,
    legacy_item: LegacySavedItemId,
    item_table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
    path: &str,
    report: &mut LegacySavedItemCheckReport,
) -> Result<()> {
    let Some(expected_block_name) = item_table.legacy_block_id(legacy_item) else {
        return push_block_issue(
            report,
            path,
            item,
            LegacySavedItemIssueKind::BlockItemMappingMissing { item: legacy_item },
        );
    };

    let block_state = read_block_state_nbt(block_tag)?;
    let legacy_block = match blocks.upgraded.match_numeric(&block_state) {
        LegacyNumericBlockMatch::Missing => {
            return push_block_issue(
                report,
                path,
                item,
                LegacySavedItemIssueKind::BlockNumericMissing { item: legacy_item },
            );
        }
        LegacyNumericBlockMatch::Ambiguous {
            first,
            second,
            matches,
        } => {
            return push_block_issue(
                report,
                path,
                item,
                LegacySavedItemIssueKind::BlockNumericAmbiguous {
                    item: legacy_item,
                    first,
                    second,
                    matches,
                },
            );
        }
        LegacyNumericBlockMatch::Unique(block) => block,
    };

    let source = blocks
        .numeric
        .get(legacy_block.numeric_id, legacy_block.metadata)
        .ok_or_else(|| {
            validation(format!(
                "forward-verified block candidate {}:{} is absent from the supplied historical numeric table",
                legacy_block.numeric_id, legacy_block.metadata
            ))
        })?;
    if source.name.as_str() != expected_block_name {
        return push_block_issue(
            report,
            path,
            item,
            LegacySavedItemIssueKind::BlockIdentityMismatch {
                item: legacy_item,
                block: legacy_block,
                expected: expected_block_name.to_string(),
                actual: source.name.clone(),
            },
        );
    }
    if i32::try_from(legacy_block.metadata).ok() != Some(legacy_item.meta) {
        return push_block_issue(
            report,
            path,
            item,
            LegacySavedItemIssueKind::BlockMetadataMismatch {
                item: legacy_item,
                block: legacy_block,
            },
        );
    }

    report.block_states_proven = report.block_states_proven.saturating_add(1);
    Ok(())
}

fn push_block_issue(
    report: &mut LegacySavedItemCheckReport,
    path: &str,
    item: &NamedSavedItemId,
    kind: LegacySavedItemIssueKind,
) -> Result<()> {
    report.block_states_incompatible = report.block_states_incompatible.saturating_add(1);
    report.issues.push(LegacySavedItemIssue {
        path: path.to_string(),
        item: item.clone(),
        kind,
    });
    Ok(())
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
    use crate::block::{
        AuthoritativeBlockStateCatalog, BlockStateSchemaSource, LegacyNumericBlockStateTable,
    };
    use crate::item::SavedItemUpgradeSource;
    use crate::nbt::serialize_root_nbt;
    use std::collections::BTreeMap;

    fn item(name: &str, damage: i16, block: Option<&str>) -> NbtTag {
        let mut root = IndexMap::from([
            ("Name".to_string(), NbtTag::String(name.to_string())),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("Damage".to_string(), NbtTag::Short(damage)),
        ]);
        if let Some(block) = block {
            root.insert(
                "Block".to_string(),
                NbtTag::Compound(IndexMap::from([(
                    "name".to_string(),
                    NbtTag::String(block.to_string()),
                )])),
            );
        }
        NbtTag::Compound(root)
    }

    fn put_var_u32(mut value: u32, output: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            output.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn numeric_block_table(entries: &[(u32, u32, &str, i32)]) -> LegacyNumericBlockStateTable {
        let mut grouped = BTreeMap::<&str, Vec<(u32, i32)>>::new();
        for (_, meta, name, version) in entries {
            grouped.entry(name).or_default().push((*meta, *version));
        }
        let mut bytes = Vec::new();
        put_var_u32(grouped.len() as u32, &mut bytes);
        for (name, metas) in grouped {
            put_var_u32(name.len() as u32, &mut bytes);
            bytes.extend_from_slice(name.as_bytes());
            put_var_u32(metas.len() as u32, &mut bytes);
            for (meta, version) in metas {
                put_var_u32(meta, &mut bytes);
                let nbt = NbtTag::Compound(IndexMap::from([
                    ("name".to_string(), NbtTag::String(name.to_string())),
                    ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                    ("version".to_string(), NbtTag::Int(version)),
                ]));
                bytes.extend_from_slice(&serialize_root_nbt(&nbt).unwrap());
            }
        }
        let ids = entries
            .iter()
            .map(|(id, _, name, _)| format!(r#""{name}":{id}"#))
            .collect::<Vec<_>>()
            .join(",");
        LegacyNumericBlockStateTable::parse(&bytes, &format!("{{{ids}}}")).unwrap()
    }

    fn block_tables() -> (LegacyNumericBlockStateTable, LegacyNumericBlockUpgradeTable) {
        let source_version = 0x0100_0000;
        let numeric = numeric_block_table(&[
            (10, 3, "minecraft:old_block", source_version),
            (11, 3, "minecraft:other_block", source_version),
        ]);
        let catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_test.json",
            json: r#"{"maxVersionMajor":1,"maxVersionMinor":1,"maxVersionPatch":0,"maxVersionRevision":0,"renamedIds":{"minecraft:old_block":"minecraft:new_block","minecraft:other_block":"minecraft:other_new"}}"#,
        }])
        .unwrap();
        let upgraded = LegacyNumericBlockUpgradeTable::build(&numeric, &catalog).unwrap();
        (numeric, upgraded)
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
                item("minecraft:new", 5, None),
                item("minecraft:missing", 0, None),
                item("minecraft:block_new", 0, Some("minecraft:block_new")),
            ]),
        )]));
        let report = check_legacy_numeric_saved_items(&player, &table).unwrap();
        assert_eq!(report.items_seen, 3);
        assert_eq!(report.named_unique, 2);
        assert_eq!(report.named_missing, 1);
        assert_eq!(report.block_states_required, 1);
        assert_eq!(report.block_states_incompatible, 0);
        assert_eq!(report.issues.len(), 2);
        assert_eq!(report.issues[0].path, "$.Inventory[1]");
        assert_eq!(report.issues[1].path, "$.Inventory[2]");
        assert!(!report.is_fully_proven());
    }

    #[test]
    fn blockitem_is_proven_only_when_item_block_name_and_metadata_agree() {
        let item_table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old_item":5}"#,
            r#"{"minecraft:old_item":"minecraft:old_block"}"#,
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old_item":"minecraft:new_item"}}"#,
            }],
        )
        .unwrap();
        let (numeric, upgraded) = block_tables();
        let blocks = LegacySavedItemBlockStateTables::new(&numeric, &upgraded);
        let stack = item("minecraft:new_item", 3, Some("minecraft:new_block"));
        let report = check_legacy_numeric_saved_items_with_blocks(&stack, &item_table, &blocks)
            .unwrap();
        assert_eq!(report.named_unique, 1);
        assert_eq!(report.block_states_proven, 1);
        assert_eq!(report.block_states_incompatible, 0);
        assert!(report.is_fully_proven());
    }

    #[test]
    fn blockitem_rejects_a_different_historical_block_identity() {
        let item_table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old_item":5}"#,
            r#"{"minecraft:old_item":"minecraft:old_block"}"#,
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old_item":"minecraft:new_item"}}"#,
            }],
        )
        .unwrap();
        let (numeric, upgraded) = block_tables();
        let blocks = LegacySavedItemBlockStateTables::new(&numeric, &upgraded);
        let stack = item("minecraft:new_item", 3, Some("minecraft:other_new"));
        let report = check_legacy_numeric_saved_items_with_blocks(&stack, &item_table, &blocks)
            .unwrap();
        assert_eq!(report.block_states_proven, 0);
        assert_eq!(report.block_states_incompatible, 1);
        assert!(matches!(
            &report.issues[0].kind,
            LegacySavedItemIssueKind::BlockIdentityMismatch { .. }
        ));
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
        let mut outer = match item("minecraft:outer", 0, Some("minecraft:outer")) {
            NbtTag::Compound(root) => root,
            _ => unreachable!(),
        };
        outer.insert(
            "tag".to_string(),
            NbtTag::Compound(IndexMap::from([(
                "Nested".to_string(),
                item("minecraft:nested", 0, None),
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
            item("minecraft:missing", 0, None),
        )]));
        let report = check_legacy_numeric_saved_items(&root, &table).unwrap();
        assert_eq!(report.issues[0].path, "$[\"custom.field\"]");
    }
}
