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

## Old BDS / Script API compatibility

Script module versions and Minecraft game versions are **different** values.
The runner maintains a version-to-module map and adapts the behavior-pack
dependency and `min_engine_version` at install time.

Official stable API milestones:

| Minecraft / BDS | Stable `@minecraft/server` | Capture |
|---|---|---|
| 1.19.70 | 1.0.0 | Capability-probed; no guaranteed NBT Structure writer |
| 1.20.0 | 1.2.0 | Capability-probed; may be partial |
| 1.20.70 | 1.9.0 | `getAllStates` stable; may lack Structure writer |
| 1.20.80 | 1.10.0 | Structure `createEmpty` stable; write not yet stable |
| 1.21.0 | 1.11.0 | Structure `setBlockPermutation` / `saveToWorld` stable |
| 1.21.20 | 1.13.0 | Structure-capable |
| 1.21.90 | 2.0.0 | Script API major-version transition |
| 1.26.60 | 2.11.0 | Modern Structure-capable |

The exporter selects a mode **from the capabilities it actually sees at
runtime**, not from presence or absence of PDB. It uses `BlockTypes.getAll`
and `BlockPermutation.resolve` when available; on versions without Structure
writes it emits `BMCBL-DUMP-PROBE` JSON observations from the API.
These are uploaded as **diagnostics**, with
`capture_method=script_api_probe`, not compiled into `.bin`.
When an old stable API lacks `BlockTypes.getAll`, the runner tries
block-name hints from `blocks.json` files shipped with **that exact BDS**.
This fallback is explicitly labeled `bds_resource_hints`, is likely incomplete
and **cannot** produce a trusted full catalog. If the game supplies neither
an API registry nor usable BDS resource hints, the capture fails explicitly.

Historical BDS archives are fetched only from Mojang's official CDN and
verified against independently pinned SHA-1 fingerprints from
[Bedrock-OSS/BDS-Versions](https://github.com/Bedrock-OSS/BDS-Versions).
Historical archive availability is not guaranteed.
To inspect a pinned old BDS, use **Actions → Bedrock block catalog (Linux BDS)
→ Run workflow → version**, e.g. `1.19.70.02`,
`1.20.80.05`, or `1.21.20.03`. Leave `latest` for automatic
production updates.

Older or API-incomplete snapshots can be completed manually. The presence
of an official `@minecraft/server` module does not prove it exports every
block type or persisted NBT version. PDB-based discovery is deliberately
not implemented here.


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
- Scheduled CI fetches the latest **stable Linux** BDS. `workflow_dispatch`
  accepts pinned historical BDS versions where the original Mojang archive is
  still available.
- A captured name is always a literal Bedrock identifier for that game
  version, **not a logical identity across versions**. The same binary object
  can be referenced by multiple different names without proving a rename.
  Historical numeric IDs / metadata and modern runtime network IDs are
  distinct namespaces and are not inferred from block-state NBT.
- `diff_catalog.py` compares two committed catalogs by literal identifiers
  and state-object hashes; shared hashes with different names are reported
  only as *unverified rename candidates*. Human-reviewed mappings require
  separate evidence, not automatic alias conversion.

See [the official download page](https://www.minecraft.net/en-us/download/server/bedrock)
for server distribution terms and system requirements.

## Historical name/ID comparison

```bash
python3 scripts/bedrock-block-catalog/diff_catalog.py \
  --from 1.21.20.03 --to 1.26.66.1 \
  --output /tmp/block-catalog-diff.json
```

This requires both versions to exist in `index.json`. It reports exact
added/removed/modified *names*. A removed and added name with the same
binary state object/default is an **unverified candidate**, not an alias or a
conversion rule. Legacy numeric block IDs and aux/meta values are not present
in saved block-state structures. Populate them from a separately sourced
per-version numeric registry, preserve source provenance and only apply
human-reviewed cross-version rename/migration rules.

Offline tests:

```bash
python3 -m unittest discover -s scripts/bedrock-block-catalog -p 'test_*.py'
```

## Maintenance and PDB data import

The public [BlockState & legacy ID maintenance guide](../../docs/BEDROCK_BLOCK_DATA_MAINTENANCE.md)
defines the source-of-truth rules for game versions, NBT storage versions,
legacy `(numeric ID, metadata)`, internal `BlockType` indices and network
runtime IDs. **They are different namespaces.** Identical block property
objects can be deduplicated; they are not automatically semantic aliases.

Use the historical backfill workflow to run multiple known BDS builds in one
serial Linux job. The backfill retains explicit `complete_structure_capture`,
`partial_probe` and `capture_failed` reports. Only complete validated captures
enter the binary palette; no unverified historical ID is added automatically.

For PDB extractions, produce JSON in the documented schema and use
`scripts/bedrock-block-catalog/import_id_registry.py`. The importer checks
duplicated/conflicting IDs, exact typed states and per-entry symbol evidence,
then writes a small content-addressed binary index plus compressed evidence.
Uncatalogued entries require explicit `--stage-unverified` and are never
considered authoritative. See the maintenance guide for a sample and CLI.
