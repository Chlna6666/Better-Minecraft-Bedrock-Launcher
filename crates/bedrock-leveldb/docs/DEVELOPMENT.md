# Development Guide

[English](DEVELOPMENT.md) | [Simplified Chinese](DEVELOPMENT.zh-CN.md)

This document is for maintainers and contributors working on
`bedrock-leveldb`. The public README explains user-facing behavior; this guide
explains how to change the crate without weakening its storage guarantees,
compatibility boundaries, diagnostics, or release quality.

## Project Boundaries

`bedrock-leveldb` is a raw key/value storage crate. It reads Bedrock/native
LevelDB files and exposes raw keys and raw values. It must not grow gameplay
semantics such as NBT interpretation, player records, actor models, chunk block
state meaning, entity data, or world editing workflows. Those belong in
applications or higher-level domain crates.

The crate is a raw native LevelDB engine layer. Native manifest, WAL, and table
files are read directly. v0.2 writes standard LevelDB write batches to WAL,
flushes native `.ldb` tables, and persists manifest version edits. Older
`BWLDB...` files remain readable for migration/backward compatibility only; new
write paths must not create the custom format.

Bedrock helpers in `bedrock.rs` are storage-layout helpers only. They may parse
documented LevelDB-era chunk keys and legacy payload layouts such as
`LegacyTerrain` and pre-paletted `SubChunkPrefix` values. They must not parse
pre-LevelDB `chunks.dat` / `entities.dat` files or gameplay-level record
semantics.

## Module Map

- `bedrock_leveldb.rs`: crate root, public re-exports, crate-level docs, lint
  policy.
- `db.rs`: `Db`, open/recovery flow, point reads, scans, overlay, snapshots,
  flush, repair entry points, and read-only enforcement.
- `manifest.rs`: native manifest parsing, version edits, and table metadata used
  for range filtering.
- `table.rs`: table footer, index/data block reads, restart arrays,
  decompression, internal key handling, and native table writing.
- `wal.rs`: LevelDB WAL record framing, fragmentation, checksums, and padding.
- `coding.rs`: varints, fixed-width coding helpers, and CRC masking.
- `options.rs`: open/read/write/scan option types and progress/cancel plumbing.
- `error.rs`: structured `LevelDbError`, `ErrorKind`, path/source/context
  helpers, and display formatting.
- `batch.rs`: public write batch representation used by write and WAL paths.
- `bedrock.rs`: documented Bedrock LevelDB key and legacy storage-layout
  helpers.

Keep modules private by default. Expose items from the crate root only when
they are a supported API surface and have rustdoc explaining purpose and error
behavior.

## Environment

Use Rust 1.87 or newer. The crate uses edition 2024 and documents Rust 1.87 as
its MSRV. Do not introduce APIs that require a newer compiler unless
`rust-version`, README, CI, and this guide are updated together.

Default features are `zlib`, `snappy`, and `async`. Feature-specific code must
compile in both default and `--no-default-features` builds. When adding optional
behavior, prefer a feature that removes the dependency entirely when disabled.
The `async` feature intentionally keeps Tokio default features disabled and
enables only the runtime pieces used by `spawn_blocking` wrappers.

This is a library crate. Do not initialize a global logger, and do not use
`println!` or `eprintln!` in library code. Runtime diagnostics must go through
the `log` facade at low-noise levels. Avoid logging raw values and avoid
logging large raw keys.

## API And Error Policy

Public API changes must be intentional, documented, and tested. The crate root
uses missing-docs enforcement for release validation, so every public type,
variant field, constant, and fallible method needs useful rustdoc. Result
returning public APIs should document the important error conditions.

Prefer structured errors over stringly typed failures. New failure modes should
fit `LevelDbError` with a stable `ErrorKind`, path context when file I/O is
involved, and source errors when preserving the original cause is useful.
Callers should be able to match `err.kind()` and inspect `err.path()` without
parsing `Display` output.

Read-only mode is strict. A read-only handle must not create missing
directories, repair files, flush, compact, write WAL records, or create native
tables. Any new mutating path must check read-only behavior and return
`ErrorKind::ReadOnly`.

Cancellation is cooperative and typed. Scan cancellation should return the
dedicated cancelled error, not a generic invalid-argument or I/O error.

## Testing

Tests should cover behavior, feature boundaries, and failure modes. Keep unit
tests close to codec and parser logic, and use integration tests for database
open/read/write/recovery behavior.

Important scenarios to preserve:

- Read-only open does not create, repair, flush, or mutate data.
- Missing and corrupt files carry path context in errors.
- WAL replay handles fragmented records and tombstones.
- Varint decoding rejects overflow and truncation.
- Native flush/reopen preserves keys, values, sequence numbers, and deletions.
- `OpenOptions::write_buffer_size = 0` keeps writes in the WAL overlay until an
  explicit flush or other native recovery/compaction path consumes them.
- Native table point reads and prefix scans honor manifest ranges and deletion
  records.
- `ReadStrategy::Borrowed` callback scans return `ValueRef::Borrowed` for
  uncompressed native blocks and legacy uncompressed `BWLDB...` compatibility
  tables, and return `ValueRef::Shared` for compressed blocks.
- `mmap` unsafe blocks have a local `SAFETY:` comment, remain feature-gated,
  and expose mapped bytes only through callback lifetimes.
- Sequential and table-parallel scans produce the same visible entries.
- Feature-disabled compression reports a typed unsupported/compression error.
- The library does not initialize a logger.

Run the focused feature matrix before publishing or after touching feature-gated
code:

```text
cargo test --all-features
cargo test --no-default-features
cargo test --no-default-features --features zlib
cargo test --no-default-features --features snappy
cargo test --no-default-features --features async
cargo test --no-default-features --features mmap
```

## Benchmarks

Criterion benchmarks are synthetic and should isolate the operation being
measured. Do not include database construction, temporary directory cleanup, or
logger initialization in hot read measurements.

The benchmark groups should stay explicit about what they measure:

- Overlay hot path.
- Flushed native table point reads.
- Native table point reads and prefix scans.
- WAL recovery.
- Sequential versus table-parallel scans.

When updating benchmark numbers in README, record the operating system, date,
Rust version, Criterion sample settings, plot backend, whether a logger backend
was installed, and the synthetic nature of the fixture.

## Release Checklist

Before a public release or release-like commit, run:

```text
cargo fmt --check
cargo clippy --all-features --all-targets -- -D warnings
cargo rustdoc --all-features -- -D missing_docs
cargo test --all-features
cargo test --no-default-features
cargo test --no-default-features --features zlib
cargo test --no-default-features --features snappy
cargo test --no-default-features --features async
cargo test --no-default-features --features mmap
cargo doc --all-features --no-deps
cargo package --allow-dirty
cargo bench --all-features
```

Update `CHANGELOG.md` when behavior, API, compatibility, or release process
changes in a way users or maintainers should notice. Pure wording fixes do not
need a changelog entry unless they change documented guarantees.
