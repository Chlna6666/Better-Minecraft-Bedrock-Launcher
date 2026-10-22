# bmcbl_plugin_api

[中文](README.zh-CN.md)

`bmcbl_plugin_api` is the WASM plugin ABI for BMCBL. It defines the host-call protocol,
plugin context, UI/layout tokens, theme/log/toast types, errors, the `#[bmcbl_plugin]` and
`#[plugin_metadata]` macros, and the `.bmcblx` packing helpers (`pack` feature).

- API version: `API_VERSION = "0.6"`.
- WASM guests build with `no_std`; the crate hosts the ABI boundary.
- Manifest schema version is `2` (see `bmcbl-plugin-macros` and the packer).

## Packing

Plugins produce `.bmcblx` packages. The recommended path is a `build.rs` call to
`bmcbl_plugin_api::pack::auto_pack_from_build_script()`; the `bmcbl-plugin-tools` binary
(`pack-cli` feature) supports manual packing.

## Build and test

```powershell
cargo check -p bmcbl_plugin_api --all-features
cargo test -p bmcbl_plugin_api --all-features
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/plugins/hello-wasm/Cargo.toml --release --target wasm32-unknown-unknown
```

## Boundaries

- ABI and packaging only; no GPUI/application dependencies.
- Host capabilities are permission-checked by the application plugin runtime.
- ABI changes require an API version bump, manifest schema review and example-plugin updates.

## Documentation

- [Architecture](../../docs/ARCHITECTURE.en.md)
- [Crate catalog](../../docs/CRATES.en.md)
