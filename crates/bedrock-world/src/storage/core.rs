//! Core world-storage traits, entries, batches and read options.

pub use super::implementation::{
    PartitionedWorldStorage, StorageBatch, StorageCancelFlag, StorageEntry, StorageEntryRef,
    StorageOp, StorageReadOptions, StorageVisitorControl, WorldStorage,
};
