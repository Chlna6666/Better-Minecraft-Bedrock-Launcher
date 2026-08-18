//! Observed Minecraft Bedrock saved-item storage forms.
//!
//! A plain string-ID item is shared by the Medieval and Modern generations, so this module reports
//! persisted structure first and only reports a generation when the bytes actually prove it.

use super::SavedItemFormat;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

/// Concrete persisted identity form observed in one recognised saved-item compound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SavedItemStorageForm {
    /// Numeric `id`, the identity form used by Classic saves.
    ClassicNumeric,
    /// String `Name`/`id` with metadata and no persisted `Block` BlockState.
    ///
    /// This form alone does not distinguish Medieval from a non-blockitem in Modern data.
    StringIdMeta,
    /// String identity with a persisted `Block` BlockState compound, proving Modern representation.
    ModernBlockState,
}

/// Non-mutating evidence collected from all recognised saved items in one NBT tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SavedItemFormatEvidence {
    /// Recognised saved-item compounds visited.
    pub items_seen: usize,
    /// Numeric-ID saved items observed.
    pub classic_numeric: usize,
    /// String-ID saved items without a `Block` payload.
    pub string_id_meta: usize,
    /// String-ID saved items carrying a `Block` BlockState.
    pub modern_block_state: usize,
    /// Numeric-ID items that also carry `Block`; this is not a canonical known generation.
    pub numeric_with_block: usize,
    /// Compounds containing both `Name` and `id`, retained as conflict evidence.
    pub identity_conflicts: usize,
}

impl SavedItemFormatEvidence {
    /// Returns whether more than one incompatible persisted identity generation is present.
    #[must_use]
    pub const fn has_mixed_identity_generations(&self) -> bool {
        self.classic_numeric > 0 && (self.string_id_meta > 0 || self.modern_block_state > 0)
    }

    /// Returns whether the observed item tree contains a non-canonical identity combination.
    #[must_use]
    pub const fn has_storage_conflicts(&self) -> bool {
        self.identity_conflicts > 0 || self.numeric_with_block > 0
    }

    /// Returns a generation only when the observed storage forms prove it.
    ///
    /// Classic is proven when every recognised item uses numeric identity and no conflicting `Block`
    /// payload exists. Modern is proven when at least one string item carries a persisted `Block`
    /// BlockState and no Classic numeric item is mixed into the tree. Plain string-ID items alone do
    /// not prove Medieval because the same representation is also valid for ordinary Modern items.
    #[must_use]
    pub const fn proven_format(&self) -> Option<SavedItemFormat> {
        if self.items_seen == 0 || self.has_storage_conflicts() {
            return None;
        }
        if self.classic_numeric == self.items_seen {
            return Some(SavedItemFormat::Classic);
        }
        if self.classic_numeric == 0 && self.modern_block_state > 0 {
            return Some(SavedItemFormat::Modern);
        }
        None
    }

    /// Returns the oldest saved-item generation capable of representing the observed canonical forms.
    ///
    /// This is a lower bound, not source-version detection. For example, a tree containing only plain
    /// string-ID items returns Medieval even though the bytes may have been written by a Modern game.
    /// Mixed Classic/string identities or non-canonical conflicts return `None`.
    #[must_use]
    pub const fn minimum_format(&self) -> Option<SavedItemFormat> {
        if self.items_seen == 0
            || self.has_storage_conflicts()
            || self.has_mixed_identity_generations()
        {
            return None;
        }
        if self.classic_numeric > 0 {
            Some(SavedItemFormat::Classic)
        } else if self.modern_block_state > 0 {
            Some(SavedItemFormat::Modern)
        } else if self.string_id_meta > 0 {
            Some(SavedItemFormat::Medieval)
        } else {
            None
        }
    }
}

/// Returns the concrete storage form of one recognised saved-item compound.
///
/// `Ok(None)` means the compound does not look like a saved item. Unknown fields are ignored.
pub fn saved_item_storage_form(
    root: &IndexMap<String, NbtTag>,
) -> Result<Option<SavedItemStorageForm>> {
    if !looks_like_item_stack(root) {
        return Ok(None);
    }
    let block = match root.get("Block") {
        Some(NbtTag::Compound(_)) => true,
        Some(other) => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "saved item Block has unexpected NBT type: {other:?}"
            )));
        }
        None => false,
    };
    if has_string_identity(root)? {
        return Ok(Some(if block {
            SavedItemStorageForm::ModernBlockState
        } else {
            SavedItemStorageForm::StringIdMeta
        }));
    }
    if has_numeric_identity(root)? {
        return Ok(Some(SavedItemStorageForm::ClassicNumeric));
    }
    Ok(None)
}

/// Inspects saved-item storage forms recursively without modifying or normalising the NBT tree.
pub fn inspect_saved_item_formats(nbt: &NbtTag) -> Result<SavedItemFormatEvidence> {
    let mut evidence = SavedItemFormatEvidence::default();
    inspect_tag(nbt, &mut evidence)?;
    Ok(evidence)
}

fn inspect_tag(tag: &NbtTag, evidence: &mut SavedItemFormatEvidence) -> Result<()> {
    match tag {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            evidence.items_seen = evidence.items_seen.saturating_add(1);
            if root.contains_key("Name") && root.contains_key("id") {
                evidence.identity_conflicts = evidence.identity_conflicts.saturating_add(1);
            }
            let form = saved_item_storage_form(root)?.ok_or_else(|| {
                BedrockWorldError::CorruptWorld(
                    "recognised saved item has no supported identity form".to_string(),
                )
            })?;
            match form {
                SavedItemStorageForm::ClassicNumeric => {
                    evidence.classic_numeric = evidence.classic_numeric.saturating_add(1);
                    if root.contains_key("Block") {
                        evidence.numeric_with_block = evidence.numeric_with_block.saturating_add(1);
                    }
                }
                SavedItemStorageForm::StringIdMeta => {
                    evidence.string_id_meta = evidence.string_id_meta.saturating_add(1);
                }
                SavedItemStorageForm::ModernBlockState => {
                    evidence.modern_block_state = evidence.modern_block_state.saturating_add(1);
                }
            }
            for (name, child) in root {
                if name == "Block" {
                    continue;
                }
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    inspect_tag(child, evidence)?;
                }
            }
        }
        NbtTag::Compound(root) => {
            for child in root.values() {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    inspect_tag(child, evidence)?;
                }
            }
        }
        NbtTag::List(values) => {
            for child in values {
                if matches!(child, NbtTag::Compound(_) | NbtTag::List(_)) {
                    inspect_tag(child, evidence)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn looks_like_item_stack(root: &IndexMap<String, NbtTag>) -> bool {
    let has_identity = root.contains_key("Name") || root.contains_key("id");
    let has_count = matches!(
        root.get("Count"),
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
    );
    has_identity && has_count
}

fn has_string_identity(root: &IndexMap<String, NbtTag>) -> Result<bool> {
    if let Some(value) = root.get("Name") {
        return match value {
            NbtTag::String(name) if !name.is_empty() => Ok(true),
            NbtTag::String(_) => Err(validation("saved item Name is empty")),
            other => Err(validation(format!(
                "saved item Name has unexpected NBT type: {other:?}"
            ))),
        };
    }
    match root.get("id") {
        Some(NbtTag::String(name)) if !name.is_empty() => Ok(true),
        Some(NbtTag::String(_)) => Err(validation("saved item string id is empty")),
        _ => Ok(false),
    }
}

fn has_numeric_identity(root: &IndexMap<String, NbtTag>) -> Result<bool> {
    match root.get("id") {
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_)) => Ok(true),
        Some(NbtTag::String(_)) | None => Ok(false),
        Some(other) => Err(validation(format!(
            "saved item id has unexpected NBT type: {other:?}"
        ))),
    }
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(identity: (&str, NbtTag), block: bool) -> NbtTag {
        let mut root = IndexMap::from([
            (identity.0.to_string(), identity.1),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]);
        if block {
            root.insert(
                "Block".to_string(),
                NbtTag::Compound(IndexMap::from([(
                    "name".to_string(),
                    NbtTag::String("minecraft:stone".to_string()),
                )])),
            );
        }
        NbtTag::Compound(root)
    }

    #[test]
    fn plain_string_items_only_prove_a_minimum_not_their_source_generation() {
        let root = NbtTag::List(vec![item(
            ("Name", NbtTag::String("minecraft:stone".to_string())),
            false,
        )]);
        let evidence = inspect_saved_item_formats(&root).unwrap();
        assert_eq!(evidence.string_id_meta, 1);
        assert_eq!(evidence.minimum_format(), Some(SavedItemFormat::Medieval));
        assert_eq!(evidence.proven_format(), None);
    }

    #[test]
    fn block_state_payload_proves_modern_when_classic_identity_is_absent() {
        let root = NbtTag::List(vec![
            item(
                ("Name", NbtTag::String("minecraft:stone".to_string())),
                true,
            ),
            item(
                ("Name", NbtTag::String("minecraft:apple".to_string())),
                false,
            ),
        ]);
        let evidence = inspect_saved_item_formats(&root).unwrap();
        assert_eq!(evidence.modern_block_state, 1);
        assert_eq!(evidence.string_id_meta, 1);
        assert_eq!(evidence.proven_format(), Some(SavedItemFormat::Modern));
        assert_eq!(evidence.minimum_format(), Some(SavedItemFormat::Modern));
    }

    #[test]
    fn numeric_only_tree_proves_classic_identity_generation() {
        let root = NbtTag::List(vec![item(("id", NbtTag::Short(1)), false)]);
        let evidence = inspect_saved_item_formats(&root).unwrap();
        assert_eq!(evidence.proven_format(), Some(SavedItemFormat::Classic));
        assert_eq!(evidence.minimum_format(), Some(SavedItemFormat::Classic));
    }

    #[test]
    fn mixed_numeric_and_string_identities_do_not_claim_one_generation() {
        let root = NbtTag::List(vec![
            item(("id", NbtTag::Short(1)), false),
            item(
                ("Name", NbtTag::String("minecraft:apple".to_string())),
                false,
            ),
        ]);
        let evidence = inspect_saved_item_formats(&root).unwrap();
        assert!(evidence.has_mixed_identity_generations());
        assert_eq!(evidence.proven_format(), None);
        assert_eq!(evidence.minimum_format(), None);
    }

    #[test]
    fn numeric_item_with_block_payload_is_retained_as_conflict_evidence() {
        let root = item(("id", NbtTag::Short(1)), true);
        let evidence = inspect_saved_item_formats(&root).unwrap();
        assert_eq!(evidence.classic_numeric, 1);
        assert_eq!(evidence.numeric_with_block, 1);
        assert!(evidence.has_storage_conflicts());
        assert_eq!(evidence.proven_format(), None);
    }
}
