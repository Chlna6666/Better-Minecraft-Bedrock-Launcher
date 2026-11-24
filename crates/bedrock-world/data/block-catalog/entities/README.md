# Entity identifiers per BDS version

Each `<version>.json` holds just the entity identifiers shipped by that exact
official BDS build:

```json
{ "game_version": "1.20.80.05", "entity_count": 121, "identifiers": ["minecraft:agent", "..."] }
```

- Produced by `scripts/bedrock-block-catalog/extract_entity_ids.py` from
  `behavior_packs/*/entities/*.json` (`minecraft:entity.description.identifier`).
- Identifiers are preserved verbatim; non-`minecraft` namespaces are kept.
- Counts grow with the game: 93 (1.9.0) → 122 (1.21.3).

Newer BDS archives (1.26.x) no longer ship vanilla entity definition JSON, so
there is no file for those builds. These are shipped identifiers, not a live
runtime registry dump, and carry no numeric or network entity IDs.
