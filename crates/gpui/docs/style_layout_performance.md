# Style and Layout Performance

[Chinese](style_layout_performance.zh-CN.md)

This document records the intended optimization areas for style and layout work
in GPUI vNext.

## Current Status

The current repository already contains structural separation for:

- `style/*`
- `layout/*`
- `elements/div/style_state.rs`
- `style/layout_style.rs` placeholder

Within `style/*`, layout-related enums such as `Display`, `Overflow`,
`FlexDirection`, and `Position` remain part of the style semantics layer rather
than the layout engine layer.

This means the codebase is prepared for later performance work, but the larger
vNext style/layout cache changes are not fully implemented yet.

## Planned Optimization Areas

Planned work includes:

- counting style refinement activity;
- counting style-to-layout conversion activity;
- separating layout-only style state from paint-only style state;
- improving retained layout cache reuse;
- avoiding unnecessary allocation in retained subtree comparison.

## Validation Direction

Later revisions should compare before/after metrics for:

- style refinement count;
- layout conversion count;
- layout cache hit rate;
- retained subtree reuse behavior.

At the current stage, `LayoutStyle` already excludes `visibility`, which
remains paint-facing rather than layout-facing.
