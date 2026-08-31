//! Minecraft Bedrock world database access.
//!
//! This module owns Bedrock world record scanning and database adapters while delegating Mojang
//! LevelDB engine mechanics to `bedrock-leveldb`. Pre-LevelDB Pocket worlds are opened through the
//! world layer so their missing historical fields remain explicit instead of being normalised into a
//! later LevelDB representation.

mod key_batch;
mod leveldb;
mod pocket_chunks;
mod storage;

pub use crate::chunk::key::{BedrockDbKey, BedrockDbKeyKind, GlobalRecordKind};
pub use crate::parsed::ParsedGlobalData;
pub use key_batch::{StorageKeyBatch, StorageKeyBatchBuilder};
pub use storage::{
    MemoryStorage, PartitionedWorldStorage, StorageBatch, StorageCachePolicy, StorageCancelFlag,
    StorageEntry, StorageEntryRef, StorageOp, StoragePipelineOptions, StorageProgressSink,
    StorageReadOptions, StorageScanMode, StorageScanOutcome, StorageScanProgress,
    StorageThreadingOptions, StorageVisitorControl, WorldStorage,
};

pub(crate) use crate::world::CancelFlag;
#[cfg(feature = "bedrock-leveldb")]
pub use leveldb::BedrockLevelDbStorage;
#[cfg(not(feature = "bedrock-leveldb"))]
pub(crate) use leveldb::BedrockLevelDbStorage;
pub(crate) use leveldb::create_bedrock_leveldb;
pub(crate) use pocket_chunks::PocketChunksDatStorage;
