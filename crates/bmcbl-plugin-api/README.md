# bmcbl_plugin_api

[中文](README.zh-CN.md)

`bmcbl_plugin_api` is the plugin contract crate for BMCBL. It owns the WIT world
(`wit/plugin.wit`, package `bmcbl:plugin@1.0.0`), the guest-side bindings generated at
build time by `wit-bindgen`, the ergonomic layer on top of them (`Plugin` trait,
`Registrations`/`View` builders, `view!`/`toast!`/`tr!` macros, `prelude`), the
`#[bmcbl_plugin]` / `#[plugin_metadata]` macros, and the `.bmcblx` packing helpers
(`pack` feature).

- API version: `API_VERSION = "1.0.0"`.
- Manifest format: `schema_version = 1`.
- Guests target `wasm32-wasip2`. `cargo build --target wasm32-wasip2` produces a component directly.

Register an interactive tool with `Page::tool(Tool::new("My tool"))`. BMCBL displays
Tools when at least one enabled plugin has registered a tool.
The tool label appears in the shared sidebar. A distinct page title appears below
it. `View::columns()` stacks its two columns in compact windows. `View::input()`
dispatches text changes with its action ID and Enter with `<action-id>-submit`.

The `app` and `world` modules call BMCBL's exported Rust functions. Public
exports are available without individual manifest entries. For example,
`world::is_slime_chunk(x, z)` calls the native `bedrock-world` library and
`app::map_render::tile_payload_fingerprint(bytes)` calls `bedrock-map-render`.
A shared declaration generates the Rust SDK methods and host dispatch. The SDK
returns `Job<T>` from BMCBL's Tokio runtime. Match `job.is_ready(&event)` and
retrieve the result with `job.try_take()`.

`network::get_text` uses BMCBL's HTTP connection pool, proxy, user agent and
cache. Declare `network.http` and an HTTPS allowlist for this API. `filesystem`
reads plugin package resources, `tasks` integrates with the task manager, and
`ui` registers pages rendered by GPUI. WIT carries data across the Wasm boundary.

## Packing

Plugins produce `.bmcblx` packages. The recommended path is a `build.rs` call to
`bmcbl_plugin_api::pack::auto_pack()`; the `bmcbl-plugin-tools` binary
(`pack-cli` feature) supports manual packing.

Set `icon = "assets/icon.png"` in `[package.metadata.bmcbl-plugin]` to show a
packaged icon in BMCBL plugin settings. The registry also publishes static PNG
icons up to 256 × 256 pixels and 64 KiB.

## Build and test

```powershell
cargo check -p bmcbl_plugin_api --all-features
cargo test -p bmcbl_plugin_api --all-features
rustup target add wasm32-wasip2
cargo build --manifest-path examples/plugins/hello-wasm/Cargo.toml --release --target wasm32-wasip2
```

## Boundaries

- Contract, guest bindings and packaging only; no GPUI/application dependencies.
- Host capabilities are permission-checked by the host runtime (`bmcbl_plugin_runtime`).
- Contract changes require an API version bump, manifest schema review and example-plugin
  updates.

## Documentation

- [Architecture](../../docs/ARCHITECTURE.en.md)
- [Crate catalog](../../docs/CRATES.en.md)
