# Historical saved-item migration

Minecraft Bedrock saved items do not contain a universal Mojang item schema version. Consequently
`bedrock-world` does not invent an item `from_version -> to_version` graph. Item migration is driven by
the observed representation and a pinned authoritative corpus.

The pinned reference is `pmmp/BedrockItemUpgradeSchema` at commit
`e19685d2e7e76eb7446115c556df34e5d627d072`, tree
`33ea604960ce4182c4113dce948603638ce69cee`, licensed CC0-1.0.

## Pipeline

Classic items first map the legacy numeric `id` to the historical string identifier. String-ID items
then split into ordinary items and blockitems using `1.12.0_item_id_to_block_id_map.json`.

Ordinary items execute all `id_meta_upgrade_schema` documents in numeric filename order. Within each
schema, `remappedMetas` has priority over `renamedIds`. A metadata remap changes the identifier,
resets metadata to zero, and later schemas continue from the new pair.

Blockitems are deliberately delegated to the block domain. Existing `Block` NBT is parsed as a
versioned `BlockState`; old ID/meta blockitems use the caller-provided `LegacyBlockItemResolver`.
Both paths then run the normal `BlockStateMigrator` and target-palette validation before the item
identifier is rewritten. If this context is not available, preservation mode leaves the complete item
unchanged instead of creating an inconsistent item ID / block-state pair.

## Preservation rules

`ItemMigrationPolicy::PreserveUnknown` is intended for editors and inspection tools. Unknown numeric
IDs, unsupported blockitems, and future layouts retain their complete item NBT. `RefuseUnknown` is for
destructive world conversion: the first unresolved item aborts the caller's migration before it
commits storage changes.

Unrelated stack fields such as `Count`, `Slot`, `tag`, `WasPickedUp`, `CanPlaceOn`, and `CanDestroy`
are not rebuilt by the item migrator. Only authoritative identity/meta fields and an explicitly
migrated `Block` payload are changed.

The recursive `migrate_item_stacks_in_nbt` entry point is shared by Player, Actor, and BlockEntity
callers. Preserved/future item compounds are not traversed internally, while recognised items may
continue into nested custom-tag data so container-like items can be handled by the same pipeline.

## Pinned resource bundle

Run `scripts/sync-item-upgrade-corpus.ps1` to download the immutable corpus to
`vendor/item-upgrade-schema`. Runtime migration performs no network requests. The loader checks every
required file by exact byte length and Git blob SHA-1 before parsing it.
