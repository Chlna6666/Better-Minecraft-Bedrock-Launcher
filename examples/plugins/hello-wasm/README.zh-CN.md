# BMCBL Essentials

[English](README.md)

`BMCBL Essentials` 是 BMCBL WASM Component 插件系统的参考插件，演示：

- 页面注册与导航；
- 安全的 UI 注入；
- host Toast 与插件窗口 API；
- 全局事件订阅；
- 通过 `.lang` 文件实现插件 i18n；
- 从 WASM 读取只读插件配置。

## 构建

```powershell
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/plugins/hello-wasm/Cargo.toml
```

构建脚本会通过 `bmcbl_plugin_api::pack::auto_pack_from_build_script()` 生成 `.bmcblx`。
插件 API 版本为 `0.6`。
