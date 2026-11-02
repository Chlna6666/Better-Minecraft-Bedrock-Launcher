# bedrock-light

[简体中文](README.zh-CN.md)

Storage-independent CPU voxel lighting for Bedrock tooling. This crate owns bounded
six-neighbor sky/block-light propagation. It has no tile renderer, UI, GPU or world I/O.
`bedrock-block-model` supplies state properties; applications assemble cell regions;
`gpui-3d` owns GPU shading. `bedrock-render` remains the 2D tile renderer.

`LightVolume::solve` preserves level-15 skylight straight down through transparent cells
and attenuates lateral/block light. Re-solving removes stale sources. Include complete
vertical columns and at least 15 horizontal halo cells; treat unknown cells as opaque.
Output packs sky in the high nibble and block light in the low nibble. It does not write
world light arrays or promise complete Bedrock state/shape behavior.

```powershell
cargo check -p bedrock-light
cargo test -p bedrock-light
```
