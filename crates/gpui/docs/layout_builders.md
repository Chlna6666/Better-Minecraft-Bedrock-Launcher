# Layout Builders

[Chinese](layout_builders.zh-CN.md)

This document tracks the layout builder surface planned for GPUI vNext.

## Current Status

The current repository now exposes these top-level builder helpers:

- `v_stack()`
- `h_stack()`
- `center()`
- `absolute_fill()`
- `relative_fill()`

These helpers are thin wrappers over the existing `div()` and styling APIs.
They improve call-site readability without changing the underlying layout model.

## ParentElement Helpers

`ParentElement` now also includes batch-oriented helpers for common UI assembly
patterns:

- `child_if(...)`
- `child_some(...)`
- `children_array(...)`
- `extend_any(...)`

## Planned Direction

These helpers are intended to improve readability at call sites, especially in
UI code that currently nests repeated flex and positioning chains.
