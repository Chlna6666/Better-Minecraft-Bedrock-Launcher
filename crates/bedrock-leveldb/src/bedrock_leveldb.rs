//! High-performance storage engine for Mojang's modified LevelDB used by Minecraft Bedrock.
//!
//! This crate is deliberately world-format agnostic. It owns WAL/SST/MANIFEST mechanics, Mojang
//! compression variants, checksums, snapshots, caches, compaction and arbitrary byte key/value I/O.
//! Minecraft chunk keys, NBT, BlockState, actors, dimensions and other game semantics belong to
//! `bedrock-world`.
#![warn(missing_docs)]

#[path = "format/batch.rs"]
mod batch;
#[path = "format/coding.rs"]
mod coding;
#[path = "engine/impl.rs"]
mod db;
/// Storage-engine error types.
pub mod error;
#[path = "format/manifest.rs"]
mod manifest;
mod options;
#[path = "format/table/impl.rs"]
mod table;
#[path = "format/wal.rs"]
mod wal;

/// Database lifecycle, snapshots, statistics, cache configuration and repair.
pub mod engine;
/// Raw byte reads/scans, borrowed views, cancellation and progress controls.
pub mod access;
/// Mojang LevelDB physical format and write policies.
pub mod format;
/// Storage I/O infrastructure reserved for mmap/file/buffer-pool implementations.
pub mod io;

// Temporary repository-internal migration surface. BMCBL/bedrock-world callers are migrated to the
// grouped modules before these root aliases are deleted from the 0.6 API.
#[doc(hidden)]
pub use batch::{WriteBatch, WriteOp};
#[doc(hidden)]
pub use db::{
    Db, DbCacheStats, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot,
    ValueRef,
};
#[doc(hidden)]
pub use error::{ErrorKind, LevelDbError, Result};
#[doc(hidden)]
pub use options::{
    CachePolicy, ChecksumMode, CompressionPolicy, NativeCacheOptions, OpenOptions, ReadOptions,
    ReadStrategy, ScanCancelFlag, ScanMode, ScanOutcome, ScanPipelineOptions, ScanProgress,
    ScanProgressSink, ThreadingOptions, VisitorControl, WriteOptions,
};
