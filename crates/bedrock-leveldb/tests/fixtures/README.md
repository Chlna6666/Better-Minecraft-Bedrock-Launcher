# Mojang LevelDB compatibility fixtures

`bedrock-leveldb` is a storage-engine library. Fixtures in this directory must contain only raw
Mojang/Bedrock LevelDB databases and must not depend on Minecraft chunk/NBT semantics.

Real databases are local-only unless they have been reduced and sanitised. A fixture directory should
contain the normal `CURRENT`, manifest, WAL and `.ldb` files required to open it read-only.

Recommended local fixture matrix:

```text
tests/fixtures/
  native-none/
  native-snappy/
  native-zlib/
  native-bedrock-raw-deflate/
  legacy-wal-replay/
  multi-table-compaction/
  truncated-repair/
```

The native table block compression tags exercised by the engine are:

- `0`: uncompressed block;
- `1`: Snappy block;
- `2`: zlib-wrapped DEFLATE block used by the Mojang-derived format supported by this crate;
- `4`: raw DEFLATE (`Bedrock zlib`) block supported for historical/current Bedrock databases.

Compatibility tests must verify opening and scanning each available fixture without rewriting it. The
storage layer must return exact key/value bytes regardless of whether higher-level `bedrock-world`
understands their meaning.

For every fixture that is safe to mutate in a temporary copy, also verify:

1. WAL replay preserves the visible key/value view.
2. point reads and sequential scans return identical values;
3. borrowed/shared/owned read strategies do not change bytes;
4. an explicit flush/reopen preserves the view;
5. repair never runs implicitly while opening a valid old database;
6. unknown user keys and values remain byte-for-byte intact.

Do not put BlockState, chunk-version, actor, biome or other Minecraft semantic expectations here; those
belong to `bedrock-world` fixtures.

## Enforcing the corpus

Normal developer tests may run without private historical databases; missing local fixtures are then
reported as skipped. Release/compatibility validation must not treat that as proof of compatibility.
Set:

```text
BEDROCK_LEVELDB_REQUIRE_HISTORICAL_FIXTURES=1
```

to make every required matrix entry mandatory. The default root is this `tests/fixtures` directory.
Use `BEDROCK_LEVELDB_FIXTURE_ROOT=/path/to/corpus` to point the test suite at a private or externally
mounted corpus. When enforcement is enabled, a missing `CURRENT` file fails the suite before an
individual compatibility test can silently skip the database.
