# Biome registry snapshots

This directory contains source snapshots used to build the compact biome registry embedded by `bedrock-world`.

The storage parser never depends on this table to decode `Data2D`, `Data2DLegacy`, or `Data3D`: persisted numeric biome IDs remain raw world facts. The registry is only a version-selected interpretation layer for vanilla identifiers and BDS runtime properties.

## Update from Endstone

Endstone's DevTools already obtains vanilla biome IDs from the running BDS `BiomeRegistry`. Export that JSON, then run:

```text
cargo run -p bedrock-world --bin bedrock-world-tool -- biome update \
  --input /path/to/biomes.json \
  --bds-metadata /path/to/metadata.json \
  --protocol-readme /path/to/protocol-docs/README.md \
  --channel release \
  --endstone-ref <commit-or-release> \
  --protocol-ref <protocol-docs-ref>
```

`--bds-metadata` accepts EndstoneMC/bedrock-server-data metadata and extracts the exact four-component BDS build from its binary URL. `--protocol-readme` accepts EndstoneMC/protocol-docs README format and extracts both Minecraft and Network Version. Explicit `--minecraft-version` / `--network-version` can be used when those source files are not available.

The command performs four operations:

1. validate the Endstone biome output and reject duplicate IDs;
2. write a normalized `snapshots/<minecraft-version>.json` source file;
3. update `manifest.json`;
4. rebuild `src/biome/registry.bin` and parse it again through `bedrock-world` for structural verification.

## Binary layout

`registry.bin` is intentionally not serde/bincode/postcard data. It is a fixed little-endian table format:

```text
header
version snapshot table      sorted by [major, minor, patch, build]
biome table                 sorted by numeric ID inside each snapshot
name hash index             xxh3_64(name), sorted for binary search
UTF-8 string pool           deduplicated across snapshots
```

Runtime lookup therefore does not deserialize the whole registry or allocate per-biome objects. `include_bytes!` embeds the file in the library, and lookups operate directly on the validated immutable bytes.

The currently committed binary may contain zero snapshots until a real BDS runtime export is imported. An empty registry is valid; fabricated vanilla IDs are not.

## Rebuild and verify

```text
cargo run -p bedrock-world --bin bedrock-world-tool -- biome pack
cargo run -p bedrock-world --bin bedrock-world-tool -- biome verify
```

`verify` rebuilds the binary from the normalized source manifest and requires byte-for-byte equality with the embedded file, preventing stale generated data from being shipped.
