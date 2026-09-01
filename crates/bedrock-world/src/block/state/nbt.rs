//! Minecraft Bedrock BlockState NBT views.
//!
//! These helpers parse the actual persisted `name`, `states` and optional `version` fields without
//! rewriting the source compound or discarding unrelated fields owned by the enclosing record.

use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use std::collections::BTreeMap;

/// Reads one persisted Bedrock BlockState compound.
///
/// `name` is required and non-empty. Missing `states` is treated as an empty permutation, matching
/// historical saved-item Block payloads and numeric tables. `version` is optional because reverse
/// semantic matching deliberately ignores the source storage version. Unknown compound fields are
/// left on the source NBT and are not copied into [`BlockState`].
pub fn read_block_state_nbt(tag: &NbtTag) -> Result<BlockState> {
    let NbtTag::Compound(root) = tag else {
        return Err(validation("BlockState payload is not an NBT compound"));
    };
    let name = match root.get("name") {
        Some(NbtTag::String(name)) if !name.is_empty() => name.clone(),
        Some(NbtTag::String(_)) => return Err(validation("BlockState name is empty")),
        Some(other) => {
            return Err(validation(format!(
                "BlockState name has invalid NBT type: {other:?}"
            )));
        }
        None => return Err(validation("BlockState payload has no name")),
    };
    let states = match root.get("states") {
        Some(NbtTag::Compound(states)) => states
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        Some(other) => {
            return Err(validation(format!(
                "BlockState states has invalid NBT type: {other:?}"
            )));
        }
        None => BTreeMap::new(),
    };
    let version = match root.get("version") {
        Some(NbtTag::Int(version)) => Some(*version),
        Some(other) => {
            return Err(validation(format!(
                "BlockState version has invalid NBT type: {other:?}"
            )));
        }
        None => None,
    };
    Ok(BlockState {
        name,
        states,
        version,
    })
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn reads_block_state_without_mutating_or_requiring_version() {
        let source = NbtTag::Compound(IndexMap::from([
            (
                "name".to_string(),
                NbtTag::String("minecraft:test".to_string()),
            ),
            (
                "states".to_string(),
                NbtTag::Compound(IndexMap::from([(
                    "facing_direction".to_string(),
                    NbtTag::Int(2),
                )])),
            ),
            ("FutureField".to_string(), NbtTag::Long(99)),
        ]));
        let parsed = read_block_state_nbt(&source).unwrap();
        assert_eq!(parsed.name, "minecraft:test");
        assert_eq!(parsed.states.get("facing_direction"), Some(&NbtTag::Int(2)));
        assert_eq!(parsed.version, None);
        let NbtTag::Compound(root) = source else {
            unreachable!()
        };
        assert_eq!(root.get("FutureField"), Some(&NbtTag::Long(99)));
    }

    #[test]
    fn missing_states_is_historical_empty_permutation() {
        let source = NbtTag::Compound(IndexMap::from([(
            "name".to_string(),
            NbtTag::String("minecraft:test".to_string()),
        )]));
        assert!(read_block_state_nbt(&source).unwrap().states.is_empty());
    }

    #[test]
    fn malformed_known_fields_are_rejected() {
        let source = NbtTag::Compound(IndexMap::from([
            (
                "name".to_string(),
                NbtTag::String("minecraft:test".to_string()),
            ),
            ("states".to_string(), NbtTag::Int(0)),
        ]));
        assert!(read_block_state_nbt(&source).is_err());
    }
}
