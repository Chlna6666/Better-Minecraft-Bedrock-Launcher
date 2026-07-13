# BMCBL Project Structure

This document describes the current repository structure and ownership model.
It is intentionally about where code belongs and how the system is assembled.
Implementation details for a specific feature should live in that feature's
own document.

## High-Level Architecture

```text
main.rs
  -> bmcbl::run()
  -> startup::run()
  -> AppBootstrap::from_config(...)
  -> Application::new_with_renderer_options(...)
  -> configure_runtime(...)
  -> build_app_state(...)
  -> open_main_window(...) / open_import_window(...)
```

```mermaid
flowchart LR
    "Process entry" --> "startup"
    "startup" --> "App bootstrap"
    "App bootstrap" --> "GPUI Application"
    "GPUI Application" --> "UI windows"
    "UI windows" --> "UI state"
    "UI windows" --> "Core services"
    "Core services" --> "Downloads / HTTP / Tasks"
    "GPUI Application" --> "vendor/gpui renderer"
    "vendor/gpui renderer" --> "nova-gfx backends"
```

The application crate owns product behavior. `vendor/gpui` owns generic UI and
rendering behavior. Local crates under `crates/` provide reusable support for
icons, hooks, graphics backends, and plugins.

## Root Files

| Path | Responsibility |
| --- | --- |
| `Cargo.toml` | Package metadata, workspace members, features, lints, dependencies, and local patches. |
| `Cargo.lock` | Locked dependency graph. |
| `build.rs` | Windows resources, embedded payload metadata, and build-time generated assets. |
| `src/main.rs` | Binary entrypoint for `BMCBL.exe`. |
| `src/lib.rs` | Application library root and module assembly. |
| `assets/` | Build-time input assets. |
| `docs/` | Architecture, GPUI, feature, and maintenance documentation. |
| `scripts/` | Build, validation, profiling, and maintenance scripts. |
| `vendor/` | Vendored framework and patched third-party crates. |

## Application Crate

### Startup And Runtime

| Path | Responsibility |
| --- | --- |
| `src/startup.rs` | Early startup orchestration, configuration loading, launch mode, and process-level checks. |
| `src/app.rs` | GPUI app construction, renderer options, image pipeline config, fonts, assets, globals, windows, lifecycle hooks. |
| `src/launch.rs` | Launch mode and Minecraft process launch entry points. |
| `src/result.rs` | Shared result and user-facing error support. |

`src/app.rs` is the application side of GPUI integration. It decides product
defaults such as renderer backend preference, adapter name, font source,
window transparency, image pipeline budget, and which windows to open.

### Configuration

| Path | Responsibility |
| --- | --- |
| `src/config/config.rs` | Persisted configuration model and normalization helpers. |
| `src/config/defaults.rs` | Default configuration values. |
| `src/config/storage.rs` | Configuration load and save support. |
| `src/config/test.rs` | Configuration test support. |

Configuration is a product concern. GPUI should not read BMCBL config directly.

### Core Domain Modules

| Path | Responsibility |
| --- | --- |
| `src/core/minecraft` | Minecraft Bedrock versions, paths, worlds, maps, screenshots, servers, resource packs, skin packs, AppX, GDK, launch preflight, and integration utilities. |
| `src/core/curseforge` | CurseForge queries and data handling. |
| `src/core/easytier` | EasyTier runtime assets, API, and networking runtime integration. |
| `src/core/inject` | PE and injection utilities. |
| `src/core/online` | Online room, peer, and ACL logic. |
| `src/core/version` | Version APIs, settings, launch versions, and GDK users. |
| `src/core/sponsors.rs` | Sponsor data loading and transformation. |
| `src/core/ui_prefs.rs` | UI preference persistence that outlives a single view. |

Core modules may use filesystem, process, network, parsing, and platform APIs
as needed. UI modules should call into core modules rather than duplicating
domain logic.

### IO, Tasks, And Services

| Path | Responsibility |
| --- | --- |
| `src/http` | HTTP request wrapper, GPUI-compatible HTTP client, and proxy handling. |
| `src/downloads` | Download manager, single and multi-file downloads, integrity, MD5, runtime support, and Windows Update client support. |
| `src/archive` | Archive extraction APIs, runtime support, and ZIP handling. |
| `src/tasks` | Background task manager and task snapshot model. |
| `src/music` | Music library, cover loading/cache, playback service, and music data types. |
| `src/plugins` | Plugin manifest, runtime, watcher, UI DSL, events, state, and plugin windows. |
| `src/utils` | Cross-cutting utilities such as logging, file operations, diagnostics, system info, network helpers, registry support, updater, and single-instance support. |

These modules own long-running work. Render code should observe their state or
request work through explicit APIs.

### Embedded Assets And Localization

| Path | Responsibility |
| --- | --- |
| `src/assets` | Application `AssetSource`, generated asset tables, and asset loading helpers. |
| `src/i18n` | Localization types and runtime language switching. |
| `assets/fonts` | Embedded font input files. |
| `assets/icons` | Application icon input files. |
| `assets/images` | Embedded image input files. |
| `assets/locales` | Translation source files. |
| `assets/bin` | Runtime payload input files. |

## UI Layer

The UI layer is a GPUI application layer, not a service layer. It owns views,
visual components, page state, window roots, overlays, and UI-only interaction
state.

Top-level structure:

| Path | Responsibility |
| --- | --- |
| `src/ui/main_window.rs` and `src/ui/main_window/` | Main window root, chrome, background, page registry, page loading, route effects, music panel, and update UI flow. |
| `src/ui/window.rs` and `src/ui/window/` | Standalone windows such as map viewer, skin preview, import, level.dat, debug, plugin, and shared window chrome. |
| `src/ui/views` | Main route pages: home, download, manage, plugin, settings, tasks, and tools. |
| `src/ui/components` | Reusable UI components with no page dependency. |
| `src/ui/state` | Cross-page UI state and global UI state. |
| `src/ui/theme` | Theme tokens and color interpolation. |
| `src/ui/overlays` | Full-window overlays and modal layers. |
| `src/ui/runtime` | Window root mounting and UI runtime glue. |
| `src/ui/hooks` | GPUI hook helpers and application hook wrappers. |

Detailed UI placement rules live in [`../src/ui/README.md`](../src/ui/README.md).

## Local Workspace Crates

| Path | Responsibility |
| --- | --- |
| `crates/gpui-hooks` | React-style hook support for GPUI views. |
| `crates/gpui-hooks-macros` | Procedural macros for hooks. |
| `crates/lucide-gpui` | Lucide icon asset crate for GPUI. |
| `crates/bmcbl-plugin-api` | Public plugin API types and pack metadata. |
| `crates/bmcbl-plugin-macros` | Plugin derive and helper macros. |
| `crates/bmcbl-plugin-tools` | Plugin packaging and validation tools when present. |
| `crates/nova-gfx` | Cross-backend graphics abstraction and backend crates for DX12, Vulkan, Metal, OpenGL, WebGL, memory, shader, and examples. |

Workspace crates should remain reusable. They may support BMCBL, but they
should not directly depend on BMCBL page modules or launcher state.

## Vendored GPUI

`vendor/gpui` is the framework. BMCBL patches it locally, but it should still
be treated as generic framework code.

Major GPUI areas:

| Path | Responsibility |
| --- | --- |
| `vendor/gpui/src/gpui.rs` | Public crate surface and re-exports. |
| `vendor/gpui/src/app` | App, contexts, entities, globals, effects, async contexts, actions, and test support. |
| `vendor/gpui/src/window` | Window lifecycle, frame scheduling, drawing, input, focus, dispatch, layout, and platform event handling. |
| `vendor/gpui/src/element` | Elements, div, text, image, SVG, lists, surfaces, event handlers, and element lifecycle. |
| `vendor/gpui/src/layout` | Layout engine, builders, cache, conversion, metrics, and tests. |
| `vendor/gpui/src/text_system` | Font fallback, line layout, wrapping, truncation, shaping, and paint support. |
| `vendor/gpui/src/scene` | Scene primitives, batching, prepared scene data, paths, meshes, transforms, and bounds trees. |
| `vendor/gpui/src/render_pipeline` | Renderer backend options, shader support, and SVG renderer bridge. |
| `vendor/gpui/src/platform` | Platform adapters and renderer implementations, including nova-gfx, Windows, Linux, macOS, tests, and legacy backend support. |
| `vendor/gpui/src/diagnostics` | Performance metrics and inspector support. |

The GPUI render path is documented in
[`GPUI_VENDOR_RENDERING.md`](GPUI_VENDOR_RENDERING.md).

## Dependency Direction

```text
src/ui
  -> src/core / src/downloads / src/tasks / src/http / src/music / src/plugins
  -> crates/gpui-hooks / crates/lucide-gpui
  -> vendor/gpui

src/core and service modules
  -> src/http / src/archive / src/downloads / src/utils
  -> external crates

vendor/gpui
  -> generic framework dependencies
  -> crates/nova-gfx through feature-gated backend paths
```

Forbidden directions:

- `vendor/gpui` must not depend on `src/*`.
- `src/core` must not depend on concrete pages under `src/ui/views`.
- `src/ui/components` must not depend on concrete pages.
- `src/app.rs` must not grow page rendering or workflow implementation.
- Render methods must not perform durable IO, downloads, parsing, or decoding.

## Where To Put New Work

| Change | Preferred location |
| --- | --- |
| New top-level page | `src/ui/views/<page>.rs` plus `src/ui/views/<page>/` if it needs internal modules. |
| New reusable visual primitive | `src/ui/components`. |
| New page-only panel or widget | The relevant page directory. |
| Cross-page UI state | `src/ui/state`. |
| Domain state or durable workflow | `src/core`, `src/downloads`, `src/tasks`, `src/music`, or `src/plugins`. |
| HTTP transport behavior | `src/http`. |
| New app startup global or window policy | `src/app.rs` if truly application-wide. |
| Generic framework rendering or input capability | `vendor/gpui`, with no BMCBL references. |
| Reusable icon, hook, graphics, or plugin support | The relevant crate under `crates/`. |

## Structural Maintenance Rules

- Keep `main.rs`, `lib.rs`, and module entry files thin.
- Prefer extending an existing responsibility module over adding a new tiny
  file for every helper.
- Split files when one file mixes state, rendering, IO, parsing, background
  tasks, and input behavior.
- Page or window root files should compose lifecycle and render entry points.
  State models, caches, parsing, background tasks, and complex interaction
  behavior should move to focused sibling modules.
- Keep module internals `pub(super)` or private by default. Use `pub(crate)`
  only when another ownership boundary genuinely needs access.
- Document large architectural changes in `docs/` at the same time as the code
  change.

## Validation Notes

Documentation-only changes should verify paths and links. Rust changes should
use the project validation commands in [`PROJECT_SPEC.md`](PROJECT_SPEC.md) and
the narrower checks required by the touched module.
