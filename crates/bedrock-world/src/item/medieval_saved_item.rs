//! MCPE 1.6-1.8 (Medieval) saved-item representability and explicit writes.

use super::legacy_saved_item::{
    LegacySavedItemId, LegacySavedItemIdTable, MedievalSavedItemId, MedievalSavedItemMatch,
    NamedSavedItemId,
};
use super::legacy_saved_item_check::LegacySavedItemBlockStateTables;
use crate::block::{LegacyNumericBlock, LegacyNumericBlockMatch, read_block_state_nbt};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

/// One Medieval target-format compatibility problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MedievalSavedItemIssue {
    /// Stable NBT path from the checked root.
    pub path: String,
    /// Problem preventing an exact 1.6-1.8 representation.
    pub kind: MedievalSavedItemIssueKind,
}

/// Reason one saved item is not proven writable in Medieval format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MedievalSavedItemIssueKind {
    /// Classic numeric air ID zero is an invalid persisted item stack and is not invented as Medieval air.
    ClassicAir,
    /// A Classic numeric ID has no authoritative historical string identifier.
    ClassicNumericIdMissing {
        /// Numeric Classic source item ID that could not be resolved.
        numeric_id: i32,
    },
    /// A string source has no proven 1.6.0 ID/meta representation.
    MissingStringId {
        /// Named source item identity that could not be mapped.
        item: NamedSavedItemId,
    },
    /// More than one distinct 1.6.0 ID/meta representation reaches the string source.
    AmbiguousStringId {
        /// Named source item identity being reversed.
        item: NamedSavedItemId,
        /// First candidate Medieval item identity.
        first: MedievalSavedItemId,
        /// Second candidate Medieval item identity.
        second: MedievalSavedItemId,
    },
    /// The Medieval metadata cannot be persisted as TAG_Short `Damage`.
    MetadataOutOfRange {
        /// Candidate Medieval item identity with out-of-range metadata.
        item: MedievalSavedItemId,
    },
    /// A modern `Block` payload exists but no block reverse tables were supplied.
    BlockStateRequired {
        /// Medieval item identity that needs block payload proof.
        item: MedievalSavedItemId,
    },
    /// The Medieval item is not present in the authoritative item->block map at the 1.12 endpoint.
    BlockItemMappingMissing {
        /// Medieval item identity that could not be mapped to a historical block.
        item: MedievalSavedItemId,
    },
    /// The persisted modern BlockState has no historical numeric block representation.
    BlockNumericMissing {
        /// Medieval item identity associated with the missing block mapping.
        item: MedievalSavedItemId,
    },
    /// More than one historical numeric block represents the persisted modern BlockState.
    BlockNumericAmbiguous {
        /// Medieval item identity associated with the ambiguous block mapping.
        item: MedievalSavedItemId,
        /// First candidate historical numeric block.
        first: LegacyNumericBlock,
        /// Second candidate historical numeric block.
        second: LegacyNumericBlock,
        /// Number of matching historical numeric block candidates.
        matches: usize,
    },
    /// The persisted BlockState resolves to a different historical block identifier.
    BlockIdentityMismatch {
        /// Medieval item identity associated with the mismatched block payload.
        item: MedievalSavedItemId,
        /// Candidate historical numeric block resolved from the source BlockState.
        block: LegacyNumericBlock,
        /// Historical block identifier expected by the item->block map.
        expected: String,
        /// Historical block identifier produced by the block reverse mapping.
        actual: String,
    },
    /// The persisted BlockState resolves to a different metadata value than the Medieval item.
    BlockMetadataMismatch {
        /// Medieval item identity associated with the mismatched block metadata.
        item: MedievalSavedItemId,
        /// Candidate historical numeric block resolved from the source BlockState.
        block: LegacyNumericBlock,
    },
}

/// Non-mutating preflight for an exact Medieval saved-item target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MedievalSavedItemCheckReport {
    /// Number of item-like NBT compounds inspected.
    pub items_seen: usize,
    /// Number of inspected items that came from Classic numeric identities.
    pub classic_sources: usize,
    /// Number of inspected items that used string identities.
    pub string_sources: usize,
    /// Number of items proven writable as Medieval string ID/meta values.
    pub representable: usize,
    /// Number of items missing from the Medieval reverse table.
    pub missing: usize,
    /// Number of items with multiple possible Medieval identities.
    pub ambiguous: usize,
    /// Number of target metadata values outside TAG_Short range.
    pub metadata_out_of_range: usize,
    /// Number of modern block payloads proven compatible with Medieval block data.
    pub block_states_proven: usize,
    /// Number of modern block payloads that require block reverse tables before conversion.
    pub block_states_required: usize,
    /// Number of modern block payloads that conflict with Medieval block data.
    pub block_states_incompatible: usize,
    /// Ordered audit trail of individual compatibility issues.
    pub issues: Vec<MedievalSavedItemIssue>,
}

impl MedievalSavedItemCheckReport {
    /// Returns true when no issue counters can block an exact Medieval write.
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.missing == 0
            && self.ambiguous == 0
            && self.metadata_out_of_range == 0
            && self.block_states_required == 0
            && self.block_states_incompatible == 0
    }
}

/// Result of explicitly rewriting a tree to Medieval saved-item representation.
#[derive(Debug, Clone, PartialEq)]
pub struct MedievalSavedItemConversionOutcome {
    /// Converted NBT tree with unrelated fields preserved.
    pub nbt: NbtTag,
    /// Preflight and mutation counters produced by the conversion.
    pub report: MedievalSavedItemConversionReport,
}

/// Counters for an explicit Medieval conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MedievalSavedItemConversionReport {
    /// Non-mutating compatibility report used to authorize the conversion.
    pub check: MedievalSavedItemCheckReport,
    /// Number of item compounds whose identity or metadata changed.
    pub items_changed: usize,
    /// Number of item `Block` payloads removed after block compatibility was proven.
    pub block_payloads_removed: usize,
}

/// Checks whether every recognised saved item can be written in MCPE 1.6-1.8 representation.
pub fn check_saved_items_for_medieval(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
) -> Result<MedievalSavedItemCheckReport> {
    check_saved_items_for_medieval_inner(nbt, table, None)
}

/// Checks Medieval representation including modern blockitem `Block` payloads.
pub fn check_saved_items_for_medieval_with_blocks(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
) -> Result<MedievalSavedItemCheckReport> {
    check_saved_items_for_medieval_inner(nbt, table, Some(blocks))
}

/// Explicitly rewrites all recognised saved items to Medieval string-ID + TAG_Short Damage.
pub fn convert_saved_items_to_medieval(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
) -> Result<MedievalSavedItemConversionOutcome> {
    let check = check_saved_items_for_medieval(nbt, table)?;
    convert_after_check(nbt, table, false, check)
}

/// Explicitly rewrites all recognised saved items to Medieval format with blockitem proof.
pub fn convert_saved_items_to_medieval_with_blocks(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
) -> Result<MedievalSavedItemConversionOutcome> {
    let check = check_saved_items_for_medieval_with_blocks(nbt, table, blocks)?;
    convert_after_check(nbt, table, true, check)
}

fn check_saved_items_for_medieval_inner(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<MedievalSavedItemCheckReport> {
    let mut report = MedievalSavedItemCheckReport::default();
    check_tag(nbt, table, blocks, "$", &mut report)?;
    Ok(report)
}

fn check_tag(
    tag: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    path: &str,
    report: &mut MedievalSavedItemCheckReport,
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
    report: &mut MedievalSavedItemCheckReport,
) -> Result<()> {
    report.items_seen = report.items_seen.saturating_add(1);
    let meta = read_item_meta(root)?;
    let target = match read_item_id(root)? {
        SourceItemId::Numeric(0) => {
            report.classic_sources = report.classic_sources.saturating_add(1);
            report.missing = report.missing.saturating_add(1);
            report.issues.push(MedievalSavedItemIssue {
                path: path.to_string(),
                kind: MedievalSavedItemIssueKind::ClassicAir,
            });
            return Ok(());
        }
        SourceItemId::Numeric(numeric_id) => {
            report.classic_sources = report.classic_sources.saturating_add(1);
            let classic = LegacySavedItemId { numeric_id, meta };
            let Some(target) = table.medieval_id_from_classic(classic) else {
                report.missing = report.missing.saturating_add(1);
                report.issues.push(MedievalSavedItemIssue {
                    path: path.to_string(),
                    kind: MedievalSavedItemIssueKind::ClassicNumericIdMissing { numeric_id },
                });
                return Ok(());
            };
            target
        }
        SourceItemId::Named(name) => {
            report.string_sources = report.string_sources.saturating_add(1);
            let source = NamedSavedItemId { name, meta };
            match table.match_medieval(&source) {
                MedievalSavedItemMatch::Missing => {
                    report.missing = report.missing.saturating_add(1);
                    report.issues.push(MedievalSavedItemIssue {
                        path: path.to_string(),
                        kind: MedievalSavedItemIssueKind::MissingStringId { item: source },
                    });
                    return Ok(());
                }
                MedievalSavedItemMatch::Ambiguous { first, second } => {
                    report.ambiguous = report.ambiguous.saturating_add(1);
                    report.issues.push(MedievalSavedItemIssue {
                        path: path.to_string(),
                        kind: MedievalSavedItemIssueKind::AmbiguousStringId {
                            item: source,
                            first,
                            second,
                        },
                    });
                    return Ok(());
                }
                MedievalSavedItemMatch::Unique(target) => target,
            }
        }
    };

    if i16::try_from(target.meta).is_err() {
        report.metadata_out_of_range = report.metadata_out_of_range.saturating_add(1);
        report.issues.push(MedievalSavedItemIssue {
            path: path.to_string(),
            kind: MedievalSavedItemIssueKind::MetadataOutOfRange { item: target },
        });
        return Ok(());
    }
    report.representable = report.representable.saturating_add(1);

    match root.get("Block") {
        Some(block @ NbtTag::Compound(_)) => {
            if let Some(blocks) = blocks {
                check_block(block, &target, table, blocks, path, report)?;
            } else {
                report.block_states_required = report.block_states_required.saturating_add(1);
                report.issues.push(MedievalSavedItemIssue {
                    path: path.to_string(),
                    kind: MedievalSavedItemIssueKind::BlockStateRequired { item: target },
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
    target: &MedievalSavedItemId,
    table: &LegacySavedItemIdTable,
    blocks: &LegacySavedItemBlockStateTables<'_>,
    path: &str,
    report: &mut MedievalSavedItemCheckReport,
) -> Result<()> {
    let Some(expected) = table.medieval_block_id(target) else {
        return block_issue(
            report,
            path,
            MedievalSavedItemIssueKind::BlockItemMappingMissing {
                item: target.clone(),
            },
        );
    };
    let state = read_block_state_nbt(block_tag)?;
    let block = match blocks.upgraded.match_numeric(&state) {
        LegacyNumericBlockMatch::Missing => {
            return block_issue(
                report,
                path,
                MedievalSavedItemIssueKind::BlockNumericMissing {
                    item: target.clone(),
                },
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
                MedievalSavedItemIssueKind::BlockNumericAmbiguous {
                    item: target.clone(),
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
            MedievalSavedItemIssueKind::BlockIdentityMismatch {
                item: target.clone(),
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
            MedievalSavedItemIssueKind::BlockMetadataMismatch {
                item: target.clone(),
                block,
            },
        );
    }
    report.block_states_proven = report.block_states_proven.saturating_add(1);
    Ok(())
}

fn block_issue(
    report: &mut MedievalSavedItemCheckReport,
    path: &str,
    kind: MedievalSavedItemIssueKind,
) -> Result<()> {
    report.block_states_incompatible = report.block_states_incompatible.saturating_add(1);
    report.issues.push(MedievalSavedItemIssue {
        path: path.to_string(),
        kind,
    });
    Ok(())
}

fn convert_after_check(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    remove_blocks: bool,
    check: MedievalSavedItemCheckReport,
) -> Result<MedievalSavedItemConversionOutcome> {
    if !check.is_fully_proven() {
        return Err(validation(format!(
            "Medieval saved-item conversion is not fully proven: missing={}, ambiguous={}, meta_range={}, block_required={}, block_incompatible={}, first_issue={:?}",
            check.missing,
            check.ambiguous,
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
            "Medieval conversion removed {block_payloads_removed} Block payloads after preflight proved {}",
            check.block_states_proven
        )));
    }
    Ok(MedievalSavedItemConversionOutcome {
        nbt: converted,
        report: MedievalSavedItemConversionReport {
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
                SourceItemId::Numeric(numeric_id) => table
                    .medieval_id_from_classic(LegacySavedItemId { numeric_id, meta })
                    .ok_or_else(|| validation("Medieval preflight/conversion numeric mismatch"))?,
                SourceItemId::Named(name) => table
                    .match_medieval(&NamedSavedItemId { name, meta })
                    .unique()
                    .ok_or_else(|| validation("Medieval preflight/conversion string mismatch"))?,
            };
            let damage = i16::try_from(target.meta)
                .map_err(|_| validation("Medieval preflight/conversion metadata width mismatch"))?;
            let mut output = root.clone();
            output.insert("Name".to_string(), NbtTag::String(target.name));
            output.shift_remove("id");
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
            other => Err(validation(format!(
                "saved item Name has invalid type: {other:?}"
            ))),
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
        other => Err(validation(format!(
            "saved item id has invalid type: {other:?}"
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
                "saved item {key} is not integer: {other:?}"
            ))),
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

    fn item(id: NbtTag, damage: i16) -> NbtTag {
        let key = if matches!(id, NbtTag::String(_)) {
            "Name"
        } else {
            "id"
        };
        NbtTag::Compound(IndexMap::from([
            (key.to_string(), id),
            ("Damage".to_string(), NbtTag::Short(damage)),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("FutureField".to_string(), NbtTag::Long(9)),
        ]))
    }

    #[test]
    fn classic_numeric_is_written_as_1_6_endpoint_string() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:nametag":421}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:nametag":"minecraft:name_tag"}}"#,
            }],
        )
        .unwrap();
        let source = item(NbtTag::Short(421), 0);
        let outcome = convert_saved_items_to_medieval(&source, &table).unwrap();
        let NbtTag::Compound(root) = outcome.nbt else {
            panic!("compound")
        };
        assert_eq!(
            root.get("Name"),
            Some(&NbtTag::String("minecraft:name_tag".to_string()))
        );
        assert_eq!(root.get("Damage"), Some(&NbtTag::Short(0)));
        assert!(!root.contains_key("id"));
        assert_eq!(root.get("FutureField"), Some(&NbtTag::Long(9)));
    }

    #[test]
    fn modern_string_is_reversed_to_proven_medieval_endpoint() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            "{}",
            &[
                SavedItemUpgradeSource {
                    name: "0001_test.json",
                    json: r#"{"renamedIds":{"minecraft:old":"minecraft:medieval"}}"#,
                },
                SavedItemUpgradeSource {
                    name: "0011_test.json",
                    json: r#"{"renamedIds":{"minecraft:medieval":"minecraft:modern"}}"#,
                },
            ],
        )
        .unwrap();
        let source = item(NbtTag::String("minecraft:modern".to_string()), 3);
        let outcome = convert_saved_items_to_medieval(&source, &table).unwrap();
        let NbtTag::Compound(root) = outcome.nbt else {
            panic!("compound")
        };
        assert_eq!(
            root.get("Name"),
            Some(&NbtTag::String("minecraft:medieval".to_string()))
        );
        assert_eq!(root.get("Damage"), Some(&NbtTag::Short(3)));
    }

    #[test]
    fn unproven_later_item_is_refused() {
        let table =
            LegacySavedItemIdTable::from_sources(r#"{"minecraft:old":1}"#, "{}", &[]).unwrap();
        let source = item(NbtTag::String("minecraft:future".to_string()), 0);
        let report = check_saved_items_for_medieval(&source, &table).unwrap();
        assert_eq!(report.missing, 1);
        assert!(!report.is_fully_proven());
        assert!(convert_saved_items_to_medieval(&source, &table).is_err());
    }

    #[test]
    fn target_damage_must_fit_short() {
        let table =
            LegacySavedItemIdTable::from_sources(r#"{"minecraft:old":1}"#, "{}", &[]).unwrap();
        let source = NbtTag::Compound(IndexMap::from([
            (
                "Name".to_string(),
                NbtTag::String("minecraft:old".to_string()),
            ),
            ("Damage".to_string(), NbtTag::Int(40_000)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let report = check_saved_items_for_medieval(&source, &table).unwrap();
        assert_eq!(report.metadata_out_of_range, 1);
        assert!(!report.is_fully_proven());
    }
}
