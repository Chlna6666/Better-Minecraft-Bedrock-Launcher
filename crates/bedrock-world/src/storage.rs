//! Storage abstraction used by `bedrock-world`.
//!
//! This layer owns world-level raw record access and adapters, while Mojang LevelDB internals remain
//! exclusively in `bedrock-leveldb`. The implementation lives under `storage/` for further splitting.

#[path = "storage/impl.rs"]
mod implementation;

pub use implementation::*;

/// Core world-storage traits, entries, batches and read options.
pub mod core {
    pub use super::{
        PartitionedWorldStorage, StorageBatch, StorageCancelFlag, StorageEntry, StorageEntryRef,
        StorageOp, StorageReadOptions, StorageVisitorControl, WorldStorage,
    };
}

/// Scan/pipeline policy, progress and threading controls.
pub mod pipeline {
    pub use super::{
        StorageCachePolicy, StoragePipelineOptions, StorageProgressSink, StorageScanMode,
        StorageScanOutcome, StorageScanProgress, StorageThreadingOptions,
    };
}

/// In-memory and historical Pocket container backends.
pub mod memory {
    pub use super::{
        MemoryStorage, POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN, PocketChunksDatStorage,
    };
}

/// Concrete storage adapters.
pub mod adapters {
    pub use super::backend::BedrockLevelDbStorage;
}
