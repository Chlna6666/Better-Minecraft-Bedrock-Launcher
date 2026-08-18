//! MCPE <= 1.5 (Classic) saved-item representability and exact target writes.

use super::legacy_saved_item::{
    LegacySavedItemId, LegacySavedItemIdTable, LegacySavedItemMatch, NamedSavedItemId,
};
use super::legacy_saved_item_check::LegacySavedItemBlockStateTables;
use crate::block::{LegacyNumericBlock, LegacyNumericBlockMatch, read_block_state_nbt};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

/// One Classic target-format compatibility problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicSavedItemIssue {
    pub path: String,
    pub kind: ClassicSavedItemIssueKind,
}

/// Reason one saved item is not proven writable in Classic format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassicSavedItemIssueKind {
    ClassicAir,
    NumericIdMissing { numeric_id: i32 },
    MissingNumericId { item: NamedSavedItemId },
    AmbiguousNumericId {
        item: NamedSavedItemId,
        first: LegacySavedItemId,
        second: LegacySavedItemId,
    },
    IdOutOfRange { item: LegacySavedItemId },
    MetadataOutOfRange { item: LegacySavedItemId },
    BlockStateRequired { item: LegacySavedItemId },
    BlockItemMappingMissing { item: LegacySavedItemId },
    BlockNumericMissing { item: LegacySavedItemId },
    BlockNumericAmbiguous {
        item: LegacySavedItemId,
        first: LegacyNumericBlock,
        second: LegacyNumericBlock,
        matches: usize,
    },
    BlockIdentityMismatch {
        item: LegacySavedItemId,
        block: LegacyNumericBlock,
        expected: String,
        actual: String,
    },
    BlockMetadataMismatch {
        item: LegacySavedItemId,
        block: LegacyNumericBlock,
    },
}

/// Non-mutating preflight for an exact Classic saved-item target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassicSavedItemCheckReport {
    pub items_seen: usize,
    pub numeric_sources: usize,
    pub string_sources: usize,
    pub representable: usize,
    pub missing: usize,
    pub ambiguous: usize,
    pub id_out_of_range: usize,
    pub metadata_out_of_range: usize,
    pub block_states_proven: usize,
    pub block_states_required: usize,
    pub block_states_incompatible: usize,
    pub issues: Vec<ClassicSavedItemIssue>,
}

impl ClassicSavedItemCheckReport {
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.missing == 0
            && self.ambiguous == 0
            && self.id_out_of_range == 0
            && self.metadata_out_of_range == 0
            && self.block_states_required == 0
            && self.block_states_incompatible == 0
    }
}

/// Result of explicitly rewriting a tree to exact Classic saved-item representation.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassicSavedItemConversionOutcome {
    pub nbt: NbtTag,
    pub report: ClassicSavedItemConversionReport,
}

/// Counters for an exact Classic conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassicSavedItemConversionReport {
    pub check: ClassicSavedItemCheckReport,
    pub items_changed: usize,
    pub block_payloads_removed: usize,
}

/// Checks whether every recognised saved item can be written as Classic TAG_Short id + Damage.
pub fn check_saved_items_for_classic(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
) -> Result<ClassicSavedItemCheckReport> {
    check_saved_items_for_classic_inner(nbt, table, None)
}

/// Checks Classic representation including modern blockitem `Block` payloads.
pub fn check_saved_items_for_classic_with_blocks(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
) -> Result<ClassicSavedItemCheckReport> {
    check_saved_items_for_classic_inner(nbt, table, Some(blocks))
}

/// Explicitly rewrites all recognised saved items to exact Classic representation.
pub fn convert_saved_items_to_classic(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
) -> Result<ClassicSavedItemConversionOutcome> {
    let check = check_saved_items_for_classic(nbt, table)?;
    convert_after_check(nbt, table, false, check)
}

/// Explicitly rewrites all recognised saved items to Classic representation with blockitem proof.
pub fn convert_saved_items_to_classic_with_blocks(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
) -> Result<ClassicSavedItemConversionOutcome> {
    let check = check_saved_items_for_classic_with_blocks(nbt, table, blocks)?;
    convert_after_check(nbt, table, true, check)
}

fn check_saved_items_for_classic_inner(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<ClassicSavedItemCheckReport> {
    let mut report = ClassicSavedItemCheckReport::default();
    check_tag(nbt, table, blocks, "$", &mut report)?;
    Ok(report)
}

fn check_tag(
    tag: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    path: &str,
    report: &mut ClassicSavedItemCheckReport,
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
    report: &mut ClassicSavedItemCheckReport,
) -> Result<()> {
    report.items_seen = report.items_seen.saturating_add(1);
    let meta = read_item_meta(root)?;
    let target = match read_item_id(root)? {
        SourceItemId::Numeric(0) => {
            report.numeric_sources = report.numeric_sources.saturating_add(1);
            report.missing = report.missing.saturating_add(1);
            report.issues.push(ClassicSavedItemIssue {
                path: path.to_string(),
                kind: ClassicSavedItemIssueKind::ClassicAir,
            });
            return Ok(());
        }
        SourceItemId::Numeric(numeric_id) => {
            report.numeric_sources = report.numeric_sources.saturating_add(1);
            let target = LegacySavedItemId { numeric_id, meta };
            if table.legacy_item_name(target).is_none() {
                report.missing = report.missing.saturating_add(1);
                report.issues.push(ClassicSavedItemIssue {
                    path: path.to_string(),
                    kind: ClassicSavedItemIssueKind::NumericIdMissing { numeric_id },
                });
                return Ok(());
            }
            target
        }
        SourceItemId::Named(name) => {
            report.string_sources = report.string_sources.saturating_add(1);
            let source = NamedSavedItemId { name, meta };
            match table.match_numeric(&source) {
                LegacySavedItemMatch::Missing => {
                    report.missing = report.missing.saturating_add(1);
                    report.issues.push(ClassicSavedItemIssue {
                        path: path.to_string(),
                        kind: ClassicSavedItemIssueKind::MissingNumericId { item: source },
                    });
                    return Ok(());
                }
                LegacySavedItemMatch::Ambiguous { first, second } => {
                    report.ambiguous = report.ambiguous.saturating_add(1);
                    report.issues.push(ClassicSavedItemIssue {
                        path: path.to_string(),
                        kind: ClassicSavedItemIssueKind::AmbiguousNumericId {
                            item: source,
                            first,
                            second,
                        },
                    });
                    return Ok(());
                }
                LegacySavedItemMatch::Unique(target) => target,
            }
        }
    };

    if i16::try_from(target.numeric_id).is_err() {
        report.id_out_of_range = report.id_out_of_range.saturating_add(1);
        report.issues.push(ClassicSavedItemIssue {
            path: path.to_string(),
            kind: ClassicSavedItemIssueKind::IdOutOfRange { item: target },
        });
        return Ok(());
    }
    if i16::try_from(target.meta).is_err() {
        report.metadata_out_of_range = report.metadata_out_of_range.saturating_add(1);
        report.issues.push(ClassicSavedItemIssue {
            path: path.to_string(),
            kind: ClassicSavedItemIssueKind::MetadataOutOfRange { item: target },
        });
        return Ok(());
    }
    report.representable = report.representable.saturating_add(1);

    match root.get("Block") {
        Some(block @ NbtTag::Compound(_)) => {
            if let Some(blocks) = blocks {
                check_block(block, target, table, blocks, path, report)?;
            } else {
                report.block_states_required = report.block_states_required.saturating_add(1);
                report.issues.push(ClassicSavedItemIssue {
                    path: path.to_string(),
                    kind: ClassicSavedItemIssueKind::BlockStateRequired { item: target },
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
    Ok(())
}

fn check_block(
    block_tag: &NbtTag,
    target: LegacySavedItemId,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
    path: &str,
    report: &mut ClassicSavedItemCheckReport,
) -> Result<()> {
    let Some(expected) = table.legacy_block_id(target) else {
        return block_issue(
            report,
            path,
            ClassicSavedItemIssueKind::BlockItemMappingMissing { item: target },
        );
    };
    let state = read_block_state_nbt(block_tag)?;
    let block = match blocks.upgraded.match_numeric(&state) {
        LegacyNumericBlockMatch::Missing => {
            return block_issue(
                report,
                path,
                ClassicSavedItemIssueKind::BlockNumericMissing { item: target },
            );
        }
        LegacyNumericBlockMatch::Ambiguous {
            first,
            second,
            matches,
        } => {
            return block_issue(
                report,
                path,
                ClassicSavedItemIssueKind::BlockNumericAmbiguous {
                    item: target,
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
        .get(block.numeric_id, block.metadata)
        .ok_or_else(|| validation("forward-verified block candidate is absent from raw table"))?;
    if source.name.as_str() != expected {
        return block_issue(
            report,
            path,
            ClassicSavedItemIssueKind::BlockIdentityMismatch {
                item: target,
                block,
                expected: expected.to_string(),
                actual: source.name.clone(),
            },
        );
    }
    if i32::try_from(block.metadata).ok() != Some(target.meta) {
        return block_issue(
            report,
            path,
            ClassicSavedItemIssueKind::BlockMetadataMismatch {
                item: target,
                block,
            },
        );
    }
    report.block_states_proven = report.block_states_proven.saturating_add(1);
    Ok(())
}

fn block_issue(
    report: &mut ClassicSavedItemCheckReport,
    path: &str,
    kind: ClassicSavedItemIssueKind,
) -> Result<()> {
    report.block_states_incompatible = report.block_states_incompatible.saturating_add(1);
    report.issues.push(ClassicSavedItemIssue {
        path: path.to_string(),
        kind,
    });
    Ok(())
}

fn convert_after_check(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    remove_blocks: bool,
    check: ClassicSavedItemCheckReport,
) -> Result<ClassicSavedItemConversionOutcome> {
    if !check.is_fully_proven() {
        return Err(validation(format!(
            "Classic saved-item conversion is not fully proven: missing={}, ambiguous={}, id_range={}, meta_range={}, block_required={}, block_incompatible={}, first_issue={:?}",
            check.missing,
            check.ambiguous,
            check.id_out_of_range,
            check.metadata_out_of_range,
            check.block_states_required,
            check.block_states_incompatible,
            check.issues.first()
        )));
    }
    let mut items_changed = 0usize;
    let mut block_payloads_removed = 0usize;
    let converted = convert_tag(
        nbt,
        table,
        remove_blocks,
        &mut items_changed,
        &mut block_payloads_removed,
    )?;
    if remove_blocks && block_payloads_removed != check.block_states_proven {
        return Err(validation(format!(
            "Classic conversion removed {block_payloads_removed} Block payloads after preflight proved {}",
            check.block_states_proven
        )));
    }
    Ok(ClassicSavedItemConversionOutcome {
        nbt: converted,
        report: ClassicSavedItemConversionReport {
            check,
            items_changed,
            block_payloads_removed,
        },
    })
}

fn convert_tag(
    tag: &NbtTag,
    table: &LegacySavedItemIdTable,
    remove_blocks: bool,
    items_changed: &mut usize,
    block_payloads_removed: &mut usize,
) -> Result<NbtTag> {
    match tag {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            let meta = read_item_meta(root)?;
            let target = match read_item_id(root)? {
                SourceItemId::Numeric(numeric_id) => LegacySavedItemId { numeric_id, meta },
                SourceItemId::Named(name) => table
                    .match_numeric(&NamedSavedItemId { name, meta })
                    .unique()
                    .ok_or_else(|| validation("Classic preflight/conversion string mismatch"))?,
            };
            let id = i16::try_from(target.numeric_id)
                .map_err(|_| validation("Classic preflight/conversion ID width mismatch"))?;
            let damage = i16::try_from(target.meta)
                .map_err(|_| validation("Classic preflight/conversion metadata width mismatch"))?;
            let mut output = root.clone();
            output.shift_remove("Name");
            output.insert("id".to_string(), NbtTag::Short(id));
            output.insert("Damage".to_string(), NbtTag::Short(damage));
            output.shift_remove("Aux");
            if remove_blocks && output.shift_remove("Block").is_some() {
                *block_payloads_removed = block_payloads_removed.saturating_add(1);
            }
            for (name, child) in &mut output {
                if name == "Block" {
                    continue;
                }
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    *child = convert_tag(
                        child,
                        table,
                        remove_blocks,
                        items_changed,
                        block_payloads_removed,
                    )?;
                }
            }
            if &output != root {
                *items_changed = items_changed.saturating_add(1);
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
                        remove_blocks,
                        items_changed,
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
                        remove_blocks,
                        items_changed,
                        block_payloads_removed,
                    )
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        other => Ok(other.clone()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceItemId {
    Numeric(i32),
    Named(String),
}

fn read_item_id(root: &IndexMap<String, NbtTag>) -> Result<SourceItemId> {
    if let Some(value) = root.get("Name") {
        return match value {
            NbtTag::String(name) if !name.is_empty() => Ok(SourceItemId::Named(name.clone())),
            NbtTag::String(_) => Err(validation("saved item Name is empty")),
            other => Err(validation(format!("saved item Name has invalid type: {other:?}"))),
        };
    }
    let value = root
        .get("id")
        .ok_or_else(|| validation("recognised saved item has neither Name nor id"))?;
    match value {
        NbtTag::String(name) if !name.is_empty() => Ok(SourceItemId::Named(name.clone())),
        NbtTag::String(_) => Err(validation("saved item string id is empty")),
        NbtTag::Byte(value) => Ok(SourceItemId::Numeric(i32::from(*value))),
        NbtTag::Short(value) => Ok(SourceItemId::Numeric(i32::from(*value))),
        NbtTag::Int(value) => Ok(SourceItemId::Numeric(*value)),
        NbtTag::Long(value) => i32::try_from(*value)
            .map(SourceItemId::Numeric)
            .map_err(|_| validation("saved item numeric id exceeds i32")),
        other => Err(validation(format!("saved item id has invalid type: {other:?}"))),
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
            other => Err(validation(format!("saved item {key} is not integer: {other:?}"))),
        };
    }
    Ok(0)
}

fn looks_like_item_stack(root: &IndexMap<String, NbtTag>) -> bool {
    (root.contains_key("Name") || root.contains_key("id"))
        && matches!(
            root.get("Count"),
            Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
        )
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

    #[test]
    fn existing_int_numeric_item_is_normalized_to_exact_classic_short_tags() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:stone":1}"#,
            "{}",
            &[],
        )
        .unwrap();
        let source = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Int(1)),
            ("Damage".to_string(), NbtTag::Int(3)),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("FutureField".to_string(), NbtTag::Long(9)),
        ]));
        let outcome = convert_saved_items_to_classic(&source, &table).unwrap();
        let NbtTag::Compound(root) = outcome.nbt else { panic!("compound") };
        assert_eq!(root.get("id"), Some(&NbtTag::Short(1)));
        assert_eq!(root.get("Damage"), Some(&NbtTag::Short(3)));
        assert_eq!(root.get("FutureField"), Some(&NbtTag::Long(9)));
        assert_eq!(outcome.report.items_changed, 1);
    }

    #[test]
    fn named_item_uses_unique_forward_verified_numeric_identity() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":4}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
            }],
        )
        .unwrap();
        let source = NbtTag::Compound(IndexMap::from([
            ("Name".to_string(), NbtTag::String("minecraft:new".to_string())),
            ("Damage".to_string(), NbtTag::Short(2)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let outcome = convert_saved_items_to_classic(&source, &table).unwrap();
        let NbtTag::Compound(root) = outcome.nbt else { panic!("compound") };
        assert_eq!(root.get("id"), Some(&NbtTag::Short(4)));
        assert!(!root.contains_key("Name"));
    }

    #[test]
    fn unknown_existing_numeric_id_is_not_blessed_as_classic_vanilla_data() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:stone":1}"#,
            "{}",
            &[],
        )
        .unwrap();
        let source = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Short(777)),
            ("Damage".to_string(), NbtTag::Short(0)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let report = check_saved_items_for_classic(&source, &table).unwrap();
        assert_eq!(report.missing, 1);
        assert!(convert_saved_items_to_classic(&source, &table).is_err());
    }

    #[test]
    fn classic_air_id_zero_is_refused() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:air":0}"#,
            "{}",
            &[],
        )
        .unwrap();
        let source = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Short(0)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let report = check_saved_items_for_classic(&source, &table).unwrap();
        assert_eq!(report.missing, 1);
        assert!(matches!(
            report.issues[0].kind,
            ClassicSavedItemIssueKind::ClassicAir
        ));
    }
}
