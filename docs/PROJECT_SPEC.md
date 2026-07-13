# BMCBL Project Spec

BMCBL is a GPUI-native Rust desktop launcher for Minecraft Bedrock, with
Windows as the primary supported target. The current product direction is a
single native executable with embedded assets, GPUI-rendered UI, and backend
logic implemented as ordinary Rust modules rather than WebView commands.

## Primary Deliverable

- Windows: one `BMCBL.exe` that starts without requiring a sibling resource
  folder.
- Runtime payloads that must exist on disk are extracted into an application
  runtime directory instead of being shipped next to the executable.
- UI is rendered through GPUI, using the vendored GPUI framework and the
  configured nova-gfx backend.

## Non-Goals

- A one-pass, 1:1 port of every historical Tauri UI behavior.
- WebView as the implementation surface for core screens.
- Moving BMCBL product defaults into GPUI framework defaults.
- Treating `src/ui` as the owner of network clients, durable caches, downloads,
  archive extraction, or Minecraft domain logic.

## Source Of Truth

- Current project structure: [`BMCBL_PROJECT_STRUCTURE.md`](BMCBL_PROJECT_STRUCTURE.md)
- Architecture boundaries: [`ARCHITECTURE_BOUNDARIES.md`](ARCHITECTURE_BOUNDARIES.md)
- GPUI structure and rendering pipeline: [`GPUI_VENDOR_RENDERING.md`](GPUI_VENDOR_RENDERING.md)
- UI module placement rules: [`../src/ui/README.md`](../src/ui/README.md)
- Map renderer design: [`MAP_RENDERER.md`](MAP_RENDERER.md)

## Workspace Layout

| Path | Role |
| --- | --- |
| `src/main.rs` | Binary entrypoint. Thin wrapper over `bmcbl::run()`. |
| `src/lib.rs` | Library root and application module assembly. |
| `src/startup.rs` | Startup orchestration, launch mode selection, and early process policy. |
| `src/app.rs` | GPUI application bootstrap, renderer options, fonts, assets, globals, windows. |
| `src/ui` | GPUI views, components, overlays, page state, window roots, visual behavior. |
| `src/core` | Non-UI domain and platform integrations. |
| `src/downloads`, `src/archive`, `src/tasks` | Download, extraction, integrity, runtime, and task snapshot workflows. |
| `src/http` | HTTP client and proxy support. |
| `src/music` | Music library, cover handling, playback service, and music state. |
| `src/plugins` | Plugin manifest, runtime, events, watcher, UI DSL, and plugin windows. |
| `crates/*` | Local reusable workspace crates. |
| `vendor/gpui` | Vendored GPUI framework. |
| `assets` | Build-time input assets embedded by the app. |
| `docs` | Project, GPUI, architecture, and feature documentation. |

## Layout Rules

- `src/app.rs` owns application startup decisions, not page rendering logic.
- `src/ui/views` owns top-level GPUI route pages.
- `src/ui/window` owns standalone utility windows and their internal modules.
- `src/ui/components` owns reusable visual components with no page dependency.
- `src/ui/theme` owns theme tokens and color helpers.
- `src/ui/state` owns cross-page UI state only.
- Page-private state and widgets stay inside the relevant page module.
- `src/i18n` owns localization implementation, while `assets/locales` is the
  translation source.
- `src/assets` owns embedded asset helpers, while `assets` contains build-time
  input files.
- `src/core`, `src/downloads`, `src/archive`, `src/http`, `src/tasks`,
  `src/music`, and `src/plugins` own non-UI implementation details.

## Asset Embedding Policy

- If the app needs a read-only file at runtime, prefer embedding it through
  `include_bytes!`, `include_str!`, generated asset tables, or the GPUI
  `AssetSource` implementation.
- Windows manifest and app icon resources are embedded through `build.rs` and
  platform resource tooling.
- Runtime payload metadata is produced by `build.rs`.
- If Windows or another subsystem requires an actual file path, extract the
  payload at runtime to:
  - `%LOCALAPPDATA%\BMCBL\runtime\...`
  - fallback: `%TEMP%\BMCBL\runtime\...`
- GPUI framework asset loading must remain generic. BMCBL asset names and
  default background policy belong in application code.

## Localization

- Runtime language switching is owned by BMCBL, not by a WebView component
  framework.
- `I18n` is registered as a GPUI global.
- Render code reads translated text through the global I18n state.
- Switching language updates global state and refreshes the relevant UI in the
  same process.
- Translation source files live under `assets/locales`.

## Renderer Policy

BMCBL configures GPUI through `RendererOptions` in `src/app.rs`.

- The configured renderer backend is parsed from launcher configuration and can
  still be overridden by `GPUI_RENDERER`.
- BMCBL currently prefers high-performance GPU selection for the app.
- Image pipeline budgets are product choices and are configured from the
  application bootstrap.
- GPUI owns the generic renderer pipeline, frame scheduling, metrics, and
  backend selection API.

For the full frame path, see [`GPUI_VENDOR_RENDERING.md`](GPUI_VENDOR_RENDERING.md).

## Backend Migration Policy

- Keep backend logic Tauri-free by default.
- Replace command-style UI boundaries with explicit Rust APIs and GPUI-native
  events or state updates.
- Keep platform-specific logic behind `cfg(...)` and target-specific modules.
- Long workflows should report progress through `src/tasks` or a focused
  domain state model rather than directly mutating view internals.

## Validation Baseline

Use focused checks for the area changed. For broad code changes:

```powershell
cargo fmt --all
./script/clippy
cargo test --workspace --all-features
```

For GPUI framework changes, add a GPUI-specific check:

```powershell
cargo check --manifest-path vendor/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
```

For documentation-only changes, verify referenced paths and links.
