# Architecture Boundaries

This document defines ownership boundaries between BMCBL application code,
local workspace crates, and the vendored GPUI framework. It is the first file
to read before changing `src/app.rs`, `src/ui`, `vendor/gpui`, renderer
startup, window policy, or application-wide runtime behavior.

Related documents:

- [`docs/BMCBL_PROJECT_STRUCTURE.md`](BMCBL_PROJECT_STRUCTURE.md): current
  BMCBL workspace and module map.
- [`docs/GPUI_VENDOR_RENDERING.md`](GPUI_VENDOR_RENDERING.md): GPUI vendor
  structure, frame scheduling, and nova-gfx rendering pipeline.
- [`src/ui/README.md`](../src/ui/README.md): UI layer placement rules and
  current `src/ui` structure.

## Ownership Model

```text
BMCBL product behavior
  src/app.rs
  src/startup.rs
  src/ui
  src/core
  src/downloads
  src/http
  src/tasks
  src/music
  src/plugins

Reusable workspace support
  crates/gpui-hooks
  crates/lucide-gpui
  crates/nova-gfx
  crates/bmcbl-plugin-*

Generic UI framework
  vendor/gpui
  vendor/gpui-router
  vendor/gpu-allocator
```

The dependency direction is from product code toward reusable crates and
framework code. Framework code must not know about BMCBL routes, pages,
launcher state, assets, default backgrounds, downloads, or product window
policy.

## GPUI Framework Code

`vendor/gpui` owns generic UI primitives and platform integration:

- application lifecycle primitives;
- `App`, `Context<T>`, `AsyncApp`, `AsyncWindowContext`, `Entity<T>`, and
  entity invalidation;
- `Window`, platform windows, input dispatch, focus, keymaps, actions, and
  hit testing;
- element lifecycle, style, layout, text shaping, asset loading, image caches,
  and scene construction;
- frame scheduling, dirty region tracking, retained frame reuse, and frame
  metrics;
- renderer options, renderer backend selection, nova-gfx renderer integration,
  swapchain presentation, and retained GPU resources.

Framework defaults must be portable and application-neutral. Low idle work,
event-driven rendering, adapter selection knobs, present mode preferences,
headless test support, and renderer diagnostics belong in GPUI because they
define behavior for every GPUI application.

Framework code must not depend on:

- BMCBL routes, pages, or tab names;
- BMCBL image names, fonts, default background choices, or locale keys;
- Minecraft, CurseForge, EasyTier, downloads, launcher workflow, or update
  policy;
- BMCBL main-window chrome decisions;
- BMCBL-specific colors, copy, product labels, or diagnostics screens.

If framework behavior needs an application decision, expose a neutral option,
callback, trait, metric, or platform capability. The application decides the
product default.

## BMCBL Application Code

BMCBL product defaults belong outside the framework:

- `src/app.rs`: renderer preference from config, GPU adapter preference,
  default fonts, image pipeline policy, asset source, global state registration,
  main/debug/import window options, and startup services.
- `src/startup.rs`: process startup orchestration, configuration loading, launch
  mode selection, and single-instance policy.
- `src/ui`: GPUI views, route composition, window chrome, overlays, page state,
  visual components, and UI-only interaction state.
- `src/core`: domain and platform integrations such as Minecraft versions,
  AppX/GDK handling, CurseForge queries, EasyTier runtime, online rooms,
  sponsors, version parsing, and UI preference persistence.
- `src/downloads`, `src/archive`, `src/http`, `src/tasks`: durable background
  workflows, transport, extraction, task snapshots, integrity, and progress.
- `src/music`: music library, cover loading, playback service, and UI-facing
  music state integration.
- `src/plugins`: plugin manifest, watcher, runtime, events, UI DSL, and plugin
  windows.

Render methods and generic UI components should coordinate UI state only.
Network IO, durable cache storage, image decoding, archive extraction, download
pipelines, filesystem mutation, launcher workflows, and long-lived background
tasks must stay outside render methods.

## UI Boundary

`src/ui` renders and coordinates UI state. It may read global UI state, call
small UI actions, request background work through existing services, and update
entities when results arrive.

`src/ui` must not become the implementation layer for:

- HTTP clients or protocol-specific request code;
- persistent caches or durable storage;
- archive extraction, MD5 or integrity verification;
- large parsing or decoding pipelines;
- Minecraft package, world, skin, AppX, or GDK domain logic;
- task scheduling internals;
- audio playback internals.

Page-owned UI state lives near the page. Cross-page UI state can live under
`src/ui/state`. Business state that outlives a page or participates in durable
workflows belongs under `src/core`, `src/downloads`, `src/tasks`, `src/music`,
or `src/plugins`.

## Renderer Boundary

GPUI owns the generic renderer pipeline:

```text
Entity invalidation
  -> Window frame scheduling
  -> element prepaint/layout/paint
  -> Scene and FrameRenderPlan
  -> platform_window.draw(...)
  -> NovaRenderer frame upload
  -> backend GPU passes
  -> swapchain present
```

BMCBL owns renderer startup policy:

- configured backend string and GPU adapter name;
- BMCBL preference for high-performance GPU selection;
- image pipeline budgets for this product;
- transparent or opaque window background policy;
- whether a specific window exists and what it renders.

Do not add BMCBL-specific renderer defaults to `vendor/gpui`. Add neutral
`RendererOptions`, metrics, or framework capabilities, then configure them in
`src/app.rs`.

## Change Rule

Before changing a file, classify the behavior:

| Behavior | Correct owner |
| --- | --- |
| Generic element, layout, text, asset, window, input, frame, renderer, or platform behavior | `vendor/gpui` |
| BMCBL startup, window choices, fonts, background policy, configured renderer, globals | `src/app.rs`, `src/startup.rs`, `src/ui` |
| Page composition, overlays, route UI, page-local UI state | `src/ui` |
| Minecraft, CurseForge, EasyTier, online, version parsing, sponsors | `src/core` |
| Downloads, archive extraction, integrity, progress, task snapshots | `src/downloads`, `src/archive`, `src/tasks` |
| HTTP transport and proxy handling | `src/http` |
| Reusable icons, hooks, plugin API, graphics abstraction | `crates/*` |

If a proposed framework change references a BMCBL route, asset, page, launcher
policy, or background selection, keep it in application code. If an application
change requires a reusable framework capability, add the smallest generic
framework API and wire the BMCBL default from `src/app.rs`.

## Validation Scope

Documentation-only changes should at minimum verify paths and links. Code or
framework changes should use the narrowest meaningful command set:

```powershell
cargo fmt --all
./script/clippy
cargo test --workspace --all-features
cargo check --manifest-path vendor/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
```

Use a focused subset when the change only touches one crate or one UI page, and
record any skipped checks in the final change summary.
