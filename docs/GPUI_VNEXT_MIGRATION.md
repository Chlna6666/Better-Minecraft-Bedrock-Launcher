# GPUI vNext Migration in BMCBL

This document tracks how the BMCBL workspace consumes the vendored GPUI vNext
migration.

## Scope Boundary

The GPUI migration only covers framework and UI adaptation work. It does not
change:

- download engine behavior;
- Minecraft business logic;
- plugin runtime logic;
- network workflow logic.

## Current Status

The workspace currently has the phase 1 framework split in place:

- GPUI `element`, `render_pipeline`, `style`, `layout`, and `Div` internals have
  been structurally separated;
- existing BMCBL code still compiles against the current public API;
- no broad BMCBL UI migration has started yet because the later breaking API
  phases are not active.

Current style/layout naming follows this boundary:

- `style/` owns style semantics, including layout-facing style enums;
- `layout/` owns layout engine, conversion, cache, fingerprint, and metrics.

## Validation

The current repository state has been validated with:

- `rtk cargo check -p gpui`
- `rtk cargo check -p gpui --examples`
- `rtk cargo check`

These checks confirm that the current structural split still integrates with the
workspace.

## Planned BMCBL Order

When GPUI vNext API changes become active, the intended BMCBL migration order is:

1. `src/ui/components/**`
2. `src/ui/main_window/**`
3. `src/ui/views/home/**`
4. `src/ui/views/download/**`
5. `src/ui/window/map_viewer/**`
6. remaining settings/tools/tasks pages

## Next Documentation Work

Later revisions of this document should add:

- concrete API mapping tables;
- compatibility layer notes;
- removed API lists;
- smoke-test checkpoints for key BMCBL windows and pages.
