# Multi-version Minecraft Bedrock world data

`bedrock-world` reads and writes multiple generations of Minecraft Bedrock world data. Opening an old
world does not rewrite it to a newer generation.

## General rule

The public API follows Bedrock data names rather than framework-layer names.

Examples:

- `SubChunk` V0, V1, V2-V7, V8 and V9;
- `LegacyTerrain`;
- `Data2D`, `Data2DLegacy` and `Data3D`;
- `Entity`, `digp` and `actorprefix`;
- `level.dat.Player`, `~local_player` and `player_<xuid>`;
- BlockState persisted `version`;
- classic numeric saved-item ID/meta and named saved items.

There is no public `migration`, `conversion`, `transcode`, `codec`, `adapter`, `schema`, `format` or
`storage` bucket that owns unrelated Bedrock data.

## Reading

The reader detects the representation from the bytes/key/tag that Bedrock actually stored. Unknown
future records are retained raw where possible. Player data only uses an exact game version when that
version is present in `level.dat`; it does not invent a Player V1/V2-style version number.

## Writing

Normal writes keep the selected Bedrock representation. Selecting another generation is explicit and
belongs to the concrete data object, for example writing a specific SubChunk version or writing
`Entity` actors as `digp`/`actorprefix`.

A reverse write is supported only when the older representation can express the data. For example,
`Data3D -> Data2D` is accepted only when every biome column is vertically uniform and every biome id
fits the `Data2D` representation.

## Player

Player modules are named after the actual record locations:

- `level.dat.Player`;
- `~local_player`;
- `player_<xuid>`.

`PlayerData` records the detected saved-item generation and optional real `level.dat` version evidence.
No read operation changes those values automatically.

## Actor records

`write_digp_from_entity()` writes a chunk `Entity` record as `digp` plus `actorprefix` payloads.
`write_entity_from_digp()` performs the reverse operation. The reverse write deliberately retains
`actorprefix` values because deleting them safely requires proving that no other `digp` references the
same actor.

## SubChunk

The next split keeps the SubChunk implementation under its real version numbers (`v0`, `v1`,
`v2_v7`, `v8`, `v9`). The leading version byte is the primary source of truth; V10+ remains raw until
the library has an implementation for that Bedrock generation.
