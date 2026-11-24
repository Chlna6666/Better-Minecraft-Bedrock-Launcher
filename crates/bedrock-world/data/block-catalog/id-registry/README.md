# Staged block identity evidence (NOT authoritative)

This directory holds auditable, stage-only block identity evidence produced by
`scripts/bedrock-block-catalog/pdb/*` and `import_id_registry.py`.

Every record in `index.json` has `status = unverified_names` and
`verified_block_names = false`. By design:

- `pdb_symbol_order` entries carry a PDB symbol/address ordinal
  (`symbol_ordinal`) which is **evidence only**, not a verified runtime type ID.
- `legacy_numeric_block` entries come from the authoritative PocketMine
  `BedrockBlockUpgradeSchema` dataset, recorded with its own dataset version and
  the hashes of BOTH input files (`id_meta_to_nbt/*.bin` and
  `block_legacy_id_map.json`) — not a BDS executable fingerprint. Early 1.9.0
  NBT has no persisted version field; those entries use explicit `version: null`
  (BMCBID03 presence flag 0) and never derive one from the game version.
- Each record is keyed by a provenance scope
  `game_version|namespace|platform|binary_sha256[|palette_fingerprint]` so data
  from different platforms/builds/palettes can coexist without overwriting.
- `objects/<sha256>.bin` are content-addressed mapping tables;
  `evidence/<sha256>.json.zlib` are compressed, reproducible source/evidence
  archives. Both are byte-verified by `test_id_registry.py`.

**No production code reads this directory.** It is never used by world
conversion, block-state migration or network runtime ID mapping. Importing more
data requires `import_id_registry.py` with an explicit source and, for
uncatalogued names, `--stage-unverified`; nothing here is promoted to
`identity_evidence_only` automatically.
