# BMCBL Bedrock block catalog acquisition

This is an independent BDS Script API + Structure exporter. It does **not**
embed or copy Helix source or require Helix Go libraries.

## Operation

The Linux runner downloads the current official Bedrock Dedicated Server from
Mojang, loads the BMCBL behavior pack into a disposable world and waits until
the script reports `BMCBL-DUMP-END`. The script enumerates Script API block
types and proposed permutations. BDS serializes those permutations into saved
structures; the game (not the script's aliases) determines the persisted
`name`, NBT `states` types and `version`. The exception
`minecraft:structure_void` is placed in a real SubChunk and read from there.

The `bedrock-world` CLI uses BMCBL's own read-only Bedrock LevelDB/NBT
implementation; no C++ or Go implementation is required.

On Ubuntu 24.04:

```bash
python3 scripts/bedrock-block-catalog/run_bds.py \
  --workdir /tmp/bmcbl-bds --metadata /tmp/bmcbl-capture.json \
  --log /tmp/bmcbl-bds.log

cargo run --locked --release -p bedrock-world --bin bedrock-block-catalog -- \
  compile --world /tmp/bmcbl-bds/worlds/bmcbl_catalog \
  --metadata /tmp/bmcbl-capture.json --log /tmp/bmcbl-bds.log \
  --out crates/bedrock-world/data/block-catalog
```

The CI workflow runs the same steps automatically on Ubuntu, uploads the
generated catalog, and commits **only validated catalog data** to `master`.
No BDS executables, world files or source logs are committed.

## Versioned binary layout

```text
crates/bedrock-world/data/block-catalog/
  index.json                 # game version -> manifest + NBT storage version
  manifests/<xxh3-128>.bin   # sorted block ID -> 128-bit object + default index
  objects/<xxh3-128>.bin     # property dictionaries + exact state matrix
```

Object `BMCBLB01` encoding:

- unsigned LEB128 property count
- for each sorted property: length-prefixed name; unsigned LEB128 value count;
  typed NBT values (tag number + little-endian data, UTF-8 for strings)
- unsigned LEB128 permutation count
- for every state, one unsigned LEB128 dictionary index per property
  (`0` means the property is absent, `1..N` selects a typed value)

Manifest `BMCBLP01` encoding:

- unsigned LEB128 block count
- for each sorted block: length-prefixed block ID, 16-byte little-endian
  XXH3-128 object ID, unsigned LEB128 default-state ordinal

The index stores the exact BDS game version, verified server archive SHA-256,
BlockState NBT storage version, state and block counts. **The storage version
is not part of the per-block objects**: if a block has identical semantics
across versions it is stored once. If full palettes match, their manifest is
stored once. Existing objects are byte-compared, so even a digest collision
cannot silently overwrite data. The index is small and stable.

## Safety and scope

- A BDS export is rejected if its `END` marker, persisted structure count,
  exact default/permutation block sets, or minimum state counts disagree.
- Capturing a known version with different evidence is rejected by default;
  `--replace` is deliberately an explicit operation.
- The Script API enumeration alone **cannot prove complete coverage** of all
  runtime-only states. Some states are normalized during structure saving,
  and new BDS releases may add extra combinations. Treat the output as
  `script_api_structure` provenance and cross-check against additional BDS
  runtime observations before making it the authoritative migration palette.
- Data is not automatically loaded as `block_state_schema.json` and does not
  rewrite any existing or older-version world.
- Mojang owns the exported game data. BDS downloads must follow Mojang terms.
- CI fetches the latest **stable Linux** BDS. The public Mojang links API
  does not provide historical releases; older data requires explicitly
  acquired historical BDS binaries and matching Script API compatibility.

See [the official download page](https://www.minecraft.net/en-us/download/server/bedrock)
for server distribution terms and system requirements.
