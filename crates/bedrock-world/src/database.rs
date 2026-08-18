//! Minecraft Bedrock world database access.
//!
//! This module owns Bedrock world record scanning and database adapters while delegating Mojang
//! LevelDB engine mechanics to `bedrock-leveldb`. Pre-LevelDB Pocket worlds are opened through the
//! world layer so their missing historical fields remain explicit instead of being normalised into a
//! later LevelDB representation.

#[cfg(not(feature = "backend-bedrock-leveldb"))]
mod backend_disabled;
mod pocket_chunks;
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
#[cfg(feature = "backend-bedrock-leveldb")]
pub mod backend {
    pub use super::storage::backend::BedrockLevelDbStorage;
}

#[cfg(feature = "backend-bedrock-leveldb")]
pub use backend::BedrockLevelDbStorage;
#[cfg(not(feature = "backend-bedrock-leveldb"))]
pub(crate) use backend_disabled::BedrockLevelDbStorage;
pub(crate) use crate::world::CancelFlag;
pub(crate) use pocket_chunks::PocketChunksDatStorage;
