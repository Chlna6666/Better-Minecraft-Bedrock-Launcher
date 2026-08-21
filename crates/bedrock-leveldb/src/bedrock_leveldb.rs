//! High-performance storage engine for Mojang's modified LevelDB used by Minecraft Bedrock.
//!
//! This crate is deliberately world-format agnostic. It owns WAL/SST/MANIFEST mechanics, Mojang
//! compression variants, checksums, snapshots, caches, compaction and arbitrary byte key/value I/O.
//! Minecraft chunk keys, NBT, BlockState, actors, dimensions and other game semantics belong to
//! `bedrock-world`.
//!
//! # Public API
//!
//! Common database types are exported directly from the crate root, matching LevelDB's public API
//! style. Implementation modules remain private and world-format agnostic.
//!
//! ```rust,no_run
//! use bedrock_leveldb::{Db, LevelDbOpenOptions, ReadOptions, WriteBatch, WriteOptions};
//!
//! # fn example() -> bedrock_leveldb::Result<()> {
//! let db = Db::open("path/to/db", LevelDbOpenOptions::default())?;
//! let _ = db.get(b"key")?;
//! let mut batch = WriteBatch::new();
//! batch.put("key", "value");
//! db.write(batch, WriteOptions::default())?;
//! # Ok(())
//! # }
//! ```
#![warn(missing_docs)]

mod batch;
mod bloom;
mod coding;
mod compaction;
mod compression;
mod db;
mod db_lock;
/// Storage-engine error types.
pub mod error;
mod manifest;
mod native_table_writer;
mod obsolete;
mod options;
mod table;
mod table_cursor;
mod table_scan;
mod version;
mod wal;

pub use batch::{WriteBatch, WriteOp};
pub use db::{
    Db, DbCacheStats, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport,
    Snapshot, ValueRef,
};
pub use error::{ErrorKind, LevelDbError, Result};
pub use options::{
    CachePolicy, ChecksumMode, CompressionPolicy, LevelDbOpenOptions, MAX_LEVELDB_THREADS,
    NativeCacheOptions, ReadOptions, ReadStrategy, ScanCancelFlag, ScanMode, ScanOutcome,
    ScanPipelineOptions, ScanProgress, ScanProgressSink, ThreadingOptions, VisitorControl,
    WriteOptions,
};
