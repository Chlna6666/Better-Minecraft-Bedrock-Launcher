//! High-performance storage engine for Mojang's modified LevelDB used by Minecraft Bedrock.
//!
//! This crate is deliberately world-format agnostic. It owns WAL/SST/MANIFEST mechanics, Mojang
//! compression variants, checksums, snapshots, caches, compaction and arbitrary byte key/value I/O.
//! Minecraft chunk keys, NBT, BlockState, actors, dimensions and other game semantics belong to
//! `bedrock-world`.
//!
//! # 0.7 public API
//!
//! Public consumers use responsibility modules only:
//!
//! - [`engine`] for database lifecycle and stateful operations;
//! - [`access`] for raw reads/scans and borrowed views;
//! - [`format`] for write batches and physical format policies;
//! - [`error`] for typed storage errors.
//!
//! The pre-0.7 crate-root re-exports have been removed.
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
