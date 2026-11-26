# gpui_hooks

[中文](README.zh-CN.md)

`gpui_hooks` provides React-style hooks for GPUI: state, memo, effect, ref, callback, reducer
and dependency tracking, plus `HookedElement`/`HookedRender`/`execute_hooked_render` and the
`#[hook_element]`/`#[hook_render]` macros from `gpui_hooks_macros`.

## Boundaries

- UI state reuse only; no application routing, IO or domain policy.
- Hook state lives for the owning element/window lifetime; do not use hooks to smuggle global
  mutable state.

## Build and test

```powershell
cargo check -p gpui_hooks
cargo test -p gpui_hooks
```

## Documentation

- [Architecture](../../../docs/ARCHITECTURE.en.md)
- [Crate catalog](../../../docs/CRATES.en.md)
