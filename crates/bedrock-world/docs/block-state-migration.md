# Authoritative BlockState migration

`bedrock-world` separates historical physical codecs from semantic BlockState migration. Numeric
`LegacyTerrain` / SubChunk v0-v7 records first resolve `(id, meta)` into a versioned BlockState; old
paletted subchunks already contain BlockState NBT and enter the semantic pipeline directly.

## Pinned corpus

The reference corpus is PocketMine `bedrock-block-upgrade-schema` 5.2.0 at commit
`5d7889c9a1cdf9e3cd814d2a104ad69b75116ec7` (CC0-1.0). The public manifest constants list all 34
schema documents plus the legacy ID map and the 1.9/1.12 numeric tables. Applications should package
those immutable resources with the server/client and pass all schema documents to
`load_pinned_block_state_catalog`; the loader refuses missing, duplicated, or mixed filenames.

The library does **not** fetch migration data at runtime. World conversion must be deterministic and
must not depend on network availability or on a mutable upstream branch.

### Historical target versions

A server or editor that intentionally writes a historical Bedrock generation should build a catalogue
with `load_pinned_block_state_catalog_for_target`. The complete pinned corpus is still validated first;
only schema groups ending at or before the requested `BlockStateStorageVersion` are then compiled.
The requested target must be an actual authoritative schema endpoint. This prevents arbitrary version
stamping and keeps the per-palette migration hot path target-specific instead of branching across the
full corpus for every BlockState.

Downgrading a newer BlockState into an older semantic schema is not inferred from forward upgrade
rules. Existing historical data may be preserved in its original representation, while destructive
conversion requires an authoritative forward path to the selected target and a target-palette check.

## Execution semantics

The authoritative executor follows the reference ordering rather than reducing the data to simple
`from_version -> to_version` edges:

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
and then metadata-0 fallback, matching the reference upgrader. Resolved historical subchunks continue
through the same `BlockStateMigrator` interface as old paletted subchunks.

For migrations targeting 1.12 or newer, use the 1.12 numeric table. The 1.9 table is retained for
callers that deliberately need an earlier BlockState target.

## Destructive-write rule

A historical chunk is written only after every source palette entry has migrated to the selected
output version and passed the caller's authoritative target-palette validator. Unknown future
subchunks/record tags remain preservation-only and block the destructive batch.
