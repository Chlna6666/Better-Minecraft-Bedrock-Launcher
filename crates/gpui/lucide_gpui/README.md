# lucide_gpui

[中文](README.zh-CN.md)

`lucide_gpui` exposes Lucide icons to GPUI applications. Its build script collects the icon
set and embeds it as a generated `Assets` source plus indexed icon bytes; the public API is
the `Assets` type consumed by GPUI's asset source.

## Boundaries

- Asset adapter only: no UI components, layout or application policy.
- Icons are generated at build time; adding icons updates the build output rather than adding
  hand-written files.

## Build and test

```powershell
cargo check -p lucide_gpui
cargo test -p lucide_gpui
```

## Documentation

- [Architecture](../../../docs/ARCHITECTURE.en.md)
- [Crate catalog](../../../docs/CRATES.en.md)
