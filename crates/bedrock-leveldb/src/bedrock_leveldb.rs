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
