# Minecraft Bedrock world compatibility matrix

This matrix describes verified storage behavior. “Round-trip” means the persisted representation is
retained unless the caller explicitly invokes an upgrade or downgrade API.

| Minecraft data | Read | Same-format write | Explicit conversion | Verification |
| --- | --- | --- | --- | --- |
| Modern LevelDB world | Yes | Yes | World migration APIs | `tests/writable_leveldb.rs`, `tests/write_visibility.rs` |
| Full LevelDB `LegacyTerrain` (83,200 bytes) | Yes | Yes | Split/merge and numeric SubChunk upgrade | `chunk::tests`, `world::legacy_terrain_storage` tests |
| Pocket `LegacyTerrain` core (82,176 bytes) | Yes | Read-only | Rejected when a complete biome-bearing record is required | `database::pocket_chunks` tests |
| SubChunk V0 | Yes | Raw/legacy V0 | Numeric upgrade requires an explicit palette target | `chunk::version::tests::legacy_subchunk_versions_roundtrip_without_implicit_upgrade` |
| SubChunk V1 | Yes | Paletted V1 | V1/V8/V9 when representable | `chunk::version::tests::paletted_subchunk_versions_roundtrip_through_their_native_writers` |
| SubChunk V2–V7 | Yes | Raw/legacy same version | Numeric upgrade requires an explicit palette target | legacy SubChunk matrix test |
| SubChunk V8 | Yes | Paletted V8 | V1/V9 when representable | paletted SubChunk matrix test |
| SubChunk V9 | Yes | Paletted V9 | V1/V8 when representable | paletted SubChunk matrix test |
| Unknown SubChunk version | Raw preservation | Identical version only | No synthesis or guessed decoding | `unknown_version_only_roundtrips_the_same_raw_target` |
| Pocket `chunks.dat` | Yes | Read-only | Explicit import accepts only complete records | location-table, origin, multi-chunk and corrupt-sector tests |

## BlockState contract

Every palette `BlockState` retains its complete named NBT state map. Callers can inspect arbitrary
current or future properties through `state`, `state_entries`, `state_boolean`, `state_integer` and
`state_string`; unknown properties are never dropped. Typed Minecraft family views additionally cover
six-way/horizontal direction, doors, trapdoors, stairs, slabs and redstone values. Family-specific
numeric direction encodings remain distinct.

## Required fixture evidence

Historical fixture validation must report the game/storage version evidence, observed SubChunk
versions, record count, chunk count and parse errors. A fixture that is absent is a skipped test, not
proof of compatibility. Real worlds and pinned corpora are read-only inputs; mutating tests must copy
them to a temporary directory first.

## Known boundaries

- Pocket `chunks.dat` is intentionally read-only. Import is an explicit operation.
- Unknown SubChunk versions are not parsed as a newer known layout merely because their version byte
  is greater than 9.
- Data3D does not persist its starting Y. Standard 24-storage Overworld data is interpreted as block
  Y `-64..=304`; other standard ranges start at block Y `0`.
- Numeric legacy SubChunks cannot be converted to palettes without authoritative versioned mapping
  data, and paletted states cannot be downgraded to numeric IDs by guessing.
