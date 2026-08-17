# Bedrock world fixture corpus

`bedrock-world` is a general-purpose Bedrock world library. Compatibility tests must therefore cover
historical storage generations instead of testing only the current game version.

Real world saves are intentionally **not committed** to Git. They may be very large and can contain
player identity, inventory, chat, map and other private data. Keep real fixtures local or provide
sanitised/minimal synthetic fixtures generated specifically for tests.

## Version matrix

The local fixture corpus should use the following directory names. A fixture may be a complete world,
a minimal LevelDB database, or a synthetic record set when a redistributable real save is unavailable.

```text
tests/fixtures/
  bedrock-0.14/
  bedrock-0.16/
  bedrock-1.0/
  bedrock-1.12/
  bedrock-1.13/
  bedrock-1.16/
  bedrock-1.17/
  bedrock-1.18.0/
  bedrock-1.18.30/
  bedrock-1.19/
  bedrock-1.20/
  bedrock-1.21/
  bedrock-1.26/
  future-unknown/
```

The matrix intentionally includes storage transition points rather than every patch release:

- 0.14/0.16: Pocket/early Bedrock legacy terrain and pre-modern world layouts.
- 1.0: early LevelDB-era world compatibility.
- 1.12/1.13: legacy numeric block data and later palette/state transitions.
- 1.16/1.17: historical subchunk/palette generations.
- 1.18.0: extended-height transition data.
- 1.18.30: modern actor digest/`actorprefix` transition coverage.
- 1.19/1.20/1.21: modern LevelDB world evolution.
- 1.26: current reference fixture.
- `future-unknown`: synthetic unknown tags/subchunk versions used to verify raw preservation and
  destructive-write refusal.

PrismarineJS also maintains Bedrock data and chunk fixtures by concrete protocol/game version; the
corpus above follows the same principle while targeting on-disk world compatibility rather than only
network chunk decoding.

## Required assertions

Each available fixture should be exercised through the same compatibility suite:

1. Open storage without rewriting it.
2. Parse `level.dat` while preserving unknown NBT fields.
3. Detect `WorldFormat` and `WorldCapabilities`.
4. Enumerate chunk keys, including old Overworld keys without an explicit dimension id.
5. Inspect `ChunkCapabilities` for every unique chunk-record family.
6. Decode every recognised subchunk generation (legacy v0, paletted v1, legacy v2-v7, paletted v8/v9).
7. Preserve unsupported/future subchunk bytes exactly.
8. Parse legacy inline `Entity` records and modern `digp`/`actorprefix` actor storage through the
   unified actor model.
9. Parse block entities, biome/height data, players, maps and known global records where present.
10. Run the world integrity audit without mutating the fixture.
11. Verify `WritePolicy::Preserve` refuses migrations and unknown/future structured rewrites.
12. When a migration fixture exists, migrate into a temporary destination and reopen the result before
    comparing canonical blocks/entities/biomes.

## Local complete-world fixture

The existing large-world benchmark accepts:

```text
tests/fixtures/sample-bedrock-world/
  level.dat
  db/CURRENT
```

Do not commit complete `.mcworld` exports or raw LevelDB table directories unless they have been
explicitly reduced and sanitised for redistribution.

## Fixture metadata

Each local fixture should optionally contain a `fixture.json` next to the world directory, for example:

```json
{
  "game_version": "1.18.30",
  "expected_storage": "leveldb",
  "expected_actor_storage": "modern-digest",
  "expected_write_policy": "migrate",
  "notes": "sanitised local fixture"
}
```

Tests must treat this metadata as expectations only; format detection must still be based on the actual
world records so partially upgraded/mixed worlds remain testable.
