# Better Minecraft Bedrock Launcher

[中文](README.md)

Better Minecraft Bedrock Launcher (BMCBL) is a Rust + GPUI desktop launcher for
Minecraft Bedrock Edition. The project has fully moved away from the old
Tauri / WebView / React stack. The current goal is a native Windows launcher
that can download, manage, launch, connect, and edit Bedrock worlds from a
single desktop application.

> Current status: Windows is the primary supported platform. Linux GDK game
> launching is in testing and can run GDK games normally through Proton / Wine,
> but Xbox achievements, presence, and other online capabilities still have
> known issues. macOS is not supported and is not planned.

## Status

| Area | Status |
| --- | --- |
| UI framework | Native GPUI, no WebView |
| Primary platform | Windows 10 / Windows 11 (primary supported platform) |
| Linux | Testing: GDK games run through Proton / Wine; achievements and presence are incomplete |
| Minecraft version types | UWP and GDK, including release / preview / education branches |
| Renderer | GPUI nova-gfx path, Nova DX12 by default on Windows, configurable Nova Vulkan |
| Plugin system | WASM sandbox plugins, API version `0.4` |
| License | GPL-3.0 |
| Changelog | [CHANGELOG.md](CHANGELOG.md) |

## Feature Support Matrix

This table is derived from the current code paths and distinguishes implemented
features from platform paths that are still under real-world testing. “Testing”
does not mean a stable release guarantee.

| Feature | Status | Notes |
| --- | --- | --- |
| Windows UWP / GDK launching | ✅ Supported | Release, preview, education, and related branches |
| Linux GDK game launching | 🧪 Testing | Proton / Wine path; basic gameplay works; only the RoundMCDev / UMU sign-in path is explicitly supported, and online capabilities still have issues |
| Game version download and installation | ✅ Supported | Search, filters, local package detection, redownload, and integrity checks |
| CurseForge resources | ✅ Supported | Browse, filter, paginate, view details, import shares, and install Bedrock resources |
| Resource and world management | ✅ Supported | Mods, packs, behavior packs, worlds, screenshots, servers, and backups |
| Skin management and preview | ✅ Supported | Manage skin packs, preview standard and 4D / custom-geometry skins, and support UWP / GDK versions |
| Advanced map window | ✅ Supported | 2D / 3D preview, chunk and record editing, player actions, undo / redo |
| Windows multi-account sign-in | ✅ Supported | Windows system-local Xbox account plus multiple BMCBL-managed accounts |
| Xbox achievements / presence for managed accounts | ⚠️ Unavailable | Achievements and presence are invalid for non-system-local account sign-in |
| New-version cloud saves for managed accounts | ⚠️ Unavailable | New-version cloud saves are unsupported; system-local accounts use Microsoft's official path |
| EasyTier online play | ✅ Windows / 🧪 Linux | Rooms, peers, NAT, ports, and connection logs; Linux remains in testing |
| WASM plugins | ✅ Windows / 🧪 Linux | Sandboxed plugins, page/window injection, permissions, and task APIs |
| macOS | ❌ Unsupported | Not in the current or planned support scope |

### Sign-in, account, and online-service limitations

- Windows supports multiple Xbox accounts. The Windows system-local Xbox account
  is handled by Microsoft's official XUser path; BMCBL-managed accounts can be
  signed in, switched, stored, and removed in the launcher.
- BMCBL-managed non-system-local accounts are passed through the BLoader XUser
  Bridge only on the Windows Win32 GDK path. Xbox achievements and presence are
  invalid on this path, and cloud saves for newer game versions are unavailable.
- UWP / AppContainer does not use the BLoader XUser Bridge. Without a valid
  BMCBL-managed session, the game continues with Microsoft's official sign-in.
- Linux does not use the BLoader XUser Bridge. It prepares GDK sign-in through
  WineGDK / Proton; only the RoundMCDev / UMU path is explicitly marked as
  sign-in capable, while other ProtonGDK sources may not sign in. The game
  itself can run normally, while achievements, presence, and other Xbox online
  features remain under testing.

## Versions And Releases

- Stable versions use `v<major>.<minor>.<patch>` tags. GitHub Actions builds a
  Windows x86_64 release asset automatically.
- A nightly prerelease is generated weekly when the default branch has new
  commits. Nightlies are for testing and are not stable releases.
- Release notes are generated from commit history, and pushes to the default
  branch update the generated commit summary in `CHANGELOG.md`.
- [View GitHub Releases](https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher/releases)

## Features

### Launching And Versions

- Scan and manage local Minecraft Bedrock UWP / GDK installations.
- Recognize release, preview, education, and education preview branches.
- Run launch prerequisite checks for UWP developer mode, UWP dependencies, and
  GDK GameInput.
- Configure launch arguments, launcher visibility after launch, and UWP minimize
  behavior.
- Support debug console, isolated mode, editor mode, disabled mod loading, mouse
  locking, and custom unlock hotkeys.
- Use BMCBL's own [`BLoader`](https://github.com/Chlna6666/BLoader) project and its `BLoader.dll` loader
  for native mod injection, injection delay, and mod type configuration. The
  Windows Win32 GDK managed-account session is also passed through its XUser
  Bridge.

### Downloads

- Download game versions with search, release / preview filters, local package
  detection, redownload, CDN probing, and source selection.
- Browse CurseForge resources with categories, subcategories, version filters,
  sorting, pagination, and list / grid views.
- Install Bedrock add-ons, maps, skins, texture packs, scripts, and related
  resource types.
- Import CurseForge share content from the clipboard.
- Configure multi-threaded downloads, automatic thread count, maximum threads,
  system / HTTP / SOCKS5 proxy modes.
- Select official, MCIM mirror, or custom CurseForge API base URLs.

### Resource And World Management

- Manage mods, resource packs, behavior packs, worlds, screenshots, and server
  lists per Minecraft version.
- Search, sort, import, back up, delete, open folders, enable, and disable
  resources.
- Mark mod types such as native, preload, hot-inject, and LSE QuickJS.
- Launch worlds, export worlds, and edit NBT / `level.dat` from world entries.
- Scan GDK user directories for screenshots.
- Read server lists and query MOTD, version, player count, and latency.
- Manage skin packs per game version, read skin textures and model geometry, and generate previews.
- Preview standard and 4D / custom-geometry skins; GDK versions read skin resources from their corresponding game directories.

### Advanced Map Window

The map window is a major GPUI feature, not just a simple world preview.

- 2D map tile rendering streams visible viewport tiles immediately without
  waiting for full-world indexing.
- Supports Surface, Biome, Height, Layer, and Cave render modes.
- Supports Overworld, Nether, End, and custom dimensions.
- Provides interactive pan, zoom, positioning, and on-demand tile chunk trees.
- Uses render sessions, cache policies, tile manifests, decoded tile caches, and
  cancellable generation-based pipelines.
- Surfaces CPU budget, cache hit / miss counts, GPU backend diagnostics, and
  fallback reasons.
- Chunk operations include range selection, selection statistics, chunk delete,
  and chunk reset.
- Record operations include deleting block entities / actors and editing block
  entities, actors, hardcoded spawn areas, HeightMap, Biome Storage, map records,
  and global records.
- Chunk copy / paste supports single-chunk and multi-chunk paste, rotation,
  mirroring, paste previews, and explicit write confirmation.
- `.mcstructure` import / export supports selected-region export, structure
  preview, and world paste.
- 3D preview is available for structures or selections.
- Player editing supports inspection plus quick actions such as move to map
  center, set dimension, and clear inventory.
- Edit history captures chunk delete, reset, paste, record save / delete, player
  edits, and `level.dat` saves, with undo / redo and restore points.
- Write mode must be explicitly enabled before world mutation.

See [docs/MAP_RENDERER.md](docs/MAP_RENDERER.md) for map rendering details.
Entity icon generation and the script rendering pipeline are documented in
[docs/PROJECT_PLAN.md](docs/PROJECT_PLAN.md).

### Online Play

- EasyTier-based online play panel.
- Create or join rooms and display room code, network name, peers, and virtual
  IPv4.
- Configure NAT checks, player name, and game ports.
- Use automatic public bootstrap peers or manual bootstrap peer lists.
- Toggle compatibility options such as `disable_p2p` and `no_tun`.
- Show peer state, game endpoint, and runtime logs.

### Customization And Settings

- Runtime language switching: auto, Simplified Chinese, Traditional Chinese,
  English, Japanese, and Korean.
- Renderer and GPU adapter selection.
- Theme color, default / local / network background, background blur, and glass
  effect settings.
- Embedded fonts, local font files, and installed system fonts.
- Stable / nightly update channels, automatic update checks, and manual update
  checks.
- Crash diagnostics, log tails, GitHub Issue flow, Sentry reporting switches,
  and Sentry test logs.
- Connectivity checks for launcher services, Microsoft / Xbox services, and
  community resource services.

### Plugins

BMCBL plugins are WASM sandbox plugins, not the old JavaScript plugin model.

- Plugin manifest file: `plugin.toml`; package extension: `.bmcblx`.
- Current plugin API version: `0.4`; manifest schema version: `2`.
- Plugins can register navigation pages, windows, UI injection slots, and global
  event subscriptions.
- Host operations include toasts, navigation, windows, modals, clipboard, HTTP
  text requests, resource reads, KV storage, task progress, config read / write,
  theme snapshots, and app info.
- Capabilities and allowlists control permissions such as `network.http`,
  `storage.kv`, `config.read`, `config.write`, `ui.page`, and `ui.window`.
- The settings page exposes plugin enabled state, permissions, config, README,
  logs, and diagnostics export.
- Examples live in `examples/plugins/hello-wasm` and
  `examples/plugins/bedrock-notes`.

## Architecture

BMCBL is now a native desktop application built with the `egpui` application
framework and GPUI GUI core. The Tauri compatibility layer has been removed.
The default build does not include a web frontend, WebView, or
Tauri command wrappers.

```mermaid
flowchart TD
    UI["src/ui\nGPUI views, components, windows, state"] --> Core["src/core\nMinecraft, downloads, CurseForge, EasyTier"]
    UI --> Plugins["src/plugins\nWASM plugin runtime"]
    UI --> Config["src/config\nConfiguration and migrations"]
    Core --> Downloads["src/downloads\nMulti-thread downloads, WU protocol, integrity"]
    Core --> Assets["src/assets / assets\nEmbedded assets, fonts, icons, locales, runtime payloads"]
UI --> GPUI["crates/gpui\nnova-gfx renderer, windows, element system"]
UI --> EGPUI["crates/egpui\nlifecycle, app runtime, UI bridge"]
EGPUI --> GPUI
    GPUI --> Nova["crates/nova-gfx\nDX12 / Vulkan / Metal / OpenGL / WebGL abstraction"]
```

Main directories:

| Path | Purpose |
| --- | --- |
| `src/ui` | GPUI pages, components, windows, theme, and app state |
| `src/core` | Minecraft, versions, maps, packs, online play, CurseForge |
| `src/downloads` | Download tasks, multi-thread downloads, WU protocol, integrity |
| `src/plugins` | WASM plugin loading, sandbox execution, UI DSL, hot reload, packages |
| `src/i18n` | Runtime localization |
| `src/config` | Config structs, defaults, migrations, and persistence |
| `assets` | Compile-time icons, fonts, images, locales, and binary payloads |
| `crates/egpui` | Desktop host, Tokio/Rayon runtime, task scopes, services, and UI bridge |
| `crates/gpui` | Independently maintained GPUI GUI core |
| `crates/nova-gfx` | Cross-backend graphics abstraction used by the GPUI nova renderer path |
| `crates/bmcbl-plugin-api` | Plugin ABI, macros, and packaging tools |
| `crates/gpui-hooks` | GPUI hooks helpers |
| `crates/lucide-gpui` | Lucide icon adapter for GPUI |

Current structure: [docs/BMCBL_PROJECT_STRUCTURE.md](docs/BMCBL_PROJECT_STRUCTURE.md).
Desktop framework tasks: [docs/EGPUI_DESKTOP_APPLICATION_FRAMEWORK_TASKS.md](docs/EGPUI_DESKTOP_APPLICATION_FRAMEWORK_TASKS.md).
Architecture boundaries: [docs/ARCHITECTURE_BOUNDARIES.md](docs/ARCHITECTURE_BOUNDARIES.md).
Async runtime and GPUI state model:
[docs/ASYNC_RUNTIME_MODEL.md](docs/ASYNC_RUNTIME_MODEL.md).
GPUI renderer notes: [docs/GPUI_VENDOR_RENDERING.md](docs/GPUI_VENDOR_RENDERING.md).
Router and hooks: [docs/GPUI_ROUTER_HOOKS.md](docs/GPUI_ROUTER_HOOKS.md).

## Implementation And Loader Notes

- BMCBL's launcher, GPUI interface, version management, downloads, account
  flows, world editor, online play, and Linux Proton / Wine adapter are
  independently implemented as Rust modules in this repository.
- This project is not a 1:1 copy of another launcher's pages or business code.
  BedrockBoot, BedrockLauncher.Core, and other projects are used only as
  protocol, file-format, and compatibility references; BMCBL maintains its own
  code and module structure.
- BMCBL uses its own [`BLoader`](https://github.com/Chlna6666/BLoader) native loader
  project and embeds `BLoader.dll` at build time. BLoader handles native mod
  loading and the Windows Win32 GDK XUser Bridge; BMCBL owns launch orchestration,
  configuration, account preparation, and UI integration.

## Development

### Requirements

- Windows 10 / Windows 11.
- Rust stable with edition 2024 support.
- MSVC toolchain and Windows SDK.
- Git.
- Network access to Cargo registry and the EasyTier Git dependency.

The current `Cargo.toml` uses several local path dependencies. By default,
`BE-Community-Dev` is expected next to this repository:

```text
workspace-root/
  BMCBL/
  BE-Community-Dev/
    mc-motd/
    bedrock-world/
    bedrock-render/
    bedrock-block-model/
```

If your layout differs, update the corresponding `path` dependencies in
`Cargo.toml`.

### Build And Run

```powershell
cargo run --bin BMCBL
cargo build --release --bin BMCBL
```

Optional features:

```powershell
cargo run --bin BMCBL --features gpui-windows-vulkan
cargo run --bin BMCBL --features preview-3d-dx12
```

`build.rs` embeds Windows icon / manifest resources, fonts, localization,
images, `BLoader.dll`, and EasyTier runtime payload metadata. EasyTier
`wintun.dll` is discovered from local vendored paths or
Cargo Git checkouts. Missing files produce warnings and may disable some online
play modes.

### Checks

```powershell
cargo fmt --all
cargo test --workspace
```

Project Rust conventions:

- edition 2024.
- `unsafe_code = "warn"`.
- Clippy `all` and `pedantic` are warnings.
- Avoid `unwrap()` in library code; propagate errors with `?`.
- Every `unsafe` block needs a `// SAFETY:` comment.
- UI render code should not own network IO, parsing, caches, downloads, or
  durable workflows.

### Plugin Development

Install the WASM target:

```powershell
rustup target add wasm32-unknown-unknown
```

Build example plugins:

```powershell
cargo build --manifest-path examples/plugins/hello-wasm/Cargo.toml --release --target wasm32-unknown-unknown
cargo build --manifest-path examples/plugins/bedrock-notes/Cargo.toml --release --target wasm32-unknown-unknown
```

The example plugin `build.rs` files call
`bmcbl_plugin_api::pack::auto_pack_from_build_script()` to generate `.bmcblx`
packages automatically. Manual packaging is available through
`bmcbl-plugin-tools` in `crates/bmcbl-plugin-api`.

### Localization

Locale files live in `assets/locales/*.lang`; user agreement markdown lives in
`assets/locales/agreement/*.md`. When adding UI strings, update the locale keys
and run:

```powershell
scripts/check_i18n_lang.ps1
scripts/check_i18n_ui.ps1
```

`check_i18n_ui.ps1` reports common hard-coded UI text candidates and missing
static translation keys. Review its output manually; element IDs, protocol
values, paths, logs, font names, and other non-user-facing strings are expected
to be filtered out. Use `-Strict` when the migration is complete.

## Development Notes

- Do not reintroduce Tauri, WebView, Vite, or React as the main UI stack.
- GPUI framework code must not depend on BMCBL routes, pages, assets, download
  services, or window policy.
- `src/ui` renders and coordinates UI state. Network IO, parsing, downloads,
  caches, and persistence belong in backend modules.
- Before changing the map window, read `docs/MAP_RENDERER.md` and preserve
  visible-tile streaming, cache behavior, and cancellation generation semantics.
- World write features must keep explicit confirmation, history capture, and
  user-visible error reporting.
- Runtime assets should be embedded through `build.rs` or `include_bytes!` /
  `include_str!`; DLLs or drivers that must exist on disk should be extracted
  into local app data / cache directories.

## Credits

- MCAPPX: version index and metadata support.
- MCMrARM / mc-w10-version-launcher: Windows Update protocol and version
  discovery references.
- BedrockLauncher.Core: GDK unpacking and Bedrock implementation references.
- EasyTier: online play foundation.
- Aetopia / AppLifecycleOptOut: UWP minimize freeze fix reference.
- MCIM: CurseForge mirror and download acceleration.
- GPUI / Zed GPUI: native UI and renderer foundation.

## License And Disclaimer

BMCBL is licensed under GPL-3.0 (the GNU General Public License, version 3).
See [LICENSE](LICENSE) for the complete license terms. It is intended for
learning, research, and community use.

Minecraft, Minecraft Bedrock Edition, related trademarks, assets, and services
belong to Mojang Studios / Microsoft. This project is not an official Mojang or
Microsoft product and is not affiliated with them.
