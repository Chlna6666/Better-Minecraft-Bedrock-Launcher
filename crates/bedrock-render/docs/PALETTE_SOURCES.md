# Palette Sources

`bedrock-render` treats the JSON palette files as auditable source data. The
default runtime palette is built directly from the embedded JSON sources.

## Files

- `data/colors/bedrock-block-color.json` stores default block colors keyed by
  block name. Schema v3 entries contain `default` plus one auditable source key
  such as `resource_pack_top`, `resource_pack_alias`,
  `resource_pack_tint_mask`, `resource_pack_special`, or
  `clean_room_fallback`. Special block entries may also contain
  `variant_colors` for block-entity-driven map colors.
- `data/colors/bedrock-biome-color.json` stores biome base colors and optional
  grass, foliage, and water tint overrides. Shared fallback tint values live in
  `defaults`, not as a synthetic biome id.
- `data/colors/bedrock-resource-pack-aliases.json` stores the small alias table
  used when Bedrock `blocks.json` does not expose a modern block id directly.
  Exact aliases and ordered family rules may point to a canonical block id,
  terrain texture alias, or `textures/blocks/...` PNG path.

## Source Policy

Every palette JSON document contains:

- `schema_version`
- `minecraft_bedrock_version`
- `sources`
- either `blocks` or `defaults` plus `biomes`

Block palette `schema_version` is currently `3`. The runtime importer remains
compatible with v1 object-map data and v2 wrapper data, but generated defaults
use v3 so each entry has a stable default color, explicit source metadata,
fallback reasons, optional `state_colors`, and optional `variant_colors`.

Biome-tinted block entries use `resource_pack_tint_mask` as the neutral texture
mask that is multiplied by grass, foliage, or water biome tint at render time.
When the mask is computed from a vanilla texture average,
`resource_pack_tint_source` is `texture_average`; when a tintable block cannot
resolve a PNG texture from the selected pack, it is `fallback_mask` with an
explicit `resource_pack_tint_reason`.

Committed default color values are marked as resource-pack-derived when they
come from the local vanilla Bedrock resource pack. Clean-room entries remain as
fallbacks for blocks or biome semantics that the resource pack does not expose
directly. Minecraft Wiki content is still used only for identifiers and semantic
cross-checking, not for copied icons or Wiki color tables.

The resource-pack pipeline parses:

- `blocks.json` for the block inventory and top-priority per-face texture specs.
- `textures/terrain_texture.json` for texture aliases, including texture arrays.
- `textures/blocks/*.png` for alpha-weighted average colors.
- `textures/entity/pistonarm/*.png` for extended piston and sticky piston arms.
- banner base colors from vanilla wool textures for banner block entities.
- `biomes_client.json` plus `textures/colormap/*.png` for biome tint defaults.

The vanilla `biomes_client.json` file is treated as read-only input. Do not edit
or commit resource-pack files; when Mojang updates the official data, rerun the
derivation command and review the generated JSON diff instead.

Top-down map rendering uses top faces first: `up`/`top`, then side faces,
`*`/`all`, then string texture aliases. Side colors are preserved for pillar
blocks so `pillar_axis = x|z` can use bark/side color while `y` or missing state
keeps the top color.

Alias policy is intentionally conservative. An alias emits `resource_pack_alias`
only when its target resolves to a vanilla texture from the selected pack. If
the target is missing, the block remains `clean_room_fallback` with an explicit
reason. Do not add aliases that point to visually wrong legacy textures; for
example, old green `bamboo_stem` is not used as bamboo planks.

State-sensitive colors are stored in JSON under `state_colors`, not hardcoded in
the renderer. Current generated states cover crop `growth`, farmland
`moisturized_amount`, and pillar `pillar_axis`.

Block-entity-sensitive colors are stored in JSON under `variant_colors`.
Current generated variants cover piston arms, banner base colors, and decorated
pot base colors. If a vanilla pack omits a special texture, such as decorated
pot textures in the local 26.13 pack at
`C:\Users\Administrator\Desktop\BMCBL\target\debug\BMCBL\versions\26.13\data\resource_packs\vanilla`,
the entry remains a clean-room fallback with
`resource-pack-special-texture-unresolved`.

Referenced public material:

- Microsoft Learn `blocks.json` structure reference:
  <https://learn.microsoft.com/en-us/minecraft/creator/reference/content/blockreference/examples/blocksjsonfilestructure?view=minecraft-bedrock-stable>
- Microsoft Learn client biome reference:
  <https://learn.microsoft.com/en-us/minecraft/creator/reference/content/clientbiomesreference/examples/components/client_biome_definition?view=minecraft-bedrock-stable>
- Microsoft Learn `minecraft:map_color` component reference:
  <https://learn.microsoft.com/minecraft/creator/reference/content/blockreference/examples/blockcomponents/minecraftblock_map_color?view=minecraft-bedrock-experimental>
- Minecraft Wiki Bedrock block ID and biome reference pages are used only for
  identifier and semantic cross-checking, not for copied icons or color values.
- Mojang Bedrock Samples client biome reference:
  <https://mojang.github.io/bedrock-samples/Client%20Biomes.html>
- Microsoft Minecraft samples repository:
  <https://github.com/microsoft/minecraft-samples>

## Maintenance Commands

Audit source shape, metadata, normalization, and semantic guardrails:

```powershell
cargo run --example palette_tool -- audit --check
```

Audit committed data against a live vanilla resource pack:

```powershell
cargo run --example palette_tool -- audit --check --pack <vanilla-resource-pack>
```

Regenerate committed defaults from a vanilla resource pack:

```powershell
cargo run --example palette_tool --features png -- derive-from-resource-pack --pack <vanilla-resource-pack> --write-defaults
```

The clean-room generator is still available for fallback-rule maintenance:
`cargo run --example palette_tool -- generate-clean-room --check|--write`.

Check whether the JSON files already match the canonical schema/ordering:

```powershell
cargo run --example palette_tool -- normalize --check
```

Rewrite the JSON files into canonical form:

```powershell
cargo run --example palette_tool -- normalize
```

Generate a local resource-pack reference palette under `target/` when you need
to compare against user-provided assets:

```powershell
cargo run --example palette_tool --features png -- derive-from-resource-pack --pack <resource-pack> --out target/resource-pack-palette.json
```

If a color change affects rendered output, update preview images and bump both
`DEFAULT_PALETTE_VERSION` and `RENDERER_CACHE_VERSION` so tile caches are
invalidated.

The opaque appearance entries for bamboo mosaic/planks, cherry/crimson/mangrove/pale-oak/
warped planks, quartz bricks and smooth quartz use verified Bedrock 26.40 vanilla texture
averages. Their previous unresolved aliases or missing textures selected synthetic fallback
colors, including dark brown for pale oak. Top and side values now agree with the conversion
palette. These are appearance colors; the separate pinned in-game item-map colors are unchanged.
The terrain atlas preserves those base colors instead of blending them with generic material
hues. Wood, built and metal surfaces use ordinary relief lighting, without the atlas's artificial
terrain texture. Cache versions 55/18 invalidate the previous renderer/palette results.

For an API-only check, the voxel `color_comparison` example also exports its flat `.mcstructure`
fixtures. `render_structure_colors <fixture.mcstructure> <output.png>` places one into a synthetic
in-memory world and runs the actual CPU tile renderer. It never opens a user's world.

The audit command also enforces semantic guardrails for high-risk entries:
bamboo must be green, bamboo material must stay yellow, farmland and paths must
stay distinguishable from ordinary dirt, leaf litter must use dry foliage rather than green
foliage tint, tintable grass/leaves must remain neutral masks, water must use a blue
mask, and key biome grass and water tints must produce visibly distinct final
surface colors.

Installed resource packs are read as the complete numeric `vanilla_*` version stack,
with newer JSON definitions and texture files overriding older ones. Modern
`biomes/*.client_biome.json` supplies explicit water, grass, foliage and dry-foliage
colors. Color-map objects require climate/position sampling; the static aggregate
palette retains its documented fallback rather than averaging a whole climate map
as if it were a single biome color. Dry foliage has a separate channel and never
inherits green foliage tint.

Foliage tint is selected by a known mask family, not a `leaf` substring.
Precolored azalea/cherry/pale-oak textures retain their appearance; birch and
spruce use their fixed foliage tints, including legacy `old_leaf_type` states.
This avoids multiplying already-colored leaves by a second biome green.
The distinction follows the material tint semantics in
[Mojang's material schema](https://github.com/Mojang/bedrock-schemas/blob/main/types/bp/blocks/components/MinecraftMaterialInstances.d.ts),
cross-checked against the fixed-versus-colormap design and values in the
[block-model-renderer source](https://github.com/ewanhowell5195/block-model-renderer/blob/master/src/core/data/colors.json).
The latter is a Java renderer reference; resource appearance remains sourced
from the installed Bedrock 26.40 packs, not from its texture/color table.

Illumination decodes sRGB to linear radiance, scales radiance, then encodes sRGB.
Highlights do not mix toward white. The same operation supplies tile, height and
3D face lighting; alpha is unchanged. Conversion matching still uses OKLab and
opaque input still excludes transparent glass candidates.
