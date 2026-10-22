# bmcbl_plugin_macros

[中文](README.zh-CN.md)

Procedural macros for BMCBL plugins: `#[bmcbl_plugin]` generates the ABI entry points for a
plugin implementation, and `#[plugin_metadata]` embeds plugin metadata read from the plugin
manifest/TOML at build time.

## Boundaries

- Macro implementation only; runtime ABI stays in `bmcbl_plugin_api`.
- `#[bmcbl_plugin(pack)]` is removed; use `build.rs` with
  `bmcbl_plugin_api::pack::auto_pack_from_build_script()`.

## Build and test

```powershell
cargo check -p bmcbl_plugin_macros
```

## Documentation

- [bmcbl_plugin_api](../bmcbl-plugin-api/README.md)
- [Crate catalog](../../docs/CRATES.en.md)
