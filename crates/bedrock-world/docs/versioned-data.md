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

- `version`: persisted game/data/storage version evidence and authoritative version data;
- `legacy`: actual historical Bedrock representations;
- `conversion`: explicit caller-requested conversion between representations.

The crate is in active development; replaced `migration` module paths are removed instead of retained
as compatibility aliases.

## SubChunk

`SubChunkVersion` is the actual payload byte (`V0` through `V9`, plus `Unknown(u8)`). Reads select the
format automatically. Unknown future versions remain raw. Same-version writes preserve the selected
SubChunk generation; a different target is only selected through explicit conversion.

## Player

Player data is detected from actual storage and NBT evidence: `level.dat.Player`, `~local_player`,
`player_<xuid>`, saved-item representation and real `level.dat` version fields. Reading never invents a
precise Player schema version.

## Actor storage

Both inline chunk `Entity` and `digp`/`actorprefix` are supported Bedrock actor-storage
representations. `entity::conversion` exposes explicit lossless conversion in both directions. The
`digest -> inline` path retains actorprefix payloads because deleting them safely requires a complete
world reference analysis.

## Other domains

Item, biome, entity, chunk and whole-world cross-version operations live under `conversion`.
BlockState schema/corpus/numeric-ID resources live under `block::version`; BlockState transforms live
under `block::conversion`.
