# bmcbl_plugin_runtime

BMCBL WASM 组件插件的宿主侧运行时：[wasmtime](https://wasmtime.dev/)（Cranelift JIT）引擎、
插件沙箱与逐调用 capability 门禁。

## 职责

- **引擎**（`engine`）：wasmtime 引擎 + epoch 中断（后台 tick 线程），内存编译缓存与按
  插件 wasm sha256 键控的 Cranelift 磁盘产物缓存。
- **沙箱**（`sandbox`）：内存硬熔断（32 MiB）+ 软预算评估（8 MiB）、KV key 校验与存储配额。
- **执行**（`execution`）：每插件独立 `Store`、受限 WASI p2（零 preopen/socket/env，guest
  stderr 捕获进插件日志）、带 capability 门禁的宿主接口实现，以及按 epoch deadline 预算的
  入口调用编排（init 1s / render 100ms / event 50ms / RPC 500ms）。
- **桥接**（`context`）：应用侧实现的 [`PluginHostContext`] trait（任务、下载、存档、版本、
  HTTP 等宿主服务），以及每次入口调用后统一消费的 [`PluginEffect`] 副作用。

## 边界

- 不依赖 GPUI 或应用代码。应用实现 `PluginHostContext` 并消费副作用；manifest 解析与
  策略组装留在应用侧。
- WIT 契约在 `crates/bmcbl-plugin-api/wit/`，两侧共用同一份文件；宿主绑定由
  `wasmtime::component::bindgen!` 构建期生成。

## 入口

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

## 测试

单元测试无头运行。端到端 fixture 测试加载真实编译组件：

```text
cd examples/plugins/hello-wasm && cargo build --target wasm32-wasip2
cargo test -p bmcbl_plugin_runtime --test fixture -- --ignored
```
