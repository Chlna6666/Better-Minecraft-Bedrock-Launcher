# bedrock-scene

[English](README.md) | [简体中文](README.zh-CN.md)

Builds CPU-side 3D geometry from decoded Minecraft Bedrock chunks and
structures. Block states, block entities, biome colors, and resource-pack
models resolve to material-tagged faces. Chunk boundaries can be retained
separately and reused when adjacent chunks are rebuilt.

- `chunk::collect` collects renderable cells for streaming. Insert collected
  chunks and any cached neighbor boundaries into a `SceneBuilder` created
  with `new_stream`, then request faces or voxel descriptions for the target chunks.
- `chunk::prepare` and `SceneBuilder::build_selection` produce indexed mesh
  snapshots for a selection. `structure::build` builds a mesh from a decoded
  structure and a caller-provided palette.

The caller owns world loading, visibility scheduling, texture loading, and GPU
submission. The crate uses CPU-side `gpui-3d` types with GPUI integration disabled.
