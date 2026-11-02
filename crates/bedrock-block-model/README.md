# bedrock-block-model

`bedrock-block-model` resolves Minecraft Bedrock resource-pack block visual data into a
single model description that renderers and OBJ exporters can consume.

`ObjTextureResolver::pbr_texture_set_for` resolves `.texture_set.json` color/normal/MER/MERS
layers in pack priority order. Image references resolve only inside the definition's own pack;
constants retain byte values. Conflicting layers, invalid paths and missing same-pack images
return errors. Results preserve resource provenance and archive access without image decoding
or GPU dependencies. Height-map/subsurface presence is reported; their rendering is the consumer's
responsibility. This is separate from the model's face-to-texture-key `TextureSet`.

`vanilla_voxel_shapes()` exposes a separate, immutable database of 218 voxel volumes
(684 boxes) decoded from the supplied Bedrock 1.21.110 behavior-pack documents.
`generated/vanilla_voxel_shapes.json` preserves their identifiers, versions and original
16-unit coordinates; the API normalizes boxes to block units and precomputes conservative
16×16 boundary coverage. Unknown identifiers return `None`. These volumes have no UVs,
materials or complete BlockState mapping and never replace captured visual geometry.
Coverage alone does not establish opacity: consumers must verify both state and opaque
material semantics before using a volume for face occlusion. No game shader bytecode is embedded.

`BlockModelRepository::light_properties` resolves emission and attenuation on a 0..=15 scale,
including resource-pack components and permutations. Default numeric metadata for 1,356 Bedrock
1.26.30 block identifiers comes from the MIT-licensed
[PrismarineJS minecraft-data snapshot](https://github.com/PrismarineJS/minecraft-data/tree/b43925ca90babfd63745de4bbcd84feb35c1b40c/data/bedrock/1.26.30).
The generated table records its source/version; attribution is in
[`generated/block_lighting.NOTICE`](generated/block_lighting.NOTICE). These are default block
properties, not complete per-state or version-specific behavior. Water/leaves attenuation and
explicit extinguished/unlit states are handled; arbitrary partial-shape transmission and legacy
numeric-state lighting require additional metadata. This crate performs no propagation or GPU work.

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

Later pack roots override earlier roots, except that `resource_packs/vanilla` is treated as
      authoritative: it is appended after the versioned `vanilla_*` overlays, so base textures win
      when both packs ship the same file. Overlays are still consulted for files the base pack
      does not contain. Pass extra packs in low-to-high priority order.

Resource reads also support BRArchive v1 under `__brarchive/`, through the
self-maintained `bedrock-brarchive` workspace crate. Archive paths mirror pack
directories: `__brarchive/textures/blocks.brarchive` + `stone.png` resolves to
`textures/blocks/stone.png`. Non-empty archived payloads override matching loose
files within one pack; zero-length registrations fall back to loose files.
Block/model JSON and PNG/TGA textures are read on demand without whole-pack
extraction. Unknown archive versions and compiled MCB in JSON consumers return
explicit errors; MCB content decoding is not implemented here.
Texture/model/block archive indexes are validated when loading a pack, including
texture directories that a custom material could reference. A malformed index
fails pack loading even if its particular payload is not requested.

OBJ source fields (`source_path` and `source_texture_path`) now hold `ResourceFile`
instead of `PathBuf`. For loose inputs use `ResourceFile::file(path)`; use
`read()` for raw bytes and `path()` only for the logical location/extension.
An archived resource's logical path need not exist on disk. Source clones retain
the open archive after the resolver is dropped, including until OBJ export completes.

## Runtime BedrockSceneCapture model library

The 3D editor resolves mesh definitions by exact BlockState from an optional
**per-installation** BedrockSceneCapture export. It never bundles old
`captured_detail_models.bin` or `native_models.json` (those catalogs were
incomplete and contained misleading geometry). Java Edition geometry is not
used as an automatic replacement for Bedrock native shapes.

- Import a selected installation's `manifest.json` and `blocks.json` once when
  opening the 3D editor, then load `meshes/<sha256>.json` lazily on demand.
- Use full state identity and neighbor-dependent rules. No cached geometry may
  be silently reused for an unmatched stair orientation or wall connection.
- Resolve texture **resource aliases**, not version-indexed paths, from local
  `terrain_texture.json` and resource packs. Missing explicit aliases do not
  fall back to an unrelated face texture.
- `BMCBL_CAPTURE_MODEL_DIR` can explicitly point to the export directory.
- Native-model completeness is limited to what the export actually captured.
  The existing procedural Bedrock model-family geometry remains a provisional
  fallback, not a claim of native tessellation fidelity.

`ObjTextureResolver::local_light_for` reads resource-pack `local_lighting/local_lighting.json`
colors (sRGB bytes) and point/static classification. Lighting-only packs are accepted; malformed
optional lighting is reported without discarding valid texture resources. `environment_texture`
resolves logical texture paths in pack priority order. These APIs return CPU data, not GPU resources.

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
