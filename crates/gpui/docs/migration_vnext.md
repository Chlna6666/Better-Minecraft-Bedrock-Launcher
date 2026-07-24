# GPUI vNext Migration

[Chinese](migration_vnext.zh-CN.md)

This document tracks the staged GPUI vNext migration inside the vendored
framework. It is written against the current repository state and intentionally
separates completed structural work from planned API and lifecycle changes.

## Goal

GPUI vNext has three primary objectives:

- reduce structural coupling in large framework files;
- create room for explicit frame lifecycle and layout/style caching work;
- let BMCBL migrate in phases instead of one mixed rewrite.

## Current Stage

The repository is currently in the first stage:

1. structure split;
2. behavior preserved;
3. existing public API still compiling;
4. no vNext lifecycle switch enabled yet.

## Completed Structural Work

The following areas have already been physically split into normal Rust modules:

- `element.rs` now acts as a facade over `render_pipeline.rs` and
  `render_pipeline/*`;
- `elements/div.rs` now acts as a facade over `elements/div/*`;
- `style.rs` now acts as a facade over `style/*`;
- `layout.rs` now acts as a facade over `layout/*`.

Within `elements/div/`, the current split includes:

- `element.rs`
- `state.rs`
- `style_state.rs`
- `scroll.rs`
- `tooltip.rs`
- `drag_drop.rs`
- `inspector.rs`
- `event.rs`
- `event_handlers.rs`
- `event_runtime.rs`

The repository also now includes the first layout ergonomics layer:

- `layout_builders.rs`
- `ParentElement::child_if`
- `ParentElement::child_some`
- `ParentElement::children_array`
- `ParentElement::extend_any`

## Not Yet Implemented

The following vNext items are still planned and are not claimed by the current
code:

- explicit `prepare/layout/prepaint/paint` frame pipeline contexts;
- layout-only style fingerprint separation;
- retained layout cache improvements described in the migration plan;
- compatibility-layer removal;
- BMCBL UI migration to any future breaking API.

## Validation Rules

Each structural step must keep these commands passing:

- `rtk cargo check -p gpui`
- `rtk cargo check -p gpui --examples`
- `rtk cargo check`

If workspace warnings or unrelated non-GPUI failures already exist, they should
be recorded separately rather than folded into the GPUI migration status.

## Next Phases

After structure splitting is stable, the intended order remains:

1. add layout ergonomics helpers;
2. introduce the explicit element lifecycle;
3. optimize style/layout hot paths and metrics;
4. migrate BMCBL UI code in batches;
5. remove compatibility layers and finish documentation.
