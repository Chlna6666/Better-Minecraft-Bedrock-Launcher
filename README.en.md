# Better Minecraft Bedrock Launcher (BMCBL)

[简体中文](README.md) | English

BMCBL is a native Minecraft Bedrock Edition launcher written in Rust and GPUI.
The current generation is a native desktop application: Tauri, WebView, Vite and React
are no longer part of the main UI path.

> Platform status: Windows 10/11 is the primary supported platform. Linux is in testing
> (GDK games can run through Proton/Wine, but Xbox achievements and presence are incomplete).
> macOS is not supported now or in the planned future.

## Status

| Area | Status |
| --- | --- |
| UI | Native GPUI, no WebView |
| Primary platform | Windows 10 / Windows 11 |
| Linux | Testing: Proton/Wine GDK launch works; achievements/presence incomplete |
| Rendering | GPUI nova-gfx path: DX11, DX12, Vulkan, OpenGL 4.5 on Windows (default DX11); Vulkan + OpenGL on Linux (default Vulkan) |
| Plugins | WASM sandbox, plugin API `0.6`, manifest schema `2` |
| License | GPL-3.0 |

## Features

- Version discovery, download, installation and integrity checks for UWP/GDK Bedrock.
- Windows local Xbox accounts plus BMCBL-managed accounts.
- CurseForge browsing/import and management of mods, resource packs, behavior packs, maps,
  screenshots and servers.
- Skin management and previews, including 4D/custom geometry skins.
- Advanced map window: streaming 2D tiles, chunk/record editing, undo/redo, player tools,
  `.mcstructure` import/export and 3D preview.
- EasyTier-based multiplayer (Windows stable, Linux testing).
- WASM plugin host with capability-based permissions and UI injection.
- Runtime i18n (Simplified/Traditional Chinese, English, Japanese, Korean), themes, fonts,
  renderer/adapter selection and diagnostics.

## Architecture

```text
src/ui            GPUI pages, components, windows, map viewer
src/core          Minecraft/GDK, versions, accounts, CurseForge, maps, networking
src/downloads     download tasks, WU protocol, verification
src/config        configuration and migration
src/plugins       WASM plugin host
crates/egpui      desktop host, Tokio/Rayon runtime, UI bridges, i18n
crates/gpui       GPUI UI core and nova renderer
crates/gpui-3d    3D scene preparation and scene-view extension
crates/nova-gfx   backend-neutral graphics API and backends
crates/bedrock-*  Bedrock world/storage/render/model crates
crates/bmcbl-plugin-api  plugin ABI and packing tools
```

See [docs/ARCHITECTURE.en.md](docs/ARCHITECTURE.en.md) and [docs/CRATES.en.md](docs/CRATES.en.md)
for the full layering and catalog. The gpui/nova-gfx boundary is recorded in
[ADR 0001](docs/adr/0001-nova-gfx-gpui-boundary.md) (Chinese).

## Build and run

Requirements: stable Rust (edition 2024), MSVC + Windows SDK on Windows, LLVM/libclang for
build scripts. Plugin development additionally needs the `wasm32-unknown-unknown` target.

```powershell
cargo run --bin BMCBL
cargo build --release --bin BMCBL
```

Windows builds DX11 by default (target dependency) and enables DX12/Vulkan/OpenGL through the
root features. The in-app default backend is DX11 and can be changed in settings.

## Documentation

- [Documentation index](docs/README.en.md)
- [Architecture](docs/ARCHITECTURE.en.md)
- [Build and test](docs/BUILD_AND_TEST.en.md)
- [Contributing](docs/CONTRIBUTING.en.md)
- [Crate catalog](docs/CRATES.en.md)
- [Command-line interface](docs/CLI.en.md)
- [Startup arguments](docs/STARTUP_ARGUMENTS.en.md)
- [Naming rules](docs/NAMING.md) (Chinese)
- [Changelog](CHANGELOG.md)

## Development rules

- Do not reintroduce Tauri/WebView/Vite/React as the main UI stack.
- `nova-gfx` is a graphics library: it must not contain UI framework concepts or GPUI code.
- `gpui` consumes `gfx-core` capability traits; it must not call backend-specific inherent
  methods.
- Put IO, parsing, downloads and persistence in `src/core`, `src/downloads` or `src/config`,
  not in `src/ui` render paths.
- Documentation must match code. Do not commit test output, raw benchmark data, rename histories
  or dated plans under `docs/`.

## License

GPL-3.0. See [LICENSE](LICENSE). The project is for learning, research and community use.
