//! Explicit conversion of named Minecraft Bedrock saved items to the historical numeric representation.
//!
//! This targets the classic persisted form used by Bedrock <= 1.5: `id` and `Damage` are TAG_Short.
//! Existing numeric saved items are retained as-is; only named items are converted. Unrelated item NBT
//! such as Count, Slot, custom `tag`, CanPlaceOn and future fields is preserved.

use super::legacy_saved_item::{
    LegacySavedItemId, LegacySavedItemIdTable, LegacySavedItemMatch, NamedSavedItemId,
};
use super::legacy_saved_item_check::{
    LegacySavedItemBlockStateTables, LegacySavedItemCheckReport, check_legacy_numeric_saved_items,
    check_legacy_numeric_saved_items_with_blocks,
};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

/// Report for one explicit named -> historical numeric saved-item conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacySavedItemConversionReport {
    /// Preflight report that proved every converted named item before mutation of the cloned tree.
    pub check: LegacySavedItemCheckReport,
    /// Named saved-item compounds converted to TAG_Short `id` + `Damage`.
    pub converted: usize,
    /// Proven modern `Block` payloads removed because the historical numeric item+meta reconstructs them.
    pub block_payloads_removed: usize,
}

/// Converted NBT tree and its conversion report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySavedItemConversionOutcome {
    /// Converted NBT. The source tree passed to the conversion function is never modified.
    pub nbt: NbtTag,
    /// Conversion diagnostics.
    pub report: LegacySavedItemConversionReport,
}

/// Converts named saved items to the historical numeric representation without BlockState context.
///
/// This succeeds only when the preflight is fully proven. Therefore a named item carrying a modern
/// `Block` payload is refused here; use [`convert_saved_items_to_legacy_numeric_with_blocks`] so that
/// the block payload can be independently proven before it is removed.
pub fn convert_saved_items_to_legacy_numeric(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
) -> Result<LegacySavedItemConversionOutcome> {
    let check = check_legacy_numeric_saved_items(nbt, table)?;
    convert_after_check(nbt, table, false, check)
}

/// Converts named saved items to the historical numeric representation with BlockState proof.
///
/// A named blockitem is converted only when the item ID/meta and its persisted `Block` state agree on
/// the same unique historical block identity and metadata. The proven `Block` payload is then removed,
/// because classic numeric blockitems reconstruct the block from the historical item->block mapping and
/// `Damage`. Existing numeric saved items remain untouched.
pub fn convert_saved_items_to_legacy_numeric_with_blocks(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
) -> Result<LegacySavedItemConversionOutcome> {
    let check = check_legacy_numeric_saved_items_with_blocks(nbt, table, blocks)?;
    convert_after_check(nbt, table, true, check)
}

fn convert_after_check(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    remove_proven_block_payloads: bool,
    check: LegacySavedItemCheckReport,
) -> Result<LegacySavedItemConversionOutcome> {
    if !check.is_fully_proven() {
        return Err(BedrockWorldError::Validation(format!(
            "saved-item historical numeric conversion is not fully proven: missing={}, ambiguous={}, block_required={}, block_incompatible={}, first_issue={:?}",
            check.named_missing,
            check.named_ambiguous,
            check.block_states_required,
            check.block_states_incompatible,
            check.issues.first()
        )));
    }

    let mut converted = 0usize;
    let mut block_payloads_removed = 0usize;
    let nbt = convert_tag(
        nbt,
        table,
        remove_proven_block_payloads,
        &mut converted,
        &mut block_payloads_removed,
    )?;
    if converted != check.named_unique {
        return Err(BedrockWorldError::Validation(format!(
            "saved-item conversion changed {converted} named items after preflight proved {}",
            check.named_unique
        )));
    }
    if remove_proven_block_payloads && block_payloads_removed != check.block_states_proven {
        return Err(BedrockWorldError::Validation(format!(
            "saved-item conversion removed {block_payloads_removed} Block payloads after preflight proved {}",
            check.block_states_proven
        )));
    }

    Ok(LegacySavedItemConversionOutcome {
        nbt,
        report: LegacySavedItemConversionReport {
            check,
            converted,
            block_payloads_removed,
        },
    })
}

fn convert_tag(
    tag: &NbtTag,
    table: &LegacySavedItemIdTable,
    remove_proven_block_payloads: bool,
    converted: &mut usize,
    block_payloads_removed: &mut usize,
) -> Result<NbtTag> {
    match tag {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            let mut output = root.clone();
            if let SavedItemId::Named(name) = read_item_id(root)? {
                let named = NamedSavedItemId {
                    name,
                    meta: read_item_meta(root)?,
                };
                let legacy = match table.match_numeric(&named) {
                    LegacySavedItemMatch::Unique(legacy) => legacy,
                    LegacySavedItemMatch::Missing => {
                        return Err(validation(format!(
                            "preflight/conversion mismatch: named item {}:{} no longer has a numeric representation",
                            named.name, named.meta
                        )));
                    }
                    LegacySavedItemMatch::Ambiguous { first, second } => {
                        return Err(validation(format!(
                            "preflight/conversion mismatch: named item {}:{} became ambiguous between {:?} and {:?}",
                            named.name, named.meta, first, second
                        )));
                    }
                };
                write_legacy_identity(&mut output, legacy)?;
                *converted = converted.saturating_add(1);
                if remove_proven_block_payloads && output.shift_remove("Block").is_some() {
                    *block_payloads_removed = block_payloads_removed.saturating_add(1);
                }
            }

            for (name, child) in &mut output {
                if name == "Block" {
                    continue;
                }
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    *child = convert_tag(
                        child,
                        table,
                        remove_proven_block_payloads,
                        converted,
                        block_payloads_removed,
                    )?;
                }
            }
            Ok(NbtTag::Compound(output))
        }
        NbtTag::Compound(root) => {
            let mut output = root.clone();
            for child in output.values_mut() {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    *child = convert_tag(
                        child,
                        table,
                        remove_proven_block_payloads,
                        converted,
                        block_payloads_removed,
                    )?;
                }
            }
            Ok(NbtTag::Compound(output))
        }
        NbtTag::List(values) => Ok(NbtTag::List(
            values
                .iter()
                .map(|child| {
                    convert_tag(
                        child,
                        table,
                        remove_proven_block_payloads,
                        converted,
                        block_payloads_removed,
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn write_legacy_identity(
    root: &mut IndexMap<String, NbtTag>,
    legacy: LegacySavedItemId,
) -> Result<()> {
    let numeric_id = i16::try_from(legacy.numeric_id).map_err(|_| {
        validation(format!(
            "historical saved-item id {} cannot fit Bedrock TAG_Short",
            legacy.numeric_id
        ))
    })?;
    let meta = i16::try_from(legacy.meta).map_err(|_| {
        validation(format!(
            "historical saved-item metadata {} cannot fit Bedrock TAG_Short",
            legacy.meta
        ))
    })?;

    root.shift_remove("Name");
    root.insert("id".to_string(), NbtTag::Short(numeric_id));
    root.insert("Damage".to_string(), NbtTag::Short(meta));
    root.shift_remove("Aux");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SavedItemId {
    Numeric,
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
        NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_) => {
            Ok(SavedItemId::Numeric)
        }
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
        return match value {
            NbtTag::Byte(value) => Ok(i32::from(*value)),
            NbtTag::Short(value) => Ok(i32::from(*value)),
            NbtTag::Int(value) => Ok(*value),
            NbtTag::Long(value) => i32::try_from(*value)
                .map_err(|_| validation(format!("saved item {key} exceeds i32"))),
            other => Err(validation(format!(
                "saved item {key} is not an integer: {other:?}"
            ))),
        };
    }
    Ok(0)
}

fn looks_like_item_stack(root: &IndexMap<String, NbtTag>) -> bool {
    let has_id = root.contains_key("Name") || root.contains_key("id");
    let has_count = matches!(
        root.get("Count"),
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
    );
    has_id && has_count
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::SavedItemUpgradeSource;

    fn named_item(name: &str, damage: i16) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("Name".to_string(), NbtTag::String(name.to_string())),
            ("Count".to_string(), NbtTag::Byte(2)),
            ("Damage".to_string(), NbtTag::Short(damage)),
            ("Slot".to_string(), NbtTag::Byte(9)),
            (
                "tag".to_string(),
                NbtTag::Compound(IndexMap::from([(
                    "FutureField".to_string(),
                    NbtTag::Long(99),
                )])),
            ),
        ]))
    }

    #[test]
    fn named_item_converts_to_short_id_and_damage_without_touching_other_fields() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":300}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
            }],
        )
        .unwrap();
        let source = named_item("minecraft:new", 7);
        let outcome = convert_saved_items_to_legacy_numeric(&source, &table).unwrap();
        let NbtTag::Compound(root) = &outcome.nbt else {
            panic!("item root must stay compound");
        };
        assert_eq!(root.get("id"), Some(&NbtTag::Short(300)));
        assert_eq!(root.get("Damage"), Some(&NbtTag::Short(7)));
        assert!(!root.contains_key("Name"));
        assert_eq!(root.get("Count"), Some(&NbtTag::Byte(2)));
        assert_eq!(root.get("Slot"), Some(&NbtTag::Byte(9)));
        assert!(root.contains_key("tag"));
        assert_eq!(outcome.report.converted, 1);
        assert_eq!(source, named_item("minecraft:new", 7));
    }

    #[test]
    fn existing_numeric_item_is_retained_exactly() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            "{}",
            &[],
        )
        .unwrap();
        let source = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Int(1)),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("Damage".to_string(), NbtTag::Int(3)),
            ("FutureField".to_string(), NbtTag::String("keep".to_string())),
        ]));
        let outcome = convert_saved_items_to_legacy_numeric(&source, &table).unwrap();
        assert_eq!(outcome.nbt, source);
        assert_eq!(outcome.report.converted, 0);
        assert_eq!(outcome.report.check.already_numeric, 1);
    }

    #[test]
    fn unproven_named_blockitem_is_refused_before_conversion() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            r#"{"minecraft:old":"minecraft:old_block"}"#,
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
            }],
        )
        .unwrap();
        let mut root = match named_item("minecraft:new", 0) {
            NbtTag::Compound(root) => root,
            _ => unreachable!(),
        };
        root.insert(
            "Block".to_string(),
            NbtTag::Compound(IndexMap::from([(
                "name".to_string(),
                NbtTag::String("minecraft:new_block".to_string()),
            )])),
        );
        assert!(convert_saved_items_to_legacy_numeric(&NbtTag::Compound(root), &table).is_err());
    }

    #[test]
    fn short_storage_overflow_is_refused_without_mutating_source() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":40000}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
            }],
        )
        .unwrap();
        let source = named_item("minecraft:new", 0);
        assert!(convert_saved_items_to_legacy_numeric(&source, &table).is_err());
        assert!(matches!(
            source,
            NbtTag::Compound(ref root) if matches!(root.get("Name"), Some(NbtTag::String(_)))
        ));
    }
}
