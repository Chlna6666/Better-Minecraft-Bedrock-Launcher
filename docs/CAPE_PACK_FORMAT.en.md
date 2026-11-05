# Cape pack format

Cape packs may be `.mccapes` or `.zip` archives, or unpacked directories. The manager extracts imported archives to the game data directory at `capes/<pack name>/`. A ZIP may contain one enclosing folder. Importing a single PNG with a 64×32 aspect ratio creates a complete one-cape pack automatically.

The pack root must contain `manifest.json` and `cape.json`; each pack contains 1 to 64 capes. A typical layout is:

```text
MyCapes/
  manifest.json
  cape.json
  textures/blue.png
  textures/blue_elytra.png
  icons/blue.png
  texts/en_US.lang
```

`manifest.json` uses a Bedrock-style header and module. The module type is `cape_pack`; header and module UUIDs must differ. Versions contain three nonnegative integers.

```json
{"format_version":1,"header":{"name":"My Capes","uuid":"11111111-2222-4333-8444-555555555555","version":[1,0,0]},"modules":[{"type":"cape_pack","uuid":"66666666-7777-4888-8999-aaaaaaaaaaaa","version":[1,0,0]}]}
```

`cape.json` defines stable localization keys and texture paths. `texture` is required and must have a 64×32 aspect ratio at an integer scale. Optional `elytra` supplies a separate wing texture; optional `icon` supplies a list icon. Without `icon`, the launcher generates and caches one from the cape texture. Optional metadata includes `rarity` (`common`, `uncommon`, `rare`, `epic`, `legendary`), `start_date` (UTC `YYYY-MM-DDTHH:MM:SSZ`), `item_uuid`, and `persona_zip`.

```json
{"serialize_name":"My Capes","localization_name":"my_capes","capes":[{"localization_name":"blue","texture":"textures/blue.png","elytra":"textures/blue_elytra.png","icon":"icons/blue.png"}]}
```

Localization files use `texts/<language>.lang`, with one `key=display name` per line. Supply `texts/en_US.lang` for the default names. Missing translations fall back to the manifest name or localization key. Resolution order: the requested language file, then `texts/en_US.lang`, then the first available `.lang` in the pack.

```text
capepack.my_capes=My Capes
cape.my_capes.blue=Blue Cape
```

Texture layout: cape textures use the standard 10×16×1 box unwrap with the outer face (seen from behind) at (1,1)-(11,17) and the inner face at (12,1)-(22,17); elytra textures use the standard `texOffs(34, 0)` 10×20×2 unwrap with the front face at (36,2)-(46,22) and the back face at (48,2)-(58,22). Icons are arbitrary-size PNGs (64×64 recommended). The retired dev layouts — legacy `pack.json` manifests and bare `cape.json` + `cape.png` single capes — are no longer read and are skipped as invalid packs.

Capes are enabled by default in version settings. Disabling them hides the manager tab. UWP versions below 1.21 use an older loader path and display an unsupported notice.
