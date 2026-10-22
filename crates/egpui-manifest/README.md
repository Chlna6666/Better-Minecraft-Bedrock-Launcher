# egpui-manifest

[中文](README.zh-CN.md)

`egpui-manifest` defines the shared application manifest schema for the egpui desktop
framework: application/locale identifiers, manifest model types, validation issues and the
supported schema version (`SUPPORTED_SCHEMA_VERSION = 1`).

## Boundaries

- Schema and validation only; it does not read resources, package applications or run
  downloads.
- Application-specific manifest policy stays in the application crate.

## Build and test

```powershell
cargo check -p egpui-manifest
cargo test -p egpui-manifest
```

## Documentation

- [Architecture](../../docs/ARCHITECTURE.en.md)
- [Crate catalog](../../docs/CRATES.en.md)
