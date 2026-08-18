# Authoritative BlockState migration

`bedrock-world` separates historical physical codecs from semantic BlockState migration. Numeric
`LegacyTerrain` / SubChunk v0-v7 records first resolve `(id, meta)` into a versioned BlockState; old
paletted subchunks already contain BlockState NBT and enter the semantic pipeline directly.

## Pinned corpus

The reference corpus is PocketMine `bedrock-block-upgrade-schema` 5.2.0 at commit
`5d7889c9a1cdf9e3cd814d2a104ad69b75116ec7` (CC0-1.0). The manifest covers all 34 schema documents,
`block_legacy_id_map.json`, the 1.9/1.12 numeric tables, and the upstream license.

The library does **not** fetch migration data at runtime. Applications distribute one immutable corpus
bundle and load it with `load_pinned_block_migration_bundle_from_dir`. Every resource is checked by
exact byte count and Git blob SHA-1 before any migration rule is accepted.

For development or packaging, PowerShell 7 can materialize the exact pinned bundle without CI:

```powershell
pwsh ./crates/bedrock-world/scripts/sync-blockstate-corpus.ps1
```

The script downloads only the fixed commit, validates every payload against `corpus.lock.json`, and
places it under `crates/bedrock-world/vendor/blockstate-schema`. The crate package include list retains
this directory when a release chooses to ship the corpus alongside the library.

## Runtime ownership and performance

`PinnedBlockMigrationBundle` owns the parsed schema catalog and both numeric tables. Raw schema JSON
and binary file buffers are temporary during bundle construction and are released afterwards. Normal
chunk migration therefore performs no filesystem access, network access, JSON parsing, or corpus hash
calculation on the hot path.

Keeping the bundle separate from the executable also avoids forcing every client/server binary to
carry a duplicate static copy. A process may place one bundle in an `Arc` and share it across world
workers because its migration interfaces are read-only and `Send + Sync`.

## Historical target versions

The default bundle loader targets the newest represented schema. A server or editor that intentionally
writes an older Bedrock generation can instead use `load_pinned_block_migration_bundle_for_target_from_dir`.
The complete 38-file bundle is still verified first; only schema groups ending at or before the
requested `BlockStateStorageVersion` are compiled. The target must be an actual authoritative schema
endpoint, matching `load_pinned_block_state_catalog_for_target`.

For classic numeric terrain the bundle selects the 1.9 table for pre-1.12 targets and the 1.12 table
for 1.12-or-newer targets. Downgrading newer BlockStates is never inferred from forward schemas.

## Execution semantics

The authoritative executor follows the PocketMine reference ordering rather than reducing the data to
simple `from_version -> to_version` edges:

- schema filename numeric IDs define ordering within equal Mojang storage-version groups;
- multiple schemas with the same storage version are all applied because Mojang has shipped semantic
  changes without incrementing the stored version;
- `remappedStates` has highest priority and short-circuits the remaining transforms for that schema;
- identifier rename/flatten, added properties, removed properties, property rename/value remap, and
  remaining value remaps are applied in reference order;
- future-version states are rejected so the caller can preserve raw world records instead of
  downgrading unknown data.

## Numeric terrain

`LegacyNumericBlockStateTable` parses the upstream `id_meta_to_nbt/*.bin` format without expanding a
4096-block subchunk into 4096 heap-owned states. The chunk adapter performs exact `(id, meta)` lookup
and then metadata-0 fallback, matching the reference upgrader.

`migrate_historical_chunk_with_pinned_bundle_blocking` connects the bundle-selected numeric resolver
and target-bound catalog to the existing atomic historical chunk migration path.

## Target palette validation

The upgrade-schema corpus describes historical transformations; it is not the complete runtime block
palette of a specific Minecraft client/server build. Destructive migration therefore still requires
`target_palette_contains` from the selected runtime palette.

A chunk is written only after every source palette entry has migrated to the selected output version
and passed that validator. Unknown future subchunks or record tags remain preservation-only and block
the destructive batch.
