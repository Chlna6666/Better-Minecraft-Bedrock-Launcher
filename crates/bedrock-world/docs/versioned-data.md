# Multi-version Bedrock data model

`bedrock-world` is a multi-version Minecraft Bedrock save/world read-write library. It is not an
implicit save upgrader.

## Rules

1. Reading detects the persisted representation from Bedrock data itself.
2. Reading never upgrades or normalises a historical representation as a side effect.
3. Raw source bytes are retained whenever they are required for an exact round-trip.
4. The default write path preserves the source representation/version.
5. Cross-version conversion is an explicit caller request.
6. Conversion support is directional and reported as `Lossless`, `Lossy` or `Unsupported`.
7. Unknown/future data is preserved rather than stamped with a guessed version.

## SubChunk

`SubChunkVersion` represents the actual payload byte: V0 through V9 plus `Unknown(u8)`.
`parse_subchunk*()` already selects the reader from that byte automatically.
`write_subchunk_preserving_version()` writes V0/V2-V7 and unknown retained payloads byte-for-byte and
re-encodes decoded V1/V8/V9 palettes in the same source version. No V0-V7 -> V9 conversion occurs
unless a caller invokes an explicit conversion path.

## Player

Player NBT does not contain one universal player schema byte. `PlayerDataFormat` therefore reports
observable evidence instead of inventing `PlayerV1`, `PlayerV2`, etc.:

- storage: `level.dat.Player`, `~local_player`, `player_<xuid>` or unknown;
- saved-item representation: legacy numeric, named, named BlockState or mixed;
- `LevelVersion`/`GameVersion` only when actual `level.dat` fields provide that evidence.

`player::storage` reads and writes each storage form directly. `player::conversion` is explicit and is
not called by those normal read/write functions.

## Ongoing dev-stage cleanup

The crate is pre-stable and does not keep compatibility aliases for replaced APIs. Existing historical
`migration` modules in other domains are being split into persisted format/version data and explicit
conversion APIs. New multi-version work must not introduce additional `xxx::migration` modules.
