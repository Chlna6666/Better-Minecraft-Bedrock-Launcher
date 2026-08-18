# BlockEntity migration

`BlockEntity` is chunk-scoped non-actor data. It is deliberately separate from the historical
`Entity -> digp/actorprefix` actor-storage migration.

## Versioning model

Unlike BlockState NBT, there is no universal Mojang schema-version field embedded in every block
entity root. `bedrock-world` therefore does not stamp block entities with a synthetic version or use
the chunk version as if it were a block-entity schema version.

`BlockEntityMigrator` receives `BlockEntityMigrationContext`, and implementations may use externally
known source/target chunk generations when they have authoritative evidence. Unknown identifiers and
unknown fields must be preserved.

## Built-in vanilla migration

`VanillaBlockEntityMigrator` currently implements only transitions that can be identified directly
from the saved NBT shape:

- old Sign `Text1` .. `Text4` lines;
- later Sign top-level `Text` blob;
- modern `FrontText` / `BackText` dual-sided storage.

The migrator adds modern Sign fields while retaining legacy fields and every unknown field. This is
intentional: a recognized migration must not discard data merely because the library does not
understand it.

`TextIgnoreLegacyBugResolved` is respected when interpreting old `IgnoreLighting` values. Without the
resolved marker, an old `IgnoreLighting=1` is not promoted to glowing text.

Other block entities currently return `Preserved` unchanged. This is preferable to guessed chest,
spawner, command-block, structure-block or modded schemas.

## Atomic chunk rewrite and raw preservation

`migrate_block_entity_chunk_blocking` walks every consecutive NBT root in the chunk's `BlockEntity`
value and completes all migration work before emitting one `StorageBatch` write. Any parse or
migration error leaves the original LevelDB value untouched.

Roots reported as `Unchanged` or `Preserved` are copied from their original consumed byte range rather
than serialized again. Consequently an unknown/future block entity remains byte-for-byte identical
even when a sibling Sign root in the same LevelDB value is upgraded.

The convenience wrapper `migrate_block_entity_chunk_to_modern_blocking` uses the conservative built-in
vanilla migrator.

## Extending coverage

Future authoritative rules should be added by block-entity family and backed by real historical save
fixtures. Candidate families include containers, furnaces/brewing stands, mob spawners, command and
structure blocks, banners, flower pots, item frames and legacy education-edition block entities.

Nested `Items` are intentionally not rewritten here; historical item-stack migration belongs to the
item/player data layer so the same rules can be reused by containers, entities and players.
