# `src/ui` Structure And Placement Rules

This document describes the current UI layer. It is the local source of truth
for where GPUI views, UI state, components, overlays, and utility windows
belong.

Broader boundaries are documented in
[`docs/ARCHITECTURE_BOUNDARIES.md`](../../docs/ARCHITECTURE_BOUNDARIES.md).
The GPUI rendering pipeline is documented in
[`docs/GPUI_VENDOR_RENDERING.md`](../../docs/GPUI_VENDOR_RENDERING.md).

## Role Of `src/ui`

`src/ui` owns GPUI presentation and UI coordination:

- route pages and page composition;
- main-window chrome, background, overlays, and page registry;
- standalone tool windows;
- reusable UI components;
- UI-only global state;
- theme and animation helpers;
- render-time formatting and small interaction glue.

`src/ui` does not own durable business workflows. Network IO, persistent
caches, archive extraction, download engines, decoding pipelines, Minecraft
domain logic, music playback internals, and plugin runtime internals belong in
non-UI modules.

## Dependency Direction

```text
src/app.rs
  -> src/ui

src/ui/views and src/ui/window
  -> src/ui/components
  -> src/ui/theme
  -> src/ui/state
  -> src/core / src/downloads / src/tasks / src/http / src/music / src/plugins
  -> gpui

src/ui/components
  -> gpui
  -> src/ui/theme where needed
```

Forbidden directions:

- `src/ui/components` must not depend on concrete pages.
- `src/core` must not depend on concrete UI pages.
- `src/ui` must not implement new HTTP clients, durable caches, parsers,
  decoders, download engines, or archive extractors.
- `src/app.rs` must not contain page rendering.

## Placement Rules

### Put code in `src/ui/components`

Only when all are true:

- reusable across pages or windows;
- no page-specific state;
- no product workflow implementation;
- no network, cache, filesystem, or decoding ownership.

Examples: buttons, inputs, markdown renderer, tabs, split panes, sliders,
virtual lists, dropdowns, modals, context menus, code editor primitives.

### Put code in a page module

Use `src/ui/views/<page>.rs` or `src/ui/views/<page>/...` when the code:

- is only used by one route page;
- carries page or domain wording in its type names;
- depends on page-specific state;
- renders panels, rows, dialogs, or toolbars for that page.

Page roots should mostly assemble layout, lifecycle, subscriptions, and render
entry points. If a page starts mixing state models, data loading, parsing,
background tasks, and many panels in one file, split it into page-private
modules.

### Put code in `src/ui/window`

Use this for standalone windows or tool windows outside the main route page
stack: map viewer, skin preview, import window, level.dat editor, debug
window, plugin window, and shared utility window chrome.

Window roots follow the same rule as page roots: compose, own lifecycle entry
points, and delegate responsibility-specific logic to sibling modules.

### Put code in `src/ui/state`

Only when the state is:

- shared across multiple pages or windows;
- UI-facing rather than durable business state;
- small enough to be observed safely by the UI;
- updated through explicit UI actions or service results.

Page-specific state should stay under the page module.

### Move code out of `src/ui`

Move or keep code outside UI when it primarily does:

- network requests;
- filesystem persistence;
- archive extraction;
- download scheduling;
- protocol parsing;
- image/audio decoding;
- cache eviction backed by disk;
- Minecraft package, AppX, GDK, skin, world, or server domain logic;
- plugin runtime management;
- audio playback control.

## Root Files

| Path | Responsibility |
| --- | --- |
| `src/ui/mod.rs` | UI module assembly. |
| `src/ui/main_window.rs` | Main window view entry point. |
| `src/ui/main_window/` | Main-window internals: background, chrome, controls, page registry, loading, route effects, music player, update flow. |
| `src/ui/window.rs` | Standalone window module assembly. |
| `src/ui/window/` | Standalone window implementations and window-specific modules. |
| `src/ui/views/` | Route page modules. |
| `src/ui/components/` | Reusable visual components. |
| `src/ui/state/` | Cross-page UI state. |
| `src/ui/theme/` | Theme colors and helpers. |
| `src/ui/overlays/` | Full-window overlays and modal layers. |
| `src/ui/runtime/` | Root view mounting and UI runtime glue. |
| `src/ui/hooks.rs`, `src/ui/hooks/` | Hook bridge and app-level hook helpers. |
| `src/ui/navigation.rs` | Route enum and navigation helpers. |
| `src/ui/animation.rs` | UI animation helpers and frame request helpers. |
| `src/ui/update_check.rs` | Internal update-check UI orchestration. |

## Main Window

| Path | Responsibility |
| --- | --- |
| `main_window/background.rs` | Main window background rendering. |
| `main_window/background_support.rs` | Background source selection and preload helpers. |
| `main_window/chrome.rs` | Main window top chrome and navigation shell. |
| `main_window/chrome_view.rs` | Chrome view entity and state-to-render bridge. |
| `main_window/controls.rs` | Page control wiring and subscriptions. |
| `main_window/music_player.rs` | Embedded music player UI. |
| `main_window/page_loading.rs` | Lazy page data loading. |
| `main_window/page_registry.rs` | Page entity creation, retention, release, and route cache. |
| `main_window/route_effects.rs` | Route-change side effects. |
| `main_window/support.rs` | Small main-window helpers. Keep this small. |
| `main_window/update_flow.rs` | Update state to main-window UI flow. |

`main_window/*` should remain a shell and coordination layer. Do not add real
download engines, HTTP clients, image decoders, music playback implementation,
or update installation internals here.

## Components

| Path | Responsibility |
| --- | --- |
| `components/adaptive.rs` | Adaptive layout helpers. |
| `components/button.rs` | Reusable button styling and behavior. |
| `components/code_editor.rs` | Code editor component initialization and rendering support. |
| `components/color_picker.rs` | Color picker UI. |
| `components/context_menu.rs` | Context menu UI primitives. |
| `components/dropdown.rs` | Dropdown and overlay selection UI. |
| `components/html_renderer.rs` | HTML rendering to GPUI elements. |
| `components/icon.rs` | Icon rendering helpers. |
| `components/input.rs` | Text input state, actions, and rendering. |
| `components/markdown_renderer.rs` | Markdown rendering and highlighter warmup support. |
| `components/minecraft_text.rs` | Minecraft-formatted text rendering. |
| `components/modal.rs` | Modal layer and dialog helpers. |
| `components/scroll.rs` | Scroll container helpers. |
| `components/slider.rs` | Slider UI component. |
| `components/split_pane.rs` | Split-pane UI component. |
| `components/tabs.rs` | Tab UI component. |
| `components/toast.rs` | Toast state and rendering. |
| `components/toggle_switch.rs` | Toggle switch component. |
| `components/virtual_list.rs` | Virtualized list sizing and rendering support. |

Components must stay generic. Page-only components should live near the page.

## UI State

| Path | Responsibility |
| --- | --- |
| `state/agreement.rs` | User agreement overlay state. |
| `state/debug.rs` | Main UI debug state bridge. |
| `state/diagnostics.rs` | Diagnostics overlay/report state. |
| `state/i18n.rs` | UI localization global state. |
| `state/launch_prereq.rs` | Launch prerequisite overlay state. |
| `state/launcher.rs` | Launcher-facing UI state. |
| `state/local_versions.rs` | Local version UI snapshot state. |
| `state/music.rs`, `state/music_loader.rs`, `state/music_types.rs` | UI-facing music state and loading bridge. |
| `state/navigation.rs` | Navigation animation and active route state. |
| `state/quit.rs` | Quit transition state. |
| `state/theme.rs` | Theme mode, accent color, and transition state. |
| `state/update.rs` | Update check and update overlay state. |

Keep persistent state and domain behavior outside `src/ui/state` unless it is
strictly a UI preference bridge.

## Runtime, Overlays, And Theme

| Path | Responsibility |
| --- | --- |
| `runtime/root_view.rs` | Generic root view wrapper used to mount a GPUI view in a window. |
| `overlays/diagnostics.rs` | Diagnostics overlay UI. |
| `overlays/launch_prereq.rs` | Launch prerequisite overlay UI. |
| `overlays/launcher.rs` | Launcher overlay UI. |
| `overlays/update.rs` | Update overlay UI. |
| `overlays/user_agreement.rs` | User agreement overlay UI. |
| `theme/colors.rs` | Theme color palettes and interpolation. |
| `theme/mod.rs` | Theme module assembly and helpers. |

`runtime` is not a miscellaneous folder. Add runtime code only when it is
window-root or UI-runtime glue.

## Route Pages

| Path | Responsibility |
| --- | --- |
| `views/home.rs`, `views/home/page.rs` | Home page entry and implementation. |
| `views/download.rs`, `views/download/` | Download center page, game downloads, CurseForge resources, resource/mod panels, toolbar, and page state. |
| `views/manage.rs`, `views/manage/` | Version and asset management page. |
| `views/plugin.rs` | Plugin page. |
| `views/settings.rs`, `views/settings/` | Settings page and settings sections. |
| `views/tasks.rs`, `views/tasks/` | Task list page and task render modules. |
| `views/tools.rs`, `views/tools/` | Tools page and online/EasyTier UI. |

Page root files may be larger than simple module entrances when GPUI view
state and subscriptions are tightly coupled, but they should still avoid
owning non-UI services.

## Download Page

| Path | Responsibility |
| --- | --- |
| `views/download/state.rs` | Download page state and CurseForge UI state. |
| `views/download/common.rs` | Download page shared UI helpers. |
| `views/download/toolbar.rs` | Download toolbar, filters, and page controls. |
| `views/download/game.rs` | Game download panel. |
| `views/download/mods.rs` | Local mod/resource pack panel. |
| `views/download/curseforge.rs` | CurseForge resource panel, sidebar/content/detail view entities, and page-level composition. |
| `views/download/curseforge/content.rs` | CurseForge content area rendering. |
| `views/download/curseforge/results.rs` | CurseForge result list entity, result cards, logo cache, placeholders, and reveal behavior. |
| `views/download/curseforge/results_state.rs` | Result query invalidation, page transitions, and result loading orchestration. |
| `views/download/curseforge/modals.rs` | CurseForge detail and install modal UI. |
| `views/download/curseforge/share_actions.rs` | Clipboard share import and copy actions. |

Watch this area carefully. `curseforge.rs` and `results.rs` are high-complexity
UI files. New network or cache behavior should go to `src/core/curseforge`,
`src/http`, or a dedicated non-UI service.

## Manage Page

| Path | Responsibility |
| --- | --- |
| `views/manage/state.rs` | Manage page UI state. |
| `views/manage/view.rs` | Manage page view entry. |
| `views/manage/layout.rs` | Page layout composition. |
| `views/manage/actions.rs` | Page actions. |
| `views/manage/lifecycle.rs` | Page lifecycle hooks. |
| `views/manage/dialogs.rs` | Dialog rendering and state bridge. |
| `views/manage/data.rs` | Page-facing data snapshots. |
| `views/manage/common.rs`, `shared.rs` | Manage page helper UI. |
| `views/manage/assets_tab.rs`, `mod_tab.rs`, `maps_tab.rs`, `screenshots_tab.rs`, `servers_tab.rs` | Tab-specific UI. |
| `views/manage/version_settings.rs` | Version settings UI. |
| `views/manage/level_dat_*` | Level.dat UI bridge, schema, and editor UI. |
| `views/manage/skin_pack_data.rs` | Skin pack page-facing data. |

Minecraft parsing, filesystem mutation, and package logic should remain under
`src/core/minecraft`.

## Settings Page

| Path | Responsibility |
| --- | --- |
| `views/settings/state.rs` | Settings page state. |
| `views/settings/content.rs` | Section switching and content routing. |
| `views/settings/tabs.rs` | Settings tabs. |
| `views/settings/rows.rs` | Settings row builders. |
| `views/settings/common.rs` | Settings visual helpers. |
| `views/settings/about.rs`, `views/settings/about/` | About page, dependencies, sponsors, and update flow UI. |
| `views/settings/customization.rs`, `views/settings/customization/` | Theme color, font, and background settings. |
| `views/settings/game.rs` | Game settings UI. |
| `views/settings/launcher.rs`, `views/settings/launcher/` | Launcher download and connectivity settings. |
| `views/settings/plugins.rs` | Plugin settings UI. |

Sponsor loading, dependency metadata, update checks, and file probing should
stay in non-UI modules. Settings UI should present and commit choices.

## Tools And Tasks Pages

| Path | Responsibility |
| --- | --- |
| `views/tools/state.rs` | Tools page and online UI state. |
| `views/tools/sidebar.rs` | Tools page sidebar. |
| `views/tools/online.rs` | Online tool page entry. |
| `views/tools/online_controls.rs` | Online controls and actions. |
| `views/tools/online_peers.rs` | Peer list UI. |
| `views/tools/online_room.rs` | Room details UI. |
| `views/tools/online_widgets.rs` | Online composite widgets. |
| `views/tools/common.rs` | Tools page helpers. |
| `views/tasks/data.rs` | Task page data projection. |
| `views/tasks/render.rs`, `views/tasks/render/` | Task page render shell, progress, card, overlay, and page rendering. |

EasyTier runtime and online domain logic belong under `src/core/easytier` and
`src/core/online`.

## Standalone Windows

| Path | Responsibility |
| --- | --- |
| `window/chrome.rs` | Shared utility window chrome. |
| `window/debug.rs`, `window/debug/` | Debug window state, view, and developer tools. |
| `window/import.rs`, `window/import/` | Import flow window. |
| `window/level_dat.rs`, `window/level_dat/` | Level.dat editor window. |
| `window/map_viewer.rs`, `window/map_viewer/` | Map viewer window, viewport, panels, tile cache/rendering, preview, history, selection, interactions, and 3D preview. |
| `window/plugin.rs` | Plugin window. |
| `window/skin_pack.rs`, `window/skin_pack/` | Skin preview window, geometry, custom geometry parser, mesh, shader, selector, UV, and preview. |

`map_viewer` and `skin_pack` are intentionally split because they combine
rendering, geometry, state, and interaction-heavy UI. Keep new responsibilities
inside their focused submodules rather than expanding the root files.

## Rendering And Animation Rules

- Use `cx.notify()` when a view entity changes.
- Use `window.request_animation_frame()` only for layout-affecting animation or
  view-driven animation.
- Use `src/ui/animation.rs` helpers for recurring UI animation.
- Do not use `cx.refresh_windows()` as a general animation driver.
- Avoid per-row entities for high-frequency lists unless rows have independent
  lifecycle or state. Prefer page snapshots plus pure render functions for
  task lists and search results.
- For high-frequency state, derive a small observable signature and notify only
  when visible output changes.
- Keep render methods free of expensive data assembly. Prepare snapshots before
  rendering where possible.

## Network And Background Work

All network work must happen off the UI thread. Preferred patterns:

- call existing core/service APIs from a background task;
- use `tokio::spawn`, `tokio::task::spawn_blocking`, or GPUI background work
  where appropriate;
- return results to UI through `cx.update_global`, `entity.update`, or a
  service-owned subscription;
- store cancellable tasks when the work should stop with the view;
- propagate errors to visible UI state instead of discarding them.

Forbidden patterns:

- network request directly inside `render`;
- filesystem persistence directly inside generic components;
- `let _ = fallible_call()` on fallible UI workflow operations;
- clone-heavy rebuilding of large `Vec`, `String`, or `HashMap` values every
  frame when an `Arc`, `SharedString`, slice, or cached snapshot would work.

## Review Checklist

Before adding or moving UI code:

- The file belongs to the requested feature boundary.
- The module has one clear responsibility.
- Components are generic, and page widgets stay near the page.
- Render methods do not own IO, parsing, decoding, downloads, or durable caches.
- New subscriptions notify only on visible state changes.
- Long-running tasks are cancellable or intentionally detached.
- Errors are surfaced to UI state or logged with context.
- Large pages or windows are split by responsibility rather than by arbitrary
  helper names.
