//! Bedrock `VILLAGE_*` database entries and their point-of-interest model.

mod poi;

use crate::nbt::NbtTag;
use bytes::Bytes;

pub use crate::chunk::key::{VillageKey, VillageRecordKind};
pub use poi::{
    InterestInstance, InterestKind, VillagerInterests, java_interest_type, parse_poi_record,
};

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
