//! High-performance access to Mojang's modified LevelDB storage used by Minecraft Bedrock.
//!
//! `bedrock-leveldb` is intentionally a **storage-engine** crate. It understands native LevelDB
//! tables, manifests, WAL records, Mojang compression variants, checksums, snapshots, caches,
//! compaction and raw byte key/value operations. It does not interpret Minecraft chunk keys, NBT,
//! BlockState, entities, biomes, dimensions, players or any other game semantics; those belong in
//! the higher-level `bedrock-world` crate.
//!
//! Native Bedrock/LevelDB table, manifest and WAL files are read lazily. Point lookups and visitor
//! scans operate on arbitrary byte keys and values, with borrowed/shared/owned value strategies for
//! zero-copy-oriented consumers. The write path appends native write batches to WAL files and flushes
//! native `.ldb` tables plus manifest edits. Older crate-specific `BWLDB...` files remain readable for
//! migration of databases produced by earlier versions of this crate.
//!
//! # Public API layers
//!
//! Prefer the grouped modules for new code:
//!
//! - [`engine`] contains database lifecycle, snapshots, cache/statistics, iterators, repair and
//!   compaction-facing types.
//! - [`access`] contains read/scan strategies, borrowed raw key/value views and cancellation/progress
//!   controls.
//! - [`format`] contains write batches and physical-format policies such as compression/checksums.
//!
//! Root-level re-exports remain during the 0.6 transition so existing consumers can migrate without
//! an all-at-once source change. Internal WAL/SST/MANIFEST/coding modules intentionally remain private.
//!
//! # Logging
//!
//! The crate emits low-noise diagnostics through the [`log`] facade at `trace`, `debug`, and `warn`
//! levels. It never installs a global logger and never writes to stdout or stderr; applications choose
//! their logging backend.
//!
//! # Errors
//!
//! Errors are returned as [`LevelDbError`]. Prefer matching [`ErrorKind`] through
//! [`LevelDbError::kind`] and using [`LevelDbError::path`] for path-aware recovery instead of parsing
//! display strings.
//!
//! # Compatibility boundary
//!
//! Historical Minecraft versions may change the meaning or layout of raw keys and values. That is
//! deliberately invisible here: if the underlying Mojang LevelDB representation is readable, this
//! crate returns the exact raw bytes. World-format compatibility, historical chunk codecs and
//! migrations are implemented by `bedrock-world`.
//!
//! # Features
//!
//! docs.rs builds this crate with all features enabled. Default builds enable `zlib`, `snappy`, and
//! `async`. The `async` feature depends on Tokio with default features disabled and enables only the
//! runtime pieces needed for `spawn_blocking` wrappers. Optional `mmap` exposes read-only mapped table
//! scans, while `repair-tools` and `bench` are reserved for tooling and benchmark-only paths.
//!
//! # Example
//!
//! ```
//! use bedrock_leveldb::{Db, OpenOptions, VisitorControl};
//!
//! # fn example() -> bedrock_leveldb::Result<()> {
//! let dir = tempfile::tempdir()?;
//! let db = Db::open(dir.path(), OpenOptions::default())?;
//! db.put(b"raw-key".as_slice(), b"raw-value".as_slice(), Default::default())?;
//!
//! assert_eq!(db.get(b"raw-key")?.as_deref(), Some(b"raw-value".as_slice()));
//!
//! db.for_each_key(Default::default(), |_key| Ok(VisitorControl::Continue))?;
//! # Ok(())
//! # }
//! ```
#![warn(missing_docs)]

mod batch;
mod coding;
mod db;
mod error;
mod manifest;
mod options;
mod table;
mod wal;

/// Database engine lifecycle, snapshots, iterators, statistics, cache configuration and repair.
pub mod engine {
    pub use crate::db::{
        Db, DbCacheStats, DbStats, PrefixIterator, RawIterator, RepairReport, Snapshot,
    };
    pub use crate::error::{ErrorKind, LevelDbError, Result};
    pub use crate::options::{CachePolicy, NativeCacheOptions, OpenOptions, ThreadingOptions};
}

/// Raw byte access, borrowed value views and scan/read execution controls.
pub mod access {
    pub use crate::db::{EntryRef, KeyRef, ValueRef};
    pub use crate::options::{
        ReadOptions, ReadStrategy, ScanCancelFlag, ScanMode, ScanOutcome, ScanPipelineOptions,
        ScanProgress, ScanProgressSink, VisitorControl,
    };
}

/// Physical write-batch and Mojang LevelDB format policies.
pub mod format {
    pub use crate::batch::{WriteBatch, WriteOp};
    pub use crate::options::{ChecksumMode, CompressionPolicy, WriteOptions};
}

// Transitional root facade. New code should prefer `engine`, `access` and `format` so the storage
// engine's public surface remains navigable while internal format modules continue to evolve.
pub use batch::{WriteBatch, WriteOp};
pub use db::{
    Db, DbCacheStats, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot,
    ValueRef,
};
pub use error::{ErrorKind, LevelDbError, Result};
pub use options::{
    CachePolicy, ChecksumMode, CompressionPolicy, NativeCacheOptions, OpenOptions, ReadOptions,
    ReadStrategy, ScanCancelFlag, ScanMode, ScanOutcome, ScanPipelineOptions, ScanProgress,
    ScanProgressSink, ThreadingOptions, VisitorControl, WriteOptions,
};
