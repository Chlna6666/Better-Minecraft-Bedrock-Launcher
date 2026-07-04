# Element Lifecycle

[Chinese](element_lifecycle.zh-CN.md)

This document describes the current GPUI element lifecycle and the planned
vNext direction.

## Current Lifecycle

The current framework still uses the existing three-step flow:

1. request layout;
2. prepaint;
3. paint.

The recent structural split moved lifecycle-related code into
`render_pipeline/*`, but it did not yet replace the active API with the planned
vNext typed frame contexts.

## Planned vNext Direction

The target model remains:

```rust
trait Element {
    type State;

    fn prepare(&mut self, cx: &mut PrepareCx) -> Self::State;
    fn layout(&mut self, state: &mut Self::State, cx: &mut LayoutCx) -> LayoutId;
    fn prepaint(&mut self, state: &mut Self::State, cx: &mut PrepaintCx);
    fn paint(&mut self, state: &mut Self::State, cx: &mut PaintCx);
}
```

This is not active in the current codebase yet. It remains a staged migration
goal.

## Why It Matters

The planned split is intended to:

- prevent layout code from registering paint-only behavior;
- prevent paint code from mutating layout state;
- keep one frame-local state value across the full pipeline;
- make future caching work easier to reason about.

## Current Structural Placeholders

The repository now includes a `render_pipeline/context.rs` module with
placeholder `PrepareCx`, `LayoutCx`, `PrepaintCx`, and `PaintCx` types.

These types are not yet the active element API. They exist to make the
intended lifecycle boundary explicit before the later lifecycle switch.
