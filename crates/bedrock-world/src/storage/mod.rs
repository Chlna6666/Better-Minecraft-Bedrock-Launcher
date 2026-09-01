//! Raw Minecraft Bedrock storage contracts and world-storage backends.
//!
//! This module owns Bedrock record storage semantics. Mojang LevelDB engine mechanics remain in the
//! `bedrock-leveldb` crate; this layer only adapts those mechanics to Bedrock world records.

mod key_batch;
mod leveldb;
mod pocket_chunks;
mod raw;

pub use crate::chunk::key::{BedrockDbKey, BedrockDbKeyKind, GlobalRecordKind};
pub use crate::scan::Global;
pub use key_batch::{StorageKeyBatch, StorageKeyBatchBuilder};
pub use raw::{
    MemoryStorage, PartitionedWorldStorage, StorageBatch, StorageCachePolicy, StorageCancelFlag,
    StorageEntry, StorageEntryView, StorageOp, StoragePipelineOptions, StorageProgressSink,
    StorageReadOptions, StorageScanMode, StorageScanOutcome, StorageScanProgress,
    StorageThreadingOptions, StorageVisitorControl, WorldStorage,
};

pub(crate) use crate::surface::CancelFlag;
#[cfg(feature = "bedrock-leveldb")]
pub use leveldb::BedrockLevelDbStorage;
#[cfg(not(feature = "bedrock-leveldb"))]
pub(crate) use leveldb::BedrockLevelDbStorage;
pub(crate) use leveldb::create_bedrock_leveldb;
pub(crate) use pocket_chunks::PocketChunksDatStorage;
pub(crate) use raw::backend;
