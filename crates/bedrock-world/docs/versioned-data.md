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

## Public responsibility names

Game-data APIs use Bedrock terminology rather than framework terminology:

- `version`: persisted game/data/storage version evidence and authoritative version data;
- `legacy`: actual historical Bedrock representations;
- `conversion`: explicit caller-requested conversion between representations.

`migration` is not a public domain/module responsibility. The crate is in active development and old
module paths are removed rather than kept as compatibility aliases.

## SubChunk

`SubChunkVersion` represents the actual payload byte: V0 through V9 plus `Unknown(u8)`.
`parse_subchunk*()` selects the reader from that byte automatically. Unknown future versions are kept
raw. `write_subchunk_preserving_version()` writes retained historical/unknown payloads without
implicitly selecting a newer SubChunk generation.

## Player

Player NBT does not contain one universal player schema byte. `PlayerDataFormat` therefore reports
observable evidence instead of inventing `PlayerV1`, `PlayerV2`, etc.:

- storage: `level.dat.Player`, `~local_player`, `player_<xuid>` or unknown;
- saved-item representation: legacy numeric, named, named BlockState or mixed;
- `LevelVersion`/`GameVersion` only when actual `level.dat` fields provide that evidence.

`player::storage` reads and writes each storage form directly. `player::conversion` is explicit and is
not called by those normal read/write functions.

## Other domains

Item, biome, entity, chunk and whole-world cross-version operations live under `conversion`. BlockState
schema/corpus/numeric-ID resources live under `block::version`; BlockState transforms live under
`block::conversion`. No new `xxx::migration` module should be added.
