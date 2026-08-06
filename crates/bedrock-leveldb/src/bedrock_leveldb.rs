//! Read-first `LevelDB` access for Minecraft Bedrock world databases.
//!
//! `bedrock-leveldb` can read native Bedrock/LevelDB table, manifest, and WAL
//! files and exposes lazy point lookups plus visitor-based scans over raw byte
//! keys and values. The write path appends standard `LevelDB` write batches to
//! WAL files and flushes native `.ldb` tables plus native manifest edits, while
//! older crate-specific `BWLDB...` files remain readable for migration. Set
//! [`OpenOptions::write_buffer_size`] to `0` to keep writes in the WAL overlay
//! until an explicit flush or compaction.
//!
//! # Logging
//!
//! The crate emits low-noise diagnostics through the [`log`] facade at
//! `trace`, `debug`, and `warn` levels. It never installs a global logger and
//! never writes to stdout or stderr; applications decide whether to connect
//! `env_logger`, `log4rs`, `tracing-log`, or another logging backend.
//!
//! # Errors
//!
//! Errors are returned as [`LevelDbError`]. Prefer matching
//! [`ErrorKind`] through [`LevelDbError::kind`] and using
//! [`LevelDbError::path`] for path-aware recovery instead of parsing display
//! strings.
//!
//! # Bedrock Record Helpers
//!
//! The crate includes small helpers for Bedrock `LevelDB` record keys and legacy
//! terrain payload families. These helpers parse documented storage bytes such
//! as `LegacyTerrain` and pre-paletted `SubChunkPrefix` values, but they do not
//! interpret NBT, actors, players, or gameplay semantics.
//!
//! # Features
//!
//! docs.rs builds this crate with all features enabled. Default builds enable
//! `zlib`, `snappy`, and `async`. The `async` feature depends on Tokio with
//! default features disabled and enables only the runtime pieces needed for
//! `spawn_blocking` wrappers. Optional `mmap` exposes read-only mapped table
//! scans, while `repair-tools` and `bench` are reserved for tooling and
//! benchmark-only paths.
//!
//! # Example
//!
//! ```
//! use bedrock_leveldb::{Db, OpenOptions, VisitorControl};
//!
//! # fn example() -> bedrock_leveldb::Result<()> {
//! let dir = tempfile::tempdir()?;
//! let db = Db::open(dir.path(), OpenOptions::default())?;
//! db.put(b"player_1".as_slice(), b"value".as_slice(), Default::default())?;
//!
//! assert_eq!(db.get(b"player_1")?.as_deref(), Some(b"value".as_slice()));
//!
//! db.for_each_key(Default::default(), |_key| Ok(VisitorControl::Continue))?;
//! # Ok(())
//! # }
//! ```
#![warn(missing_docs)]

mod batch;
mod bedrock;
mod coding;
mod db;
mod error;
mod manifest;
mod options;
mod table;
mod wal;

pub use batch::{WriteBatch, WriteOp};
pub use bedrock::{
    BedrockKey, ChunkCoordinates, ChunkKey, ChunkRecordTag, Dimension,
    LEGACY_SUBCHUNK_MIN_VALUE_LEN, LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN,
    LEGACY_TERRAIN_BLOCK_COUNT, LEGACY_TERRAIN_VALUE_LEN, LegacyBiomeSample, LegacySubChunk,
    LegacyTerrain, SUBCHUNK_BLOCK_COUNT, SubChunkIndex, SubChunkPayload,
};
pub use db::{
    Db, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot, ValueRef,
};
pub use error::{ErrorKind, LevelDbError, Result};
pub use options::{
    CachePolicy, ChecksumMode, CompressionPolicy, OpenOptions, ReadOptions, ReadStrategy,
    ScanCancelFlag, ScanMode, ScanOutcome, ScanPipelineOptions, ScanProgress, ScanProgressSink,
    ThreadingOptions, VisitorControl, WriteOptions,
};
