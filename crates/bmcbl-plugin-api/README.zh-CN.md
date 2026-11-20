# bmcbl_plugin_api

[English](README.md)

`bmcbl_plugin_api` 是 BMCBL 的插件契约 crate：持有 WIT 世界（`wit/plugin.wit`，包
`bmcbl:plugin@1.0.0`）、`wit-bindgen` 构建期生成的 guest 侧绑定、其上的人机工程层
（`Plugin` trait、`Registrations`/`View` 构建器、`view!`/`toast!`/`tr!` 宏、`prelude`）、
`#[bmcbl_plugin]`/`#[plugin_metadata]` 宏，以及 `.bmcblx` 打包辅助（`pack` feature）。

- API 版本：`API_VERSION = "1.0.0"`。
- 清单格式：`schema_version = 1`。
- guest 以 `wasm32-wasip2` 为目标。`cargo build --target wasm32-wasip2` 直接产出组件。

用 `Page::tool(Tool::new("我的工具"))` 注册交互页面。至少有一个启用的插件
注册工具后，主导航显示「工具」。
工具名称显示在统一侧栏，页面标题不同时显示为副标题。`View::columns()`
在紧凑窗口中将两栏纵向排列。`View::input()` 以输入框的动作 ID 上报文本变化，
按 Enter 时发送 `<action-id>-submit`。

SDK 的 `app` 和 `world` 暴露宿主已导出的 Rust 函数，插件清单无需逐函数申请。
例如 `world::is_slime_chunk(x, z)` 调用 BMCBL 进程里的 `bedrock-world`，
`app::map_render::tile_payload_fingerprint(bytes)` 调用宿主的 `bedrock-map-render`。
一份导出声明生成 SDK 方法和宿主分发代码，实际实现仍由原库负责。SDK 返回 `Job<T>`，在
事件处理中用 `job.is_ready(&event)` 和 `job.try_take()` 接收 BMCBL Tokio 任务结果。

`network::get_text` 复用 BMCBL 的 HTTP 连接池、代理、UA 和缓存，需要声明
`network.http` 和 HTTPS 允许列表。`filesystem` 提供插件包资源读取，`tasks`
接入任务管理器，`ui` 注册由 GPUI 渲染的页面。WIT 负责 Wasm 与宿主之间的数据传递。

## 打包

插件产物为 `.bmcblx`。推荐在 `build.rs` 调用
`bmcbl_plugin_api::pack::auto_pack()`；`pack-cli` feature 提供
`bmcbl-plugin-tools` 手动打包工具。

在 `[package.metadata.bmcbl-plugin]` 配置 `icon = "assets/icon.png"`，
即可在 BMCBL 插件设置中显示包内图标。Registry 也会发布最长边 256 像素、
最大 64 KiB 的静态 PNG 图标。

## 构建与测试

```powershell
cargo check -p bmcbl_plugin_api --all-features
cargo test -p bmcbl_plugin_api --all-features
rustup target add wasm32-wasip2
cargo build --manifest-path examples/plugins/hello-wasm/Cargo.toml --release --target wasm32-wasip2
```

## 边界

- 只负责契约、guest 绑定与打包；不依赖 GPUI/应用代码。
- host 能力由宿主运行时（`bmcbl_plugin_runtime`）做权限校验。
- 契约变更必须同步 API 版本、manifest schema 评审与示例插件。

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)
