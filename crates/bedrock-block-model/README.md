# bedrock-block-model

`bedrock-block-model` resolves Minecraft Bedrock resource-pack block visual data into a
single model description that renderers and OBJ exporters can consume.

The crate currently loads:

- `blocks.json` in the legacy vanilla resource-pack format.
- Custom block files using `minecraft:block`, including `components` and `permutations`.
- `textures/terrain_texture.json`, without falling back to `item_texture.json`.
- `minecraft:material_instances`, including render method and tint metadata.
- Geometry JSON files under `models/**`, including `minecraft:geometry` arrays and legacy
  `geometry.*` roots.
- OBJ material-library semantics for block exports, including cutout `map_d`, transparent
  `illum`, preview tint, and biome-tinted texture metadata for grass, foliage, plants,
  redstone wire, cobweb, panes, and similar non-full blocks.
- Reusable OBJ export primitives such as mesh-face adaptation, quad face text writing, OBJ
  document assembly, vertex offset calculation, export-material records, export statistics,
  and diffuse/alpha/tinted texture-copy targets.
- World resource-pack discovery for OBJ exports, including `world_resource_packs.json`,
  resource-pack `manifest.json` UUID matching, local world `resource_packs`, sibling
  `com.mojang/resource_packs`, and shared `Users/Shared/games/com.mojang/resource_packs`.
- Model-family classification for common vanilla shape families. This covers legacy names and
  current 1.26-era variants such as slabs, stairs, fences, walls, panes/bars, trapdoors,
  redstone wire, cross plants, containers, shulker boxes, pots, hoppers, chains, candles,
  lanterns, and copper/pale-oak/resin variants.
- Reusable model-shape output for migrated families. `model_shape_for_block_state` currently
  emits state-driven meshes for fences, fence gates, walls, panes/bars, trapdoors, slabs,
  stairs, ladders, chains, doors, buttons, pressure plates, carpets, rails, vines,
  cross plants/cobwebs, redstone wire, torches, lanterns, candles, portals, and several
  complex block-shell families, including material slot and UV metadata where the vanilla
  model family needs it.

Permutation conditions support the common `query.block_state(...)` and `q.block_state(...)`
comparisons used by Bedrock block packs. Unknown expressions resolve to false and are reported
through resolver warnings instead of panicking.

Later pack roots override earlier roots. Pass packs in low-to-high priority order.

## Boundary

`bedrock-block-model` owns resource-pack parsing, block-state permutation resolution,
geometry/material lookup, terrain texture resolution, and reusable OBJ material semantics.
Applications should consume these resolved structures instead of reading `blocks.json`,
`terrain_texture.json`, or material alpha/tint rules directly.

Applications may adapt their own mesh buffers through `ObjMeshFaceSource`, `ObjMeshFace`,
`ObjFace`, or `ModelShape`, but OBJ syntax, MTL writing, `ObjExport` assembly, material alpha/tint
behavior, terrain texture lookup, mesh-face normal/material sampling, reusable vanilla block-shape
rules, and standard OBJ file write-out belong in this crate. A viewer/exporter should normally
resolve block detail geometry with `model_shape_for_block_state`, expose GPU or CPU mesh chunks
through `ObjMeshFaceSource`, call `export_obj_from_face_sources_with_package_roots`, and then use
`ObjExportTarget` plus `write_obj_export_files` for normal `.obj`/`.mtl`/texture output.
Lower-level callers can still use `obj_export_from_mesh_face_groups_with_progress`,
`obj_mesh_face_materials`, `obj_mesh_faces_string`, and `obj_export_from_parts` when they need
custom scheduling or streaming.

Block-state variant normalization also belongs here. Callers that start from world NBT should
convert NBT values into `BlockStateQuery` and then use `canonical_block_name_for_state` before
resource-pack lookup, or call `detail_material_block_name_for_state` when choosing the base block
material for a non-full-block mesh. These helpers cover legacy Bedrock variant states such as
`red_flower`, `double_plant`, `fence`, `cobblestone_wall`, colored carpet/wool/glass, shulker
boxes, and stone slab generations. Applications should not duplicate these material alias tables
in OBJ exporters.

When the caller starts from a Bedrock package/version root instead of explicit resource-pack roots,
use `export_obj_from_face_sources_with_package_roots` for normal OBJ exports. Lower-level callers
can use `ObjTextureResolver::with_package_roots` directly, but applications should not recreate
the vanilla pack-root expansion. The crate expands vanilla pack roots and versioned `vanilla_*`
overlays in priority order so applications do not need to duplicate that path logic.

World-context rules still belong at the application boundary. For example, if an old Bedrock pane
state omits explicit north/south/east/west connection booleans, a viewer may infer those
connections from neighboring blocks, inject the booleans into `BlockStateQuery`, and then call
`model_shape_for_block_state`. The generated pane mesh and UVs still come from this crate.

World resource-pack discovery is not UI policy and belongs here. Call `world_resource_pack_paths`
when an export should honor `world_resource_packs.json`; the crate parses relaxed Bedrock JSON,
normalizes pack UUIDs, matches resource-pack manifests, and searches the standard local/shared
Bedrock resource-pack folders. Applications may prepend or append extra package roots from their
own launcher/version state, but should use `push_unique_resource_pack_path` instead of duplicating
path de-duplication or manifest parsing.

OBJ target selection remains application-level, but standard target derivation, file write-out,
and reusable texture-copy behavior belong here. A caller chooses the selected `.obj` path, then
uses `ObjExportTarget::from_obj_path` and `write_obj_export_files`; this crate writes the OBJ
document, MTL document, texture copies, `.tga` to `.png` conversion, alpha-mask generation for
cutout materials, biome texture tinting, and relative texture-path validation. Lower-level custom
layouts may call `write_obj_texture_copy` directly, but application UI code should not duplicate
the standard OBJ/MTL/texture write path. The block/model decision and OBJ texture processing should
stay in this crate so Bedrock viewers and OBJ exporters share the same behavior.

## Model Family Modules

Reusable vanilla model behavior should be split by family in this crate instead of being appended
to an application exporter. The intended next modules are:

- `cross_plant`: flowers, grass, crops, web/cobweb, vines, coral fans, and other alpha-tested
  planes. Crossed-plane mesh output is implemented, and vines/wall-fan style attachments now
  have basic wall-plane shape output.
- `redstone_wire`: flat and wall-climbing wire meshes, power tint, and cross/line textures.
  State-driven plane mesh output is implemented.
- `connectors`: fences, fence gates, walls, iron bars, glass panes, copper bars, chains, and
  connected thin panels. State-driven cuboid mesh output with per-face UVs is implemented for
  the migrated connector families.
- `shapes`: trapdoors, doors, slabs, stairs, ladders, buttons, pressure plates, carpets, rails,
  and signs. Trapdoors, doors, slabs, stairs, ladders, buttons, pressure plates, carpets, and
  basic rail planes are implemented as state-driven mesh families.
- `special`: torches, lanterns, candles, portals, and other small non-cube or alpha-tested
  families. Torches, lanterns, candles, and portals are implemented.
- `container`: chest, trapped/ender/copper chest, barrel, hopper, and related block entities.
- `block_shell`: shulker boxes, anvils, stonecutters, decorated pots, cauldrons, signs, rails,
  and other complex block shells that still need full resource-pack geometry/UV consumption.

The current family list is checked against local BMCBL Bedrock 26.21 vanilla data and Mojang's
`bedrock-samples` `v1.26.20.4` resource pack/vanilla metadata. Add new vanilla names to the
family classifier first, then route mesh generation to the corresponding family module.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
