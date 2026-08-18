//! Exact conversion of Modern (MCPE 1.9+) saved items to one concrete older Modern release.
//!
//! This path is intentionally Modern -> older Modern only. Classic numeric source identities are
//! refused instead of being forward-upgraded implicitly. Item IDs, BlockStates and blockitem identity
//! are all resolved through [`ModernSavedItemTarget`] before a converted clone is returned.

use super::{ModernSavedItemTarget, ModernSavedItemTargetMatch, NamedSavedItemId};
use crate::chunk::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ModernSavedItemIssue {
    pub path: String,
    pub kind: ModernSavedItemIssueKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModernSavedItemIssueKind {
    IdentityConflict,
    MetadataConflict { damage: i32, aux: i32 },
    NumericSource { numeric_id: i32 },
    MissingItem { source: NamedSavedItemId },
    AmbiguousItem {
        source: NamedSavedItemId,
        first: NamedSavedItemId,
        second: NamedSavedItemId,
        matches: usize,
    },
    MetadataOutOfRange { target: NamedSavedItemId },
    UnexpectedSourceBlock { target: NamedSavedItemId },
    AmbiguousTargetBlockItem {
        target: NamedSavedItemId,
        first_block: String,
        second_block: String,
        matches: usize,
    },
    SourceBlockRequired {
        target: NamedSavedItemId,
        target_block_id: String,
    },
    MissingBlockState {
        target: NamedSavedItemId,
        target_block_id: String,
    },
    AmbiguousBlockState {
        target: NamedSavedItemId,
        target_block_id: String,
        first: BlockState,
        second: BlockState,
        matches: usize,
    },
    BlockIdentityMismatch {
        target: NamedSavedItemId,
        expected_block_id: String,
        actual_block_id: String,
        block: BlockState,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModernSavedItemCheckReport {
    pub items_seen: usize,
    pub string_sources: usize,
    pub numeric_sources: usize,
    pub identity_conflicts: usize,
    pub metadata_conflicts: usize,
    pub items_proven: usize,
    pub block_items_proven: usize,
    pub item_missing: usize,
    pub item_ambiguous: usize,
    pub metadata_out_of_range: usize,
    pub block_incompatible: usize,
    pub issues: Vec<ModernSavedItemIssue>,
}

impl ModernSavedItemCheckReport {
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.numeric_sources == 0
            && self.identity_conflicts == 0
            && self.metadata_conflicts == 0
            && self.item_missing == 0
            && self.item_ambiguous == 0
            && self.metadata_out_of_range == 0
            && self.block_incompatible == 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModernSavedItemConversionOutcome {
    pub nbt: NbtTag,
    pub report: ModernSavedItemConversionReport,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModernSavedItemConversionReport {
    pub check: ModernSavedItemCheckReport,
    pub items_changed: usize,
    pub block_states_changed: usize,
}

pub fn check_saved_items_for_modern_target(
    nbt: &NbtTag,
    target: &ModernSavedItemTarget,
) -> Result<ModernSavedItemCheckReport> {
    let mut report = ModernSavedItemCheckReport::default();
    check_tag(nbt, target, "$", &mut report)?;
    Ok(report)
}

pub fn convert_saved_items_to_modern_target(
    nbt: &NbtTag,
    target: &ModernSavedItemTarget,
) -> Result<ModernSavedItemConversionOutcome> {
    let check = check_saved_items_for_modern_target(nbt, target)?;
    if !check.is_fully_proven() {
        return Err(validation(format!(
            "Modern saved-item target {} is not fully proven: numeric_sources={}, identity_conflicts={}, metadata_conflicts={}, item_missing={}, item_ambiguous={}, meta_range={}, block_incompatible={}, first_issue={:?}",
            target.target_game_version(),
            check.numeric_sources,
            check.identity_conflicts,
            check.metadata_conflicts,
            check.item_missing,
            check.item_ambiguous,
            check.metadata_out_of_range,
            check.block_incompatible,
            check.issues.first()
        )));
    }

    let mut items_changed = 0usize;
    let mut block_states_changed = 0usize;
    let nbt = convert_tag(nbt, target, &mut items_changed, &mut block_states_changed)?;
    Ok(ModernSavedItemConversionOutcome {
        nbt,
        report: ModernSavedItemConversionReport {
            check,
            items_changed,
            block_states_changed,
        },
    })
}

fn check_tag(
    tag: &NbtTag,
    target: &ModernSavedItemTarget,
    path: &str,
    report: &mut ModernSavedItemCheckReport,
) -> Result<()> {
    match tag {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            check_item(root, target, path, report)?;
            for (name, child) in root {
                if name == "Block" {
                    continue;
                }
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = child_path(path, name);
                    check_tag(child, target, &child_path, report)?;
                }
            }
        }
        NbtTag::Compound(root) => {
            for (name, child) in root {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    let child_path = child_path(path, name);
                    check_tag(child, target, &child_path, report)?;
                }
            }
        }
        NbtTag::List(values) => {
            for (index, child) in values.iter().enumerate() {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    check_tag(child, target, &format!("{path}[{index}]"), report)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_item(
    root: &IndexMap<String, NbtTag>,
    target: &ModernSavedItemTarget,
    path: &str,
    report: &mut ModernSavedItemCheckReport,
) -> Result<()> {
    report.items_seen = report.items_seen.saturating_add(1);
    if root.contains_key("Name") && root.contains_key("id") {
        report.identity_conflicts = report.identity_conflicts.saturating_add(1);
        push_issue(report, path, ModernSavedItemIssueKind::IdentityConflict);
        return Ok(());
    }

    let meta = match read_item_meta(root)? {
        ItemMeta::Conflict { damage, aux } => {
            report.metadata_conflicts = report.metadata_conflicts.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::MetadataConflict { damage, aux },
            );
            return Ok(());
        }
        ItemMeta::Value(meta) => meta,
    };

    let source = match read_item_identity(root, meta)? {
        SourceItemIdentity::Numeric(numeric_id) => {
            report.numeric_sources = report.numeric_sources.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::NumericSource { numeric_id },
            );
            return Ok(());
        }
        SourceItemIdentity::Named(source) => {
            report.string_sources = report.string_sources.saturating_add(1);
            source
        }
    };
    let source_block = read_source_block(root)?;

    match target.match_item(&source, source_block.as_ref())? {
        ModernSavedItemTargetMatch::MissingItem => {
            report.item_missing = report.item_missing.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::MissingItem { source },
            );
        }
        ModernSavedItemTargetMatch::AmbiguousItem {
            first,
            second,
            matches,
        } => {
            report.item_ambiguous = report.item_ambiguous.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::AmbiguousItem {
                    source,
                    first,
                    second,
                    matches,
                },
            );
        }
        ModernSavedItemTargetMatch::Item { item } => {
            if i16::try_from(item.meta).is_err() {
                report.metadata_out_of_range = report.metadata_out_of_range.saturating_add(1);
                push_issue(
                    report,
                    path,
                    ModernSavedItemIssueKind::MetadataOutOfRange { target: item },
                );
            } else {
                report.items_proven = report.items_proven.saturating_add(1);
            }
        }
        ModernSavedItemTargetMatch::BlockItem { item, .. } => {
            if i16::try_from(item.meta).is_err() {
                report.metadata_out_of_range = report.metadata_out_of_range.saturating_add(1);
                push_issue(
                    report,
                    path,
                    ModernSavedItemIssueKind::MetadataOutOfRange { target: item },
                );
            } else {
                report.block_items_proven = report.block_items_proven.saturating_add(1);
            }
        }
        ModernSavedItemTargetMatch::UnexpectedSourceBlock { item } => {
            report.block_incompatible = report.block_incompatible.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::UnexpectedSourceBlock { target: item },
            );
        }
        ModernSavedItemTargetMatch::AmbiguousTargetBlockItem {
            item,
            first_block,
            second_block,
            matches,
        } => {
            report.block_incompatible = report.block_incompatible.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::AmbiguousTargetBlockItem {
                    target: item,
                    first_block,
                    second_block,
                    matches,
                },
            );
        }
        ModernSavedItemTargetMatch::SourceBlockRequired {
            item,
            target_block_id,
        } => {
            report.block_incompatible = report.block_incompatible.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::SourceBlockRequired {
                    target: item,
                    target_block_id,
                },
            );
        }
        ModernSavedItemTargetMatch::MissingBlockState {
            item,
            target_block_id,
        } => {
            report.block_incompatible = report.block_incompatible.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::MissingBlockState {
                    target: item,
                    target_block_id,
                },
            );
        }
        ModernSavedItemTargetMatch::AmbiguousBlockState {
            item,
            target_block_id,
            first,
            second,
            matches,
        } => {
            report.block_incompatible = report.block_incompatible.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::AmbiguousBlockState {
                    target: item,
                    target_block_id,
                    first,
                    second,
                    matches,
                },
            );
        }
        ModernSavedItemTargetMatch::BlockIdentityMismatch {
            item,
            expected_block_id,
            actual_block_id,
            block,
        } => {
            report.block_incompatible = report.block_incompatible.saturating_add(1);
            push_issue(
                report,
                path,
                ModernSavedItemIssueKind::BlockIdentityMismatch {
                    target: item,
                    expected_block_id,
                    actual_block_id,
                    block,
                },
            );
        }
    }
    Ok(())
}

fn convert_tag(
    tag: &NbtTag,
    target: &ModernSavedItemTarget,
    items_changed: &mut usize,
    block_states_changed: &mut usize,
) -> Result<NbtTag> {
    match tag {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            if root.contains_key("Name") && root.contains_key("id") {
                return Err(validation("Modern preflight/conversion identity mismatch"));
            }
            let ItemMeta::Value(meta) = read_item_meta(root)? else {
                return Err(validation("Modern preflight/conversion metadata mismatch"));
            };
            let SourceItemIdentity::Named(source) = read_item_identity(root, meta)? else {
                return Err(validation("Modern preflight/conversion source identity mismatch"));
            };
            let source_block = read_source_block(root)?;
            let (target_item, target_block) = match target.match_item(&source, source_block.as_ref())? {
                ModernSavedItemTargetMatch::Item { item } => (item, None),
                ModernSavedItemTargetMatch::BlockItem { item, block } => (item, Some(block)),
                other => {
                    return Err(validation(format!(
                        "Modern preflight/conversion mismatch for {source:?}: {other:?}"
                    )));
                }
            };

            let damage = i16::try_from(target_item.meta)
                .map_err(|_| validation("Modern preflight/conversion metadata width mismatch"))?;
            let mut output = root.clone();
            output.insert("Name".to_string(), NbtTag::String(target_item.name));
            output.shift_remove("id");
            output.insert("Damage".to_string(), NbtTag::Short(damage));
            output.shift_remove("Aux");
            if let Some(block) = target_block {
                let existing = output.get("Block");
                let encoded = merge_block_state(existing, &block)?;
                if existing != Some(&encoded) {
                    *block_states_changed = block_states_changed.saturating_add(1);
                }
                output.insert("Block".to_string(), encoded);
            }

            for (name, child) in &mut output {
                if name == "Block" {
                    continue;
                }
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    *child = convert_tag(child, target, items_changed, block_states_changed)?;
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
                    *child = convert_tag(child, target, items_changed, block_states_changed)?;
                }
            }
            Ok(NbtTag::Compound(output))
        }
        NbtTag::List(values) => Ok(NbtTag::List(
            values
                .iter()
                .map(|child| convert_tag(child, target, items_changed, block_states_changed))
                .collect::<Result<Vec<_>>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn push_issue(report: &mut ModernSavedItemCheckReport, path: &str, kind: ModernSavedItemIssueKind) {
    report.issues.push(ModernSavedItemIssue {
        path: path.to_string(),
        kind,
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceItemIdentity {
    Numeric(i32),
    Named(NamedSavedItemId),
}

fn read_item_identity(root: &IndexMap<String, NbtTag>, meta: i32) -> Result<SourceItemIdentity> {
    if let Some(value) = root.get("Name") {
        return match value {
            NbtTag::String(name) if !name.is_empty() => Ok(SourceItemIdentity::Named(
                NamedSavedItemId {
                    name: name.clone(),
                    meta,
                },
            )),
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
        NbtTag::String(name) if !name.is_empty() => Ok(SourceItemIdentity::Named(
            NamedSavedItemId {
                name: name.clone(),
                meta,
            },
        )),
        NbtTag::String(_) => Err(validation("saved item string id is empty")),
        NbtTag::Byte(value) => Ok(SourceItemIdentity::Numeric(i32::from(*value))),
        NbtTag::Short(value) => Ok(SourceItemIdentity::Numeric(i32::from(*value))),
        NbtTag::Int(value) => Ok(SourceItemIdentity::Numeric(*value)),
        NbtTag::Long(value) => i32::try_from(*value)
            .map(SourceItemIdentity::Numeric)
            .map_err(|_| validation("saved item numeric id exceeds i32")),
        other => Err(validation(format!(
            "saved item id has invalid NBT type: {other:?}"
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemMeta {
    Value(i32),
    Conflict { damage: i32, aux: i32 },
}

fn read_item_meta(root: &IndexMap<String, NbtTag>) -> Result<ItemMeta> {
    let damage = root
        .get("Damage")
        .map(|value| integer_i32(value, "Damage"))
        .transpose()?;
    let aux = root
        .get("Aux")
        .map(|value| integer_i32(value, "Aux"))
        .transpose()?;
    match (damage, aux) {
        (Some(damage), Some(aux)) if damage != aux => Ok(ItemMeta::Conflict { damage, aux }),
        (Some(value), _) | (_, Some(value)) => Ok(ItemMeta::Value(value)),
        (None, None) => Ok(ItemMeta::Value(0)),
    }
}

fn integer_i32(value: &NbtTag, field: &str) -> Result<i32> {
    match value {
        NbtTag::Byte(value) => Ok(i32::from(*value)),
        NbtTag::Short(value) => Ok(i32::from(*value)),
        NbtTag::Int(value) => Ok(*value),
        NbtTag::Long(value) => i32::try_from(*value)
            .map_err(|_| validation(format!("saved item {field} exceeds i32"))),
        other => Err(validation(format!(
            "saved item {field} is not an integer: {other:?}"
        ))),
    }
}

fn read_source_block(root: &IndexMap<String, NbtTag>) -> Result<Option<BlockState>> {
    let Some(block) = root.get("Block") else {
        return Ok(None);
    };
    crate::block::read_block_state_nbt(block).map(Some)
}

fn merge_block_state(existing: Option<&NbtTag>, state: &BlockState) -> Result<NbtTag> {
    let mut root = match existing {
        Some(NbtTag::Compound(root)) => root.clone(),
        Some(other) => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "saved item Block has unexpected NBT type: {other:?}"
            )));
        }
        None => IndexMap::new(),
    };
    root.insert("name".to_string(), NbtTag::String(state.name.clone()));
    root.insert(
        "states".to_string(),
        NbtTag::Compound(
            state
                .states
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        ),
    );
    let version = state.version.ok_or_else(|| {
        validation(format!(
            "target BlockState {} has no storage version",
            state.name
        ))
    })?;
    root.insert("version".to_string(), NbtTag::Int(version));
    Ok(NbtTag::Compound(root))
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
    use crate::block::{
        AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateVersionTarget,
        VanillaBlockStatePalette,
    };
    use crate::item::{
        SavedItemUpgradeSource, SavedItemVersionTable, VanillaSavedItemBlockMap,
        VanillaSavedItemPalette,
    };
    use crate::version::GameVersion;
    use std::collections::BTreeMap;

    fn block(name: &str, version: i32) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::from([("variant".to_string(), NbtTag::Int(0))]),
            version: Some(version),
        }
    }

    fn target() -> ModernSavedItemTarget {
        let item_rules = SavedItemVersionTable::from_sources(&[
            SavedItemUpgradeSource {
                name: "0001_1.6_beta_to_1.6.0.json",
                json: "{}",
            },
            SavedItemUpgradeSource {
                name: "0011_1.11.4_to_1.12.0.json",
                json: r#"{"renamedIds":{"minecraft:old_apple":"minecraft:apple","minecraft:item.old_door":"minecraft:item.new_door"}}"#,
            },
        ])
        .unwrap();
        let item_palette = VanillaSavedItemPalette::from_required_item_list_json(
            GameVersion::new(vec![1, 9, 0]).unwrap(),
            r#"{
                "minecraft:old_apple":{"runtime_id":1},
                "minecraft:item.old_door":{"runtime_id":2}
            }"#,
        )
        .unwrap();
        let items = item_rules
            .older_target(&GameVersion::new(vec![1, 12, 0]).unwrap(), &item_palette)
            .unwrap();

        let target_block_version =
            crate::block::BlockStateStorageVersion::from_components(1, 9, 0, 1).raw();
        let block_catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_test.json",
            json: r#"{
                "maxVersionMajor":1,"maxVersionMinor":12,"maxVersionPatch":0,"maxVersionRevision":1,
                "renamedIds":{"minecraft:old_door":"minecraft:new_door"}
            }"#,
        }])
        .unwrap();
        let block_palette = VanillaBlockStatePalette::new(
            GameVersion::new(vec![1, 9, 0]).unwrap(),
            vec![block("minecraft:old_door", target_block_version)],
        )
        .unwrap();
        let blocks = BlockStateVersionTarget::build(&block_catalog, &block_palette).unwrap();
        let item_blocks = VanillaSavedItemBlockMap::from_block_id_to_item_id_map_json(
            GameVersion::new(vec![1, 9, 0]).unwrap(),
            r#"{"minecraft:old_door":"minecraft:item.old_door"}"#,
        )
        .unwrap();
        ModernSavedItemTarget::new(items, blocks, item_blocks).unwrap()
    }

    fn item(name: &str, damage: NbtTag, block: Option<NbtTag>) -> NbtTag {
        let mut root = IndexMap::from([
            ("Name".to_string(), NbtTag::String(name.to_string())),
            ("Damage".to_string(), damage),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("FutureField".to_string(), NbtTag::Long(77)),
        ]);
        if let Some(block) = block {
            root.insert("Block".to_string(), block);
        }
        NbtTag::Compound(root)
    }

    fn block_nbt(name: &str, version: i32) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String(name.to_string())),
            (
                "states".to_string(),
                NbtTag::Compound(IndexMap::from([(
                    "variant".to_string(),
                    NbtTag::Int(0),
                )])),
            ),
            ("version".to_string(), NbtTag::Int(version)),
            ("FutureBlockField".to_string(), NbtTag::Long(99)),
        ]))
    }

    #[test]
    fn ordinary_item_rewrites_target_identity_and_preserves_unknown_fields() {
        let source = item("minecraft:apple", NbtTag::Int(4), None);
        let outcome = convert_saved_items_to_modern_target(&source, &target()).unwrap();
        let NbtTag::Compound(root) = outcome.nbt else { panic!("compound") };
        assert_eq!(
            root.get("Name"),
            Some(&NbtTag::String("minecraft:old_apple".to_string()))
        );
        assert_eq!(root.get("Damage"), Some(&NbtTag::Short(4)));
        assert_eq!(root.get("FutureField"), Some(&NbtTag::Long(77)));
        assert_eq!(outcome.report.items_changed, 1);
    }

    #[test]
    fn blockitem_rewrites_target_block_and_preserves_unknown_block_fields() {
        let target = target();
        let source = item(
            "minecraft:item.new_door",
            NbtTag::Short(0),
            Some(block_nbt(
                "minecraft:new_door",
                target.source_block_state_version().raw(),
            )),
        );
        let outcome = convert_saved_items_to_modern_target(&source, &target).unwrap();
        let NbtTag::Compound(root) = outcome.nbt else { panic!("compound") };
        assert_eq!(
            root.get("Name"),
            Some(&NbtTag::String("minecraft:item.old_door".to_string()))
        );
        let Some(NbtTag::Compound(block)) = root.get("Block") else { panic!("Block") };
        assert_eq!(
            block.get("name"),
            Some(&NbtTag::String("minecraft:old_door".to_string()))
        );
        assert_eq!(
            block.get("version"),
            Some(&NbtTag::Int(target.target_block_state_version().raw()))
        );
        assert_eq!(block.get("FutureBlockField"), Some(&NbtTag::Long(99)));
        assert_eq!(outcome.report.block_states_changed, 1);
    }

    #[test]
    fn numeric_source_is_rejected() {
        let source = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Short(1)),
            ("Damage".to_string(), NbtTag::Short(0)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let check = check_saved_items_for_modern_target(&source, &target()).unwrap();
        assert_eq!(check.numeric_sources, 1);
        assert!(!check.is_fully_proven());
    }

    #[test]
    fn identity_and_metadata_conflicts_are_not_normalized() {
        let mut root = match item("minecraft:apple", NbtTag::Short(1), None) {
            NbtTag::Compound(root) => root,
            _ => unreachable!(),
        };
        root.insert("id".to_string(), NbtTag::String("minecraft:apple".to_string()));
        let check = check_saved_items_for_modern_target(&NbtTag::Compound(root), &target()).unwrap();
        assert_eq!(check.identity_conflicts, 1);

        let mut root = match item("minecraft:apple", NbtTag::Short(1), None) {
            NbtTag::Compound(root) => root,
            _ => unreachable!(),
        };
        root.insert("Aux".to_string(), NbtTag::Short(2));
        let check = check_saved_items_for_modern_target(&NbtTag::Compound(root), &target()).unwrap();
        assert_eq!(check.metadata_conflicts, 1);
    }
}
