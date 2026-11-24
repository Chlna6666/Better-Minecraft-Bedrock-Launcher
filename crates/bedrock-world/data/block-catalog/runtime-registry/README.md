# Live BDS runtime block registry dumps (evidence only)

Each `<version>.json` is produced by `scripts/bedrock-block-catalog/pdb/runtime_dump.py`
while a real `bedrock_server.exe` of that exact version is RUNNING. It attaches
with Frida, walks the initialized `BlockTypeRegistry::mBlockLookupMap`
(`std::map<HashedString, SharedPtr<BlockLegacy>>`) and reads each block's name
from the game's own objects.

This is **runtime observation**, not static PDB reversing: the PDB only supplied
the map's static address; every name comes from the live, populated registry.

Properties:

- `method = live_frida_block_type_registry_dump`
- `declared_map_size` equals `named_blocks` when the walk is complete.
- Heap addresses are omitted (`--names-only`) because they are per-run.
- Names are literal identifiers for that exact build; they are **not** a
  cross-version identity and carry no legacy numeric or network runtime ID.

The 1.20.80.05 dump (1078 names) matches the archived bds-mod-mapping palette
for the same build with zero name differences, and 752/765 PDB-evidenced names.
