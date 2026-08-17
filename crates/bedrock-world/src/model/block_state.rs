//! Canonical Bedrock block-state identity helpers.
//!
//! Bedrock stores block-state compounds in NBT. Compound insertion order is not semantic, but
//! byte-level hashing is order-sensitive. This module centralises the canonical representation used
//! by editors, servers and network adapters so every consumer derives the same identity.

use crate::codec::{NbtTag, serialize_root_nbt};
use crate::error::Result;
use crate::model::BlockState;
use indexmap::IndexMap;

impl BlockState {
    /// Returns a canonical NBT representation of the semantic block state.
    ///
    /// The canonical identity contains only the block identifier and named state assignment. The
    /// storage `version` is intentionally excluded: it describes the persisted schema generation,
    /// not the semantic identity of a permutation. State keys are emitted in lexicographic order;
    /// `BlockState::states` is a `BTreeMap`, so the ordering is deterministic regardless of the
    /// original NBT compound insertion order.
    #[must_use]
    pub fn canonical_nbt(&self) -> NbtTag {
        let states = self
            .states
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<IndexMap<_, _>>();
        NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String(self.name.clone())),
            ("states".to_string(), NbtTag::Compound(states)),
        ]))
    }

    /// Serialises [`Self::canonical_nbt`] into stable Bedrock NBT bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        serialize_root_nbt(&self.canonical_nbt())
    }

    /// Returns whether two block states are semantically identical.
    ///
    /// Storage version metadata is ignored. This is suitable for map-editing and palette identity
    /// checks; callers that need to compare the exact persisted representation should use `PartialEq`.
    #[must_use]
    pub fn semantic_eq(&self, other: &Self) -> bool {
        self.name == other.name && self.states == other.states
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn state(version: Option<i32>) -> BlockState {
        BlockState {
            name: "minecraft:test".to_string(),
            states: BTreeMap::from([
                ("b_state".to_string(), NbtTag::Int(2)),
                ("a_state".to_string(), NbtTag::Byte(1)),
            ]),
            version,
        }
    }

    #[test]
    fn canonical_identity_ignores_storage_version() {
        let old = state(Some(17_000_000));
        let current = state(Some(18_168_865));
        assert!(old.semantic_eq(&current));
        assert_eq!(old.canonical_bytes().unwrap(), current.canonical_bytes().unwrap());
    }

    #[test]
    fn canonical_state_keys_are_stable() {
        let state = state(None);
        let NbtTag::Compound(root) = state.canonical_nbt() else {
            panic!("canonical root must be compound");
        };
        let Some(NbtTag::Compound(states)) = root.get("states") else {
            panic!("canonical states must be compound");
        };
        assert_eq!(
            states.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["a_state", "b_state"]
        );
    }
}
