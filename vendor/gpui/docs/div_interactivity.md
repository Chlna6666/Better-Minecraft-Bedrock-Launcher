# Div Interactivity

[Chinese](div_interactivity.zh-CN.md)

This guide describes the current `Div` interactivity structure after the
behavior-preserving split. It documents where the existing API lives today,
without claiming the later vNext lifecycle rewrite is already finished.

## Public Surface

`gpui::Div` still exposes the familiar fluent API:

- mouse handlers such as `.on_click()` and `.on_mouse_down()`;
- focus and hover helpers;
- scroll tracking helpers;
- tooltip and drag/drop helpers.

Call sites should continue importing `InteractiveElement` and
`StatefulInteractiveElement` as before.

## Current Internal Split

The implementation is now separated by responsibility:

- `elements/div/element.rs`: `Div` element wrapper and child composition;
- `elements/div/state.rs`: `Interactivity` data and frame entry points;
- `elements/div/style_state.rs`: computed style and group-hover helpers;
- `elements/div/scroll.rs`: scroll handle state and scroll runtime logic;
- `elements/div/tooltip.rs`: tooltip lifecycle helpers;
- `elements/div/drag_drop.rs`: drag/drop payload structs;
- `elements/div/inspector.rs`: debug and inspector state;
- `elements/div/event.rs`: public event traits;
- `elements/div/event_handlers.rs`: listener registrations and callback storage;
- `elements/div/event_runtime.rs`: runtime listener binding and click/drag state transitions.

## State Model

`Interactivity` remains the single data owner for:

- element identity and focus state;
- hover/active/group style refinements;
- listener registrations;
- scroll and tooltip handles;
- drag/drop and click state.

The structural split did not change who owns these fields. It only moved the
code that reads and mutates them into narrower modules.

## Current Constraint

The current split is still based on the existing GPUI frame model:

- request layout;
- prepaint;
- paint.

The future explicit `prepare/layout/prepaint/paint` vNext lifecycle is planned
separately and is not yet the active API.
