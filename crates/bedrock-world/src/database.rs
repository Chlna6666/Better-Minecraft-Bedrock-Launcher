//! Minecraft Bedrock world database access.
//!
//! This module owns Bedrock world record scanning and database adapters while delegating Mojang
//! LevelDB engine mechanics to `bedrock-leveldb`. Pre-LevelDB Pocket worlds are opened through the
//! world layer so their missing historical fields remain explicit instead of being normalised into a
//! later LevelDB representation.

mod storage;

pub use crate::chunk::key::{BedrockDbKey, BedrockDbKeyKind, GlobalRecordKind};
pub use crate::parsed::ParsedGlobalData;
pub use storage::{
    MemoryStorage, PartitionedWorldStorage, StorageBatch, StorageCachePolicy, StorageCancelFlag,
    StorageEntry, StorageEntryRef, StorageOp, StoragePipelineOptions, StorageProgressSink,
    StorageReadOptions, StorageScanMode, StorageScanOutcome, StorageScanProgress,
    StorageThreadingOptions, StorageVisitorControl, WorldStorage,
};

/// Concrete Mojang LevelDB storage backend.
pub mod backend {
    pub use super::storage::backend::BedrockLevelDbStorage;
}

pub use backend::BedrockLevelDbStorage;
pub(crate) use crate::world::CancelFlag;
