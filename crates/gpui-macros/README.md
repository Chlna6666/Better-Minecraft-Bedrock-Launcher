# gpui-macros

[中文](README.zh-CN.md)

`gpui-macros` is the BMCBL-maintained mirror of GPUI's procedural macros (`IntoElement`,
`Render`, actions, app/visual contexts, inspector reflection and the benchmark harness).
It is consumed by `gpui`; applications should not depend on it directly.

## Boundaries

- Macro implementation only; runtime behaviour lives in `gpui`.
- Keep behaviour aligned with the upstream revision recorded in `Cargo.toml`.

## Build and test

```powershell
cargo check -p gpui-macros
cargo test -p gpui
```

## Documentation

- [gpui](../gpui/README.md)
- [Crate catalog](../../docs/CRATES.en.md)
