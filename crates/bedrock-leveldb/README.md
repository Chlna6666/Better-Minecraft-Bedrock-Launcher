# bedrock-leveldb

[English](README.md) | [简体中文](README.zh-CN.md)

`bedrock-leveldb` is a pure Rust raw key/value storage library for
Minecraft Bedrock world databases. The performance target is benchmark-backed
zero-copy where possible, lock-free read hot paths after a short state snapshot,
and explicit owned allocation when callers request it. It focuses on the storage layer only:
chunk, actor, player, and NBT semantics are intentionally out of scope and
belong in application code or domain-specific layers.

The crate can read native Bedrock/LevelDB manifests, WAL records, and table
files. v0.2 writes standard LevelDB WAL batches, flushes native `.ldb` tables,
and persists manifest version edits. Older `BWLDB...` files remain readable for
migration/backward compatibility only.

Maintainers and contributors should also read the
[development guide](docs/DEVELOPMENT.md).

## Quick Start

```rust
use bedrock_leveldb::{
    Db, OpenOptions, ReadOptions, ScanMode, ScanPipelineOptions, VisitorControl, WriteOptions,
};

fn main() -> bedrock_leveldb::Result<()> {
    let db = Db::open("path/to/world/db", OpenOptions::default())?;

    if let Some(value) = db.get(b"player_1")? {
        println!("player_1 has {} raw bytes", value.len());
    }

    let outcome = db.for_each_prefix(
        b"player_",
        ReadOptions {
            scan_mode: ScanMode::ParallelTables,
            pipeline: ScanPipelineOptions {
                queue_depth: 64,
                ..ScanPipelineOptions::default()
            },
            ..ReadOptions::default()
        },
        |key, value| {
            println!("{} -> {} bytes", String::from_utf8_lossy(key), value.len());
            Ok(VisitorControl::Continue)
        },
    )?;

    println!(
        "visited {} entries across {} tables on {} workers",
        outcome.visited, outcome.tables_scanned, outcome.worker_threads
    );

    db.put(b"tool_key".as_slice(), b"tool_value".as_slice(), WriteOptions::default())?;
    Ok(())
}
```

For read-only analysis of real Bedrock worlds, set `OpenOptions::read_only =
true` and `create_if_missing = false`. Read-only handles never initialize,
repair, flush, or write to the database directory.

## Supported Surface

| Area | Status |
| --- | --- |
| Native LevelDB manifest replay | Implemented for the metadata needed to find tables |
| Native LevelDB WAL replay | Implemented for write batches |
| Native LevelDB table reads | Footer, index block, data blocks, restart arrays, internal key trailer |
| Compression reads | Snappy, zlib, and Bedrock raw deflate when features are enabled |
| Lazy point lookup | Implemented with manifest range filtering and seeked block reads |
| Visitor scans | Key, entry, prefix, sequential, and table-parallel modes |
| Native block cache | Bounded decoded block cache |
| Bedrock chunk key helpers | Parse and encode documented LevelDB chunk keys |
| Legacy `LegacyTerrain` values | Validate and expose the 83,200-byte early LevelDB terrain layout, including `[biome_id, red, green, blue]` biome samples |
| Legacy subchunk values | Classify paletted subchunks and expose pre-paletted block ID/metadata arrays |
| Batch exact reads | `Db::get_many_owned` preserves input order for legacy and modern render keys |
| Native writes by this crate | WAL batch append, native `.ldb` flush, manifest edit persistence |
| Production LevelDB compaction | Correctness-first native range compaction |
| Arbitrary corrupt database repair | Partial, writes native recovered output from readable data |
| Pre-LevelDB worlds | Not supported; `chunks.dat` and `entities.dat` are outside this crate |
| `mmap` read path | Feature-gated callback scans can borrow uncompressed custom/native table values |

## API Notes

- `Db::open(path, OpenOptions)` loads `CURRENT`, manifest metadata, and the WAL
  overlay. It does not eagerly materialize every native table value.
- `Db::get(key)` is the compatibility owned/shared read path. `Db::get_ref`
  and `Db::get_with_ref` return `ValueRef`, which can represent borrowed,
  shared, or explicitly owned values. Cross-function point lookups stay shared
  or owned so they cannot return dangling table slices.
- `Db::for_each_entry_ref` and `Db::for_each_prefix_ref` are the true
  borrowed-first APIs. With `ReadStrategy::Borrowed` and sequential scan mode,
  uncompressed native LevelDB blocks return `ValueRef::Borrowed` inside the
  visitor callback. Compressed blocks, WAL/overlay values, and non-callback
  point reads return `Shared`/`Owned`.
  Enabling the `mmap` feature maps table files read-only so those borrowed
  slices are backed by the mapping for the duration of the callback.
- `Db::get_many_owned(keys, ReadOptions)` is the preferred renderer path for
  exact chunk records such as `LegacyTerrain` (`0x30`), `Data2D`, subchunks, and
  block entities. It preserves input order and avoids prefix scans during tile
  rendering. It returns raw values byte-for-byte; X/Y/Z coordinate interpretation
  and legacy biome priority are intentionally tested and implemented in
  `bedrock-world`/`bedrock-render`.
- With the default `async` feature, `Arc<Db>` now provides owned async read
  helpers: `get_async`, `get_with_async`, `collect_keys_owned_async`,
  `collect_prefix_keys_owned_async`, and `collect_prefix_owned_async`. They use
  Tokio `spawn_blocking` and are intended for GUI or server runtimes that must
  keep foreground tasks responsive.
- `Db::collect_keys_owned`, `Db::collect_prefix_keys_owned`, and
  `Db::collect_prefix_owned` return owned data without forcing callers to write
  visitor glue for common indexing paths.
- `Db::write_batch_native`, `Db::flush_memtable`,
  `Db::compact_range_native`, and `Db::recover_native` are the explicit v0.2
  native write/recovery entry points. `Db::write`, `Db::flush`,
  `Db::compact_range`, and `Db::repair` delegate to the same native paths.
- `OpenOptions::write_buffer_size` controls automatic native table flushes.
  The default is 4 MiB. Set it to `0` to keep writes in the WAL overlay until
  `Db::flush`, compaction, or recovery explicitly consumes them.
- `ReadOptions::cache_policy` defaults to `Bypass`, so normal reads do not
  contend on the shared block cache. Set it to `Use` only when cross-request
  block reuse is worth the lock cost.
- `ReadOptions::pipeline` configures local Rayon scan scheduling. `queue_depth`,
  `table_batch_size`, and `progress_interval` use automatic defaults when set to
  zero. `ScanOutcome` reports `tables_scanned`, `worker_threads`,
  `queue_wait_ms`, and `cancel_checks` so renderers can tune without fixed
  machine-specific timing thresholds.
- Old LevelDB worlds are still LevelDB databases. This crate reads native zlib
  compression tag `2`, Bedrock raw deflate tag `4`, WAL + `.ldb` overlays, and
  exact `LegacyTerrain` keys; pre-LevelDB `chunks.dat` parsing intentionally
  lives in `bedrock-world`.
- `Db::for_each_key`, `Db::for_each_entry`, and `Db::for_each_prefix` stream
  borrowed keys and `Bytes` values to visitors.
- `Db::for_each_prefix_key` is the preferred render-index path when callers only
  need keys. It avoids value callbacks and lets native table scans seek directly
  into the requested prefix range.
- Visitors return `VisitorControl::Continue` or `VisitorControl::Stop`; normal
  early termination is reported in `ScanOutcome`, not as an error.
- `stats_fast()` is metadata/overlay-only. `stats_full()`, snapshots,
  materialized iterators, repair, and compaction are explicit expensive paths.

### Migration: full prefix values to key-only scans

Old render index code often read every chunk value just to discover whether a
chunk had renderable records:

```rust
let mut keys = Vec::new();
db.for_each_prefix(b"chunk-prefix", ReadOptions::default(), |key, _value| {
    keys.push(bytes::Bytes::copy_from_slice(key));
    Ok(bedrock_leveldb::VisitorControl::Continue)
})?;
```

Prefer the key-only API for viewport and region indexes:

```rust
let mut keys = Vec::new();
db.for_each_prefix_key(b"chunk-prefix", ReadOptions::default(), |key| {
    keys.push(bytes::Bytes::copy_from_slice(key));
    Ok(bedrock_leveldb::VisitorControl::Continue)
})?;
```

Async callers should share the database handle instead of reopening it for each
request:

```rust
let db = std::sync::Arc::new(Db::open("path/to/world/db", OpenOptions::default())?);
let keys = db
    .clone()
    .collect_prefix_keys_owned_async(
        bytes::Bytes::from_static(b"chunk-prefix"),
        ReadOptions::default(),
    )
    .await?;
```

## Bedrock Record Helpers

The database APIs stay raw key/value APIs. For old Bedrock LevelDB worlds, the
crate also provides storage-level helpers for documented record families:

```rust
use bedrock_leveldb::{
    BedrockKey, ChunkRecordTag, Db, LegacyTerrain, OpenOptions,
};

# fn example() -> bedrock_leveldb::Result<()> {
let db = Db::open("path/to/world/db", OpenOptions::default())?;

db.for_each_entry(Default::default(), |key, value| {
    if let BedrockKey::Chunk(chunk_key) = BedrockKey::parse(key) {
        if chunk_key.tag == ChunkRecordTag::LegacyTerrain {
            let terrain = LegacyTerrain::parse(value)?;
            let _block_id = terrain.block_id(0, 64, 0);
        }
    }
    Ok(bedrock_leveldb::VisitorControl::Continue)
})?;
# Ok(())
# }
```

The helpers cover the LevelDB-era legacy layouts described by the Bedrock
format history, including `LegacyTerrain` and old `SubChunkPrefix` payload
families. They intentionally do not parse pre-LevelDB `chunks.dat` /
`entities.dat` worlds, NBT payloads, actor records, or gameplay-level chunk
semantics.

## Logging

This is a library crate, so it only emits diagnostics through the standard
`log` facade. It does not initialize a global logger and never calls
`println!` or `eprintln!`. Applications can connect any compatible backend:

```rust
fn main() -> bedrock_leveldb::Result<()> {
    // Example only: choose env_logger, log4rs, tracing-log, or your own logger
    // at the application boundary.
    env_logger::init();

    let db = bedrock_leveldb::Db::open("path/to/world/db", Default::default())?;
    let _ = db.get(b"player_1")?;
    Ok(())
}
```

Log events are intentionally low-noise and avoid raw values. Useful events are
emitted around database open, manifest/WAL replay, table scans, custom flushes,
repair paths that discard unreadable files, parallel table workers, cancellation,
and key-only prefix scans. Applications using `tracing` can bridge these events
with `tracing_log::LogTracer`.

## Errors

All fallible APIs return `bedrock_leveldb::Result<T>`, an alias for
`Result<T, LevelDbError>`. `LevelDbError` is structured; prefer matching
`ErrorKind` and using `path()` instead of parsing display strings:

```rust
use bedrock_leveldb::{Db, ErrorKind, OpenOptions};

let err = Db::open(
    "missing-db",
    OpenOptions {
        read_only: true,
        create_if_missing: false,
        ..OpenOptions::default()
    },
)
.expect_err("missing database should fail");

assert_eq!(err.kind(), ErrorKind::NotFound);
assert!(err.path().is_some());
```

Cooperative scan cancellation returns `ErrorKind::Cancelled`. Read-only handles
return `ErrorKind::ReadOnly` for writes, flushes, repair, and compaction.

## Features

| Feature | Default | Meaning |
| --- | --- | --- |
| `zlib` | yes | Enables zlib and Bedrock raw-deflate decompression/compression |
| `snappy` | yes | Enables Snappy table decompression/compression |
| `async` | yes | Adds Tokio `spawn_blocking` async wrappers; Tokio default features stay disabled |
| `mmap` | no | Reserved for a future mapped read path |
| `repair-tools` | no | Reserved for expanded repair tooling |
| `bench` | no | Reserved for benchmark-only code paths |

docs.rs builds with all features enabled, so the hosted API reference includes
async helpers, compression backends, mapped scan types, and repair-tool entry
points. The crates.io package includes the English and Chinese READMEs, the
guide documents under `docs/`, the changelog, licenses, source, tests, and
benchmarks.

MSRV is Rust 1.87.

## Testing And Benchmarks

Release checks used before the first public commit:

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

The Criterion suite is synthetic. It separates overlay hot reads, flushed
native table reads, native table point/prefix reads, WAL recovery, and
sequential versus table-parallel scans. Large-world behavior should still be
validated with real Bedrock fixtures in higher-level crates because this crate
does not interpret world keys or NBT payloads. Latest local numbers are tracked
in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license
