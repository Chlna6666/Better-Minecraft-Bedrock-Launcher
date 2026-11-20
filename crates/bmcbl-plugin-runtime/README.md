# bmcbl_plugin_runtime

Host-side runtime for BMCBL WASM component plugins: a [wasmtime](https://wasmtime.dev/)
(Cranelift JIT) engine, the plugin sandbox, and per-call capability gating.

## Scope

- **Engine** (`engine`): wasmtime engine with epoch interruption (background tick thread),
  in-memory compiled-component memo and an on-disk Cranelift artifact cache keyed by the
  plugin wasm sha256.
- **Sandbox** (`sandbox`): memory hard-limit limiter (32 MiB) with soft-budget evaluation
  (8 MiB), KV key validation, and storage quota accounting.
- **Execution** (`execution`): one `Store` per plugin, restricted WASI p2 (no preopens, no
  sockets, no env; guest stderr is captured into plugin logs), host-import implementations
  with capability checks, and entry-call orchestration with epoch-deadline budgets
  (init 1s / render 100ms / event 50ms / RPC 500ms).
- **Bridge** (`context`): the [`PluginHostContext`] trait the application implements to
  service host calls (tasks, downloads, worlds, versions, HTTP), plus the [`PluginEffect`]
  enum drained after each entry call.

## Boundaries

- Does not depend on GPUI or any application code. The application implements
  `PluginHostContext` and consumes effects; the manifest parsing and policy resolution
  stay in the app.
- The WIT contract lives in `crates/bmcbl-plugin-api/wit/` and is shared verbatim; host
  bindings are generated at build time via `wasmtime::component::bindgen!`.

## Entry points

```rust
let mut engine = PluginEngine::new()?;
let artifact = engine.load_component(&wasm_bytes, Some(&cache_dir))?;
let mut execution = PluginExecution::instantiate(
    &engine, &artifact.component, policy, Arc::new(app_context), init_state,
)?;
let registrations = execution.init(plugin_context)?;
let tree = execution.render_page(request)?;
let effects = execution.take_effects();
```

## Testing

Unit tests run headless. The end-to-end fixture test loads a real compiled component:

```text
cd examples/plugins/hello-wasm && cargo build --target wasm32-wasip2
cargo test -p bmcbl_plugin_runtime --test fixture -- --ignored
```
