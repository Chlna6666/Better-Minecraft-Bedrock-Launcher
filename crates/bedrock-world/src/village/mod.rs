//! Bedrock `VILLAGE_*` database entries.

use crate::nbt::NbtTag;
use bytes::Bytes;

pub use crate::chunk::key::{VillageKey, VillageRecordKind};

#[derive(Debug, Clone, PartialEq)]
/// One persisted village database entry.
pub struct Entry {
    /// Decoded storage key for this entry.
    pub key: VillageKey,
    /// Consecutive Bedrock NBT roots decoded from the value.
    pub roots: Vec<NbtTag>,
    /// Original raw value retained for inspection or roundtrip preservation.
    pub raw: Bytes,
}
