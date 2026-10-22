# bmcbl_plugin_api

[English](README.md)

`bmcbl_plugin_api` 是 BMCBL 的 WASM 插件 ABI：定义 host-call 协议、插件上下文、
UI/布局 token、主题/日志/Toast 类型、错误、`#[bmcbl_plugin]`/`#[plugin_metadata]` 宏，
以及 `.bmcblx` 打包辅助（`pack` feature）。

- API 版本：`API_VERSION = "0.6"`。
- WASM guest 使用 `no_std` 构建；ABI 边界由本 crate 承担。
- manifest schema 版本为 `2`（见 `bmcbl-plugin-macros` 与打包器）。

## 打包

插件产物为 `.bmcblx`。推荐在 `build.rs` 调用
`bmcbl_plugin_api::pack::auto_pack_from_build_script()`；`pack-cli` feature 提供
`bmcbl-plugin-tools` 手动打包工具。

## 构建与测试

```powershell
cargo check -p bmcbl_plugin_api --all-features
cargo test -p bmcbl_plugin_api --all-features
rustup target add wasm32-unknown-unknown
cargo build --manifest-path examples/plugins/hello-wasm/Cargo.toml --release --target wasm32-unknown-unknown
```

## 边界

- 只负责 ABI 与打包；不依赖 GPUI/应用代码。
- host 能力由应用插件运行时做权限校验。
- ABI 变更必须同步 API 版本、manifest schema 评审与示例插件。

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
