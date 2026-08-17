//! High-performance storage engine for Mojang's modified LevelDB used by Minecraft Bedrock.
//!
//! This crate is deliberately world-format agnostic. It owns WAL/SST/MANIFEST mechanics, Mojang
//! compression variants, checksums, snapshots, caches, compaction and arbitrary byte key/value I/O.
//! Minecraft chunk keys, NBT, BlockState, actors, dimensions and other game semantics belong to
//! `bedrock-world`.
//!
//! # Public API
//!
//! Common database types are exported directly from the crate root, matching the shape of LevelDB's
//! own public API. Physical implementation modules remain internal and must stay world-format agnostic.
//!
//! ```rust,no_run
//! use bedrock_leveldb::{Db, Options, ReadOptions, WriteBatch, WriteOptions};
//!
//! # fn example() -> bedrock_leveldb::Result<()> {
//! let db = Db::open("path/to/db", Options::default())?;
//! let _ = db.get(b"key")?;
//! let mut batch = WriteBatch::new();
//! batch.put("key", "value");
//! db.write(batch, WriteOptions::default())?;
//! # Ok(())
//! # }
//! ```
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

pub use batch::{WriteBatch, WriteOp};
pub use db::{
    Db, DbCacheStats, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot,
    ValueRef,
};
pub use error::{ErrorKind, LevelDbError, Result};
pub use options::{
    CachePolicy, ChecksumMode, CompressionPolicy, NativeCacheOptions, OpenOptions as Options,
    ReadOptions, ReadStrategy, ScanCancelFlag, ScanMode, ScanOutcome, ScanPipelineOptions,
    ScanProgress, ScanProgressSink, ThreadingOptions, VisitorControl, WriteOptions,
    MAX_LEVELDB_THREADS,
};
