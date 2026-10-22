# bedrock-voxel

[中文](README.zh-CN.md)

`bedrock-voxel` converts images and OBJ models into local Minecraft Bedrock block placement
plans. It does not access world storage: plans stay local until a caller validates and applies
them through `bedrock-world`.

## Public surface

- `block_image`: image-to-block planning;
- `obj` / `obj_voxel`: OBJ loading, materials and voxelization;
- `palette`: default block candidates;
- `relief`: height/relief conversion helpers.

## Boundaries

- Pure conversion: no world I/O, no UI, no GPU.
- Callers own validation, confirmation and application of the produced plan.

## Build and test

```powershell
cargo check -p bedrock-voxel
cargo test -p bedrock-voxel
```

## Documentation

- [Architecture](../../docs/ARCHITECTURE.en.md)
- [Crate catalog](../../docs/CRATES.en.md)
